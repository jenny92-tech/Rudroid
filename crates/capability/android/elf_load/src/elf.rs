//! ELF64 (AArch64) parsing: headers, program headers, dynamic table,
//! dynamic symbol/string tables and relocation sections. Pure inspection —
//! no memory mapping here (see `GuestModule` in the crate root).
#![forbid(unsafe_code)]

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ElfError {
    TooSmall,
    BadMagic,
    Not64Bit,
    NotLittleEndian,
    WrongMachine(u16),
    TruncatedHeader,
    NoProgramHeaders,
    TruncatedProgramHeaders,
    TruncatedSection(&'static str),
    NoDynamicSegment,
    TruncatedDynamic,
    MissingDynamic(u32, &'static str),
    /// PT_LOAD claims more file bytes than memory bytes (ticket 47).
    FileszExceedsMemsz,
    /// A PT_LOAD extends past the end of the file (ticket 47).
    SegmentOutsideFile,
    /// Header/segment arithmetic overflowed while parsing (ticket 47).
    Overflow(&'static str),
    /// The materialized image span would exceed the loader limit (ticket 47).
    ImageTooLarge(u64),
    /// DT_PLTREL does not name DT_RELA — PLT handling is RELA-only here (48).
    UnsupportedPltRel(u64),
    /// A dynamic-string access left the range described by
    /// DT_STRTAB/DT_STRSZ (ticket 64). The table itself must exist and be
    /// file-backed; offsets and their NUL must stay inside it.
    DynstrBounds(&'static str),
    /// Legacy diagnostic retained for API compatibility; current APS2 failures use
    /// InvalidPackedRelocations after checking the real address/size tags.
    UnsupportedPackedRelocations,
    InvalidPackedRelocations(&'static str),
    /// Exactly one of DT_RELR/DT_RELRSZ (or their Android twins) is present.
    IncompleteRelr,
    /// The RELR table is malformed: bad entry size, empty or unaligned
    /// DT_RELRSZ, or a range no PT_LOAD segment backs.
    InvalidRelr(&'static str),
    /// Exactly one of DT_INIT_ARRAY/DT_INIT_ARRAYSZ is present.
    IncompleteInitArray,
    /// The init-array is malformed: empty or unaligned DT_INIT_ARRAYSZ, or a
    /// range no PT_LOAD segment backs.
    InvalidInitArray(&'static str),
    /// The symbol-version tables are malformed (ticket 86): a versioned
    /// dynsym index with no DT_VERNEED, or a VERNEED walk that leaves the
    /// file. Version support is precise-reject, never silent name binding.
    InvalidVerneed(&'static str),
    /// A hash-table chain walk revisits entries without terminating — the
    /// table is malformed (cyclic), not truncated.
    CyclicHashChain(&'static str),
}

impl std::fmt::Display for ElfError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ElfError::TooSmall => write!(f, "guest ELF smaller than ELF64 header"),
            ElfError::BadMagic => write!(f, "guest ELF magic mismatch (not an ELF)"),
            ElfError::Not64Bit => write!(f, "guest ELF is not ELF64"),
            ElfError::NotLittleEndian => write!(f, "guest ELF is not little-endian"),
            ElfError::WrongMachine(m) => {
                write!(f, "guest ELF target machine 0x{m:x} is not AArch64 (0xb7)")
            }
            ElfError::TruncatedHeader => write!(f, "guest ELF header truncated"),
            ElfError::NoProgramHeaders => write!(f, "guest ELF has no program headers"),
            ElfError::TruncatedProgramHeaders => write!(f, "guest ELF program headers truncated"),
            ElfError::TruncatedSection(name) => write!(f, "guest ELF section `{name}` truncated"),
            ElfError::NoDynamicSegment => write!(f, "guest ELF has no PT_DYNAMIC segment"),
            ElfError::TruncatedDynamic => write!(f, "guest ELF dynamic table truncated"),
            ElfError::MissingDynamic(tag, name) => {
                write!(f, "guest ELF dynamic table misses DT_{name} ({tag})")
            }
            ElfError::FileszExceedsMemsz => {
                write!(f, "guest ELF PT_LOAD has p_filesz > p_memsz (malformed)")
            }
            ElfError::SegmentOutsideFile => {
                write!(f, "guest ELF PT_LOAD file bytes extend past the end of the file")
            }
            ElfError::Overflow(what) => {
                write!(f, "guest ELF {what} arithmetic overflows while parsing")
            }
            ElfError::ImageTooLarge(limit) => write!(
                f,
                "guest ELF memory image span exceeds the {limit}-byte load limit"
            ),
            ElfError::UnsupportedPltRel(value) => write!(
                f,
                "guest ELF DT_PLTREL is {value}, but only DT_RELA PLT relocations are supported"
            ),
            ElfError::DynstrBounds(what) => write!(
                f,
                "guest ELF dynamic string table bounds violated: {what} \
                 (DT_STRTAB/DT_STRSZ)"
            ),
            ElfError::UnsupportedPackedRelocations => write!(
                f,
                "guest ELF uses Android packed sleb128 relocations \
                 (DT_ANDROID_REL/DT_ANDROID_RELA), which this loader does not support"
            ),
            ElfError::InvalidPackedRelocations(reason)=>write!(f,"guest ELF Android packed sleb128 relocations are invalid: {reason}"),
            ElfError::IncompleteRelr => write!(
                f,
                "guest ELF declares DT_RELR without DT_RELRSZ (or vice versa)"
            ),
            ElfError::InvalidRelr(what) => {
                write!(f, "guest ELF RELR table is invalid: {what}")
            }
            ElfError::IncompleteInitArray => write!(
                f,
                "guest ELF declares DT_INIT_ARRAY without DT_INIT_ARRAYSZ (or vice versa)"
            ),
            ElfError::InvalidInitArray(what) => {
                write!(f, "guest ELF init-array is invalid: {what}")
            }
            ElfError::InvalidVerneed(what) => {
                write!(f, "guest ELF symbol-version tables are invalid: {what}")
            }
            ElfError::CyclicHashChain(table) => {
                write!(f, "guest ELF {table} hash chain is cyclic (malformed table)")
            }
        }
    }
}

impl std::error::Error for ElfError {}

pub const EM_AARCH64: u16 = 0xb7;
pub const PT_LOAD: u32 = 1;
pub const PT_DYNAMIC: u32 = 2;

/// p_flags bits (ticket 49: executable backend page permissions).
pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

pub const DT_NULL: u64 = 0;
pub const DT_NEEDED: u64 = 1;
pub const DT_PLTRELSZ: u64 = 2;
pub const DT_HASH: u64 = 4;
pub const DT_STRTAB: u64 = 5;
pub const DT_SYMTAB: u64 = 6;
pub const DT_RELA: u64 = 7;
pub const DT_RELASZ: u64 = 8;
pub const DT_STRSZ: u64 = 10;
pub const DT_SYMENT: u64 = 11;
pub const DT_PLTREL: u64 = 20;
pub const DT_JMPREL: u64 = 23;
/// GNU symbol versioning (ticket 86 rework): the per-symbol version index
/// table and the version-requirement table. Parsed so versioned undefined
/// imports can be refused PRECISELY instead of binding by bare name.
pub const DT_VERSYM: u64 = 0x6fff_fff0;
pub const DT_VERNEED: u64 = 0x6fff_fffe;
pub const DT_VERNEEDNUM: u64 = 0x6fff_ffff;
pub const DT_RELRSZ: u64 = 35;
pub const DT_RELR: u64 = 36;
pub const DT_RELRENT: u64 = 37;
pub const DT_INIT_ARRAY: u64 = 25;
pub const DT_INIT_ARRAYSZ: u64 = 27;
pub const DT_GNU_HASH: u64 = 0x6ffffef5;
/// Transitional Android tags for packed-relative relocations; the payload is
/// identical to DT_RELR (bionic matched these before the generic tags were
/// standardized).
pub const DT_ANDROID_RELR: u64 = 0x6fffe000;
pub const DT_ANDROID_RELRSZ: u64 = 0x6fffe001;
pub const DT_ANDROID_RELRENT: u64 = 0x6fffe003;
/// Android packed sleb128 relocations — decoded by the reference `so_util`,
/// rejected explicitly here (see [`ElfError::UnsupportedPackedRelocations`]).
pub const DT_ANDROID_REL: u64 = 0x6000000f;
pub const DT_ANDROID_RELSZ:u64=0x60000010;
pub const DT_ANDROID_RELA: u64 = 0x60000011;
pub const DT_ANDROID_RELASZ:u64=0x60000012;

pub const R_AARCH64_ABS64: u32 = 257;
pub const R_AARCH64_GLOB_DAT: u32 = 1025;
pub const R_AARCH64_JUMP_SLOT: u32 = 1026;
pub const DT_RELA_TAG_VALUE: u64 = 7; // DT_RELA as a value inside DT_PLTREL
pub const R_AARCH64_RELATIVE: u32 = 1027;

const ELFCLASS64: u8 = 2;
const ELFDATA2LSB: u8 = 1;

/// Upper bound for the materialized memory image (`sum/vmax of p_memsz`).
/// Guest libraries are megabytes; a hostile header claiming gigabytes must
/// fail with a diagnostic instead of allocating (ticket 47).
pub const MAX_IMAGE_SPAN: u64 = 512 * 1024 * 1024;

/// Parsed views into a guest ELF image. All offsets are vaddr-based (the PIE
/// convention); the caller translates vaddr ↔ host pointers via its base.
#[derive(Debug)]
pub struct ElfView<'a> {
    pub bytes: &'a [u8],
    /// PT_LOAD segments: (vaddr, offset, filesz, memsz).
    pub loads: Vec<LoadSegment>,
    /// vaddr of the dynamic symbol table.
    pub symtab: u64,
    /// vaddr of the dynamic string table.
    pub strtab: u64,
    pub strsz: u64,
    /// Number of dynsym entries — derived from hash tables at lookup time.
    pub gnu_hash: Option<u64>,
    pub sysv_hash: Option<u64>,
    /// (vaddr, size in bytes) of DT_RELA and of DT_JMPREL (.rela.plt).
    pub rela: Option<(u64, u64)>,
    plt_rela: Option<(u64, u64)>,
    /// (vaddr, size in bytes) of DT_RELR/DT_ANDROID_RELR (.relr.dyn).
    pub relr: Option<(u64, u64)>,
    pub packed:[Option<(u64,u64)>;2],
    /// (vaddr, size in bytes) of DT_INIT_ARRAY — the constructor pointer
    /// table the executable backend runs after relocations (reference
    /// `so_initialize`; DT_INIT is NOT part of the reference semantics).
    pub init_array: Option<(u64, u64)>,
    /// vaddr of the DT_VERSYM index table (u16 per dynsym entry), when the
    /// module carries GNU symbol versioning (ticket 86).
    pub versym: Option<u64>,
    /// (vaddr, entry count) of the DT_VERNEED requirement table (ticket 86).
    pub verneed: Option<(u64, u64)>,
    /// String-table offsets of DT_NEEDED entries, in declaration order.
    pub needed: Vec<u64>,
}

/// A resolved version requirement (ticket 86): `name` is the version
/// string (e.g. `GLIBC_2.34`) from the matching VERNEED aux entry, `file`
/// the library whose Verneed required it. Binding an import that carries
/// one requires a version-aware provider — identical names do not prove ABI compatibility.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SymbolVersion<'a> {
    pub name: &'a str,
    pub file: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadSegment {
    pub vaddr: u64,
    pub offset: u64,
    pub filesz: u64,
    pub memsz: u64,
    /// Raw p_flags (PF_X|PF_W|PF_R); consumed by the executable backend (49).
    pub flags: u32,
}

fn u16_le(bytes: &[u8], at: usize) -> Result<u16, ElfError> {
    bytes
        .get(at..at + 2)
        .map(|s| u16::from_le_bytes(s.try_into().unwrap()))
        .ok_or(ElfError::TruncatedHeader)
}

fn u32_le(bytes: &[u8], at: usize) -> Result<u32, ElfError> {
    bytes
        .get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or(ElfError::TruncatedHeader)
}

fn u64_le(bytes: &[u8], at: usize) -> Result<u64, ElfError> {
    bytes
        .get(at..at + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or(ElfError::TruncatedHeader)
}

impl<'a> ElfView<'a> {
    /// Validate the header and collect the dynamic layout.
    pub fn parse(bytes: &'a [u8]) -> Result<ElfView<'a>, ElfError> {
        if bytes.len() < 64 {
            return Err(ElfError::TooSmall);
        }
        if bytes[0..4] != [0x7f, b'E', b'L', b'F'] {
            return Err(ElfError::BadMagic);
        }
        if bytes[4] != ELFCLASS64 {
            return Err(ElfError::Not64Bit);
        }
        if bytes[5] != ELFDATA2LSB {
            return Err(ElfError::NotLittleEndian);
        }
        let machine = u16_le(bytes, 18)?;
        if machine != EM_AARCH64 {
            return Err(ElfError::WrongMachine(machine));
        }

        let phoff = u64_le(bytes, 32)?;
        let phentsize = u16_le(bytes, 54)? as usize;
        let phnum = u16_le(bytes, 56)? as usize;
        if phnum == 0 || phentsize < 56 {
            return Err(ElfError::NoProgramHeaders);
        }

        let mut loads = Vec::new();
        let mut dynamic: Option<(u64, u64)> = None; // (vaddr, filesz)
        for index in 0..phnum {
            // Checked entry arithmetic: hostile phoff/phentsize values must
            // produce a diagnostic, never a wrap-around (ticket 47).
            let entry = (phoff as usize).checked_add(index.checked_mul(phentsize).ok_or(
                ElfError::Overflow("program header index"),
            )?);
            let base = match entry {
                Some(base) if base.saturating_add(phentsize) <= bytes.len() => base,
                _ => return Err(ElfError::TruncatedProgramHeaders),
            };
            let p_type = u32_le(bytes, base)?;
            match p_type {
                PT_LOAD => {
                    let segment = LoadSegment {
                        offset: u64_le(bytes, base + 8)?,
                        vaddr: u64_le(bytes, base + 16)?,
                        filesz: u64_le(bytes, base + 32)?,
                        memsz: u64_le(bytes, base + 40)?,
                        flags: u32_le(bytes, base + 4)?,
                    };
                    // Segment sanity (ticket 47): filesz <= memsz, the file
                    // part must exist, and vaddr + memsz must not wrap.
                    if segment.filesz > segment.memsz {
                        return Err(ElfError::FileszExceedsMemsz);
                    }
                    if segment.offset.checked_add(segment.filesz).map(|end| end > bytes.len() as u64) != Some(false)
                    {
                        return Err(ElfError::SegmentOutsideFile);
                    }
                    if segment.vaddr.checked_add(segment.memsz).is_none() {
                        return Err(ElfError::Overflow("PT_LOAD span"));
                    }
                    loads.push(segment);
                }
                PT_DYNAMIC => {
                    dynamic = Some((u64_le(bytes, base + 16)?, u64_le(bytes, base + 32)?));
                }
                _ => {}
            }
        }
        if loads.is_empty() {
            return Err(ElfError::NoProgramHeaders);
        }
        let Some((dyn_vaddr, dyn_filesz)) = dynamic else {
            return Err(ElfError::NoDynamicSegment);
        };

        let mut view = ElfView {
            bytes,
            loads,
            symtab: 0,
            strtab: 0,
            strsz: 0,
            gnu_hash: None,
            sysv_hash: None,
            rela: None,
            plt_rela: None,
            relr: None,
            packed:[None,None],
            init_array: None,
            versym: None,
            verneed: None,
            needed: Vec::new(),
        };
        // The dynamic table is addressed by vaddr; translate through the
        // PT_LOAD segments (p_vaddr == p_offset is common but not required).
        // Entries are collected first: DT_RELA/DT_RELASZ and DT_JMPREL/
        // DT_PLTRELSZ may appear in either order.
        let dynamic_raw = view.vaddr_slice(dyn_vaddr, dyn_filesz)?;
        let mut entries: Vec<(u64, u64)> = Vec::new();
        let mut offset = 0usize;
        while offset + 16 <= dynamic_raw.len() {
            let tag = u64_le(dynamic_raw, offset)?;
            if tag == DT_NULL {
                break;
            }
            let value = u64_le(dynamic_raw, offset + 8)?;
            entries.push((tag, value));
            offset += 16;
        }
        let find = |tag: u64| entries.iter().find(|(t, _)| *t == tag).map(|(_, v)| *v);
        view.needed = entries
            .iter()
            .filter(|(t, _)| *t == DT_NEEDED)
            .map(|(_, v)| *v)
            .collect();
        view.strtab = find(DT_STRTAB).ok_or(ElfError::MissingDynamic(DT_STRTAB as u32, "STRTAB"))?;
        // DT_STRSZ bounds every dynamic-string read (ticket 64): a missing or
        // zero size is a diagnostic — the parser never falls back to scanning
        // the whole file.
        view.strsz = find(DT_STRSZ).ok_or(ElfError::DynstrBounds("DT_STRSZ is missing"))?;
        if view.strsz == 0 {
            return Err(ElfError::DynstrBounds("DT_STRSZ is zero"));
        }
        view.symtab = find(DT_SYMTAB).ok_or(ElfError::MissingDynamic(DT_SYMTAB as u32, "SYMTAB"))?;
        view.sysv_hash = find(DT_HASH);
        view.gnu_hash = find(DT_GNU_HASH);
        if let (Some(addr), Some(size)) = (find(DT_RELA), find(DT_RELASZ)) {
            view.rela = Some((addr, size));
        }
        if let (Some(addr), Some(size)) = (find(DT_JMPREL), find(DT_PLTRELSZ)) {
            view.plt_rela = Some((addr, size));
        }
        if let Some(pltrel) = find(DT_PLTREL) {
            if pltrel != DT_RELA_TAG_VALUE {
                return Err(ElfError::UnsupportedPltRel(pltrel));
            }
        }
        // Android APS2 uses four distinct tags; real RELA is 0x60000011, not RELSZ.
        for (index,(address_tag,size_tag)) in [(DT_ANDROID_REL,DT_ANDROID_RELSZ),(DT_ANDROID_RELA,DT_ANDROID_RELASZ)].into_iter().enumerate(){
            match (find(address_tag),find(size_tag)){
                (None,None)=>{},(Some(address),Some(size))=>{let bytes=view.vaddr_slice(address,size).map_err(|_|ElfError::InvalidPackedRelocations("table range is not file-backed"))?;super::packed::Decoder::new(bytes,index==1).map_err(ElfError::InvalidPackedRelocations)?;view.packed[index]=Some((address,size));},_=>return Err(ElfError::InvalidPackedRelocations("address/size tag pair is incomplete")),
            }
        }
        let relr_addr = find(DT_RELR).or(find(DT_ANDROID_RELR));
        let relr_size = find(DT_RELRSZ).or(find(DT_ANDROID_RELRSZ));
        match (relr_addr, relr_size) {
            (Some(addr), Some(size)) => {
                if find(DT_RELRENT)
                    .or(find(DT_ANDROID_RELRENT))
                    .is_some_and(|entry_size| entry_size != 8)
                {
                    return Err(ElfError::InvalidRelr("entry size is not 8"));
                }
                if size == 0 || size % 8 != 0 {
                    return Err(ElfError::InvalidRelr(
                        "DT_RELRSZ is zero or not word-aligned",
                    ));
                }
                // The table must be backed by a PT_LOAD file range so the
                // decode reads only validated bytes (same posture as the
                // DT_STRTAB check below).
                view.vaddr_slice(addr, size)
                    .map_err(|_| ElfError::InvalidRelr("table range is not file-backed"))?;
                view.relr = Some((addr, size));
            }
            (None, None) => {}
            _ => return Err(ElfError::IncompleteRelr),
        }
        // The constructor pointer table (.init_array): the executable
        // backend runs it after relocations, reference `so_initialize`
        // semantics. Same half-pair / unaligned / not-file-backed posture
        // as RELR above.
        match (find(DT_INIT_ARRAY), find(DT_INIT_ARRAYSZ)) {
            (Some(addr), Some(size)) => {
                if size == 0 || size % 8 != 0 {
                    return Err(ElfError::InvalidInitArray(
                        "DT_INIT_ARRAYSZ is zero or not word-aligned",
                    ));
                }
                view.vaddr_slice(addr, size)
                    .map_err(|_| ElfError::InvalidInitArray("table range is not file-backed"))?;
                view.init_array = Some((addr, size));
            }
            (None, None) => {}
            _ => return Err(ElfError::IncompleteInitArray),
        }
        // GNU symbol versioning (ticket 86 rework): record the tables when
        // present, including Android/Bionic LIBC requirements. Acceptance
        // belongs to the relocation backend's version-aware provider, not
        // this parser; no tables means the ordinary name lookup path.
        if let Some(addr) = find(DT_VERSYM) {
            view.versym = Some(addr);
        }
        match (find(DT_VERNEED), find(DT_VERNEEDNUM)) {
            (Some(addr), Some(count)) => {
                if count == 0 {
                    return Err(ElfError::InvalidVerneed("DT_VERNEEDNUM is zero"));
                }
                view.verneed = Some((addr, count));
            }
            (None, None) => {}
            _ => return Err(ElfError::InvalidVerneed("exactly one of DT_VERNEED/DT_VERNEEDNUM")),
        }
        // The declared string table must be backed by a PT_LOAD file range
        // (ticket 64); string reads slice only these bytes, never scan.
        // Ticket 67: this is the single range selection shared with `str_at`
        // — under overlapping PT_LOADs the first segment in program-header
        // order whose file-backed part covers the WHOLE table is used for
        // validation, reads and diagnostics alike.
        view.vaddr_slice(view.strtab, view.strsz)
            .map_err(|_| ElfError::DynstrBounds("table range is not file-backed"))?;
        Ok(view)
    }

    /// Translate a vaddr to a raw file offset through the PT_LOAD segments
    /// (ticket 48): DT_SYMTAB/DT_STRTAB and friends carry virtual addresses;
    /// treating them as file offsets only works for identity-mapped images.
    /// The vaddr must lie inside the file-backed part of one segment.
    pub fn vaddr_to_file_offset(&self, vaddr: u64) -> Result<usize, ElfError> {
        for load in &self.loads {
            let file_end = load
                .vaddr
                .checked_add(load.filesz)
                .ok_or(ElfError::Overflow("PT_LOAD span"))?;
            if vaddr >= load.vaddr && vaddr < file_end {
                return Ok((load.offset as usize)
                    .checked_add((vaddr - load.vaddr) as usize)
                    .ok_or(ElfError::Overflow("file offset"))?);
            }
        }
        Err(ElfError::TruncatedSection("vaddr translation"))
    }

    /// File offset of dynsym entry `index` (ticket 48: vaddr translated).
    pub fn symtab_entry_offset(&self, index: usize) -> Result<usize, ElfError> {
        let entry = 24usize;
        let table = self.vaddr_to_file_offset(self.symtab)?;
        table
            .checked_add(index.checked_mul(entry).ok_or(ElfError::TruncatedSection("dynsym"))?)
            .ok_or(ElfError::TruncatedSection("dynsym"))
    }

    /// The DT_NEEDED entries as strings (ticket 48).
    pub fn needed_names(&self) -> Result<Vec<&'a str>, ElfError> {
        self.needed.iter().map(|off| self.str_at(*off)).collect()
    }

    /// Translate a vaddr to a slice into the raw file bytes. All segment
    /// arithmetic is checked; hostile vaddr/len values produce diagnostics
    /// instead of wrapping around (ticket 47).
    pub fn vaddr_slice(&self, vaddr: u64, len: u64) -> Result<&'a [u8], ElfError> {
        let end = vaddr.checked_add(len).ok_or(ElfError::Overflow("vaddr range"))?;
        for load in &self.loads {
            let file_end = load
                .vaddr
                .checked_add(load.filesz)
                .ok_or(ElfError::Overflow("PT_LOAD span"))?;
            if vaddr >= load.vaddr && end <= file_end {
                let start = (load.offset as usize)
                    .checked_add((vaddr - load.vaddr) as usize)
                    .ok_or(ElfError::Overflow("file offset"))?;
                return self
                    .bytes
                    .get(start..start.checked_add(len as usize).ok_or(ElfError::Overflow("file offset"))?)
                    .ok_or(ElfError::TruncatedSection("vaddr range"));
            }
        }
        Err(ElfError::TruncatedSection("vaddr range"))
    }

    /// Read one dynamic symbol: (name, value, shndx). DT_SYMTAB is a virtual
    /// address and is translated through the PT_LOAD segments (ticket 48).
    pub fn dynsym(&self, index: usize) -> Result<(&'a str, u64, u16), ElfError> {
        let base = self.symtab_entry_offset(index)?;
        if base.checked_add(24).map(|end| end > self.bytes.len()) != Some(false) {
            return Err(ElfError::TruncatedSection("dynsym"));
        }
        let st_name = u32_le(self.bytes, base)? as u64;
        let st_value = u64_le(self.bytes, base + 8)?;
        let st_shndx = u16_le(self.bytes, base + 6)?;
        let name = self.str_at(st_name)?;
        Ok((name, st_value, st_shndx))
    }

    /// Look `name` up in the section-header symbol tables (`SHT_SYMTAB`,
    /// i.e. `.symtab` of an unstripped file) — the reference
    /// `so_symbol_all` fallback. Mirrors its walk and bounds checks: the
    /// section-header table, each SHT_SYMTAB section and its `sh_link`
    /// SHT_STRTAB must lie inside the file; undefined entries and names
    /// outside the string table are skipped; the first defined match wins.
    /// Additionally a name must end (NUL) inside its string table.
    /// Returns `(st_value, st_info)`; `None` on a miss or a stripped file.
    pub fn section_symtab_lookup(&self, name: &str) -> Option<(u64, u8)> {
        const SHDR_SIZE: usize = 64;
        const SYM_SIZE: usize = 24;
        const SHT_SYMTAB: u32 = 2;
        const SHT_STRTAB: u32 = 3;
        let bytes = self.bytes;
        let file_size = bytes.len();
        let e_shoff = usize::try_from(u64_le(bytes, 0x28).ok()?).ok()?;
        let e_shnum = u16_le(bytes, 0x3c).ok()? as usize;
        let shdr_bytes = e_shnum * SHDR_SIZE;
        if e_shoff > file_size || shdr_bytes > file_size - e_shoff {
            return None;
        }
        // (sh_type, sh_offset, sh_size, sh_link) of section `index`.
        let section = |index: usize| -> Option<(u32, usize, usize, usize)> {
            let at = e_shoff + index * SHDR_SIZE;
            let sh_type = u32_le(bytes, at + 4).ok()?;
            let sh_offset = usize::try_from(u64_le(bytes, at + 24).ok()?).ok()?;
            let sh_size = usize::try_from(u64_le(bytes, at + 32).ok()?).ok()?;
            let sh_link = u32_le(bytes, at + 40).ok()? as usize;
            Some((sh_type, sh_offset, sh_size, sh_link))
        };
        for index in 0..e_shnum {
            let Some((sh_type, sh_offset, sh_size, sh_link)) = section(index) else {
                continue;
            };
            if sh_type != SHT_SYMTAB
                || sh_offset > file_size
                || sh_size > file_size - sh_offset
                || sh_link >= e_shnum
            {
                continue;
            }
            let Some((str_type, str_offset, str_size, _)) = section(sh_link) else {
                continue;
            };
            if str_type != SHT_STRTAB
                || str_offset > file_size
                || str_size > file_size - str_offset
            {
                continue;
            }
            let strings = &bytes[str_offset..str_offset + str_size];
            for n in 0..sh_size / SYM_SIZE {
                let at = sh_offset + n * SYM_SIZE;
                let (Ok(st_name), Ok(st_shndx)) = (u32_le(bytes, at), u16_le(bytes, at + 6)) else {
                    break;
                };
                let st_name = st_name as usize;
                if st_shndx == 0 || st_name >= str_size {
                    continue;
                }
                let tail = &strings[st_name..];
                let Some(len) = tail.iter().position(|b| *b == 0) else {
                    continue;
                };
                if &tail[..len] == name.as_bytes() {
                    let st_value = u64_le(bytes, at + 8).ok()?;
                    return Some((st_value, bytes[at + 4]));
                }
            }
        }
        None
    }

    /// The version requirement attached to dynamic symbol `index`, when the
    /// module carries GNU symbol versioning (ticket 86 rework).
    ///
    /// `Ok(None)` means "no requirement": no DT_VERSYM table at all, or the
    /// index values 0 (LOCAL) / 1 (`*global*` baseline) which carry none.
    /// `Ok(Some(_))` names the version and the file whose VERNEED entry
    /// requires it — the relocation layer must then REFUSE to bind that
    /// import by bare name instead of guessing which implementation the
    /// version belongs to. `Err` marks malformed version tables.
    pub fn symbol_version(&self, index: usize) -> Result<Option<SymbolVersion<'a>>, ElfError> {
        let Some(versym) = self.versym else {
            return Ok(None);
        };
        let table = self.vaddr_to_file_offset(versym)?;
        let at = table
            .checked_add(index.checked_mul(2).ok_or(ElfError::Overflow("versym index"))?)
            .ok_or(ElfError::TruncatedSection("versym"))?;
        let raw = self
            .bytes
            .get(at..at + 2)
            .ok_or(ElfError::TruncatedSection("versym"))?;
        let version_index =
            (u16::from_le_bytes(raw.try_into().expect("2 bytes")) & 0x7fff) as usize;
        if version_index <= 1 {
            return Ok(None);
        }
        // Walk the VERNEED list: Verneed { vn_version: u16, vn_cnt: u16,
        // vn_file: u32, vn_aux: u32, vn_next: u32 }, then its Vernaux list
        // { vna_hash: u32, vna_flags: u16, vna_other: u16, vna_name: u32,
        // vna_next: u32 }. The aux `vna_other` carries the version index the
        // dynsym VERSYM entries point at (hidden bit masked off).
        let Some((verneed, count)) = self.verneed else {
            return Err(ElfError::InvalidVerneed(
                "versioned dynsym index but no DT_VERNEED",
            ));
        };
        let mut need_offset = self.vaddr_to_file_offset(verneed)?;
        // A Verneed entry is 16 bytes: the file bounds the walk, so a
        // hostile DT_VERNEEDNUM cannot spin past the image.
        for _ in 0..count.min(self.bytes.len() as u64 / 16) {
            let need = self
                .bytes
                .get(need_offset..need_offset + 16)
                .ok_or(ElfError::InvalidVerneed("Verneed entry leaves the file"))?;
            let read32 = |at: usize| u32::from_le_bytes(need[at..at + 4].try_into().expect("4"));
            let vn_cnt = u16::from_le_bytes(need[2..4].try_into().expect("2")) as usize;
            let vn_file = read32(4);
            let vn_aux = read32(8) as usize;
            let vn_next = read32(12);
            let file = self.str_at(vn_file as u64)?;
            let mut aux_offset = need_offset
                .checked_add(vn_aux)
                .ok_or(ElfError::InvalidVerneed("Verneed aux offset overflows"))?;
            for _ in 0..vn_cnt {
                let aux = self
                    .bytes
                    .get(aux_offset..aux_offset + 16)
                    .ok_or(ElfError::InvalidVerneed("Vernaux entry leaves the file"))?;
                let a32 = |at: usize| u32::from_le_bytes(aux[at..at + 4].try_into().expect("4"));
                let vna_other = u16::from_le_bytes(aux[6..8].try_into().expect("2")) & 0x7fff;
                let vna_name = a32(8);
                let vna_next = a32(12);
                if vna_other as usize == version_index {
                    let name = self.str_at(vna_name as u64)?;
                    return Ok(Some(SymbolVersion { name, file }));
                }
                if vna_next == 0 {
                    break;
                }
                aux_offset = aux_offset
                    .checked_add(vna_next as usize)
                    .ok_or(ElfError::InvalidVerneed("Vernaux next overflows"))?;
            }
            if vn_next == 0 {
                break;
            }
            need_offset = need_offset
                .checked_add(vn_next as usize)
                .ok_or(ElfError::InvalidVerneed("Verneed next overflows"))?;
        }
        Err(ElfError::InvalidVerneed(
            "no VERNEED entry matches the versioned dynsym index",
        ))
    }

    fn str_at(&self, offset: u64) -> Result<&'a str, ElfError> {
        // Every dynamic-string read stays inside the DT_STRTAB/DT_STRSZ range
        // (ticket 64): the offset must precede the table end and the NUL must
        // appear within it — the scan never leaves the declared table.
        if offset >= self.strsz {
            return Err(ElfError::DynstrBounds("string offset at or past DT_STRSZ"));
        }
        // Ticket 67: index the exact bytes that parse validated. `vaddr_slice`
        // applies ONE deterministic PT_LOAD choice — the first segment in
        // program-header order whose file-backed part covers the whole
        // DT_STRTAB..DT_STRSZ range — and that same choice is shared by the
        // parse-time table check, every string read (dynsym and DT_NEEDED)
        // and the diagnostics. Re-translating only the table start here would
        // let an earlier overlapping segment win and read bytes that were
        // never validated as a whole table.
        let table = self
            .vaddr_slice(self.strtab, self.strsz)
            .map_err(|_| ElfError::DynstrBounds("table range is not file-backed"))?;
        let remaining = &table[offset as usize..];
        let len = remaining
            .iter()
            .position(|&b| b == 0)
            .ok_or(ElfError::DynstrBounds("no NUL inside DT_STRSZ"))?;
        std::str::from_utf8(&remaining[..len])
            .map_err(|_| ElfError::TruncatedSection("dynstr utf8"))
    }

    /// Borrow and decode DT_RELA, then DT_JMPREL, one entry at a time with
    /// constant storage. Validate BOTH complete file ranges before returning
    /// the iterator, so an invalid PLT table cannot cause partial writes.
    pub fn rela_iter(&self) -> Result<impl Iterator<Item = (u64, u32, usize, i64)> + 'a, ElfError> {
        let mut raw_tables: [&'a [u8]; 2] = [&[], &[]];
        for (raw, table) in raw_tables.iter_mut().zip([self.rela, self.plt_rela]) {
            if let Some((addr, size)) = table {
                *raw = self.vaddr_slice(addr, size)?;
            }
        }
        Ok(raw_tables.into_iter().flat_map(|raw| raw.chunks_exact(24)).map(|chunk| {
            // chunks_exact guarantees all fields fit; no pointers into the
            // executable image are read while relocation writes proceed.
            let offset = u64::from_le_bytes(chunk[0..8].try_into().unwrap());
            let info = u64::from_le_bytes(chunk[8..16].try_into().unwrap());
            let addend = i64::from_le_bytes(chunk[16..24].try_into().unwrap());
            (offset, info as u32, (info >> 32) as usize, addend)
        }))
    }

    /// Owned snapshot for inspection callers that explicitly need one.
    /// Production relocation uses `rela_iter` and never expands the table.
    pub fn rela_entries(&self) -> Result<Vec<(u64, u32, usize, i64)>, ElfError> {
        let entries = self.rela_iter()?;
        // Both ranges were validated above; reserve their exact entry count
        // only for callers requesting an owned snapshot.
        let capacity = self.rela.into_iter().chain(self.plt_rela)
            .map(|(_, size)| size as usize / 24).sum();
        let mut out = Vec::with_capacity(capacity);
        out.extend(entries);
        Ok(out)
    }

    /// Total memory image span: max(vaddr + memsz) of PT_LOAD segments,
    /// checked against arithmetic overflow and [`MAX_IMAGE_SPAN`] (ticket 47).
    pub fn image_span(&self) -> Result<u64, ElfError> {
        let mut span = 0u64;
        for load in &self.loads {
            let end = load
                .vaddr
                .checked_add(load.memsz)
                .ok_or(ElfError::Overflow("PT_LOAD span"))?;
            if end > span {
                span = end;
            }
        }
        if span > MAX_IMAGE_SPAN {
            return Err(ElfError::ImageTooLarge(MAX_IMAGE_SPAN));
        }
        Ok(span)
    }
}

// Private companion sections are unnecessary; DT order independence is
// handled inline during parse via `rela_size`.

#[cfg(test)]
#[path = "../tests/unit/elf.rs"]
mod tests;
