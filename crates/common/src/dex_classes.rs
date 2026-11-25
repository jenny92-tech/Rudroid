//! INPUT: immutable ordinary little-endian DEX035 bytes
//! OUTPUT: bounded names of classes actually defined by class_defs
//! POS: metadata indexing only; no bytecode execution or class initialization
#![forbid(unsafe_code)]

// Header/table layout: https://source.android.com/docs/core/runtime/dex-format
// Only class-definition descriptors are decoded. Other string constants (which
// may contain SDK configuration) are neither visited nor returned. This is an
// index reader, not a verifier for method bodies, inheritance or linkability.
pub const MAX_DEX_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_CLASSES: usize = 100_000;

pub fn valid_class_name(name: &str) -> bool {
    !name.is_empty() && name.len() <= 4096
        && !name.starts_with('/') && !name.ends_with('/') && !name.contains("//")
        && name.chars().all(|c| c == '/' || c.is_ascii_alphanumeric()
            || matches!(c, '$'|'-'|'_'|'\u{00a1}'..='\u{1fff}'|'\u{2010}'..='\u{2027}'
                |'\u{2030}'..='\u{d7ff}'|'\u{e000}'..='\u{ffef}'|'\u{10000}'..='\u{10ffff}'))
}

fn word(data: &[u8], offset: usize) -> Result<usize, &'static str> {
    let bytes = data.get(offset..offset.checked_add(4).ok_or("DEX offset overflow")?)
        .ok_or("truncated DEX word")?;
    Ok(u32::from_le_bytes(bytes.try_into().unwrap()) as usize)
}

fn table(data: &[u8], count_at: usize, width: usize) -> Result<(usize, usize), &'static str> {
    let count = word(data, count_at)?;
    let offset = word(data, count_at + 4)?;
    let bytes = count.checked_mul(width).ok_or("DEX table overflow")?;
    let end = offset.checked_add(bytes).ok_or("DEX table overflow")?;
    if (count == 0 && offset != 0) || (count != 0 && (offset < 112 || offset % 4 != 0))
        || end > data.len() {
        return Err("DEX table outside file");
    }
    Ok((offset, count))
}

fn utf16_length(data: &[u8], at: &mut usize) -> Result<usize, &'static str> {
    let mut value = 0u32;
    for shift in (0..35).step_by(7) {
        let byte = *data.get(*at).ok_or("truncated descriptor length")?;
        *at += 1;
        if shift == 28 && byte > 15 { return Err("descriptor length overflow"); }
        value |= u32::from(byte & 127) << shift;
        if byte & 128 == 0 { return Ok(value as usize); }
    }
    Err("descriptor length overflow")
}

pub fn defined_classes(data: &[u8]) -> Result<Vec<String>, &'static str> {
    if data.len() < 112 || data.len() > MAX_DEX_BYTES { return Err("DEX size outside bound"); }
    if &data[..8] != b"dex\n035\0" {
        return Err("unsupported DEX magic/version");
    }
    if word(data, 32)? != data.len() || word(data, 36)? != 112
        || word(data, 40)? != 0x12345678 { return Err("unsupported DEX header"); }
    let (strings, string_count) = table(data, 56, 4)?;
    let (types, type_count) = table(data, 64, 4)?;
    let (classes, class_count) = table(data, 96, 32)?;
    if class_count > MAX_CLASSES { return Err("too many DEX classes"); }
    let data_start = word(data, 108)?;
    let data_end = data_start.checked_add(word(data, 104)?).ok_or("DEX data overflow")?;
    if data_start < 112 || data_end != data.len() { return Err("DEX data outside file"); }
    let mut result = Vec::with_capacity(class_count);
    let mut seen = std::collections::HashSet::with_capacity(class_count);
    for index in 0..class_count {
        let class = word(data, classes + index * 32)?;
        if class >= type_count || !seen.insert(class) { return Err("invalid/duplicate class definition"); }
        let string = word(data, types + class * 4)?;
        if string >= string_count { return Err("invalid class descriptor index"); }
        let mut at = word(data, strings + string * 4)?;
        if at < data_start || at >= data_end { return Err("descriptor outside DEX data"); }
        let length = utf16_length(data, &mut at)?;
        let limit = at.saturating_add(4099).min(data_end);
        let tail = data.get(at..limit).ok_or("truncated class descriptor")?;
        let end = tail.iter().position(|b| *b == 0).ok_or("unterminated/oversized class descriptor")?;
        if tail[..end].iter().any(|b| *b >= 0xf0) { return Err("four-byte UTF8 is not DEX MUTF8"); }
        // BMP modified-UTF8 descriptors equal UTF8. Surrogate encodings reject
        // explicitly: the current VM's class-name interface also requires UTF8.
        let descriptor = std::str::from_utf8(&tail[..end]).map_err(|_| "unsupported descriptor encoding")?;
        if descriptor.encode_utf16().count() != length { return Err("wrong descriptor length"); }
        let name = descriptor.strip_prefix('L').and_then(|s| s.strip_suffix(';'))
            .filter(|name| valid_class_name(name)).ok_or("invalid class definition descriptor")?;
        result.push(name.to_owned());
    }
    Ok(result)
}

#[cfg(test)]
#[path = "../tests/unit/dex_classes.rs"]
mod tests;
