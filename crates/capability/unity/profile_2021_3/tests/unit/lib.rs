//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use std::sync::Mutex;

#[test]
fn swappy_fallback_interval_follows_the_reference_clamp() {
    assert_eq!(swappy::refresh_rate(144), 144);
    assert_eq!(swappy::refresh_rate(1000), 1000);
    assert_eq!(swappy::refresh_rate(0), 60);
    assert_eq!(swappy::refresh_rate(-5), 60);
    assert_eq!(swappy::refresh_rate(1001), 60);
    assert_eq!(swappy::fallback_interval_ns(60), 16_666_666);
    assert_eq!(swappy::fallback_interval_ns(144), 6_944_444);
}

#[test]
fn swappy_missing_symbol_logs_the_reference_line() {
    let logs = std::cell::RefCell::new(Vec::<String>::new());
    // 144 Hz like the live test: both write the shared fallback slot.
    let installed = unsafe { swappy::install(None, 144, &|m| logs.borrow_mut().push(m.to_owned())) };
    assert_eq!(installed, None);
    assert_eq!(
        *logs.borrow(),
        ["SwappyGL_getSwapIntervalNS not found; fallback not installed"]
    );
}

/// The native phase end to end on a hand-assembled
/// `SwappyGL_getSwapIntervalNS` (returns `UINT64_MAX`) that only the
/// `.symtab`-capable lookup finds: init subscribes to the Unity event,
/// the event resolves the target through `so_symbol_all` and detours it;
/// callers then get the refresh interval, while a valid interval from
/// the original passes through unchanged.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
#[test]
fn swappy_fallback_installs_through_so_symbol_all_on_the_unity_event() {
    use rudroid_common::arm64_detour::{flush_code, patch_word};
    use rudroid_common::host_api::HostApiV0;
    use rudroid_common::registry::{RudroidModuleLoadedCallback, HostStr};
    use std::sync::atomic::AtomicUsize;

    const MOVN_X0_0: u32 = 0x9280_0000; // mov x0, #-1 (UINT64_MAX)
    const MOVZ_X0_1234: u32 = 0xD280_9A40; // mov x0, #1234
    const RET: u32 = 0xD65F_03C0;

    /// A leaked block owning every page it spans; `patch_word`
    /// re-seals the page RX, which makes the block executable.
    fn function(words: [u32; 2]) -> usize {
        const BLOCK: usize = 64 * 1024;
        let layout = std::alloc::Layout::from_size_align(BLOCK, BLOCK).unwrap();
        // SAFETY: non-zero size; leaked for the process lifetime.
        let base = unsafe { std::alloc::alloc_zeroed(layout) } as usize;
        assert_ne!(base, 0);
        // SAFETY: inside the fresh block; flushed before execution.
        unsafe {
            (base as *mut u32).write(words[0]);
            ((base + 4) as *mut u32).write(words[1]);
            flush_code(base as *mut u8, 8);
            patch_word(base, words[0]).expect("seal the block executable");
        }
        base
    }
    fn call(address: usize) -> u64 {
        // SAFETY: hand-assembled `() -> u64` leaf functions.
        unsafe { std::mem::transmute::<usize, extern "C" fn() -> u64>(address)() }
    }

    static SUBSCRIPTION: Mutex<Option<(u32, RudroidModuleLoadedCallback)>> = Mutex::new(None);
    static MODULE: AtomicUsize = AtomicUsize::new(0);
    static TARGET: AtomicUsize = AtomicUsize::new(0);
    static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

    extern "C" fn register(
        kind: u32,
        callback: Option<RudroidModuleLoadedCallback>,
        _user: *mut c_void,
    ) -> c_int {
        *SUBSCRIPTION.lock().unwrap() = Some((kind, callback.unwrap()));
        0
    }
    extern "C" fn get_bool(key: HostStr<'_>, fallback: c_int) -> c_int {
        if key.to_cstr() == Some(c"unity.swappy_invalid_interval_fallback") { 1 } else { fallback }
    }
    extern "C" fn get_i64(key: HostStr<'_>, fallback: i64) -> i64 {
        if key.to_cstr() == Some(c"device.displayRefreshRate") { 144 } else { fallback }
    }
    /// Dynamic-symbol lookup: the hidden Swappy function is not there.
    extern "C" fn so_symbol(_module: *mut c_void, _name: HostStr<'_>) -> usize {
        0
    }
    /// `.symtab`-capable lookup on the module the event reported.
    extern "C" fn so_symbol_all(module: *mut c_void, name: HostStr<'_>) -> usize {
        if module as usize == MODULE.load(Ordering::Acquire)
            && name.to_cstr() == Some(c"SwappyGL_getSwapIntervalNS")
        {
            TARGET.load(Ordering::Acquire)
        } else {
            0
        }
    }
    extern "C" fn log(tag: HostStr<'_>, message: HostStr<'_>) {
        let (tag, message) = (tag.to_cstr().unwrap_or_default(), message.to_cstr().unwrap_or_default());
        LOGS.lock().unwrap().push(format!(
            "{}: {}",
            tag.to_string_lossy(),
            message.to_string_lossy()
        ));
    }

    let api: &'static HostApiV0 = Box::leak(Box::new(HostApiV0 {
        config_get_bool: get_bool,
        config_get_i64: get_i64,
        so_symbol,
        log,
        register_module_loaded: register,
        so_symbol_all,
        ..HostApiV0::stubbed()
    }));
    // SAFETY: the leaked table uses static callbacks and the owned idle test
    // image; the callback below delivers only that image's module event.
    assert_eq!(unsafe { rudroid_plugin_init(Some(api)) }, 0);
    let (kind, callback) = SUBSCRIPTION.lock().unwrap().expect("Unity subscription");
    assert_eq!(kind, ModuleKind::Unity as u32);

    let target = function([MOVN_X0_0, RET]);
    assert_eq!(call(target), u64::MAX);
    let module = function([RET, RET]);
    MODULE.store(module, Ordering::Release);
    TARGET.store(target, Ordering::Release);
    let event = RudroidModuleEvent {
        kind: ModuleKind::Unity as u32,
        base: module,
        name: rudroid_common::registry::HostStr::new(c"lib/arm64-v8a/libunity.so"),
    };
    callback(Some(&event), std::ptr::null_mut());
    assert_eq!(
        LOGS.lock().unwrap().last().cloned(),
        Some(format!(
            "UNITY2021: installed invalid-interval fallback at {target:#x} (144 Hz)"
        ))
    );
    assert_eq!(call(target), 6_944_444, "UINT64_MAX becomes the 144 Hz interval");
    // One install attempt per process (reference g_install_attempted).
    callback(Some(&event), std::ptr::null_mut());
    assert_eq!(
        LOGS.lock().unwrap().iter().filter(|l| l.contains("installed")).count(),
        1
    );

    let trampoline = swappy::ORIGINAL.address();
    assert_eq!(call(trampoline), u64::MAX, "the original still answers UINT64_MAX");
    let valid = function([MOVZ_X0_1234, RET]);
    // SAFETY: a hand-assembled `() -> u64` leaf function.
    unsafe { swappy::ORIGINAL.set(valid) };
    assert_eq!(call(target), 1234, "a valid interval passes through");
    // SAFETY: the trampoline read above.
    unsafe { swappy::ORIGINAL.set(trampoline) };
}

#[test]
fn spec_matches_reference_literal() {
    // Lifecycle ABI against projects/unityloader/plugins/unity_2021_3.
    assert_eq!(ENGINE_SPEC.id, "unity_2021_3");
    assert_eq!(ENGINE_SPEC.log_tag, c"UNITY2021");
    assert_eq!(ENGINE_SPEC.player_class, "com/unity3d/player/UnityPlayer");
    assert_eq!(ENGINE_SPEC.player_constructor_signature, "()V");
    assert_eq!(ENGINE_SPEC.init_jni_signature, "(Landroid/content/Context;)V");
    assert_eq!(ENGINE_SPEC.flags, RUDROID_UNITY_PROFILE_NONE);
    assert_eq!(ENGINE_SPEC.init_jni_int_arg, 0);
    assert_eq!(ENGINE_SPEC.render_sleep_us, 0);
    assert!(ENGINE_SPEC.extra_player_callbacks.is_empty());
    assert!(ENGINE_SPEC.publishes_current_activity);
    assert!(ENGINE_SPEC.register_java_services.is_some());
    assert_eq!(ENGINE_SPEC.validate(), Ok(()));
}

/// The registration contract: the profile registers only against a live
/// session, and the registry-bound struct carries the spec verbatim
/// (the integration test proves the dynamic half).
#[test]
fn registration_contract_names_its_requirements() {
    assert_eq!(rudroid_common::host_api::HOST_API_VERSION, 4);
    assert!(!ENGINE_SPEC.player_class.is_empty());
    assert!(!ENGINE_SPEC.player_constructor_signature.is_empty());
    assert!(!ENGINE_SPEC.init_jni_signature.is_empty());
}
