//! INPUT:  validated ELF tables, an exclusively writable image and import resolver
//! OUTPUT:  shared checked RELR/APS2/RELA application and conservative written span
//! POS:    native image boundary; packed decoding remains safe and streaming
//! Relocation application shared by the buffer backend (`GuestModule`) and
//! the Linux executable backend (`exec`, ticket 49).
//!
//! Operates on a raw target range `(image, image_len)` at `base`: every
//! relocated word must lie fully inside that range (ticket 47) AND inside
//! the merged memory coverage of the PT_LOAD segments (ticket 63 — a target
//! in an unmapped gap between segments, below the first segment, or with an
//! overflowing end is rejected even when the allocation is large enough).
//! ABS64 / GLOB_DAT / JUMP_SLOT add the RELA addend (ticket 48), and unknown
//! relocation types fail with a structured error instead of being skipped.

use super::elf::{
    ElfError, ElfView, SymbolVersion, R_AARCH64_ABS64, R_AARCH64_GLOB_DAT, R_AARCH64_JUMP_SLOT,
    R_AARCH64_RELATIVE,
};
use super::LoadError;

/// Conservative union of successful relocation writes. Gaps between writes
/// may be included; this can only request extra instruction-cache work.
/// Constant space regardless of relocation count, with no retained pointers.
#[derive(Clone, Copy, Default)]
pub(super) struct RelocationWrites {
    span: Option<(u64, u64)>,
}

impl RelocationWrites {
    fn record(&mut self, offset: u64) {
        // Successful writes already passed the full-word coverage check.
        // Saturation also keeps this pure helper conservative at the edge.
        let end = offset.saturating_add(std::mem::size_of::<usize>() as u64);
        self.span = Some(match self.span {
            Some((lo, hi)) => (lo.min(offset), hi.max(end)),
            None => (offset, end),
        });
    }

    pub(super) fn intersects(self, start: u64, end: u64) -> bool {
        start < end && self.span.is_some_and(|(lo, hi)| lo < end && start < hi)
    }
}

/// Apply all relocations of `view` against the image at `base`.
///
/// Symbol binding follows the reference `so_relocate_rel`: a symbol DEFINED
/// in this module binds to `base + st_value` directly; only undefined
/// imports go through `resolve_import` (the backend's `so_resolve_link`
/// equivalent), then weak → 0.
/// # Safety
/// Each covered PT_LOAD range at image is initialized, writable and exclusively owned
/// for this call; image_len bounds its reservation and imports have exact native ABIs.
pub(super) unsafe fn apply(
    view: &ElfView,
    base: usize,
    image: *mut u8,
    image_len: usize,
    resolve_import: &dyn Fn(&str, Option<SymbolVersion<'_>>) -> Option<usize>,
) -> Result<RelocationWrites, LoadError> {
    let coverage = load_coverage(view);
    let mut writes = RelocationWrites::default();
    // RELR runs first, mirroring the reference `so_relocate_all` ordering.
    apply_relr(view, base, image, image_len, &coverage, &mut writes)?;
    for (index, table) in view.packed.iter().enumerate() {
        if let Some((address, size)) = table {
            let bytes = view.vaddr_slice(*address, *size)?;
            let decoder = super::packed::Decoder::new(bytes, index == 1)
                .map_err(|e| LoadError::Elf(ElfError::InvalidPackedRelocations(e)))?;
            for decoded in decoder {
                let entry =
                    decoded.map_err(|e| LoadError::Elf(ElfError::InvalidPackedRelocations(e)))?;
                // SAFETY: inherited writable image ownership; each target is checked below.
                unsafe {
                    apply_entry(
                        view,
                        base,
                        image,
                        image_len,
                        &coverage,
                        resolve_import,
                        entry.offset,
                        entry.info as u32,
                        (entry.info >> 32) as usize,
                        (index == 1).then_some(entry.addend),
                    )?;
                }
                if entry.info as u32 != 0 { writes.record(entry.offset); }
            }
        }
    }
    for (offset, kind, symbol, addend) in view.rela_iter()? {
        // SAFETY: inherited writable image ownership and checked relocation targets.
        unsafe {
            apply_entry(
                view,
                base,
                image,
                image_len,
                &coverage,
                resolve_import,
                offset,
                kind,
                symbol,
                Some(addend),
            )?;
        }
        if kind != 0 { writes.record(offset); }
    }

    Ok(writes)
}

unsafe fn apply_entry(
    view: &ElfView,
    base: usize,
    image: *mut u8,
    image_len: usize,
    coverage: &[(u64, u64)],
    resolve_import: &dyn Fn(&str, Option<SymbolVersion<'_>>) -> Option<usize>,
    r_offset: u64,
    r_type: u32,
    r_sym: usize,
    r_addend: Option<i64>,
) -> Result<(), LoadError> {
    if r_type == 0 {
        return Ok(());
    } // R_AARCH64_NONE performs no access.
      // The write covers a full pointer-sized word: the whole 8-byte range
      // must sit inside the image allocation (ticket 47) and inside real
      // PT_LOAD memory (ticket 63).
    let place = match vaddr_ptr_for_write(r_offset, &coverage, image, image_len) {
        Some(place) => place,
        None => {
            return Err(LoadError::Relocation {
                r_offset,
                kind: r_type,
                reason: "target write range is not fully covered by PT_LOAD segments \
                             (unmapped gap, outside the segments, or overflowing)"
                    .to_owned(),
            })
        }
    };
    let addend = match r_addend {
        Some(addend) => addend,
        None => {
            // SAFETY: caller owns the writable image; the same full-word PT_LOAD check
            // protects the implicit REL addend read as well as its subsequent write.
            unsafe { (place as *const usize).read_unaligned() as i64 }
        }
    };
    match r_type {
        R_AARCH64_RELATIVE => {
            let value = base.wrapping_add(addend as usize);
            // SAFETY: the full 8-byte range was bounds-checked above.
            unsafe { (place as *mut usize).write_unaligned(value) };
        }
        R_AARCH64_ABS64 | R_AARCH64_GLOB_DAT | R_AARCH64_JUMP_SLOT => {
            let resolved = resolve_symbol(view, base, r_sym, resolve_import).map_err(|reason| {
                LoadError::Relocation {
                    r_offset,
                    kind: r_type,
                    reason,
                }
            })?;
            // ELF semantics: the addend is part of the computed value.
            let value = resolved.wrapping_add(addend as usize);
            // SAFETY: bounds-checked above.
            unsafe { (place as *mut usize).write_unaligned(value) };
        }
        other => {
            return Err(LoadError::Relocation {
                r_offset,
                kind: other,
                reason: "relocation type not implemented in this slice".to_owned(),
            })
        }
    }
    Ok(())
}

/// RELR packed-relative relocations, ported from the reference
/// `so_relr_relocate`: an entry with bit 0 clear is the vaddr of the first
/// relocated word of a run; odd entries are 63-bit bitmaps covering the next
/// words of that run (after a full bitmap the run continues 63 words later).
/// Every write goes through the same bounds/PT_LOAD checks as the RELA path
/// (tickets 47/63); the value semantics are `*where += base`.
fn apply_relr(
    view: &ElfView,
    base: usize,
    image: *mut u8,
    image_len: usize,
    coverage: &[(u64, u64)],
    writes: &mut RelocationWrites,
) -> Result<(), LoadError> {
    let Some((vaddr, size)) = view.relr else {
        return Ok(());
    };
    // Parse validated the range as file-backed; read the entries from the
    // same bytes (the table itself is never relocated).
    let raw = view
        .vaddr_slice(vaddr, size)
        .map_err(|_| LoadError::Elf(ElfError::InvalidRelr("table range is not file-backed")))?;
    // Streaming decode (ticket 89): entries are decoded and written one at a
    // time — no allocation is proportional to the relocation count. A decode
    // or write failure mid-table fails the whole load; the image is never
    // published (bring-up discards it), so already-applied writes are safe.
    let mut decoder = RelrDecoder::new(raw);
    while let Some(decoded) = decoder.next() {
        let word_vaddr = decoded.map_err(|failed| LoadError::Relocation {
            r_offset: failed.0,
            kind: R_AARCH64_RELATIVE,
            reason: format!("RELR decode: {}", failed.1),
        })?;
        relr_write(word_vaddr, base, image, image_len, coverage)?;
        writes.record(word_vaddr);
    }
    Ok(())
}

/// Streaming RELR decoder (ticket 89): yields the ordered stream of
/// relocated word vaddrs with O(1) state — no allocation proportional to
/// the relocation count. Decode rules mirror the reference
/// `so_relr_relocate`; `Err` carries the offending raw entry and the reason.
struct RelrDecoder<'a> {
    /// Raw RELR table bytes (u64 LE words, parse-validated range).
    entries: &'a [u8],
    pos: usize,
    /// Vaddr the next bitmap bit refers to; `None` until the first address
    /// entry (a leading bitmap is malformed — the reference would have
    /// written at base+0 here).
    cursor: Option<u64>,
    /// Remaining bits of the bitmap entry being expanded (0 = none).
    bitmap_bits: u64,
    /// Bit index of the next candidate within the current bitmap.
    bitmap_index: u64,
    /// True while a bitmap entry is being expanded.
    in_bitmap: bool,
}

impl<'a> RelrDecoder<'a> {
    fn new(entries: &'a [u8]) -> Self {
        RelrDecoder {
            entries,
            pos: 0,
            cursor: None,
            bitmap_bits: 0,
            bitmap_index: 0,
            in_bitmap: false,
        }
    }

    fn decode_err(entry: u64, reason: &'static str) -> (u64, &'static str) {
        (entry, reason)
    }
}

impl Iterator for RelrDecoder<'_> {
    type Item = Result<u64, (u64, &'static str)>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            // Expand the in-progress bitmap bit by bit (constant state).
            if self.in_bitmap {
                if self.bitmap_bits != 0 {
                    let bit = self.bitmap_bits & 1;
                    self.bitmap_bits >>= 1;
                    let index = self.bitmap_index;
                    self.bitmap_index += 1;
                    if bit == 1 {
                        let start = self.cursor.unwrap_or(0);
                        let offset = match index.checked_mul(8) {
                            Some(offset) => offset,
                            None => {
                                return Some(Err(Self::decode_err(0, "bitmap index overflows")))
                            }
                        };
                        let word = match start.checked_add(offset) {
                            Some(word) => word,
                            None => {
                                return Some(Err(Self::decode_err(0, "bitmap target overflows")))
                            }
                        };
                        return Some(Ok(word));
                    }
                    continue;
                }
                // Bitmap drained: the run advances by 63 words
                // (`8 * sizeof(Elf_Relr) - 1` in the reference).
                self.in_bitmap = false;
                if let Some(start) = self.cursor {
                    match start.checked_add(63 * 8) {
                        Some(next) => self.cursor = Some(next),
                        None => return Some(Err(Self::decode_err(0, "run advance overflows"))),
                    }
                }
                self.bitmap_index = 0;
            }
            let word_bytes = self.entries.get(self.pos..self.pos + 8)?;
            self.pos += 8;
            let entry = u64::from_le_bytes(word_bytes.try_into().unwrap());
            if entry & 1 == 0 {
                // Address entry: relocate that word; the cursor moves one
                // word past it for the following bitmap.
                let word = match entry.checked_add(8) {
                    Some(word) => word,
                    None => return Some(Err(Self::decode_err(entry, "address entry overflows"))),
                };
                self.cursor = Some(word);
                return Some(Ok(entry));
            }
            // Odd entry: a 63-bit bitmap over the words after the cursor.
            if self.cursor.is_none() {
                return Some(Err(Self::decode_err(
                    entry,
                    "bitmap entry appears before any address entry",
                )));
            }
            self.bitmap_bits = entry >> 1;
            self.bitmap_index = 0;
            self.in_bitmap = true;
        }
    }
}

fn relr_write(
    word_vaddr: u64,
    base: usize,
    image: *mut u8,
    image_len: usize,
    coverage: &[(u64, u64)],
) -> Result<(), LoadError> {
    let place = match vaddr_ptr_for_write(word_vaddr, coverage, image, image_len) {
        Some(place) => place,
        None => {
            return Err(LoadError::Relocation {
                r_offset: word_vaddr,
                kind: R_AARCH64_RELATIVE,
                reason: "RELR target write range is not fully covered by PT_LOAD segments \
                         (unmapped gap, outside the segments, or overflowing)"
                    .to_owned(),
            })
        }
    };
    // SAFETY: the full 8-byte range was bounds-checked above.
    let current = unsafe { (place as *mut usize).read_unaligned() };
    // SAFETY: same checked, writable 8-byte range.
    unsafe { (place as *mut usize).write_unaligned(current.wrapping_add(base)) };
    Ok(())
}

/// Merged memory coverage of the PT_LOAD segments: `(vaddr, end)` pairs over
/// `p_memsz` (BSS included). Adjacent and overlapping segments are merged so
/// a write spanning the boundary of CONTIGUOUS coverage stays legal; a gap
/// between segments stays uncovered (ticket 63).
fn load_coverage(view: &ElfView) -> Vec<(u64, u64)> {
    let mut ranges: Vec<(u64, u64)> = view
        .loads
        .iter()
        .map(|load| {
            (
                load.vaddr,
                load.vaddr.checked_add(load.memsz).unwrap_or(u64::MAX),
            )
        })
        .collect();
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => {
                if end > last.1 {
                    last.1 = end;
                }
            }
            _ => merged.push((start, end)),
        }
    }
    merged
}

fn vaddr_ptr_for_write(
    vaddr: u64,
    coverage: &[(u64, u64)],
    image: *mut u8,
    image_len: usize,
) -> Option<*mut u8> {
    if vaddr > usize::MAX as u64 {
        return None;
    }
    let offset = vaddr as usize;
    // Belt: the allocation itself must hold the whole word (ticket 47).
    let end = offset.checked_add(std::mem::size_of::<usize>())?;
    if end > image_len {
        return None;
    }
    // Suspender: the whole word must also be real PT_LOAD memory (ticket 63).
    let range_end = vaddr.checked_add(std::mem::size_of::<usize>() as u64)?;
    if coverage
        .iter()
        .any(|(start, covered_end)| vaddr >= *start && range_end <= *covered_end)
    {
        // SAFETY: offset + size checked against image_len.
        Some(unsafe { image.add(offset) })
    } else {
        None
    }
}

/// `st_info` of dynsym entry `index` (offset +4 of the translated symtab
/// entry, ticket 48).
fn symbol_info(view: &ElfView, index: usize) -> Option<u8> {
    view.symtab_entry_offset(index)
        .ok()
        .and_then(|offset| view.bytes.get(offset + 4))
        .copied()
}

/// STB_WEAK = 2 in the high nibble of st_info.
fn is_weak(info: Option<u8>) -> bool {
    info.map(|info| info >> 4 == 2).unwrap_or(false)
}

/// STT_TLS = 6 in the low nibble of st_info.
pub(super) const STT_TLS: u8 = 6;

/// `Err` carries the reason string for relocation error enrichment.
fn resolve_symbol(
    view: &ElfView,
    base: usize,
    r_sym: usize,
    resolve_import: &dyn Fn(&str, Option<SymbolVersion<'_>>) -> Option<usize>,
) -> Result<usize, String> {
    if r_sym == 0 {
        return Ok(0);
    }
    let (name, st_value, st_shndx) = match view.dynsym(r_sym) {
        Ok(found) => found,
        Err(e) => return Err(format!("cannot read dynsym[{r_sym}]: {e}")),
    };
    let info = symbol_info(view, r_sym);
    // A TLS symbol's st_value is an offset into the TLS block, not an image
    // address: an address-type relocation against it (defined or imported)
    // is refused instead of binding a bogus pointer (ticket 86).
    if info.map(|info| info & 0xf) == Some(STT_TLS) {
        return Err(format!(
            "symbol `{name}` is a TLS (STT_TLS) symbol — its value is a TLS-block \
             offset, not an address; TLS binding is not supported (ticket 86)"
        ));
    }
    if st_shndx != 0 {
        // Defined inside this module (reference `so_relocate_rel`).
        return Ok(base.wrapping_add(st_value as usize));
    }
    // Preserve the whole requirement for the backend: a name-only source,
    // explicit name map or guest definition cannot attest to a versioned
    // ABI. This applies to weak imports too; an unsupported weak tuple is 0.
    let version = view
        .symbol_version(r_sym)
        .map_err(|e| format!("cannot read version of dynsym[{r_sym}] (`{name}`): {e}"))?;
    if let Some(address) = resolve_import(name, version) {
        return Ok(address);
    }
    if is_weak(info) {
        return Ok(0);
    }
    if let Some(version) = version {
        return Err(format!(
            "symbol `{name}` requires version `{}` from `{}` — no version-aware \
             provider accepted this ABI; refusing to bind by bare name (ticket 86)",
            version.name, version.file
        ));
    }
    Err(format!(
        "undefined symbol `{name}` and no provider module supplies it"
    ))
}

#[cfg(test)]
#[path = "../tests/unit/relocations.rs"]
mod tests;
