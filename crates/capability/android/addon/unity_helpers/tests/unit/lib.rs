//! INPUT:  `MODULE` of `src/lib.rs` via `super`
//! OUTPUT:  the reference library path, log tag and module event for `sdk_unity_helpers`
//! POS:    pins this addon's data; the shared chain is tested in after_unity_module, the kit path in the loader tests
use super::*;

#[test]
fn module_matches_reference() {
    assert_eq!(MODULE.library, "lib/arm64-v8a/libUnityHelpers_Android.so");
    assert_eq!(MODULE.log_tag, "UNITYHELP");
    assert_eq!(MODULE.kind, ModuleKind::UnityHelpers);
    assert_eq!(MODULE.label, "unity_helpers");
}
