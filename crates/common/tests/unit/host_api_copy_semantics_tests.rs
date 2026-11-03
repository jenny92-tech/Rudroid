//! INPUT:  private items of `src/host_api.rs` via `super`
//! OUTPUT:  `copy_semantics_tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/host_api.rs`; declared there with `#[path]`
use super::*;

/// Ticket 80: the table is a plain value copy — no field is dropped by a
/// future slot addition (the compiler enforces exhaustiveness in
/// `copy_table`, so this guard only needs to pin behavior).
fn copy_table(api: &HostApiV0) -> HostApiV0 {
    *api
}

#[test]
fn copy_preserves_every_slot() {
    extern "C" fn log_capture(_tag: HostStr<'_>, _message: HostStr<'_>) {}
    let api = HostApiV0 { log: log_capture, ..HostApiV0::stubbed() };
    let copy = copy_table(&api);
    assert_eq!(copy.abi_version, api.abi_version);
    assert_eq!(copy.log as usize, api.log as usize);
    assert_eq!(copy.fire_module_event as usize, api.fire_module_event as usize);
    assert_eq!(copy.register_module_loaded as usize, api.register_module_loaded as usize);
    assert_eq!(copy.so_symbol_all as usize, api.so_symbol_all as usize);
}

/// log_message escapes embedded NULs and forwards empty messages; the
/// tag and message arrive as one C-string pair each.
#[test]
fn log_message_escapes_nul_and_forwards_empty() {
    static CAPTURED: std::sync::Mutex<Vec<(String, String)>> =
        std::sync::Mutex::new(Vec::new());
    extern "C" fn capture(tag: HostStr<'_>, message: HostStr<'_>) {
        let text = |s: HostStr<'_>| s.to_cstr().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        CAPTURED.lock().unwrap().push((text(tag), text(message)));
    }
    log_message(capture, c"t", "");
    log_message(capture, c"t", "a\0b");
    let captured = CAPTURED.lock().unwrap().clone();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0], ("t".to_owned(), "".to_owned()));
    assert_eq!(captured[1], ("t".to_owned(), "a\\0b".to_owned()));
}

#[test]
fn string_getter_keeps_the_c_pointer_abi_and_handles_small_capacities() {
    type RawGetter = unsafe extern "C" fn(HostStr<'_>, HostStr<'_>, *mut c_char, u32) -> c_int;
    assert_eq!(std::mem::size_of::<ConfigGetStringFn>(), std::mem::size_of::<RawGetter>());
    assert_eq!(std::mem::align_of::<ConfigGetStringFn>(), std::mem::align_of::<RawGetter>());
    unsafe extern "C" fn getter(key: HostStr<'_>, _: HostStr<'_>, out: *mut c_char, size: u32) -> c_int {
        if key.to_cstr() != Some(c"present") { return 0; }
        let count = b"abcd".len().min(size as usize - 1);
        // SAFETY: the getter contract supplies a nonempty writable buffer;
        // this copies at most size - 1 bytes and appends its terminator.
        unsafe {
            std::ptr::copy_nonoverlapping(b"abcd".as_ptr().cast(), out, count);
            out.add(count).write(0);
        }
        1
    }
    let api = HostApiV0 {
        // SAFETY: the static callback bounds its writes, terminates successes
        // and retains neither input strings nor the output buffer.
        config_get_string: unsafe { ConfigGetStringFn::new(getter) },
        ..HostApiV0::stubbed()
    };
    assert_eq!(api.config_string(c"present", 0).as_deref(), Some(""));
    assert_eq!(api.config_string(c"present", 1).as_deref(), Some(""));
    assert_eq!(api.config_string(c"present", 3).as_deref(), Some("ab"));
    assert_eq!(api.config_string(c"present", 5).as_deref(), Some("abcd"));
    assert_eq!(api.config_string(c"missing", 5), None);
    assert_eq!(api.config_string_with_fallback(c"missing", c"fallback", 5), Some((false, "fall".to_owned())));
    assert_eq!(api.config_string_with_fallback(c"present", c"fallback", 5), Some((true, "abcd".to_owned())));
    // A size the C ABI cannot represent is rejected before allocating or
    // calling the callback (the stub would abort if it were called).
    if let Some(too_large) = (u32::MAX as usize).checked_add(1) {
        assert_eq!(HostApiV0::stubbed().config_string(c"present", too_large), None);
    }
}
