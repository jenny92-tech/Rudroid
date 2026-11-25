//! INPUT:  Unity profile Spec flag constants from the crate root
//! OUTPUT:  reference flag values
//! POS:    pins Spec flags beside unity-common after leaving rudroid-common
use crate::{RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG, RUDROID_UNITY_PROFILE_NONE};

#[test]
fn profile_flags_match_reference() {
    assert_eq!(RUDROID_UNITY_PROFILE_NONE, 0);
    assert_eq!(RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG, 1);
}
