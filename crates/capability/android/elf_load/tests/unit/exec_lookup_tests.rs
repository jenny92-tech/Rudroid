//! INPUT:  private items of `src/exec.rs` via `super`
//! OUTPUT:  `lookup_tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/exec.rs`; declared there with `#[path]`
use super::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::CStr;
use std::sync::atomic::{AtomicI32, Ordering};

static SERIAL: Mutex<()> = Mutex::new(());
static REENTRANT_SOURCE: AtomicI32 = AtomicI32::new(-1);
thread_local! {
    static COST: Cell<(bool, usize, usize)> = const { Cell::new((false, 0, 0)) };
}
struct Allocator;
fn record(bytes: usize) {
    let _ = COST.try_with(|cost| {
        let (enabled, allocations, requested) = cost.get();
        if enabled { cost.set((true, allocations + 1, requested + bytes)); }
    });
}
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, bytes: usize) -> *mut u8 {
        record(bytes);
        unsafe { System.realloc(ptr, layout, bytes) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

fn measure(kind: &str, operations: usize, f: impl FnOnce()) -> (usize, usize) {
    COST.with(|cost| cost.set((true, 0, 0)));
    let started = std::time::Instant::now();
    f();
    let elapsed = started.elapsed().as_nanos();
    let (_, allocations, requested) = COST.with(|cost| cost.replace((false, 0, 0)));
    println!("ELF_COST {{\"kind\":\"{kind}\",\"operations\":{operations},\"allocations\":{allocations},\"requested_bytes\":{requested},\"elapsed_ns\":{elapsed}}}");
    (allocations, requested)
}

unsafe extern "C" fn answer(name: *const c_char, _user: *mut c_void) -> usize {
    match unsafe { CStr::from_ptr(name) }.to_bytes() {
        b"host_hit" | b"inner" => 0x1234,
        _ => 0,
    }
}

unsafe extern "C" fn reentrant(name: *const c_char, _user: *mut c_void) -> usize {
    if unsafe { CStr::from_ptr(name) } != c"outer" { return 0; }
    // Fail instead of deadlocking on an old implementation. After
    // probing, exercise real unregister/register/lookup re-entry.
    let Ok(probe) = symbol_source_registry().try_lock() else { return 0; };
    drop(probe);
    let id = REENTRANT_SOURCE.load(Ordering::Relaxed);
    if unsafe { rudroid_elf_symbol_source_unregister(id) } != 0 { return 0; }
    let inner = unsafe { rudroid_elf_symbol_source_register(c"inner".as_ptr(), answer, std::ptr::null_mut()) };
    let found = host_symbol_lookup("inner").unwrap_or(0);
    unsafe { rudroid_elf_symbol_source_unregister(inner) };
    found
}

#[test]
fn source_callbacks_can_reenter_and_withdraw_themselves() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let id = unsafe { rudroid_elf_symbol_source_register(c"reentrant".as_ptr(), reentrant, std::ptr::null_mut()) };
    REENTRANT_SOURCE.store(id, Ordering::Relaxed);
    let found = host_symbol_lookup("outer");
    // Cleanup also on red: the old callback returned without revoking.
    unsafe { rudroid_elf_symbol_source_unregister(id) };
    assert_eq!(found, Some(0x1234), "source lookup must release its registry lock before callbacks");
    assert_eq!(host_symbol_lookup("outer"), None, "withdrawal affects the next lookup");
}

unsafe extern "C" fn reentrant_versioned(name: *const c_char, file: *const c_char, version: *const c_char, user: *mut c_void) -> usize {
    if file.is_null() || version.is_null() { return 0; }
    if unsafe { CStr::from_ptr(file) } != c"libc.so" || unsafe { CStr::from_ptr(version) } != c"LIBC" { return 0; }
    unsafe { reentrant(name, user) }
}

#[test]
fn versioned_source_callbacks_can_reenter_and_withdraw_themselves() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let id = unsafe { rudroid_elf_symbol_source_register_versioned(
        c"reentrant-versioned".as_ptr(), Some(reentrant_versioned), std::ptr::null_mut(),
    ) };
    assert!(id > 0);
    REENTRANT_SOURCE.store(id, Ordering::Relaxed);
    let found = host_symbol_lookup_versioned("outer", Some(SymbolVersion { file: "libc.so", name: "LIBC" }));
    unsafe { rudroid_elf_symbol_source_unregister(id); }
    assert_eq!(found, Some(0x1234));
    assert_eq!(host_symbol_lookup_versioned("outer", Some(SymbolVersion { file: "libc.so", name: "LIBC" })), None);
}

#[test]
fn host_lookup_uses_one_owned_c_string_without_a_second_copy() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let id = unsafe { rudroid_elf_symbol_source_register(c"cost".as_ptr(), answer, std::ptr::null_mut()) };
    assert_eq!(host_symbol_lookup("host_hit"), Some(0x1234));
    let (allocations, _) = measure("host_hit", 10_000, || {
        for _ in 0..10_000 { assert_eq!(host_symbol_lookup("host_hit"), Some(0x1234)); }
    });
    unsafe { rudroid_elf_symbol_source_unregister(id) };
    assert!(allocations <= 10_000, "one conversion suffices for an ordinary borrowed Rust name");
}

#[test]
fn guest_chain_queries_do_not_allocate_module_list_copies() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let path = std::env::temp_dir().join(format!("rudroid-chain-cost-{}.so", std::process::id()));
    std::fs::write(&path, crate::fixture::build_minimal_aarch64_so()).unwrap();
    let module = unsafe { ExecutableModule::map_and_register(&path, &HashMap::new()) }.unwrap();
    let expected = unsafe { &*module }.symbol(crate::fixture::ANSWER_SYMBOL).unwrap();
    let (allocations, _) = measure("guest_chain_hit_miss", 20_000, || {
        for _ in 0..10_000 {
            assert_eq!(guest_chain_lookup(crate::fixture::ANSWER_SYMBOL), Some(expected));
            assert_eq!(guest_chain_lookup("not_present"), None);
        }
    });
    unsafe { rudroid_elf_exec_destroy(module) };
    std::fs::remove_file(path).unwrap();
    assert_eq!(allocations, 0, "module snapshots must share storage");
}

#[test]
fn image_snapshot_keeps_destroyed_handle_metadata_alive_then_releases_it() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let path = std::env::temp_dir().join(format!("rudroid-chain-lease-{}.so", std::process::id()));
    std::fs::write(&path, crate::fixture::build_minimal_aarch64_so()).unwrap();
    let handle = unsafe { ExecutableModule::map_and_register(&path, &HashMap::new()) }.unwrap();
    let snapshot = guest_image_chain();
    let image = snapshot.iter().find(|image| Arc::as_ptr(image) == handle).unwrap();
    let weak = Arc::downgrade(image);
    let expected = image.symbol(crate::fixture::ANSWER_SYMBOL).unwrap();
    unsafe { rudroid_elf_exec_destroy(handle) };
    assert_eq!(guest_chain_lookup(crate::fixture::ANSWER_SYMBOL), None);
    assert_eq!(image.symbol(crate::fixture::ANSWER_SYMBOL), Some(expected));
    assert!(weak.upgrade().is_some(), "in-flight snapshot pins the mapping");
    drop(snapshot);
    assert!(weak.upgrade().is_none(), "last snapshot releases the withdrawn image");
    std::fs::remove_file(path).unwrap();
}

#[test]
fn reopened_module_survives_root_destroy_until_last_dlclose() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let path = std::env::temp_dir().join(format!("rudroid-reopen-{}.so", std::process::id()));
    std::fs::write(&path, crate::fixture::build_minimal_aarch64_so()).unwrap();
    let handle = unsafe { ExecutableModule::map_and_register(&path, &HashMap::new()) }.unwrap();
    let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let first = reopen_guest(&name).unwrap();
    let second = reopen_guest(&name).unwrap();
    assert_eq!(first, handle); assert_eq!(second, handle);
    let weak = { let snapshot = guest_image_chain();
        Arc::downgrade(snapshot.iter().find(|m| Arc::as_ptr(m) == handle).unwrap()) };
    let expected = guest_handle_symbol(first, crate::fixture::ANSWER_SYMBOL).unwrap();
    unsafe { rudroid_elf_exec_destroy(handle) };
    assert_eq!(guest_handle_symbol(first, crate::fixture::ANSWER_SYMBOL), Some(expected));
    assert!(close_guest(first)); assert!(weak.upgrade().is_some());
    assert!(close_guest(second)); assert!(weak.upgrade().is_none());
    assert!(!close_guest(second));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn dynamic_queries_and_reopen_share_storage_without_hot_allocations() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    let path = std::env::temp_dir().join(format!("rudroid-dl-cost-{}.so", std::process::id()));
    std::fs::write(&path, crate::fixture::build_minimal_aarch64_so()).unwrap();
    let handle = unsafe { ExecutableModule::map_and_register(&path, &HashMap::new()) }.unwrap();
    let name = std::ffi::CString::new(path.to_str().unwrap()).unwrap();
    let symbol = std::ffi::CString::new(crate::fixture::ANSWER_SYMBOL).unwrap();
    let expected = dynamic_symbol(&symbol).unwrap();
    let (allocations, bytes) = measure("dynamic-query-reopen-close", 40_000, || {
        for _ in 0..10_000 {
            assert_eq!(dynamic_symbol(&symbol), Some(expected));
            let reopened = reopen_guest(&name).unwrap();
            assert!(guest_address_info(expected).is_some());
            assert!(close_guest(reopened));
        }
    });
    assert_eq!((allocations,bytes),(0,0));
    unsafe { rudroid_elf_exec_destroy(handle) };
    std::fs::remove_file(path).unwrap();
}
