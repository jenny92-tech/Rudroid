//! INPUT:  Android's byte-aligned 8-byte mbstate_t and UTF-8/UTF-32 conversion contracts
//! OUTPUT:  allocation-free restartable scalar/string converters and their provider addresses
//! POS:    preserves guest sequence state without borrowing GNU mbstate_t or native locale globals
use std::cell::Cell;
use std::ffi::{c_char, c_int};
use std::ptr;

const ERROR: usize = usize::MAX;
const INCOMPLETE: usize = usize::MAX - 1;
thread_local! {
    static DECODE: Cell<[u8; 4]> = const { Cell::new([0; 4]) };
    static ENCODE: Cell<[u8; 4]> = const { Cell::new([0; 4]) };
    static DECODE_STRING: Cell<[u8; 4]> = const { Cell::new([0; 4]) };
    static ENCODE_STRING: Cell<[u8; 4]> = const { Cell::new([0; 4]) };
}
// Only the first four bytes are state; the other four reserved bytes stay
// untouched. Byte-array reads/writes permit Android's alignment of one.
// Rust mutable references below refer to owned stack copies, never to guest
// memory which C can alias.
unsafe fn state_call(
    ps: *mut u8,
    local: &'static std::thread::LocalKey<Cell<[u8; 4]>>,
    call: impl FnOnce(&mut [u8; 4]) -> usize,
) -> usize {
    if ps.is_null() {
        local.with(|slot| {
            let mut state = slot.get();
            let result = call(&mut state);
            slot.set(state);
            result
        })
    } else {
        let mut state = ptr::read(ps.cast::<[u8; 4]>());
        let result = call(&mut state);
        ptr::write(ps.cast::<[u8; 4]>(), state);
        result
    }
}
unsafe fn invalid(state: &mut [u8; 4], error: c_int) -> usize {
    *state = [0; 4];
    *libc::__errno_location() = error;
    ERROR
}
unsafe fn decode(out: *mut u32, s: *const u8, n: usize, state: &mut [u8; 4]) -> usize {
    if state[3] != 0 {
        return invalid(state, libc::EINVAL);
    }
    let (s, n, out) = if s.is_null() {
        (c"".as_ptr().cast::<u8>(), 1, ptr::null_mut())
    } else {
        (s, n, out)
    };
    if n == 0 {
        return INCOMPLETE;
    }
    let stored = state.iter().rposition(|&b| b != 0).map_or(0, |i| i + 1);
    let first = if stored == 0 { *s } else { state[0] };
    if stored == 0 && first < 0x80 {
        if !out.is_null() {
            out.write(first as u32);
        }
        return usize::from(first != 0);
    }
    let width = match first {
        0xc0..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf7 => 4,
        _ => return invalid(state, libc::EILSEQ),
    };
    if stored >= width {
        return invalid(state, libc::EINVAL);
    }
    let mut bytes = *state;
    let consumed = (width - stored).min(n);
    for i in 0..consumed {
        let byte = *s.add(i);
        if stored + i != 0 && byte & 0xc0 != 0x80 {
            return invalid(state, libc::EILSEQ);
        }
        bytes[stored + i] = byte;
    }
    if stored + consumed < width {
        *state = bytes;
        return INCOMPLETE;
    }
    // The standard decoder validates overlong forms, surrogates and the
    // Unicode upper limit; it reads at most the four stack bytes above.
    let Ok(text) = std::str::from_utf8(&bytes[..width]) else {
        return invalid(state, libc::EILSEQ);
    };
    if !out.is_null() {
        out.write(text.chars().next().unwrap() as u32);
    }
    *state = [0; 4];
    consumed
}
unsafe fn encode(s: *mut u8, value: u32, state: &mut [u8; 4]) -> usize {
    if s.is_null() || value == 0 {
        if !s.is_null() {
            *s = 0;
        }
        *state = [0; 4];
        return 1;
    }
    if state.iter().any(|&b| b != 0) {
        return invalid(state, libc::EILSEQ);
    }
    // Preserve Bionic's historical encoder range (21 bits); its decoder
    // still rejects non-Unicode scalars and overlong byte sequences.
    let (size, lead) = match value {
        0..=0x7f => (1, 0),
        0x80..=0x7ff => (2, 0xc0),
        0x800..=0xffff => (3, 0xe0),
        0x10000..=0x1fffff => (4, 0xf0),
        _ => return invalid(state, libc::EILSEQ),
    };
    let mut remainder = value;
    for i in (1..size).rev() {
        *s.add(i) = (remainder as u8 & 0x3f) | 0x80;
        remainder >>= 6;
    }
    *s = remainder as u8 | lead;
    size
}
unsafe extern "C" fn mbsinit(ps: *const u8) -> c_int {
    c_int::from(ps.is_null() || ptr::read(ps.cast::<[u8; 4]>()) == [0; 4])
}
unsafe extern "C" fn mbrtowc(out: *mut u32, s: *const c_char, n: usize, ps: *mut u8) -> usize {
    state_call(ps, &DECODE, |state| decode(out, s.cast(), n, state))
}
unsafe extern "C" fn mbrlen(s: *const c_char, n: usize, ps: *mut u8) -> usize {
    mbrtowc(ptr::null_mut(), s, n, ps)
}
unsafe extern "C" fn mbtowc(out: *mut u32, s: *const c_char, n: usize) -> c_int {
    if s.is_null() {
        return 0;
    }
    let result = decode(out, s.cast(), n, &mut [0; 4]);
    if result == ERROR || result == INCOMPLETE {
        *libc::__errno_location() = libc::EILSEQ;
        -1
    } else {
        result as c_int
    }
}
unsafe extern "C" fn wcrtomb(s: *mut c_char, value: u32, ps: *mut u8) -> usize {
    state_call(ps, &ENCODE, |state| encode(s.cast(), value, state))
}
unsafe fn decode_string(
    dst: *mut u32,
    source: *mut *const c_char,
    bytes: usize,
    len: usize,
    state: &mut [u8; 4],
) -> usize {
    let start = *source;
    let mut used = 0;
    let mut count = 0;
    if bytes > 0 && state.iter().any(|&b| b != 0) && (*start as u8) < 0x80 {
        return invalid(state, libc::EILSEQ);
    }
    while used < bytes && (dst.is_null() || count < len) {
        let out = if dst.is_null() {
            ptr::null_mut()
        } else {
            dst.add(count)
        };
        let result = decode(out, start.cast::<u8>().add(used), bytes - used, state);
        if result == ERROR || result == INCOMPLETE {
            if !dst.is_null() {
                *source = start.add(if result == INCOMPLETE { bytes } else { used });
            }
            // Android reports an incomplete bounded string as EILSEQ and
            // clears its sequence, even in measurement mode.
            return invalid(state, libc::EILSEQ);
        }
        if result == 0 {
            if !dst.is_null() {
                *source = ptr::null();
            }
            *state = [0; 4];
            return count;
        }
        used += result;
        count += 1;
    }
    if !dst.is_null() {
        *source = start.add(used);
    }
    *state = [0; 4];
    count
}
unsafe extern "C" fn mbsnrtowcs(
    dst: *mut u32,
    source: *mut *const c_char,
    bytes: usize,
    len: usize,
    ps: *mut u8,
) -> usize {
    state_call(ps, &DECODE_STRING, |state| {
        decode_string(dst, source, bytes, len, state)
    })
}
unsafe extern "C" fn mbsrtowcs(
    dst: *mut u32,
    source: *mut *const c_char,
    len: usize,
    ps: *mut u8,
) -> usize {
    mbsnrtowcs(dst, source, usize::MAX, len, ps)
}
unsafe fn encode_string(
    dst: *mut c_char,
    source: *mut *const u32,
    wide: usize,
    len: usize,
    state: &mut [u8; 4],
) -> usize {
    if state.iter().any(|&b| b != 0) {
        return invalid(state, libc::EILSEQ);
    }
    let start = *source;
    let mut count = 0;
    let mut written = 0;
    while count < wide && (dst.is_null() || written < len) {
        let value = *start.add(count);
        let mut bytes = [0; 4];
        let size = encode(bytes.as_mut_ptr(), value, state);
        if size == ERROR {
            if !dst.is_null() {
                *source = start.add(count);
            }
            return ERROR;
        }
        if !dst.is_null() {
            if size > len - written {
                break;
            }
            ptr::copy_nonoverlapping(bytes.as_ptr(), dst.cast::<u8>().add(written), size);
        }
        if value == 0 {
            if !dst.is_null() {
                *source = ptr::null();
            }
            return written;
        }
        written += size;
        count += 1;
    }
    if !dst.is_null() {
        *source = start.add(count);
    }
    written
}
unsafe extern "C" fn wcsnrtombs(
    dst: *mut c_char,
    source: *mut *const u32,
    wide: usize,
    len: usize,
    ps: *mut u8,
) -> usize {
    state_call(ps, &ENCODE_STRING, |state| {
        encode_string(dst, source, wide, len, state)
    })
}
unsafe extern "C" fn wcsrtombs(
    dst: *mut c_char,
    source: *mut *const u32,
    len: usize,
    ps: *mut u8,
) -> usize {
    wcsnrtombs(dst, source, usize::MAX, len, ps)
}
unsafe extern "C" fn mbstowcs(dst: *mut u32, source: *const c_char, len: usize) -> usize {
    let mut cursor = source;
    decode_string(dst, &mut cursor, usize::MAX, len, &mut [0; 4])
}
unsafe extern "C" fn wcstombs(dst: *mut c_char, source: *const u32, len: usize) -> usize {
    let mut cursor = source;
    encode_string(dst, &mut cursor, usize::MAX, len, &mut [0; 4])
}
symbol_provider! {
    b"mbsinit" => mbsinit as *const () as usize,
    b"mbrtowc" | b"mbrtoc32" => mbrtowc as *const () as usize,
    b"mbrlen" => mbrlen as *const () as usize,
    b"mbtowc" => mbtowc as *const () as usize,
    b"wcrtomb" | b"c32rtomb" => wcrtomb as *const () as usize,
    b"mbsnrtowcs" => mbsnrtowcs as *const () as usize,
    b"mbsrtowcs" => mbsrtowcs as *const () as usize,
    b"wcsnrtombs" => wcsnrtombs as *const () as usize,
    b"wcsrtombs" => wcsrtombs as *const () as usize,
    b"mbstowcs" => mbstowcs as *const () as usize,
    b"wcstombs" => wcstombs as *const () as usize,
}

#[cfg(test)]
#[path = "../tests/unit/multibyte.rs"]
mod tests;
