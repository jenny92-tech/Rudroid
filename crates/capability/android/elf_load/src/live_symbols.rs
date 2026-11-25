//! INPUT:  original-file ELF layout plus owned runtime image base
//! OUTPUT: bounded live-table evidence and strict unversioned export fallback
//! POS:    evidence stays independent of initialized-image binding eligibility
#![forbid(unsafe_code)]

use std::fmt;

use rudroid_common::mapped_memory::MappedMemory;

use super::elf::{
    ElfView, DT_GNU_HASH, DT_HASH, DT_NULL, DT_STRSZ, DT_STRTAB, DT_SYMENT, DT_SYMTAB, DT_VERDEF,
    DT_VERDEFNUM, DT_VERNEED, DT_VERNEEDNUM, DT_VERSYM, PF_R, PF_X,
};
use super::hash;
use super::relocations::STT_TLS;

const MAX_DYNAMIC_BYTES: u64 = 64 * 1024;
const MAX_READ_BYTES: usize = 64 * 1024;
const MAX_NAME_BYTES: u64 = 4096;
const MAX_HASH_TABLE_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SYMBOL_INDEX: usize = 4_000_000;
const MAX_CHAIN_STEPS: usize = 1_000_000;
#[cfg(not(test))]
const MAX_SYSV_CHAIN_STEPS: usize = 1_000_000;
#[cfg(test)]
const MAX_SYSV_CHAIN_STEPS: usize = 4;
const SHN_ABS: u16 = 0xfff1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LiveResult {
    Hit(usize),
    Miss,
    Rejected(&'static str),
    Inconclusive(String),
}

impl fmt::Display for LiveResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Hit(address) => write!(f, "hit(0x{address:x})"),
            Self::Miss => f.write_str("miss"),
            Self::Rejected(reason) => write!(f, "rejected({reason})"),
            Self::Inconclusive(reason) => write!(f, "inconclusive({reason:?})"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LiveReport {
    pub result: LiveResult,
    pub hash: &'static str,
    pub tags: String,
}

/// Read the standard dynamic table at the runtime load bias and compare the
/// selected symbol/string/hash metadata with the original file view. Every
/// copy is bounded by an owned, readable PT_LOAD range from that view. A read
/// failure or an out-of-range table is inconclusive, never a no-export proof.
pub(crate) fn diagnose(view: &ElfView<'_>, base: usize, name: &str) -> LiveReport {
    if base == 0 {
        return LiveReport {
            result: LiveResult::Inconclusive("runtime base is zero".to_owned()),
            hash: "unknown",
            tags: "unknown".to_owned(),
        };
    }
    let memory = match MappedMemory::open() {
        Ok(memory) => memory,
        Err(error) => {
            return LiveReport {
                result: LiveResult::Inconclusive(format!("cannot open /proc/self/mem: {error}")),
                hash: "unknown",
                tags: "unknown".to_owned(),
            }
        }
    };
    LiveReader { view, base, memory: &memory, binding: false }.diagnose(name)
}

/// The caller owns/pins this image and has observed successful initializers.
/// Missing/malformed live metadata is never authority to invent an address.
/// Versioned lookup remains file-only: this intentionally supports no live
/// version tables, IFUNC resolvers, TLS addresses or absolute symbols.
pub(crate) fn bind(
    view: &ElfView<'_>, base: usize, name: &str,
) -> Result<Option<usize>, super::ExportReject> {
    if base == 0 || name.is_empty() || name.len() >= MAX_NAME_BYTES as usize {
        return Ok(None);
    }
    let (dynamic, bytes) = view.dynamic;
    // Reject unsupported images before opening memory or allocating scratch
    // buffers: an ordinary miss on an unreadable image stays allocation-free.
    if !(16..=MAX_DYNAMIC_BYTES).contains(&bytes)
        || view.versym.is_some() || view.verdef.is_some() || view.verneed.is_some()
        || !view.loads.iter().any(|load| load.flags & PF_R != 0
            && dynamic >= load.vaddr
            && dynamic.checked_add(bytes).zip(load.vaddr.checked_add(load.memsz))
                .is_some_and(|(end, limit)| end <= limit)) {
        return Ok(None);
    }
    // /proc/self/mem descriptors refer to the opener's address space. Reopen
    // on the first lookup after fork. FileExt reads keep no shared cursor.
    thread_local! {
        static MEMORY: std::cell::RefCell<Option<(u32, MappedMemory)>> = const { std::cell::RefCell::new(None) };
    }
    MEMORY.try_with(|slot| {
        let Ok(mut cached) = slot.try_borrow_mut() else { return Ok(None) };
        let pid = std::process::id();
        if cached.as_ref().is_none_or(|(owner, _)| *owner != pid) {
            *cached = MappedMemory::open().ok().map(|memory| (pid, memory));
        }
        let Some((_, memory)) = cached.as_ref() else { return Ok(None) };
        let reader = LiveReader { view, base, memory, binding: true };
        let Ok(entries) = reader.dynamic_entries() else { return Ok(None) };
        let versions = [DT_VERSYM, DT_VERDEF, DT_VERDEFNUM, DT_VERNEED, DT_VERNEEDNUM];
        if entries.iter().any(|(tag, _)| versions.contains(tag)) {
            return Ok(None);
        }
        // Unlike a diagnostic, binding cannot select one of conflicting tags.
        for tag in [DT_SYMTAB, DT_STRTAB, DT_STRSZ, DT_SYMENT, DT_GNU_HASH, DT_HASH] {
            if entries.iter().filter(|(kind, _)| *kind == tag).count() > 1 {
                return Ok(None);
            }
        }
        match reader.live_lookup(&entries, name) {
            Ok(LiveResult::Hit(address)) => Ok(Some(address)),
            Ok(LiveResult::Rejected("tls")) => Err(super::ExportReject::Tls),
            _ => Ok(None),
        }
    }).unwrap_or(Ok(None))
}

struct LiveReader<'v, 'f> {
    view: &'v ElfView<'f>,
    base: usize,
    memory: &'v MappedMemory,
    binding: bool,
}

impl LiveReader<'_, '_> {
    fn diagnose(&self, name: &str) -> LiveReport {
        let entries = match self.dynamic_entries() {
            Ok(entries) => entries,
            Err(reason) => {
                return LiveReport {
                    result: LiveResult::Inconclusive(reason),
                    hash: "unknown",
                    tags: "unknown".to_owned(),
                }
            }
        };
        let hash = live_hash(&entries);
        let tags = self.changed_tags(&entries);
        let result = match self.live_lookup(&entries, name) {
            Ok(result) => result,
            Err(reason) => LiveResult::Inconclusive(reason),
        };
        LiveReport { result, hash, tags }
    }

    fn dynamic_entries(&self) -> Result<Vec<(u64, u64)>, String> {
        let (vaddr, filesz) = self.view.dynamic;
        if filesz < 16 || filesz > MAX_DYNAMIC_BYTES {
            return Err("PT_DYNAMIC size is outside the diagnostic bound".to_owned());
        }
        // Dynamic tables are commonly much shorter than their reserved segment.
        // Read bounded groups and stop as soon as the terminating tag is found.
        let mut entries = Vec::new();
        let mut offset = 0;
        let mut raw = [0u8; 256];
        while filesz - offset >= 16 {
            let len = (filesz - offset).min(raw.len() as u64) as usize / 16 * 16;
            self.read(vaddr.checked_add(offset).ok_or("PT_DYNAMIC address overflow")?, &mut raw[..len])?;
            for chunk in raw[..len].chunks_exact(16) {
                let tag = u64::from_le_bytes(chunk[..8].try_into().expect("8 bytes"));
                if tag == DT_NULL { return Ok(entries); }
                entries.push((tag, u64::from_le_bytes(chunk[8..].try_into().expect("8 bytes"))));
            }
            offset += len as u64;
        }
        Err("PT_DYNAMIC has no DT_NULL within its file-sized range".to_owned())
    }

    fn changed_tags(&self, entries: &[(u64, u64)]) -> String {
        let checks: [(&str, u64, Option<u64>); 11] = [
            ("SYMTAB", DT_SYMTAB, Some(self.view.symtab)),
            ("STRTAB", DT_STRTAB, Some(self.view.strtab)),
            ("STRSZ", DT_STRSZ, Some(self.view.strsz)),
            ("SYMENT", DT_SYMENT, Some(self.view.syment)),
            ("GNU_HASH", DT_GNU_HASH, self.view.gnu_hash),
            ("HASH", DT_HASH, self.view.sysv_hash),
            ("VERSYM", DT_VERSYM, self.view.versym),
            ("VERDEF", DT_VERDEF, self.view.verdef.map(|value| value.0)),
            (
                "VERDEFNUM",
                DT_VERDEFNUM,
                self.view.verdef.map(|value| value.1),
            ),
            (
                "VERNEED",
                DT_VERNEED,
                self.view.verneed.map(|value| value.0),
            ),
            (
                "VERNEEDNUM",
                DT_VERNEEDNUM,
                self.view.verneed.map(|value| value.1),
            ),
        ];
        let changed: Vec<&str> = checks
            .into_iter()
            .filter_map(|(label, tag, expected)| {
                let actual = if tag == DT_SYMENT {
                    Some(tag_value(entries, tag).unwrap_or(24))
                } else {
                    tag_value(entries, tag)
                };
                (actual != expected).then_some(label)
            })
            .collect();
        if changed.is_empty() {
            "none".to_owned()
        } else {
            changed.join(",")
        }
    }

    fn live_lookup(&self, entries: &[(u64, u64)], name: &str) -> Result<LiveResult, String> {
        let symtab = required_tag(entries, DT_SYMTAB, "DT_SYMTAB")?;
        let strtab = required_tag(entries, DT_STRTAB, "DT_STRTAB")?;
        let strsz = required_tag(entries, DT_STRSZ, "DT_STRSZ")?;
        if strsz == 0 {
            return Err("live DT_STRSZ is zero".to_owned());
        }
        if tag_value(entries, DT_SYMENT).is_some_and(|size| size != 24) {
            return Err("live DT_SYMENT is not the ELF64 size 24".to_owned());
        }
        if self.binding {
            if symtab == 0 || strtab == 0 || strsz > MAX_HASH_TABLE_BYTES {
                return Err("invalid live binding table range".to_owned());
            }
            self.ensure_range(symtab, 24)?;
            self.ensure_range(strtab, strsz)?;
        }
        if let Some(table) = tag_value(entries, DT_GNU_HASH) {
            return self.lookup_gnu(table, symtab, strtab, strsz, name);
        }
        if let Some(table) = tag_value(entries, DT_HASH) {
            return self.lookup_sysv(table, symtab, strtab, strsz, name);
        }
        Err("live dynamic table has neither DT_GNU_HASH nor DT_HASH".to_owned())
    }

    fn lookup_gnu(
        &self,
        table: u64,
        symtab: u64,
        strtab: u64,
        strsz: u64,
        name: &str,
    ) -> Result<LiveResult, String> {
        if table == 0 {
            return Err("live DT_GNU_HASH is zero".to_owned());
        }
        let header = self.read_array::<16>(table)?;
        let nbuckets = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes")) as usize;
        let symoffset = u32::from_le_bytes(header[4..8].try_into().expect("4 bytes")) as usize;
        let bloom_size = u32::from_le_bytes(header[8..12].try_into().expect("4 bytes")) as usize;
        let bloom_shift = u32::from_le_bytes(header[12..16].try_into().expect("4 bytes"));
        if nbuckets == 0 || bloom_size == 0 {
            return Err("live GNU hash has zero buckets or bloom words".to_owned());
        }
        let bloom_bytes = bloom_size
            .checked_mul(8)
            .ok_or_else(|| "live GNU hash bloom size overflows".to_owned())?;
        let buckets_offset = 16usize
            .checked_add(bloom_bytes)
            .ok_or_else(|| "live GNU hash bucket offset overflows".to_owned())?;
        let bucket_bytes = nbuckets
            .checked_mul(4)
            .ok_or_else(|| "live GNU hash bucket count overflows".to_owned())?;
        let chains_offset = buckets_offset
            .checked_add(bucket_bytes)
            .ok_or_else(|| "live GNU hash chain offset overflows".to_owned())?;
        if u64::try_from(chains_offset)
            .ok()
            .is_none_or(|value| value > MAX_HASH_TABLE_BYTES)
        {
            return Err("live GNU hash header exceeds the diagnostic bound".to_owned());
        }
        self.ensure_range(table, u64::try_from(chains_offset).unwrap())?;

        let hash = hash::gnu_hash(name) as usize;
        let bloom_word = self.read_u64(
            table
                .checked_add(16)
                .and_then(|value| value.checked_add(((hash / 64) % bloom_size * 8) as u64))
                .ok_or_else(|| "live GNU hash bloom address overflows".to_owned())?,
        )?;
        let bit = 1u64 << (hash % 64);
        let second_bit = 1u64 << ((hash >> (bloom_shift % 32)) % 64);
        if bloom_word & bit == 0 || bloom_word & second_bit == 0 {
            return Ok(LiveResult::Miss);
        }

        let bucket_offset = buckets_offset
            .checked_add((hash % nbuckets) * 4)
            .ok_or_else(|| "live GNU hash bucket address overflows".to_owned())?;
        let mut index = self.read_u32(
            table
                .checked_add(u64::try_from(bucket_offset).unwrap())
                .ok_or_else(|| "live GNU hash bucket address overflows".to_owned())?,
        )? as usize;
        if index == 0 {
            return Ok(LiveResult::Miss);
        }
        if index < symoffset {
            return Err("live GNU hash bucket points below symoffset".to_owned());
        }
        for _ in 0..MAX_CHAIN_STEPS {
            if index > MAX_SYMBOL_INDEX {
                return Err("live GNU hash symbol index exceeds the diagnostic bound".to_owned());
            }
            let chain_offset = chains_offset
                .checked_add(
                    index
                        .checked_sub(symoffset)
                        .and_then(|value| value.checked_mul(4))
                        .ok_or_else(|| "live GNU hash chain address overflows".to_owned())?,
                )
                .ok_or_else(|| "live GNU hash chain address overflows".to_owned())?;
            let chain = self.read_u32(
                table
                    .checked_add(u64::try_from(chain_offset).unwrap())
                    .ok_or_else(|| "live GNU hash chain address overflows".to_owned())?,
            )?;
            if (chain | 1) == ((hash as u32) | 1) {
                if let Some(result) = self.candidate(index, symtab, strtab, strsz, name)? {
                    return Ok(result);
                }
            }
            if chain & 1 == 1 {
                return Ok(LiveResult::Miss);
            }
            index = index
                .checked_add(1)
                .ok_or_else(|| "live GNU hash symbol index overflows".to_owned())?;
        }
        Err("live GNU hash chain exceeds the diagnostic bound".to_owned())
    }

    fn lookup_sysv(
        &self,
        table: u64,
        symtab: u64,
        strtab: u64,
        strsz: u64,
        name: &str,
    ) -> Result<LiveResult, String> {
        if table == 0 {
            return Err("live DT_HASH is zero".to_owned());
        }
        let header = self.read_array::<8>(table)?;
        let nbuckets = u32::from_le_bytes(header[0..4].try_into().expect("4 bytes")) as usize;
        let nchain = u32::from_le_bytes(header[4..8].try_into().expect("4 bytes")) as usize;
        if nbuckets == 0 {
            return Err("live SYSV hash has zero buckets".to_owned());
        }
        let entries = nbuckets
            .checked_add(nchain)
            .ok_or_else(|| "live SYSV hash entry count overflows".to_owned())?;
        let size = entries
            .checked_mul(4)
            .and_then(|value| value.checked_add(8))
            .ok_or_else(|| "live SYSV hash size overflows".to_owned())?;
        if u64::try_from(size)
            .ok()
            .is_none_or(|value| value > MAX_HASH_TABLE_BYTES)
        {
            return Err("live SYSV hash exceeds the diagnostic bound".to_owned());
        }
        self.ensure_range(table, u64::try_from(size).unwrap())?;

        let hash = hash::sysv_hash(name) as usize;
        let bucket_offset = 8usize
            .checked_add((hash % nbuckets) * 4)
            .ok_or_else(|| "live SYSV hash bucket address overflows".to_owned())?;
        let mut index = self.read_u32(
            table
                .checked_add(u64::try_from(bucket_offset).unwrap())
                .ok_or_else(|| "live SYSV hash bucket address overflows".to_owned())?,
        )? as usize;
        let steps = nchain.saturating_add(1).min(MAX_SYSV_CHAIN_STEPS);
        for _ in 0..steps {
            if index == 0 {
                return Ok(LiveResult::Miss);
            }
            if index >= nchain {
                return Err("live SYSV hash index is outside nchain".to_owned());
            }
            let chain_offset = 8usize
                .checked_add(
                    nbuckets
                        .checked_add(index)
                        .and_then(|value| value.checked_mul(4))
                        .ok_or_else(|| "live SYSV hash chain address overflows".to_owned())?,
                )
                .ok_or_else(|| "live SYSV hash chain address overflows".to_owned())?;
            let chain = self.read_u32(
                table
                    .checked_add(u64::try_from(chain_offset).unwrap())
                    .ok_or_else(|| "live SYSV hash chain address overflows".to_owned())?,
            )? as usize;
            if let Some(result) = self.candidate(index, symtab, strtab, strsz, name)? {
                return Ok(result);
            }
            index = chain;
        }
        Err("live SYSV hash chain exceeds the diagnostic step bound".to_owned())
    }

    fn candidate(
        &self,
        index: usize,
        symtab: u64,
        strtab: u64,
        strsz: u64,
        name: &str,
    ) -> Result<Option<LiveResult>, String> {
        let offset = index
            .checked_mul(24)
            .and_then(|value| u64::try_from(value).ok())
            .ok_or_else(|| "live dynsym index overflows".to_owned())?;
        let entry = self.read_array::<24>(
            symtab
                .checked_add(offset)
                .ok_or_else(|| "live dynsym address overflows".to_owned())?,
        )?;
        let name_offset = u32::from_le_bytes(entry[0..4].try_into().expect("4 bytes")) as u64;
        if !self.name_matches(strtab, strsz, name_offset, name.as_bytes())? {
            return Ok(None);
        }
        let st_info = entry[4];
        let st_other = entry[5];
        let st_shndx = u16::from_le_bytes(entry[6..8].try_into().expect("2 bytes"));
        if st_shndx == SHN_ABS {
            return Ok(Some(LiveResult::Inconclusive(
                "absolute symbol value is not rebased by the live diagnostic".to_owned(),
            )));
        }
        if st_shndx == 0 {
            return Ok(Some(LiveResult::Inconclusive(
                "first matching symbol is undefined; later duplicate definitions were not examined"
                    .to_owned(),
            )));
        }
        if st_info >> 4 == 0 {
            return Ok(Some(LiveResult::Inconclusive(
                "first matching symbol is local; later duplicate definitions were not examined"
                    .to_owned(),
            )));
        }
        let visibility = st_other & 3;
        if visibility == 1 {
            return Ok(Some(LiveResult::Inconclusive(
                "first matching symbol is internal; later duplicate definitions were not examined"
                    .to_owned(),
            )));
        }
        if visibility == 2 {
            return Ok(Some(LiveResult::Inconclusive(
                "first matching symbol is hidden; later duplicate definitions were not examined"
                    .to_owned(),
            )));
        }
        if st_info & 0xf == STT_TLS {
            return Ok(Some(LiveResult::Rejected("tls")));
        }
        let value = u64::from_le_bytes(entry[8..16].try_into().expect("8 bytes"));
        if self.binding {
            let kind = st_info & 0xf;
            if !matches!(st_info >> 4, 1 | 2) || !matches!(kind, 0 | 1 | 2)
                || st_shndx >= 0xff00 || st_other & !3 != 0
                || (kind == 2 && value % 4 != 0) {
                return Ok(Some(LiveResult::Rejected("unsupported-export")));
            }
            let size = u64::from_le_bytes(entry[16..24].try_into().expect("8 bytes"));
            let end = value.checked_add(size.max(1))
                .ok_or_else(|| "live export target overflows".to_owned())?;
            let owned = self.view.loads.iter().any(|load| {
                load.flags & (if kind == 2 { PF_X } else { PF_R }) != 0
                    && value >= load.vaddr
                    && load.vaddr.checked_add(load.memsz).is_some_and(|limit| end <= limit)
            });
            if !owned { return Ok(Some(LiveResult::Rejected("outside-owned-target"))); }
        }
        let value =
            usize::try_from(value).map_err(|_| "live symbol value exceeds usize".to_owned())?;
        let address = self
            .base
            .checked_add(value)
            .ok_or_else(|| "live symbol address overflows".to_owned())?;
        Ok(Some(LiveResult::Hit(address)))
    }

    fn name_matches(&self, table: u64, size: u64, offset: u64, name: &[u8]) -> Result<bool, String> {
        if offset >= size {
            return Err("live dynamic string offset is outside DT_STRSZ".to_owned());
        }
        let vaddr = table
            .checked_add(offset)
            .ok_or_else(|| "live dynamic string address overflows".to_owned())?;
        let remaining = size - offset;
        let limit = remaining.min(MAX_NAME_BYTES);
        let load_end = self.readable_load_end(vaddr)?;
        let available = load_end
            .checked_sub(vaddr)
            .ok_or_else(|| "live dynamic string starts outside its PT_LOAD".to_owned())?;
        let len = limit.min(available);
        if len == 0 {
            return Err("live dynamic string has no readable bytes".to_owned());
        }
        let mut bytes = [0u8; 128];
        let mut pos = 0;
        let mut matches = true;
        while pos < len {
            let count = (len - pos).min(bytes.len() as u64) as usize;
            self.read(vaddr.checked_add(pos).ok_or("symbol name address overflow")?, &mut bytes[..count])?;
            let nul = bytes[..count].iter().position(|&byte| byte == 0);
            let part = nul.unwrap_or(count);
            let end = pos as usize + part;
            matches &= name.get(pos as usize..end) == Some(&bytes[..part]);
            if nul.is_some() { return Ok(matches && end == name.len()); }
            pos += count as u64;
        }
        Err("live dynamic string has no NUL within the diagnostic bound".into())
    }

    fn read_array<const N: usize>(&self, vaddr: u64) -> Result<[u8; N], String> {
        let mut bytes = [0u8; N];
        self.read(vaddr, &mut bytes)?;
        Ok(bytes)
    }

    fn read_u32(&self, vaddr: u64) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.read_array::<4>(vaddr)?))
    }

    fn read_u64(&self, vaddr: u64) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.read_array::<8>(vaddr)?))
    }

    fn ensure_range(&self, vaddr: u64, len: u64) -> Result<(), String> {
        let end = vaddr
            .checked_add(len)
            .ok_or_else(|| "live table range overflows".to_owned())?;
        let load = self
            .view
            .loads
            .iter()
            .find(|load| {
                load.flags & PF_R != 0
                    && vaddr >= load.vaddr
                    && load
                        .vaddr
                        .checked_add(load.memsz)
                        .is_some_and(|load_end| end <= load_end)
            })
            .ok_or_else(|| "live table is outside an owned readable PT_LOAD".to_owned())?;
        self.absolute_address(vaddr, len, load.vaddr, load.memsz)
            .map(|_| ())
    }

    fn readable_load_end(&self, vaddr: u64) -> Result<u64, String> {
        let load = self
            .view
            .loads
            .iter()
            .find(|load| {
                load.flags & PF_R != 0
                    && vaddr >= load.vaddr
                    && load
                        .vaddr
                        .checked_add(load.memsz)
                        .is_some_and(|end| vaddr < end)
            })
            .ok_or_else(|| "live address is outside an owned readable PT_LOAD".to_owned())?;
        load.vaddr
            .checked_add(load.memsz)
            .ok_or_else(|| "live PT_LOAD end overflows".to_owned())
    }

    fn read(&self, vaddr: u64, output: &mut [u8]) -> Result<(), String> {
        if output.len() > MAX_READ_BYTES {
            return Err("live read exceeds the diagnostic bound".to_owned());
        }
        let len = u64::try_from(output.len()).map_err(|_| "live read length exceeds u64")?;
        let end = vaddr
            .checked_add(len)
            .ok_or_else(|| "live read range overflows".to_owned())?;
        let load = self
            .view
            .loads
            .iter()
            .find(|load| {
                load.flags & PF_R != 0
                    && vaddr >= load.vaddr
                    && load
                        .vaddr
                        .checked_add(load.memsz)
                        .is_some_and(|load_end| end <= load_end)
            })
            .ok_or_else(|| "live read is outside an owned readable PT_LOAD".to_owned())?;
        let address = self.absolute_address(vaddr, len, load.vaddr, load.memsz)?;
        self.memory
            .read_exact(address, output)
            .map_err(|error| format!("live PT_LOAD read failed: {error}"))
    }

    fn absolute_address(
        &self,
        vaddr: u64,
        len: u64,
        load_vaddr: u64,
        load_memsz: u64,
    ) -> Result<usize, String> {
        let end = vaddr
            .checked_add(len)
            .ok_or_else(|| "live address range overflows".to_owned())?;
        let load_end = load_vaddr
            .checked_add(load_memsz)
            .ok_or_else(|| "live PT_LOAD end overflows".to_owned())?;
        if vaddr < load_vaddr || end > load_end {
            return Err("live address is outside its PT_LOAD".to_owned());
        }
        let relative =
            usize::try_from(vaddr).map_err(|_| "live PT_LOAD address exceeds usize".to_owned())?;
        self.base
            .checked_add(relative)
            .ok_or_else(|| "live absolute address overflows".to_owned())
    }
}

fn tag_value(entries: &[(u64, u64)], tag: u64) -> Option<u64> {
    entries
        .iter()
        .find_map(|(entry_tag, value)| (*entry_tag == tag).then_some(*value))
}

fn required_tag(entries: &[(u64, u64)], tag: u64, name: &str) -> Result<u64, String> {
    tag_value(entries, tag).ok_or_else(|| format!("live dynamic table has no {name}"))
}

fn live_hash(entries: &[(u64, u64)]) -> &'static str {
    if tag_value(entries, DT_GNU_HASH).is_some() {
        "gnu"
    } else if tag_value(entries, DT_HASH).is_some() {
        "sysv"
    } else {
        "none"
    }
}
