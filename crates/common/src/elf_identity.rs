//! INPUT:  borrowed immutable ELF64 AArch64 file and bounded program/note tables
//! OUTPUT:  file length and optional unique GNU build identity without moving its cursor
//! POS:    safe cold file preflight; does not map, relocate or execute native code
#![forbid(unsafe_code)]
use std::{
    fs::File,
    io::{self, ErrorKind},
    os::unix::fs::FileExt,
};

#[derive(Debug, PartialEq, Eq)]
pub struct ElfIdentity {
    pub file_len: u64,
    pub build_id: Option<Vec<u8>>,
}

/// Coarse libc ABI classification for an AArch64 ELF shared library (`ET_DYN`),
/// inferred from its `DT_NEEDED` entries (`libc.so` vs `libc.so.6`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfAbiClass {
    AndroidBionic,
    LinuxGnu,
    Ambiguous,
}
fn bad() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidData,
        "unsupported or malformed AArch64 ELF identity",
    )
}
fn u16_(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

pub fn aarch64_identity(file: &File) -> io::Result<ElfIdentity> {
    aarch64_identity_kind(file, false)
}

/// Like [`aarch64_identity`], but requires an ELF shared object (`ET_DYN`).
pub fn aarch64_dyn_identity(file: &File) -> io::Result<ElfIdentity> {
    aarch64_identity_kind(file, true)
}

fn aarch64_identity_kind(file: &File, dyn_only: bool) -> io::Result<ElfIdentity> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(bad());
    }
    let mut header = [0; 64];
    file.read_exact_at(&mut header, 0)?;
    let etype = u16_(&header, 16);
    let valid_type = if dyn_only {
        etype == 3
    } else {
        matches!(etype, 2 | 3)
    };
    if &header[..7] != b"\x7fELF\x02\x01\x01"
        || u16_(&header, 18) != 183
        || !valid_type
        || u32_(&header, 20) != 1
    {
        return Err(bad());
    }
    let (offset, stride, count) = (
        u64_(&header, 32),
        usize::from(u16_(&header, 54)),
        usize::from(u16_(&header, 56)),
    );
    if !(56..=256).contains(&stride) || !(1..=128).contains(&count) {
        return Err(bad());
    }
    let size = stride.checked_mul(count).ok_or_else(bad)?;
    if offset
        .checked_add(size as u64)
        .is_none_or(|end| end > meta.len())
    {
        return Err(bad());
    }
    let mut programs = vec![0; size];
    file.read_exact_at(&mut programs, offset)?;
    let mut found = None;
    let mut note_budget = 64 * 1024usize;
    for p in programs.chunks_exact(stride) {
        if u32_(p, 0) != 4 {
            continue;
        }
        let (at, len) = (u64_(p, 8), u64_(p, 32));
        let len = usize::try_from(len).map_err(|_| bad())?;
        if len > note_budget
            || at
                .checked_add(len as u64)
                .is_none_or(|end| end > meta.len())
        {
            return Err(bad());
        }
        note_budget -= len;
        let mut notes = vec![0; len];
        file.read_exact_at(&mut notes, at)?;
        let mut cursor = 0usize;
        while cursor < notes.len() {
            if notes.len() - cursor < 12 {
                if notes[cursor..].iter().any(|v| *v != 0) {
                    return Err(bad());
                }
                break;
            }
            let (namesz, descsz, kind) = (
                u32_(&notes, cursor) as usize,
                u32_(&notes, cursor + 4) as usize,
                u32_(&notes, cursor + 8),
            );
            let name = cursor + 12;
            let desc = name
                .checked_add(namesz.checked_add(3).ok_or_else(bad)? & !3)
                .ok_or_else(bad)?;
            let next = desc
                .checked_add(descsz.checked_add(3).ok_or_else(bad)? & !3)
                .ok_or_else(bad)?;
            if next > notes.len() {
                return Err(bad());
            }
            if kind == 3 && namesz == 4 && &notes[name..name + namesz] == b"GNU\0" {
                if found.is_some() || !(1..=64).contains(&descsz) {
                    return Err(bad());
                }
                found = Some(notes[desc..desc + descsz].to_vec());
            }
            cursor = next;
        }
    }
    Ok(ElfIdentity {
        file_len: meta.len(),
        build_id: found,
    })
}

/// Inspect an AArch64 `ET_DYN` library's `PT_DYNAMIC` `DT_NEEDED` entries to
/// distinguish Android Bionic (`libc.so`) from Linux GNU (`libc.so.6`).
pub fn aarch64_dyn_abi_class(file: &File) -> io::Result<ElfAbiClass> {
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(bad());
    }
    let mut header = [0; 64];
    file.read_exact_at(&mut header, 0)?;
    if &header[..7] != b"\x7fELF\x02\x01\x01"
        || u16_(&header, 16) != 3
        || u16_(&header, 18) != 183
        || u32_(&header, 20) != 1
    {
        return Err(bad());
    }
    let (offset, stride, count) = (
        u64_(&header, 32),
        usize::from(u16_(&header, 54)),
        usize::from(u16_(&header, 56)),
    );
    if !(56..=256).contains(&stride) || !(1..=128).contains(&count) {
        return Err(bad());
    }
    let size = stride.checked_mul(count).ok_or_else(bad)?;
    if offset
        .checked_add(size as u64)
        .is_none_or(|end| end > meta.len())
    {
        return Err(bad());
    }
    let mut programs = vec![0; size];
    file.read_exact_at(&mut programs, offset)?;
    let mut loads: Vec<(u64, u64, u64)> = Vec::new();
    let mut dynamic: Option<(u64, usize)> = None;
    for p in programs.chunks_exact(stride) {
        match u32_(p, 0) {
            1 => {
                // PT_LOAD: (p_vaddr, p_offset, p_filesz)
                let (p_off, p_vaddr, p_filesz) = (u64_(p, 8), u64_(p, 16), u64_(p, 32));
                if p_off
                    .checked_add(p_filesz)
                    .is_some_and(|end| end <= meta.len())
                {
                    loads.push((p_vaddr, p_off, p_filesz));
                }
            }
            2 => {
                // PT_DYNAMIC
                let (at, len) = (u64_(p, 8), u64_(p, 32));
                let len = usize::try_from(len).map_err(|_| bad())?;
                if len > 256 * 1024
                    || at
                        .checked_add(len as u64)
                        .is_none_or(|end| end > meta.len())
                {
                    return Err(bad());
                }
                dynamic = Some((at, len));
            }
            _ => {}
        }
    }
    let Some((dyn_off, dyn_len)) = dynamic else {
        return Ok(ElfAbiClass::Ambiguous);
    };
    let mut dyn_bytes = vec![0; dyn_len];
    file.read_exact_at(&mut dyn_bytes, dyn_off)?;
    let mut strtab_vaddr: Option<u64> = None;
    let mut strsz: Option<usize> = None;
    let mut needed: Vec<u64> = Vec::new();
    for entry in dyn_bytes.chunks_exact(16) {
        let tag = u64_(entry, 0);
        let val = u64_(entry, 8);
        match tag {
            0 => break,
            1 => {
                if needed.len() < 256 {
                    needed.push(val);
                }
            }
            5 => strtab_vaddr = Some(val),
            10 => strsz = usize::try_from(val).ok(),
            _ => {}
        }
    }
    let (Some(strtab_vaddr), Some(strsz)) = (strtab_vaddr, strsz) else {
        return Ok(ElfAbiClass::Ambiguous);
    };
    if strsz == 0 || strsz > 4 * 1024 * 1024 {
        return Ok(ElfAbiClass::Ambiguous);
    }
    let strtab_file_off = loads.iter().find_map(|&(vaddr, off, filesz)| {
        let delta = strtab_vaddr.checked_sub(vaddr)?;
        let end = delta.checked_add(strsz as u64)?;
        (end <= filesz).then_some(off.checked_add(delta)?)
    });
    let Some(strtab_file_off) = strtab_file_off else {
        return Ok(ElfAbiClass::Ambiguous);
    };
    let mut strtab = vec![0; strsz];
    file.read_exact_at(&mut strtab, strtab_file_off)?;
    let mut saw_bionic = false;
    let mut saw_gnu = false;
    for name_off in needed {
        let Ok(start) = usize::try_from(name_off) else {
            continue;
        };
        if start >= strtab.len() {
            continue;
        }
        let Some(rel_end) = strtab[start..].iter().position(|&b| b == 0) else {
            continue;
        };
        let soname = &strtab[start..start + rel_end];
        if matches!(soname, b"libc.so" | b"liblog.so" | b"libandroid.so") {
            saw_bionic = true;
        } else if matches!(soname, b"libc.so.6" | b"ld-linux-aarch64.so.1") {
            saw_gnu = true;
        }
    }
    Ok(match (saw_bionic, saw_gnu) {
        (true, false) => ElfAbiClass::AndroidBionic,
        (false, true) => ElfAbiClass::LinuxGnu,
        _ => ElfAbiClass::Ambiguous,
    })
}

#[cfg(test)]
#[path = "../tests/unit/elf_identity.rs"]
mod tests;
