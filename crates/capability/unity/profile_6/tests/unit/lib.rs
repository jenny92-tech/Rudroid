//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

#[test]
fn spec_matches_reference_literal() {
    // Lifecycle ABI against projects/unityloader/plugins/unity_6.
    assert_eq!(ENGINE_SPEC.id, "unity_6");
    assert_eq!(ENGINE_SPEC.log_tag, c"UNITY6");
    assert_eq!(
        ENGINE_SPEC.player_class,
        "com/unity3d/player/UnityPlayerForActivityOrService"
    );
    assert_eq!(
        ENGINE_SPEC.player_constructor_signature,
        "(Landroid/content/Context;)V"
    );
    assert_eq!(ENGINE_SPEC.init_jni_signature, "(Landroid/content/Context;I)V");
    assert_eq!(ENGINE_SPEC.flags, RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG);
    assert_eq!(ENGINE_SPEC.init_jni_int_arg, 0);
    assert_eq!(ENGINE_SPEC.render_sleep_us, 0, "pacing uses target_fps, not the legacy extra 16ms");
    let extras: Vec<_> = ENGINE_SPEC.extra_player_callbacks.iter().map(|c| (c.name, c.body)).collect();
    assert_eq!(extras, [(c"initializeGoogleAr", CallbackBody::False)]);
    assert!(ENGINE_SPEC.publishes_current_activity);
    assert!(ENGINE_SPEC.register_java_services.is_some());
    assert_eq!(ENGINE_SPEC.validate(), Ok(()));
}

#[test]
fn tls_guard_without_the_export_is_skipped_with_the_reference_log() {
    let logs = std::cell::RefCell::new(Vec::<String>::new());
    let patched = unsafe { tls_guard::install(None, &|m| logs.borrow_mut().push(m.to_owned())) };
    assert_eq!(patched, None);
    assert_eq!(*logs.borrow(), ["il2cpp_runtime_invoke not found; TLS guard skipped"]);
}

/// Hand-built `il2cpp_runtime_invoke` images: entry `B` to an
/// implementation that carries the reference epilogue pattern.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
mod tls_guard_live {
    use super::super::tls_guard;
    use rudroid_common::arm64_detour::encode_b;

    const IMPLEMENTATION: usize = 0x100;

    /// A leaked block that owns every page it spans (64 KiB covers the
    /// largest AArch64 Linux page size, so the guard's mprotect window
    /// never touches foreign heap data): word 0 is `entry`, the pattern
    /// sits at word `index` of the implementation; everything else is
    /// `udf`. Nothing here is executed — the guard only reads and
    /// patches.
    fn image(entry: Option<u32>, index: Option<usize>) -> usize {
        const BLOCK: usize = 64 * 1024;
        let layout = std::alloc::Layout::from_size_align(BLOCK, BLOCK).unwrap();
        // SAFETY: non-zero size; leaked for the process lifetime.
        let base = unsafe { std::alloc::alloc_zeroed(layout) } as usize;
        assert_ne!(base, 0);
        let words = base as *mut u32;
        let entry = entry.unwrap_or_else(|| encode_b(base, base + IMPLEMENTATION).unwrap());
        // SAFETY: all writes stay inside the fresh block.
        unsafe {
            words.write(entry);
            if let Some(index) = index {
                for (offset, word) in tls_guard::RUNTIME_INVOKE_TLS_CHECK.iter().enumerate() {
                    words.add(IMPLEMENTATION / 4 + index + offset).write(*word);
                }
            }
        }
        base
    }

    fn word(address: usize) -> u32 {
        // SAFETY: readable test page.
        unsafe { (address as *const u32).read_volatile() }
    }

    #[test]
    fn pattern_is_found_inside_the_reference_scan_window() {
        for index in [0, tls_guard::SCAN_INSTRUCTIONS - 4] {
            let base = image(None, Some(index));
            let found = unsafe { tls_guard::find_check(base) };
            assert_eq!(found, Some(base + IMPLEMENTATION + index * 4), "index {index}");
        }
        // A pattern that would extend past the window is not matched.
        let base = image(None, Some(tls_guard::SCAN_INSTRUCTIONS - 3));
        assert_eq!(unsafe { tls_guard::find_check(base) }, None);
        // An entry that is not a direct B is not the Unity 6 wrapper.
        let base = image(Some(tls_guard::NOP), Some(0));
        assert_eq!(unsafe { tls_guard::find_check(base) }, None);
        // No pattern at all.
        let base = image(None, None);
        assert_eq!(unsafe { tls_guard::find_check(base) }, None);
    }

    #[test]
    fn unreadable_export_is_rejected_without_dereferencing_it() {
        let logs = std::cell::RefCell::new(Vec::<String>::new());
        assert_eq!(unsafe { tls_guard::install(Some(4), &|m| logs.borrow_mut().push(m.to_owned())) }, None);
        assert_eq!(*logs.borrow(), ["runtime-invoke scan window unreadable; guard skipped"]);
    }

    #[test]
    fn install_replaces_only_the_comparison_branch() {
        let base = image(None, Some(17));
        let logs = std::cell::RefCell::new(Vec::<String>::new());
        let check = unsafe {
            tls_guard::install(Some(base), &|m| logs.borrow_mut().push(m.to_owned()))
        }
        .expect("guard installs");
        assert_eq!(check, base + IMPLEMENTATION + 17 * 4);
        let pattern = tls_guard::RUNTIME_INVOKE_TLS_CHECK;
        assert_eq!(
            [word(check), word(check + 4), word(check + 8), word(check + 12)],
            [pattern[0], pattern[1], pattern[2], tls_guard::NOP]
        );
        assert_eq!(
            *logs.borrow(),
            [format!("installed glibc/Bionic runtime-invoke TLS guard at {check:#x}")]
        );

        let absent = image(None, None);
        logs.borrow_mut().clear();
        assert_eq!(
            unsafe { tls_guard::install(Some(absent), &|m| logs.borrow_mut().push(m.to_owned())) },
            None
        );
        assert_eq!(*logs.borrow(), ["runtime-invoke TLS pattern not present; guard skipped"]);
    }

    /// The reference native phase end to end: init refuses before the
    /// JNI phase, then subscribes to the il2cpp module event, and the
    /// event resolves `il2cpp_runtime_invoke` and patches the guard.
    #[test]
    fn plugin_init_installs_the_guard_on_the_il2cpp_event() {
        use super::super::*;
        use rudroid_common::host_api::HostApiV0;
        use std::sync::Mutex;
        use rudroid_common::registry::{RudroidModuleLoadedCallback, HostStr};
        use std::sync::atomic::AtomicUsize;

        static SUBSCRIPTION: Mutex<Option<(u32, RudroidModuleLoadedCallback)>> = Mutex::new(None);
        static IMAGE: AtomicUsize = AtomicUsize::new(0);
        static LOGS: Mutex<Vec<String>> = Mutex::new(Vec::new());

        extern "C" fn register(
            kind: u32,
            callback: Option<RudroidModuleLoadedCallback>,
            _user: *mut c_void,
        ) -> c_int {
            *SUBSCRIPTION.lock().unwrap() = Some((kind, callback.unwrap()));
            0
        }
        extern "C" fn so_symbol(module: *mut c_void, name: HostStr<'_>) -> usize {
            let image = IMAGE.load(Ordering::Acquire);
            if module as usize == image && name.to_cstr() == Some(c"il2cpp_runtime_invoke") {
                image
            } else {
                0
            }
        }
        extern "C" fn log(tag: HostStr<'_>, message: HostStr<'_>) {
            let (tag, message) = (tag.to_cstr().unwrap(), message.to_cstr().unwrap());
            LOGS.lock().unwrap().push(format!(
                "{}: {}",
                tag.to_string_lossy(),
                message.to_string_lossy()
            ));
        }

        let api: &'static HostApiV0 = Box::leak(Box::new(HostApiV0 {
            so_symbol,
            log,
            register_module_loaded: register,
            so_symbol_all: so_symbol,
            ..HostApiV0::stubbed()
        }));
        assert_eq!(
            // SAFETY: the leaked fixture table supplies static callbacks and
            // resolves only the owned, idle test image used below.
            unsafe { rudroid_plugin_init(Some(api)) },
            -1,
            "the native phase requires the JNI phase (reference g_jni_registered)"
        );
        JNI_REGISTERED.store(true, Ordering::Release);
        // SAFETY: the same pinned fixture table and idle image as above.
        assert_eq!(unsafe { rudroid_plugin_init(Some(api)) }, 0);
        let (kind, callback) = SUBSCRIPTION.lock().unwrap().expect("il2cpp subscription");
        assert_eq!(kind, ModuleKind::Il2cpp as u32);

        let base = image(None, Some(5));
        IMAGE.store(base, Ordering::Release);
        let event = RudroidModuleEvent {
            kind: ModuleKind::Il2cpp as u32,
            base,
            name: HostStr::new(c"lib/arm64-v8a/libil2cpp.so"),
        };
        callback(Some(&event), std::ptr::null_mut());
        let check = base + IMPLEMENTATION + 5 * 4;
        assert_eq!(word(check + 12), tls_guard::NOP, "b.ne __stack_chk_fail is gone");
        assert_eq!(word(check + 8), tls_guard::RUNTIME_INVOKE_TLS_CHECK[2], "cmp stays");
        assert_eq!(
            *LOGS.lock().unwrap(),
            [format!("UNITY6: installed glibc/Bionic runtime-invoke TLS guard at {check:#x}")]
        );
    }
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
