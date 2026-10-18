//! INPUT:  file-backed Android APS2 bytes and REL/RELA section identity
//! OUTPUT:  bounded streaming relocation entries or explicit malformed-data errors
//! POS:    safe SLEB128/group decoder; native image reads/writes stay in relocations.rs
#![forbid(unsafe_code)]
// Format reference: AOSP bionic linker_reloc_iterators.h; no unbounded allocation.
const LIMIT: u64 = 8_388_608;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Entry {
    pub offset: u64,
    pub info: u64,
    pub addend: i64,
}
pub(super) struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
    left: u64,
    group: u64,
    flags: u8,
    delta: i64,
    offset: u64,
    info: u64,
    addend: i64,
    rela: bool,
    failed: bool,
}
impl<'a> Decoder<'a> {
    pub fn new(bytes: &'a [u8], rela: bool) -> Result<Self, &'static str> {
        if bytes.get(..4) != Some(b"APS2") {
            return Err("missing APS2 magic");
        }
        let mut d = Self {
            bytes,
            position: 4,
            left: 0,
            group: 0,
            flags: 0,
            delta: 0,
            offset: 0,
            info: 0,
            addend: 0,
            rela,
            failed: false,
        };
        let count = d.sleb()?;
        if count < 0 || count as u64 > LIMIT {
            return Err("relocation count out of bounds");
        }
        d.left = count as u64;
        d.offset = u64::try_from(d.sleb()?).map_err(|_| "negative initial relocation offset")?;
        Ok(d)
    }
    fn sleb(&mut self) -> Result<i64, &'static str> {
        let mut value = 0u64;
        for index in 0..10 {
            let byte = *self.bytes.get(self.position).ok_or("truncated SLEB128")?;
            self.position += 1;
            let bits = byte & 0x7f;
            if index == 9 && (byte & 0x80 != 0 || !matches!(bits, 0 | 127)) {
                return Err("SLEB128 overflows i64");
            }
            value |= u64::from(bits) << (index * 7);
            if byte & 0x80 == 0 {
                let used = (index + 1) * 7;
                if used < 64 && byte & 0x40 != 0 {
                    value |= u64::MAX << used;
                }
                return Ok(value as i64);
            }
        }
        Err("SLEB128 overflows i64")
    }
    fn entry(&mut self) -> Result<Entry, &'static str> {
        if self.group == 0 {
            let group = self.sleb()?;
            if group <= 0 || group as u64 > self.left {
                return Err("zero or oversized relocation group");
            }
            self.group = group as u64;
            let flags = self.sleb()?;
            if !(0..=15).contains(&flags) {
                return Err("unknown relocation group flags");
            }
            self.flags = flags as u8;
            if self.flags & 4 != 0 && self.flags & 8 == 0 {
                return Err("grouped addend without an addend");
            }
            if !self.rela && self.flags & 8 != 0 {
                return Err("REL section contains explicit addends");
            }
            self.delta = if self.flags & 2 != 0 { self.sleb()? } else { 0 };
            if self.flags & 1 != 0 {
                self.info = self.sleb()? as u64;
            }
            if self.flags & 12 == 12 {
                self.addend = self
                    .addend
                    .checked_add(self.sleb()?)
                    .ok_or("group addend overflow")?;
            } else if self.flags & 8 == 0 {
                self.addend = 0;
            }
        }
        let delta = if self.flags & 2 != 0 {
            self.delta
        } else {
            self.sleb()?
        };
        self.offset = self
            .offset
            .checked_add_signed(delta)
            .ok_or("relocation offset overflow")?;
        if self.flags & 1 == 0 {
            self.info = self.sleb()? as u64;
        }
        if self.flags & 12 == 8 {
            self.addend = self
                .addend
                .checked_add(self.sleb()?)
                .ok_or("relocation addend overflow")?;
        }
        self.left -= 1;
        self.group -= 1;
        Ok(Entry {
            offset: self.offset,
            info: self.info,
            addend: self.addend,
        })
    }
}
impl Iterator for Decoder<'_> {
    type Item = Result<Entry, &'static str>;
    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.left == 0 {
            return None;
        }
        let result = self.entry();
        self.failed = result.is_err();
        Some(result)
    }
}
#[cfg(test)]
#[path = "../tests/unit/packed.rs"]
mod tests;
