//! INPUT:  private native companion authority and optional real host libm copy under an approved name
//! OUTPUT:  allowlist, synthetic compatibility, real loading, stale-handle and code-pinning checks
//! POS:    native device oracle without requiring proprietary Steamworks binaries
use super::*;
#[test]
fn exact_allowlist_rejects_paths_and_other_host_libraries() {
    for name in [
        c"steam_api64",
        c"steam_api64.so",
        c"libsteam_api64",
        c"libsteam_api64.so",
    ] {
        assert_eq!(approved(name), Some((0, "libsteam_api64.so")));
    }
    for name in [c"sdkencryptedappticket64", c"libsdkencryptedappticket64.so"] {
        assert_eq!(approved(name), Some((1, "libsdkencryptedappticket64.so")));
    }
    for name in [
        c"../libsteam_api64.so",
        c"/usr/lib/libsteam_api64.so",
        c"libm.so.6",
        c"libparty.so",
        c"STEAM_API64",
    ] {
        assert!(approved(name).is_none());
    }
}
#[test]
fn synthetic_handles_resolve_wrappers_without_loading_host_drivers() {
    unsafe {
        let handle = super::super::open(c"libandroid.so".as_ptr(), 2);
        assert!(!handle.is_null());
        assert!(!super::super::lookup(handle, c"dlopen".as_ptr()).is_null());
        assert_eq!(super::super::close(handle), 0);
        assert!(super::super::open(c"/usr/lib/libEGL.so".as_ptr(), 2).is_null());
    }
}
#[test]
#[ignore = "requires RUDROID_NATIVE_COMPANION_TEST_DIR containing a private executable-mount libm copy"]
fn real_native_aliases_own_handles_and_keep_returned_code_live_after_close() {
    let path = std::env::var_os("RUDROID_NATIVE_COMPANION_TEST_DIR")
        .expect("native companion fixture directory");
    directory(PathBuf::from(path)).unwrap();
    assert!(open(c"steam_api64", true).is_err());
    let handle = open(c"steam_api64", false).unwrap();
    let alias = open(c"libsteam_api64.so", false).unwrap();
    assert_ne!(handle, alias);
    let pointer = symbol(handle, c"cos").unwrap();
    let call: unsafe extern "C" fn(f64) -> f64 = unsafe { std::mem::transmute(pointer) };
    assert_eq!(unsafe { call(0.0) }, 1.0);
    assert!(close(handle));
    assert!(!close(handle));
    assert!(symbol(handle, c"cos").is_none());
    assert!(symbol(alias, c"cos").is_some());
    assert!(close(alias));
    assert_eq!(unsafe { call(0.0) }, 1.0);
    let reopened = open(c"steam_api64", true).unwrap();
    assert_ne!(reopened, handle);
    assert!(close(reopened));
    assert!(open(c"sdkencryptedappticket64", false).is_err());
}
