//! INPUT:  bounded PEM/base64/DER cases for the native TLS root bridge
//! OUTPUT:  strict decoding and corruption/size rejection regressions
//! POS:    trust-material parsing tests; does not assert peer verification
use super::*;

#[test]
fn base64_preserves_bytes_and_rejects_invalid_padding() {
    assert_eq!(decode_base64(b" AIAH \n"), Some(vec![0, 128, 7]));
    assert_eq!(decode_base64(b"/w=="), Some(vec![255]));
    assert_eq!(decode_base64(b"//8="), Some(vec![255, 255]));
    for invalid in [b"".as_slice(), b"AA=", b"AB==", b"AAB=", b"AA==AAAA", b"AA$A", b"===="] {
        assert_eq!(decode_base64(invalid), None, "{invalid:?}");
    }
}

#[test]
fn pem_bounds_and_der_lengths_reject_corrupt_trust_material() {
    let pem = b"-----BEGIN CERTIFICATE-----\nMAMCAQA=\n-----END CERTIFICATE-----";
    assert_eq!(parse_bundle(pem).unwrap(), [vec![0x30, 3, 2, 1, 0]]);
    assert!(parse_bundle(b"unrelated text").is_err());
    assert!(parse_bundle(b"-----BEGIN CERTIFICATE-----MAMCAQA=").is_err());
    assert!(parse_bundle(&vec![b'x'; MAX_BUNDLE + 1]).is_err());
    for bytes in [&[0x30, 0][..], &[0x30, 4, 2, 1, 0], &[0x30, 0x80, 0, 0], &[0x30, 0x81, 3, 2, 1, 0]] {
        assert!(!der_sequence(bytes), "{bytes:?}");
    }
    let many = pem.repeat(MAX_CERTIFICATES + 1);
    assert!(parse_bundle(&many).is_err());
}
