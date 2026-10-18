//! Dynamic symbol lookup through SYSV and GNU hash tables.
#![forbid(unsafe_code)]

use super::elf::ElfError;

/// SYSV ELF hash (System V ABI).
pub fn sysv_hash(name: &str) -> u32 {
    let mut h: u32 = 0;
    for byte in name.bytes() {
        h = h.rotate_left(4).wrapping_add(u32::from(byte));
        let g = h & 0xf000_0000;
        if g != 0 {
            h ^= g >> 24;
        }
        h &= !g;
    }
    h
}

/// GNU hash.
pub fn gnu_hash(name: &str) -> u32 {
    let mut h: u32 = 5381;
    for byte in name.bytes() {
        h = h.wrapping_mul(33).wrapping_add(u32::from(byte));
    }
    h
}

fn u32_at(bytes: &[u8], at: usize) -> Result<u32, ElfError> {
    bytes
        .get(at..at + 4)
        .map(|s| u32::from_le_bytes(s.try_into().unwrap()))
        .ok_or(ElfError::TruncatedSection("hash table"))
}

fn u64_at(bytes: &[u8], at: usize) -> Result<u64, ElfError> {
    bytes
        .get(at..at + 8)
        .map(|s| u64::from_le_bytes(s.try_into().unwrap()))
        .ok_or(ElfError::TruncatedSection("hash table"))
}

/// How candidate symbol names are fetched during a chain walk: the callback
/// returns the name of dynsym entry `index`.
pub type SymbolName<'a> = dyn Fn(usize) -> Result<&'a str, ElfError> + 'a;

/// SYSV hash table (`DT_HASH`): `[nbuckets, nchain, buckets[], chains[]]`.
pub struct SysvTable<'a> {
    raw: &'a [u8],
    nbuckets: usize,
    nchain: usize,
}

impl<'a> SysvTable<'a> {
    /// `raw` must cover the whole table (header + buckets + chains).
    pub fn parse(raw: &'a [u8]) -> Result<SysvTable<'a>, ElfError> {
        let nbuckets = u32_at(raw, 0)? as usize;
        let nchain = u32_at(raw, 4)? as usize;
        // A zero bucket count would make lookup's modulo panic (ticket 47).
        if nbuckets == 0 {
            return Err(ElfError::TruncatedSection("DT_HASH nbuckets"));
        }
        if raw.len() < (nbuckets + nchain) * 4 + 8 {
            return Err(ElfError::TruncatedSection("DT_HASH"));
        }
        Ok(SysvTable { raw, nbuckets, nchain })
    }

    pub fn lookup(&self, name: &str, name_of: &SymbolName<'_>) -> Result<Option<usize>, ElfError> {
        let mut index = u32_at(
            self.raw,
            8 + (sysv_hash(name) as usize % self.nbuckets) * 4,
        )? as usize;
        // A well-formed chain visits each of the `nchain` entries at most
        // once; one more step than that can only be a cycle.
        for _ in 0..=self.nchain {
            if index == 0 || index >= self.nchain {
                return Ok(None);
            }
            if name_of(index)? == name {
                return Ok(Some(index));
            }
            let chain = u32_at(self.raw, 8 + (self.nbuckets + index) * 4)?;
            index = chain as usize;
        }
        Err(ElfError::CyclicHashChain("DT_HASH"))
    }
}

/// GNU hash table (`DT_GNU_HASH`):
/// `[nbuckets, symoffset, bloom_size, bloom_shift, bloom(u64)[], buckets[], chains[]]`.
pub struct GnuTable<'a> {
    raw: &'a [u8],
    nbuckets: usize,
    symoffset: usize,
    bloom_size: usize,
    bloom_shift: u32,
}

impl<'a> GnuTable<'a> {
    /// `raw` must start at the table and extend at least over header + bloom
    /// + buckets (chains are walked lazily inside `raw`).
    pub fn parse(raw: &'a [u8]) -> Result<GnuTable<'a>, ElfError> {
        if raw.len() < 16 {
            return Err(ElfError::TruncatedSection("DT_GNU_HASH"));
        }
        let nbuckets = u32_at(raw, 0)? as usize;
        let symoffset = u32_at(raw, 4)? as usize;
        let bloom_size = u32_at(raw, 8)? as usize;
        let bloom_shift = u32_at(raw, 12)?;
        // Zero counts would make lookup's modulo operations panic (ticket 47).
        if nbuckets == 0 || bloom_size == 0 {
            return Err(ElfError::TruncatedSection("DT_GNU_HASH counts"));
        }
        let minimum = 16 + bloom_size * 8 + nbuckets * 4;
        if raw.len() < minimum {
            return Err(ElfError::TruncatedSection("DT_GNU_HASH"));
        }
        Ok(GnuTable {
            raw,
            nbuckets,
            symoffset,
            bloom_size,
            bloom_shift,
        })
    }

    fn buckets_offset(&self) -> usize {
        16 + self.bloom_size * 8
    }

    fn chains_offset(&self) -> usize {
        self.buckets_offset() + self.nbuckets * 4
    }

    /// The total chain count is not stored; derive the number of dynsym
    /// entries by walking chain words until the end-of-chain bit at `target`'s
    /// chain position (used for coverage checks only).
    pub fn lookup(&self, name: &str, name_of: &SymbolName<'_>) -> Result<Option<usize>, ElfError> {
        let h = gnu_hash(name);
        // Bloom filter (ELF64: 8-byte words).
        let bloom_word = u64_at(
            self.raw,
            16 + (h as usize / 64 % self.bloom_size) * 8,
        )?;
        let bit = 1u64 << (h % 64);
        // Mask the shift amount: a hostile bloom_shift >= 32 must not shift a
        // u32 by more than its width (ticket 47).
        let second_bit = 1u64 << ((h >> (self.bloom_shift % 32)) % 64);
        if bloom_word & bit == 0 || bloom_word & second_bit == 0 {
            return Ok(None);
        }

        let bucket = u32_at(self.raw, self.buckets_offset() + (h as usize % self.nbuckets) * 4)?
            as usize;
        if bucket < self.symoffset {
            return Ok(None);
        }

        let mut index = bucket;
        loop {
            let chain = u32_at(self.raw, self.chains_offset() + (index - self.symoffset) * 4)?;
            if chain | 1 == h | 1 {
                if name_of(index)? == name {
                    return Ok(Some(index));
                }
            }
            if chain & 1 == 1 {
                return Ok(None);
            }
            index += 1;
        }
    }
}

#[cfg(test)]
#[path = "../tests/unit/hash.rs"]
mod tests;
