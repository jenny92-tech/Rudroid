//! INPUT:  Android Uri.decode UTF-16 cases, including malformed escapes
//! OUTPUT:  safe conversion regressions independent of the JNI fixture
//! POS:    framework URI semantic tests
use super::decode_units;

#[test]
fn percent_utf8_and_literals_remain_distinct() {
    for (input, expected) in [
        ("%E4%B8%AD+%2B%00", "中++\0"),
        ("x%2fy%20z", "x/y z"),
        ("%F0%9F%8E%AE", "🎮"),
        ("%252F", "%2F"),
        ("", ""),
    ] {
        assert_eq!(decode_units(&input.encode_utf16().collect::<Vec<_>>()), expected.encode_utf16().collect::<Vec<_>>());
    }
    assert_eq!(decode_units(&[0xd800, b'+' as u16, 0, 0xdc00]), [0xd800, b'+' as u16, 0, 0xdc00]);
}

#[test]
fn malformed_octets_and_escapes_use_android_replacement_policy() {
    for (input, expected) in [
        ("abc%", "abc�"), ("%A", "�"), ("%80", "�"),
        ("%E4%B8", "�"), ("%E4x%B8", "�x�"),
        ("%ZZ", "�\0Z"), ("%1Z", "�\u{1}"),
    ] {
        assert_eq!(decode_units(&input.encode_utf16().collect::<Vec<_>>()), expected.encode_utf16().collect::<Vec<_>>(), "{input}");
    }
}
