//! INPUT:  APS2 grouped/ungrouped, negative deltas and malformed SLEB fixtures
//! OUTPUT:  streaming equivalence, bounds and terminal-error assertions
//! POS:    exercises untrusted relocation bytes without mappings or native writes
use super::*;
fn sleb(mut value: i64, out: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 127) as u8;
        value >>= 7;
        let done = (value == 0 && byte & 64 == 0) || (value == -1 && byte & 64 != 0);
        if !done {
            byte |= 128;
        }
        out.push(byte);
        if done {
            break;
        }
    }
}
fn table(values: &[i64]) -> Vec<u8> {
    let mut out = b"APS2".to_vec();
    for value in values {
        sleb(*value, &mut out);
    }
    out
}
#[test]
fn grouped_and_per_entry_addends_follow_reference_deltas() {
    let raw = table(&[3, 100, 2, 15, 8, 1027, 5, 1, 8, 8, 1027, -2]);
    let entries = Decoder::new(&raw, true)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        entries,
        [
            Entry {
                offset: 108,
                info: 1027,
                addend: 5
            },
            Entry {
                offset: 116,
                info: 1027,
                addend: 5
            },
            Entry {
                offset: 124,
                info: 1027,
                addend: 3
            }
        ]
    );
}
#[test]
fn negative_offset_deltas_and_addend_reset_are_checked() {
    let raw = table(&[2, 100, 1, 8, 8, 1027, -5, 1, 3, -8, 1027]);
    let e = Decoder::new(&raw, true)
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        (e[0].offset, e[0].addend, e[1].offset, e[1].addend),
        (108, -5, 100, 0)
    );
}
#[test]
fn bad_groups_overflows_and_truncation_fail_once() {
    for values in [
        &[1, 0, 0, 0][..],
        &[1, 0, 2, 0][..],
        &[1, 0, 1, 4][..],
        &[1, 0, 1, 16][..],
        &[1, 0, 1, 3, -1, 1027][..],
        &[1, 0, 1, 0, 8][..],
    ] {
        let raw = table(values);
        let mut d = Decoder::new(&raw, true).unwrap();
        assert!(d.next().unwrap().is_err());
        assert!(d.next().is_none());
    }
    let raw = table(&[1, 0, 1, 8, 8, 1027, 1]);
    assert!(Decoder::new(&raw, false).unwrap().next().unwrap().is_err());
    for mut raw in [b"APS2".to_vec(), b"APS2".to_vec()] {
        raw.extend_from_slice(&[0x80; 10]);
        assert!(Decoder::new(&raw, true).is_err());
    }
}
#[test]
fn i64_endpoints_roundtrip_without_truncation() {
    for value in [i64::MIN, i64::MAX, -129, -128, -1, 0, 63, 64, 127, 128] {
        let mut bytes = Vec::new();
        sleb(value, &mut bytes);
        let mut d = Decoder {
            bytes: &bytes,
            position: 0,
            left: 0,
            group: 0,
            flags: 0,
            delta: 0,
            offset: 0,
            info: 0,
            addend: 0,
            rela: true,
            failed: false,
        };
        assert_eq!(d.sleb().unwrap(), value);
    }
    assert!(Decoder::new(&table(&[LIMIT as i64 + 1, 0]), true).is_err());
}
