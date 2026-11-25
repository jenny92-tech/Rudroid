//! INPUT:  private post-Main load bridge and a temporary already-mapped path
//! OUTPUT: proof an existing preload/normal image is rejected before exec load
//! POS:    keeps unit bodies out of `src/guest.rs`
use super::*;
use rudroid_common::registry::HostStr;
use std::ffi::CString;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());
static EXEC_LOADS: AtomicUsize = AtomicUsize::new(0);
static LAST_ERROR_CALLS: AtomicUsize = AtomicUsize::new(0);
static FETCHED_ERROR: Mutex<Vec<u8>> = Mutex::new(Vec::new());

unsafe extern "C" fn counted_load(_: *const c_char) -> *mut c_void {
    EXEC_LOADS.fetch_add(1, Ordering::SeqCst);
    std::ptr::null_mut()
}

thread_local! {
    static REENTER_BRIDGE: std::cell::Cell<*mut c_void> = const { std::cell::Cell::new(std::ptr::null_mut()) };
    static REENTER_RESULT: std::cell::Cell<c_int> = const { std::cell::Cell::new(-1) };
}

// Simulate the constructor calling the same load hook before exec_load returns.
unsafe extern "C" fn reentrant_load(_: *const c_char) -> *mut c_void {
    EXEC_LOADS.fetch_add(1, Ordering::SeqCst);
    let bridge = REENTER_BRIDGE.with(|slot| slot.replace(std::ptr::null_mut()));
    if !bridge.is_null() {
        // SAFETY: the outer synchronous call still owns the bridge and API.
        let result = unsafe { load_bridge_jni_on_load(bridge, HostStr::new(c"lib/arm64-v8a/libbroken.so"), std::ptr::null_mut()) };
        REENTER_RESULT.with(|slot| slot.set(result));
    }
    std::ptr::null_mut()
}

unsafe extern "C" fn zero_symbol(_: *const c_void, _: *const c_char) -> usize {
    0
}

unsafe extern "C" fn zero_base(_: *const c_void) -> usize {
    0
}

unsafe extern "C" fn destroy_nothing(_: *mut c_void) {}

unsafe extern "C" fn no_last_error(_: *mut c_char, _: usize) -> usize {
    0
}

unsafe extern "C" fn counted_last_error(buffer: *mut c_char, capacity: usize) -> usize {
    LAST_ERROR_CALLS.fetch_add(1, Ordering::SeqCst);
    let text = b"fixture relocation failure\0";
    if !buffer.is_null() && capacity > 0 {
        let copied = (text.len() - 1).min(capacity - 1);
        std::ptr::copy_nonoverlapping(text.as_ptr(), buffer.cast(), copied);
        buffer.add(copied).write(0);
        *FETCHED_ERROR.lock().unwrap_or_else(|p| p.into_inner()) =
            std::slice::from_raw_parts(buffer.cast::<u8>(), copied).to_vec();
    }
    text.len()
}

#[test]
fn explicit_load_rejects_a_path_already_mapped_by_bring_up() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    EXEC_LOADS.store(0, Ordering::SeqCst);
    let root = std::env::temp_dir().join(format!(
        "rudroid-guest-load-bridge-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let library = root.join("lib/arm64-v8a/libalready.so");
    std::fs::create_dir_all(library.parent().unwrap()).unwrap();
    std::fs::write(&library, b"fixture").unwrap();
    let root = root.canonicalize().unwrap();
    let library = library.canonicalize().unwrap();

    let api = ElfLoadApi {
        from_path: counted_load,
        load_base: zero_base,
        symbol: zero_symbol,
        destroy: destroy_nothing,
        last_error: no_last_error,
        exec_load: Some(counted_load),
        exec_symbol: Some(zero_symbol),
        exec_symbol_all: None,
        exec_load_base: Some(zero_base),


    };
    let mut images = HashMap::new();
    images.insert(library, ExplicitImageState::ReservedByBringUp);
    let bridge = LoadBridge {
        api: &api,
        game_files: &root,
        images: Mutex::new(images),
    };
    let relative = CString::new("lib/arm64-v8a/libalready.so").unwrap();
    let mut base = 0x5a17usize;
    // SAFETY: the bridge and relative string outlive this synchronous call;
    // the test is exercising the duplicate decision before any native load.
    let result = unsafe {
        load_bridge_jni_on_load(
            std::ptr::from_ref(&bridge).cast_mut().cast(),
            HostStr::new(&relative),
            &mut base,
        )
    };
    assert_eq!(result, 6);
    assert_eq!(EXEC_LOADS.load(Ordering::SeqCst), 0, "must not remap the existing image");
    assert_eq!(base, 0x5a17, "a rejected duplicate must not publish a base");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn explicit_load_failure_fetches_the_existing_error_channel() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    EXEC_LOADS.store(0, Ordering::SeqCst);
    LAST_ERROR_CALLS.store(0, Ordering::SeqCst);
    FETCHED_ERROR.lock().unwrap_or_else(|p| p.into_inner()).clear();
    let root = std::env::temp_dir().join(format!(
        "rudroid-guest-load-error-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    let library = root.join("lib/arm64-v8a/libbroken.so");
    std::fs::create_dir_all(library.parent().unwrap()).unwrap();
    std::fs::write(&library, b"fixture").unwrap();
    let root = root.canonicalize().unwrap();

    let api = ElfLoadApi {
        from_path: counted_load,
        load_base: zero_base,
        symbol: zero_symbol,
        destroy: destroy_nothing,
        last_error: counted_last_error,
        exec_load: Some(reentrant_load),
        exec_symbol: Some(zero_symbol),
        exec_symbol_all: None,
        exec_load_base: Some(zero_base),


    };
    let bridge = LoadBridge {
        api: &api,
        game_files: &root,
        images: Mutex::new(HashMap::new()),
    };
    let relative = CString::new("lib/arm64-v8a/libbroken.so").unwrap();
    REENTER_BRIDGE.with(|slot| slot.set(std::ptr::from_ref(&bridge).cast_mut().cast()));
    let mut base = 0x5a17usize;
    // SAFETY: the bridge and relative string outlive this synchronous call;
    // the fixture fails before constructing any guest mapping.
    let result = unsafe {
        load_bridge_jni_on_load(
            std::ptr::from_ref(&bridge).cast_mut().cast(),
            HostStr::new(&relative),
            &mut base,
        )
    };
    assert_eq!(result, 4);
    assert_eq!(REENTER_RESULT.with(|slot| slot.get()), 6, "recursive same-image load must see InProgress");
    assert_eq!(EXEC_LOADS.load(Ordering::SeqCst), 1);
    assert_eq!(
        LAST_ERROR_CALLS.load(Ordering::SeqCst),
        2,
        "the C error channel is queried for its length, then filled"
    );
    assert_eq!(
        FETCHED_ERROR.lock().unwrap_or_else(|p| p.into_inner()).as_slice(),
        b"fixture relocation failure",
        "the bridge must fetch the actual pinned error text, not only its length"
    );
    assert_eq!(base, 0x5a17, "a failed load must not publish a base");
    let _ = std::fs::remove_dir_all(root);
}
