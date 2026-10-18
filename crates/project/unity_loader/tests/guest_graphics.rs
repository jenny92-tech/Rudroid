//! INPUT: dynamically rebuilt SDL/ELF capabilities, guest EGL/GLES C fixture, Xvfb/Mesa
//! OUTPUT: executable guest imports draw, read pixels, swap, signal hooks and release GPU objects
//! POS: validates the production loader connection to the same SDL graphics service
#[cfg(not(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl)))]
#[test]
#[ignore = "requires Linux ARM64 SDL kit with Xvfb/Mesa; use docker-rust.sh test --sdl"]
fn mapped_guest_graphics_requires_sdl_arm64() {}

#[cfg(all(target_os = "linux", target_arch = "aarch64", rudroid_kit_sdl))]
mod real {

    use rudroid_common::registry::RudroidAndroidHooks;
    use rudroid_test_support::cargo::{
        assert_kit_features, build_loader_fixture, ensure_workspace_cdylib,
    };
    use rudroid_unity_loader::{bring_up, guest, load_deploy_dir, runtime, GuestPlan, Selection};
    use std::ffi::{c_char, c_void};
    use std::path::Path;
    use std::process::Command;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static PRESENTED: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn presented() {
        PRESENTED.fetch_add(1, Ordering::Relaxed);
    }
    fn compile(source: &Path, output: &Path, placeholder: bool) {
        let mut command = Command::new("gcc");
        command.args([
            "-shared",
            "-fPIC",
            "-nostdlib",
            "-fno-stack-protector",
            "-O2",
        ]);
        if placeholder {
            command.arg("-DRUDROID_PLACEHOLDER");
        }
        let result = command.arg(source).arg("-o").arg(output).output().unwrap();
        assert!(
            result.status.success(),
            "guest compile: {}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    fn mapped_guest_draws_through_static_imports_and_dynamic_proc_lookup() {
        assert_kit_features(platform_sdl_runtime::SDL_ENABLED);
        let root = std::env::temp_dir().join(format!("rudroid-guest-graphics-{}", std::process::id()));
        let deploy = root.join("kit");
        let game = root.join("game");
        let libs = game.join("lib/arm64-v8a");
        std::fs::create_dir_all(&deploy).unwrap();
        std::fs::create_dir_all(&libs).unwrap();
        for (artifact, name) in [
            (
                build_loader_fixture("probe_capability", "rudroid_probe_capability"),
                "libandroid_base.so",
            ),
            (
                ensure_workspace_cdylib("rudroid-linux-sdl-runtime", "platform_sdl_runtime"),
                "libplatform_sdl_runtime.so",
            ),
            (
                ensure_workspace_cdylib("rudroid-android-elf-load", "android_elf_load"),
                "libandroid_elf_load.so",
            ),
        ] {
            std::fs::copy(artifact, deploy.join(name)).unwrap();
        }
        let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/guest_graphics.c");
        compile(&source, &libs.join("libmain.so"), false);
        compile(&source, &libs.join("libil2cpp.so"), true);
        let config = rudroid_host_config::HostConfig::parse(&format!(
            "[device]\ndisplayWidth=64\ndisplayHeight=64\n[paths]\ngame_files={:?}\n",
            game.to_str().unwrap()
        ))
        .unwrap();
        let capabilities =
            bring_up(unsafe { load_deploy_dir(&deploy, &Selection::All) }.unwrap(), None).unwrap();
        let hooks = RudroidAndroidHooks {
            flush_preferences: None,
            signal_vsync: Some(presented),
        };
        assert_eq!(runtime::register_android_hooks(Some(&hooks)), 0);
        let report = runtime::bring_up_sdl_session(&capabilities, &config)
            .unwrap()
            .unwrap();
        assert_eq!((report.window_w, report.window_h), (64, 64));
        let error = runtime::bring_up_sdl_session(&capabilities, &config).unwrap_err();
        assert!(error.to_string().contains("already active"), "{error}");
        // The failed second activation must preserve the first source/session.
        let events =
            unsafe { guest::bring_up(&capabilities, &GuestPlan::from_config(&config).unwrap()) }.unwrap();
        let main = events
            .iter()
            .find(|event| event.kind == rudroid_common::ModuleKind::Main)
            .unwrap();
        unsafe {
            let function = |name: &str| -> unsafe extern "C" fn() -> i32 {
                let address = runtime::guest_exec_symbol(main.base, name).expect("guest entry");
                std::mem::transmute(address)
            };
            assert_eq!(
                function("rudroid_graphics_begin")(),
                0,
                "guest EGL init, shader compile/link and texture upload"
            );
            let frame = function("rudroid_graphics_frame");
            for _ in 0..6 {
                assert_eq!(frame(), 0, "actual guest triangle/readback/swap");
            }
            assert_eq!(
                PRESENTED.load(Ordering::Relaxed),
                6,
                "only successful guest presents signal the existing hook"
            );
            assert_eq!(
                function("rudroid_graphics_end")(),
                0,
                "driver glIs* confirms guest GPU objects are gone"
            );
            let sdl = capabilities
                .iter()
                .find(|c| c.descriptor.id == "platform_sdl_runtime")
                .unwrap();
            let symbol: libloading::Symbol<
                unsafe extern "C" fn(*const c_char, *mut c_void) -> usize,
            > = sdl
                .library()
                .get(runtime::sdl_api::GUEST_SYMBOL.as_bytes())
                .unwrap();
            assert_eq!(
                symbol(
                    c"definitely_not_a_graphics_api".as_ptr(),
                    std::ptr::null_mut()
                ),
                0
            );
        }
        println!("[GUEST GRAPHICS E2E] mapped ARM64 guest: shaders linked, texture uploaded, 6 triangle frames/readbacks/swaps, 6 hooks, GPU objects deleted");
        runtime::shutdown();
        std::fs::remove_dir_all(root).unwrap();
    }
}
