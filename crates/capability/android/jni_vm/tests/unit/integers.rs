//! INPUT: pure UTF-16 decimal parser
//! OUTPUT: signed endpoints, Unicode digits and malformed/overflow rejection
//! POS: Integer conversion regression independent of native dispatch
use super::*;
#[test]
fn signed_decimal_values_preserve_endpoints_and_unicode_digits() {
    for (text,expected) in [("0",0),("-0",0),("+48000",48000),("256",256),
        ("2147483647",i32::MAX),("-2147483648",i32::MIN),("00000000000000001",1),
        ("４８０００",48000),("-٢٥٦",-256),("१२३",123)] {
        assert_eq!(parse(&text.encode_utf16().collect::<Vec<_>>()),Some(expected),"{text}");
    }
}
#[test]
fn invalid_or_out_of_range_values_do_not_wrap_or_accept_prefixes() {
    for text in ["","+","-"," 1","1 ","1x","1.0","++1","+-1","2147483648",
        "-2147483649","99999999999999999999999999","\0","²","𝟙"] {
        assert_eq!(parse(&text.encode_utf16().collect::<Vec<_>>()),None,"{text:?}");
    }
    assert_eq!(parse(&[0xd800]),None);
}
