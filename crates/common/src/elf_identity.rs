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
    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(bad());
    }
    let mut header = [0; 64];
    file.read_exact_at(&mut header, 0)?;
    if &header[..7] != b"\x7fELF\x02\x01\x01"
        || u16_(&header, 18) != 183
        || !matches!(u16_(&header, 16), 2 | 3)
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

#[cfg(test)]
#[path = "../tests/unit/elf_identity.rs"]
mod tests;
