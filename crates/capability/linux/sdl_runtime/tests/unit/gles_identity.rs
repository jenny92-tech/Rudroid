//! INPUT:  safe GLES identity policy and explicit version configuration
//! OUTPUT:  disabled/native fallback, zero-minor and malformed override cases
//! POS:    policy tests; actual native queries and Mali rendering require device acceptance
use super::*;
#[test]
fn identity_only_overrides_version_queries() {
    let s = Identity::new(b"OpenGL ES 3.0 Rudroid".to_vec(), 3, 0).unwrap();
    assert_eq!(s.version.as_deref(), Some(c"OpenGL ES 3.0 Rudroid"));
    assert_eq!(s.integer(0x821b), Some(3));
    assert_eq!(s.integer(0x821c), Some(0));
    for name in [0x0ba2, 0x1f00, 0x0d33] {
        assert_eq!(s.integer(name), None);
    }
    let native = Identity::new(Vec::new(), -1, -1).unwrap();
    assert!(native.version.is_none());
    assert_eq!(native.integer(0x821b), None);
}
#[test]
fn malformed_identity_fails_before_native_queries() {
    assert!(Identity::new(b"bad\0version".to_vec(), 3, 0).is_err());
    for value in [-2, 10, i64::MAX] {
        assert!(Identity::new(Vec::new(), value, 0).is_err());
    }
}
