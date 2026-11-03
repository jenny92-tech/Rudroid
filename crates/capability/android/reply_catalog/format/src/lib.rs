//! Reply Catalog binary format: little-endian, versioned, rebuild-on-change.
//!
//! Lives in its own dependency-free crate so both the catalog cdylib and its
//! build script can encode/decode the blob (`[build-dependencies]` cannot
//! link the crate being built). No compat matrix: the blob is rebuilt from the
//! in-repo source table on every change and distributed same-build.
//!
//! This crate is the shared plain library of the catalog domain: besides the
//! blob codec it hosts the pure query model (`Catalog`, module `catalog`) and
//! the `ReplyValue` type. Capability crates depend on it for those types —
//! never on the `reply_catalog` capability rlib, which must stay behind its
//! own dynamic C API (ticket 43).
//!
//! Layout v1 (all integers little-endian):
//!
//! ```text
//! offset  size                field
//! 0       4                   magic "BDRC"
//! 4       4                   format version (u32, currently 1)
//! 8       4                   entry count (u32)
//! 12      4                   string pool size in bytes (u32)
//! 16      string_pool_size    string pool (UTF-8, offsets relative to pool start)
//! +0      entry_count * 44    entry records:
//!                             class_off u32, class_len u32,
//!                             method_off u32, method_len u32,
//!                             sig_off u32, sig_len u32,
//!                             value_kind u32 (0=bool, 1=int, 2=str),
//!                             value payload 16 bytes:
//!                               i64 (bool 0/1 or int value; unused for str)
//!                               u32 str_off, u32 str_len (str only)
//! ```
#![forbid(unsafe_code)]

/// A fixed reply value stored in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplyValue {
    Bool(bool),
    Int(i64),
    Str(String),
}

pub mod catalog;
pub mod source;

pub use catalog::{Catalog, CatalogLoadError};

pub const MAGIC: [u8; 4] = *b"BDRC";
pub const FORMAT_VERSION: u32 = 1;
pub const ENTRY_SIZE: usize = 44;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    TooShort,
    BadMagic,
    UnsupportedVersion(u32),
    TruncatedPool,
    TruncatedEntries { expected: usize, got: usize },
    InvalidUtf8,
    UnknownValueKind(u32),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::TooShort => write!(f, "catalog blob too short for header"),
            DecodeError::BadMagic => write!(f, "catalog blob magic mismatch"),
            DecodeError::UnsupportedVersion(v) => {
                write!(f, "catalog blob version {v} not supported (want {FORMAT_VERSION})")
            }
            DecodeError::TruncatedPool => write!(f, "catalog blob string pool truncated"),
            DecodeError::TruncatedEntries { expected, got } => {
                write!(f, "catalog blob entries truncated: want {expected}, got {got}")
            }
            DecodeError::InvalidUtf8 => write!(f, "catalog blob string pool is not UTF-8"),
            DecodeError::UnknownValueKind(kind) => write!(f, "unknown catalog value kind {kind}"),
        }
    }
}

impl std::error::Error for DecodeError {}

struct StringPool<'a> {
    bytes: &'a [u8],
}

impl<'a> StringPool<'a> {
    fn get(&self, offset: u32, len: u32) -> Result<&'a str, DecodeError> {
        let start = offset as usize;
        let end = start.checked_add(len as usize).ok_or(DecodeError::TruncatedPool)?;
        let bytes = self.bytes.get(start..end).ok_or(DecodeError::TruncatedPool)?;
        std::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8)
    }
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("slice len checked"))
}

fn read_i64(bytes: &[u8], offset: usize) -> i64 {
    i64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("slice len checked"))
}

/// One catalog key/value used while encoding.
pub struct Entry<'a> {
    pub class: &'a str,
    pub method: &'a str,
    pub sig: &'a str,
    pub value: ReplyValue,
}

struct PoolBuilder {
    bytes: Vec<u8>,
}

impl PoolBuilder {
    fn push(&mut self, text: &str) -> (u32, u32) {
        let offset = self.bytes.len() as u32;
        self.bytes.extend_from_slice(text.as_bytes());
        (offset, text.len() as u32)
    }
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn push_str_entry(pool: &mut PoolBuilder, text: &str, out: &mut Vec<u8>) {
    let (off, len) = pool.push(text);
    write_u32(out, off);
    write_u32(out, len);
}

/// Encode entries into the v1 blob. `Entry` order is preserved so re-encoding
/// the same source table is byte-identical (rebuild-on-change, deterministic).
pub fn encode(entries: &[Entry<'_>]) -> Vec<u8> {
    let mut pool = PoolBuilder { bytes: Vec::new() };
    let mut records = Vec::with_capacity(entries.len() * ENTRY_SIZE);
    for entry in entries {
        push_str_entry(&mut pool, entry.class, &mut records);
        push_str_entry(&mut pool, entry.method, &mut records);
        push_str_entry(&mut pool, entry.sig, &mut records);
        let (kind, int_value, str_ref) = match &entry.value {
            ReplyValue::Bool(b) => (0u32, *b as i64, None),
            ReplyValue::Int(i) => (1u32, *i, None),
            ReplyValue::Str(s) => (2u32, 0, Some(s.as_str())),
        };
        write_u32(&mut records, kind);
        write_i64(&mut records, int_value);
        match str_ref {
            Some(text) => {
                let (off, len) = pool.push(text);
                write_u32(&mut records, off);
                write_u32(&mut records, len);
            }
            None => {
                write_u32(&mut records, 0);
                write_u32(&mut records, 0);
            }
        }
    }

    let mut out = Vec::with_capacity(16 + pool.bytes.len() + records.len());
    out.extend_from_slice(&MAGIC);
    write_u32(&mut out, FORMAT_VERSION);
    write_u32(&mut out, entries.len() as u32);
    write_u32(&mut out, pool.bytes.len() as u32);
    out.extend_from_slice(&pool.bytes);
    out.extend_from_slice(&records);
    out
}

/// Decode a v1 blob into `(class, method, sig) -> ReplyValue` pairs.
pub fn decode(bytes: &[u8]) -> Result<Vec<(&str, &str, &str, ReplyValue)>, DecodeError> {
    if bytes.len() < 16 {
        return Err(DecodeError::TooShort);
    }
    if bytes[0..4] != MAGIC {
        return Err(DecodeError::BadMagic);
    }
    let version = read_u32(bytes, 4);
    if version != FORMAT_VERSION {
        return Err(DecodeError::UnsupportedVersion(version));
    }
    let entry_count = read_u32(bytes, 8) as usize;
    let pool_size = read_u32(bytes, 12) as usize;
    let pool_end = 16usize
        .checked_add(pool_size)
        .ok_or(DecodeError::TruncatedPool)?;
    if bytes.len() < pool_end {
        return Err(DecodeError::TruncatedPool);
    }
    let pool = StringPool {
        bytes: &bytes[16..pool_end],
    };
    let records = &bytes[pool_end..];
    if records.len() < entry_count * ENTRY_SIZE {
        return Err(DecodeError::TruncatedEntries {
            expected: entry_count * ENTRY_SIZE,
            got: records.len(),
        });
    }

    let mut decoded = Vec::with_capacity(entry_count);
    for index in 0..entry_count {
        let base = index * ENTRY_SIZE;
        let class = pool.get(read_u32(records, base), read_u32(records, base + 4))?;
        let method = pool.get(read_u32(records, base + 8), read_u32(records, base + 12))?;
        let sig = pool.get(read_u32(records, base + 16), read_u32(records, base + 20))?;
        let kind = read_u32(records, base + 24);
        let int_value = read_i64(records, base + 28);
        let str_off = read_u32(records, base + 36);
        let str_len = read_u32(records, base + 40);
        let value = match kind {
            0 => ReplyValue::Bool(int_value != 0),
            1 => ReplyValue::Int(int_value),
            2 => ReplyValue::Str(pool.get(str_off, str_len)?.to_owned()),
            other => return Err(DecodeError::UnknownValueKind(other)),
        };
        decoded.push((class, method, sig, value));
    }
    Ok(decoded)
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
