//! INPUT: borrowed Java UTF-16 decimal strings
//! OUTPUT: checked signed 32-bit values without temporary strings or native leases
//! POS: pure Java Integer.parseInt(String) conversion under the shared VM
#![forbid(unsafe_code)]

pub(super) fn parse(units: &[u16]) -> Option<i32> {
    let (negative, digits) = match units.first()? {
        0x2d => (true, &units[1..]),
        0x2b => (false, &units[1..]),
        _ => (false, units),
    };
    if digits.is_empty() { return None; }
    let limit = if negative { i32::MIN } else { -i32::MAX };
    let mut value = 0i32;
    for &unit in digits {
        value = value.checked_mul(10)?.checked_sub(decimal_digit(unit)?)?;
        if value < limit { return None; }
    }
    if negative { Some(value) } else { value.checked_neg() }
}

fn decimal_digit(unit: u16) -> Option<i32> {
    if (0x30..=0x39).contains(&unit) { return Some(i32::from(unit - 0x30)); }
    // Java's char-based decimal digit lookup; UTF-16 surrogate units are not
    // digits. Every BMP Nd range is a contiguous run of the values 0..9.
    const ZEROES: &[u16] = &[
        0x0660, 0x06f0, 0x07c0, 0x0966, 0x09e6, 0x0a66, 0x0ae6, 0x0b66,
        0x0be6, 0x0c66, 0x0ce6, 0x0d66, 0x0de6, 0x0e50, 0x0ed0, 0x0f20,
        0x1040, 0x1090, 0x17e0, 0x1810, 0x1946, 0x19d0, 0x1a80, 0x1a90,
        0x1b50, 0x1bb0, 0x1c40, 0x1c50, 0xa620, 0xa8d0, 0xa900, 0xa9d0,
        0xa9f0, 0xaa50, 0xabf0, 0xff10,
    ];
    ZEROES.iter().find_map(|&zero| unit.checked_sub(zero).filter(|&n|n<10).map(i32::from))
}

#[cfg(test)]
#[path = "../tests/unit/integers.rs"]
mod tests;
