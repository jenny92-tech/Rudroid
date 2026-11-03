//! INPUT:  Java UTF-16 URI components containing percent-encoded UTF-8 octets
//! OUTPUT:  Android Uri.decode native, preserving plus signs, null and literal UTF-16 units
//! POS:    safe URI conversion for Android framework callers; no filesystem or network access
#![forbid(unsafe_code)]

use rudroid_common::jni_env::{JObject, JniEnv};

pub(super) extern "C" fn decode(env: JniEnv, _class: JObject, input: JObject) -> JObject {
    let Some(input) = env.string_utf16(input) else {
        return std::ptr::null_mut();
    };
    env.new_string(&decode_units(&input))
}

// Android Uri uses UriCodec with convertPlus=false and throwOnFailure=false.
// Keep literal UTF-16 units intact (including unpaired surrogates); only
// consecutive escaped octets pass through the UTF-8 decoder.
fn decode_units(input: &[u16]) -> Vec<u16> {
    let mut output = Vec::with_capacity(input.len());
    let mut octets = Vec::new();
    let mut index = 0;
    while let Some(&unit) = input.get(index) {
        index += 1;
        if unit != u16::from(b'%') {
            flush(&mut output, &mut octets);
            output.push(unit);
            continue;
        }
        let mut value = 0u8;
        for _ in 0..2 {
            let Some(&digit) = input.get(index) else {
                flush(&mut output, &mut octets);
                output.push(0xfffd);
                return output;
            };
            index += 1;
            let Some(digit) = hex(digit) else {
                flush(&mut output, &mut octets);
                output.push(0xfffd);
                break;
            };
            value = value * 16 + digit;
        }
        octets.push(value);
    }
    flush(&mut output, &mut octets);
    output
}

fn hex(unit: u16) -> Option<u8> {
    match unit {
        48..=57 => Some((unit - 48) as u8),
        65..=70 => Some((unit - 65 + 10) as u8),
        97..=102 => Some((unit - 97 + 10) as u8),
        _ => None,
    }
}

fn flush(output: &mut Vec<u16>, octets: &mut Vec<u8>) {
    output.extend(String::from_utf8_lossy(octets).encode_utf16());
    octets.clear();
}

#[cfg(test)]
#[path = "../tests/unit/uri.rs"]
mod tests;
