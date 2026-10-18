//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

static SERIAL: Mutex<()> = Mutex::new(());
thread_local! { static TRACK: Cell<bool> = const { Cell::new(false) }; }
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
struct Tracking;
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) { ALLOCS.fetch_add(1, Ordering::Relaxed); }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { unsafe { System.dealloc(ptr, layout) } }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) { ALLOCS.fetch_add(1, Ordering::Relaxed); }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static TRACKING: Tracking = Tracking;

fn allocations<T>(f: impl FnOnce() -> T) -> (T, usize) {
    struct Window;
    impl Drop for Window { fn drop(&mut self) { TRACK.with(|flag| flag.set(false)); } }
    ALLOCS.store(0, Ordering::Relaxed);
    TRACK.with(|flag| flag.set(true));
    let window = Window;
    let result = f();
    drop(window);
    (result, ALLOCS.load(Ordering::Relaxed))
}

// The test owns one accepted provider (one Arc reference in `PROVIDERS`) in
// this slot. It is only queried while owned and destroyed once after take,
// exactly as the ABI requires; callbacks never retain borrowed keys.
struct HandshakeSlot(Mutex<Option<CatalogProviderV0>>);
unsafe impl Sync for HandshakeSlot {}
static HANDSHAKE: HandshakeSlot = HandshakeSlot(Mutex::new(None));
extern "C" fn register(provider: Option<&CatalogProviderV0>) -> c_int {
    let mut slot = HANDSHAKE.0.lock().unwrap();
    if slot.is_some() { return 1; }
    *slot = provider.copied();
    0
}
extern "C" fn reject(_: Option<&CatalogProviderV0>) -> c_int { 1 }
fn api(register_catalog_provider: rudroid_common::host_api::RegisterCatalogProviderFn) -> HostApiV0 {
    HostApiV0 { register_catalog_provider, ..HostApiV0::stubbed() }
}
/// A process-lifetime table for the `Option<&'static HostApiV0>` entry.
fn table(register_catalog_provider: rudroid_common::host_api::RegisterCatalogProviderFn) -> &'static HostApiV0 {
    Box::leak(Box::new(api(register_catalog_provider)))
}
/// The handle table allocates its first node once per process; registrations
/// measured after this allocate nothing.
fn warm_provider_table() {
    PROVIDERS.remove(PROVIDERS.insert_arc(Arc::clone(provider_catalog())));
}
fn answers(provider: CatalogProviderV0) -> bool {
    let mut out = -1;
    let hit = (provider.query_int.unwrap())(provider.user,
        HostStr::new(c"android/media/AudioManager"), HostStr::new(c"getStreamVolume"), HostStr::new(c"(I)I"), Some(&mut out));
    hit == 1 && out == 100
}

#[test]
fn rejected_registration_releases_only_its_arc_ownership() {
    let _serial = SERIAL.lock().unwrap();
    let catalog = provider_catalog();
    let (api, before) = (table(reject), Arc::strong_count(catalog));
    warm_provider_table();
    let (rc, count) = allocations(|| rudroid_plugin_early_init(Some(api)));
    assert_eq!(rc, 1);
    assert_eq!(count, 0, "rejection must not clone or allocate a data graph");
    assert_eq!(Arc::strong_count(catalog), before, "rejected ownership was consumed exactly once");
}

#[test]
fn accepted_provider_shares_the_single_catalog_until_destroy() {
    let _serial = SERIAL.lock().unwrap();
    let catalog = provider_catalog();
    let (api, before) = (table(register), Arc::strong_count(catalog));
    warm_provider_table();
    let (rc, count) = allocations(|| rudroid_plugin_early_init(Some(api)));
    assert_eq!((rc, count), (0, 0));
    let provider = HANDSHAKE.0.lock().unwrap().take().unwrap();
    assert!(
        PROVIDERS.get(provider.user).is_some_and(|shared| Arc::ptr_eq(&shared, catalog)),
        "provider views the same entity, not a clone"
    );
    assert_eq!(Arc::strong_count(catalog), before + 1);
    let (answer, count) = allocations(|| answers(provider));
    assert!(answer);
    assert_eq!(count, 0, "provider queries must borrow their keys");
    (provider.destroy.unwrap())(provider.user);
    assert_eq!(Arc::strong_count(catalog), before, "destroy releases exactly the accepted owner");
    assert_eq!(catalog.query_int("android/media/AudioManager", "getStreamVolume", "(I)I", -1), 100);
}

#[test]
fn duplicate_rejection_keeps_first_provider_and_balances_both_owners() {
    let _serial = SERIAL.lock().unwrap();
    let catalog = provider_catalog();
    let (api, before) = (table(register), Arc::strong_count(catalog));
    assert_eq!(rudroid_plugin_early_init(Some(api)), 0);
    assert_eq!(Arc::strong_count(catalog), before + 1);
    let (rc, count) = allocations(|| rudroid_plugin_early_init(Some(api)));
    assert_eq!((rc, count), (1, 0));
    assert_eq!(Arc::strong_count(catalog), before + 1, "only the duplicate's owner was released");
    let first = HANDSHAKE.0.lock().unwrap().take().unwrap();
    assert!(answers(first));
    (first.destroy.unwrap())(first.user);
    assert_eq!(Arc::strong_count(catalog), before);
}

#[test]
fn ffi_queries_borrow_valid_keys_without_allocating() {
    let _serial = SERIAL.lock().unwrap();
    let blob = encode(&[Entry { class: "a/B", method: "read", sig: "()I", value: ReplyValue::Int(42) }]);
    let handle = unsafe { ffi::rudroid_reply_catalog_from_bytes(blob.as_ptr(), blob.len()) };
    assert!(!handle.is_null());
    let (_, count) = allocations(|| {
        for _ in 0..1000 {
            let mut out = -1;
            assert_eq!(unsafe { ffi::rudroid_reply_catalog_query_int(handle, c"a/B".as_ptr(), c"read".as_ptr(), c"()I".as_ptr(), &mut out) }, 1);
            assert_eq!(out, 42);
        }
    });
    unsafe { ffi::rudroid_reply_catalog_destroy(handle); }
    assert_eq!(count, 0);
}

#[test]
fn borrowed_keys_reject_null_and_invalid_utf8_without_writing_output() {
    let _serial = SERIAL.lock().unwrap();
    let blob = encode(&[Entry { class: "a/B", method: "read", sig: "()I", value: ReplyValue::Int(42) }]);
    let handle = unsafe { ffi::rudroid_reply_catalog_from_bytes(blob.as_ptr(), blob.len()) };
    let invalid = [0xffu8, 0];
    for class in [std::ptr::null(), invalid.as_ptr().cast()] {
        let mut out = 73;
        assert_eq!(unsafe { ffi::rudroid_reply_catalog_query_int(handle, class, c"read".as_ptr(), c"()I".as_ptr(), &mut out) }, 0);
        assert_eq!(out, 73);
    }
    unsafe { ffi::rudroid_reply_catalog_destroy(handle); }
}

#[test]
fn build_script_blob_decodes() {
    let _serial = SERIAL.lock().unwrap();
    let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/reply_catalog.bin"));
    assert_eq!(Catalog::from_blob(bytes).unwrap().len(), SOURCE_TABLE.len());
}

#[test]
fn provider_catalog_answers_the_setdefaults() {
    let _serial = SERIAL.lock().unwrap();
    assert_eq!(provider_catalog().query_int("android/media/AudioManager", "getStreamVolume", "(I)I", -1), 100);
    assert!(!provider_catalog().query_bool("android/media/AudioManager", "isBluetoothA2dpOn", "()Z", true));
}

#[test]
fn provider_catalog_answers_the_fmod_register_reply() {
    let _serial = SERIAL.lock().unwrap();
    assert!(provider_catalog().query_bool("org/fmod/FMOD", "checkInit", "()Z", false));
}
