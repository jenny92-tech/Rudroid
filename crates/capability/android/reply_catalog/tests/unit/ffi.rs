//! INPUT:  private items of `src/ffi.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/ffi.rs`; declared there with `#[path]`
use super::*;
use std::ffi::CString;

fn build_handle() -> *mut RudroidReplyCatalog {
    let blob = crate::encode(&[crate::Entry {
        class: "a/B",
        method: "volume",
        sig: "(I)I",
        value: crate::ReplyValue::Int(100),
    }]);
    unsafe {
        rudroid_reply_catalog_from_bytes(blob.as_ptr(), blob.len())
    }
}

#[test]
fn ffi_query_and_destroy() {
    let handle = build_handle();
    assert!(!handle.is_null());
    let mut out: i64 = 0;
    let class = CString::new("a/B").unwrap();
    let method = CString::new("volume").unwrap();
    let sig = CString::new("(I)I").unwrap();
    let hit = unsafe {
        rudroid_reply_catalog_query_int(handle, class.as_ptr(), method.as_ptr(), sig.as_ptr(), &mut out)
    };
    assert_eq!(hit, 1);
    assert_eq!(out, 100);

    let miss_method = CString::new("other").unwrap();
    let miss = unsafe {
        rudroid_reply_catalog_query_int(handle, class.as_ptr(), miss_method.as_ptr(), sig.as_ptr(), &mut out)
    };
    assert_eq!(miss, 0);

    unsafe { rudroid_reply_catalog_destroy(handle) };
}

#[test]
fn ffi_null_safety() {
    unsafe {
        assert!(rudroid_reply_catalog_from_bytes(ptr::null(), 0).is_null());
        assert!(rudroid_reply_catalog_from_path(ptr::null()).is_null());
        // Null handle queries are misses, destroy is a no-op.
        assert_eq!(
            rudroid_reply_catalog_query_int(ptr::null(), ptr::null(), ptr::null(), ptr::null(), ptr::null_mut()),
            0
        );
        rudroid_reply_catalog_destroy(ptr::null_mut());
    }
}

#[test]
fn ffi_rejects_malformed_blob() {
    let junk = b"not a catalog";
    unsafe {
        assert!(rudroid_reply_catalog_from_bytes(junk.as_ptr(), junk.len()).is_null());
    }
}
