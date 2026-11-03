//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

fn sample_entries() -> Vec<Entry<'static>> {
    vec![
        Entry {
            class: "a/B",
            method: "flag",
            sig: "()Z",
            value: ReplyValue::Bool(false),
        },
        Entry {
            class: "a/B",
            method: "volume",
            sig: "(I)I",
            value: ReplyValue::Int(100),
        },
        Entry {
            class: "a/B",
            method: "name",
            sig: "()Ljava/lang/String;",
            value: ReplyValue::Str("Rudroid".to_owned()),
        },
    ]
}

#[test]
fn header_is_little_endian_and_versioned() {
    let blob = encode(&sample_entries());
    assert_eq!(&blob[0..4], b"BDRC");
    assert_eq!(u32::from_le_bytes(blob[4..8].try_into().unwrap()), 1);
    assert_eq!(u32::from_le_bytes(blob[8..12].try_into().unwrap()), 3);
}

#[test]
fn encode_is_deterministic() {
    assert_eq!(encode(&sample_entries()), encode(&sample_entries()));
}

#[test]
fn round_trip_preserves_values() {
    let blob = encode(&sample_entries());
    let decoded = decode(&blob).unwrap();
    assert_eq!(decoded.len(), 3);
    assert_eq!(decoded[0].3, ReplyValue::Bool(false));
    assert_eq!(decoded[1].3, ReplyValue::Int(100));
    assert_eq!(decoded[2].3, ReplyValue::Str("Rudroid".to_owned()));
    assert_eq!(decoded[2].0, "a/B");
    assert_eq!(decoded[2].1, "name");
    assert_eq!(decoded[2].2, "()Ljava/lang/String;");
}

#[test]
fn empty_table_round_trips() {
    let blob = encode(&[]);
    assert_eq!(blob.len(), 16);
    assert!(decode(&blob).unwrap().is_empty());
}

#[test]
fn decode_rejects_bad_headers() {
    assert_eq!(decode(b""), Err(DecodeError::TooShort));
    assert_eq!(decode(&[0u8; 16]), Err(DecodeError::BadMagic));
    let mut blob = encode(&[]);
    blob[4] = 9;
    assert_eq!(
        decode(&blob),
        Err(DecodeError::UnsupportedVersion(9))
    );
    // Truncated entry section.
    let full = encode(&sample_entries());
    assert!(matches!(
        decode(&full[..full.len() - 10]),
        Err(DecodeError::TruncatedEntries { .. })
    ));
    // Truncated pool section.
    let mut cut = encode(&sample_entries());
    cut.truncate(20);
    assert!(matches!(decode(&cut), Err(DecodeError::TruncatedPool)));
}

#[test]
fn decode_rejects_unknown_value_kind() {
    let mut blob = encode(&[Entry {
        class: "a/B",
        method: "m",
        sig: "()V",
        value: ReplyValue::Int(1),
    }]);
    // value_kind lives at record base + 24; the pool precedes the
    // records, so read its size from the header.
    let pool_size = u32::from_le_bytes(blob[12..16].try_into().unwrap()) as usize;
    blob[16 + pool_size + 24] = 7;
    assert_eq!(decode(&blob), Err(DecodeError::UnknownValueKind(7)));
}
