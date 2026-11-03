//! INPUT:  prepared production kit and an isolated extracted base/pack tree
//! OUTPUT:  native asset lifetimes and live cross-library JNI connectivity-query acceptance
//! POS:    handheld-run ABI probe; builds no fixture and touches no original game file
use std::ffi::{c_char, c_void, CStr};

#[test]
#[ignore = "run on the handheld with RUDROID_RESOURCE_KIT pointing at the complete production kit"]
fn native_assets_share_the_host_resource_service() {
    use rudroid_common::{android_assets as q, host_api::HostApiV0};
    use rudroid_unity_loader::host_api;
    use std::{io::Read, os::fd::FromRawFd};
    let kit = std::path::PathBuf::from(
        std::env::var_os("RUDROID_RESOURCE_KIT").expect("prepared production kit required"),
    );
    let root = std::env::temp_dir().join(format!("rudroid-native-assets-{}", std::process::id()));
    assert!(!root.exists());
    std::fs::create_dir_all(root.join("assets/sub")).unwrap();
    std::fs::create_dir_all(root.join("assetpacks/UnityDataAssetPack/assets/sub")).unwrap();
    std::fs::write(root.join("assets/sub/a"), b"base").unwrap();
    std::fs::write(
        root.join("assetpacks/UnityDataAssetPack/assets/sub/b"),
        b"0123456789",
    )
    .unwrap();
    let boot_player=std::env::var_os("RUDROID_BOOT_PLAYER");
    const ORIGINAL_BOOT:&str="force-gfx-direct=1\ngfx-threading-mode=0\ngfx-disable-mt-rendering=1\n";
    const PROJECTED_BOOT:&str="force-gfx-direct=0\ngfx-threading-mode=1\ngfx-disable-mt-rendering=0\n";
    if let Some(player)=&boot_player {
        std::fs::create_dir_all(root.join("assets/bin/Data")).unwrap();
        std::fs::create_dir_all(root.join("lib/arm64-v8a")).unwrap();
        std::fs::write(root.join("assets/bin/Data/boot.config"),ORIGINAL_BOOT).unwrap();
        std::os::unix::fs::symlink(player,root.join("lib/arm64-v8a/libunity.so")).unwrap();
    }
    let config = rudroid_host_config::HostConfig::parse(&format!(
        "[paths]\ngame_files = {:?}\n{}",
        root.to_str().unwrap(),
        if boot_player.is_some() {"[runtime]\nmemory_mode='performance'\n[unity]\nrender_thread_mode='memory_mode'\n"}else{""}
    ))
    .unwrap();
    host_api::install_config(Some(config)).unwrap();
    let host = host_api::table();
    // SAFETY: only this prepared, trusted kit is loaded; libraries are process-pinned like the production loader.
    unsafe {
        let packs = std::mem::ManuallyDrop::new(
            libloading::Library::new(kit.join("libplatform_android_asset_packs.so")).unwrap(),
        );
        let enable: libloading::Symbol<extern "C" fn(Option<&HostApiV0>) -> i32> =
            packs.get(b"rudroid_plugin_early_init").unwrap();
        assert_eq!(enable(Some(host)), 0);
        let assets = std::mem::ManuallyDrop::new(
            libloading::Library::new(kit.join("libandroid_assets.so")).unwrap(),
        );
        let init: libloading::Symbol<extern "C" fn(Option<&HostApiV0>) -> i32> =
            assets.get(b"rudroid_plugin_init").unwrap();
        assert_eq!(init(Some(host)), 0);
        let query: libloading::Symbol<
            unsafe extern "C" fn(*const c_char, *const c_char, *const c_char, *mut c_void) -> usize,
        > = assets.get(b"rudroid_assets_symbol").unwrap();
        let address = |name: &CStr| {
            let value = query(
                name.as_ptr(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null_mut(),
            );
            assert_ne!(value, 0, "missing {name:?}");
            value
        };
        assert_eq!(
            query(
                c"AAsset_read".as_ptr(),
                c"libc.so".as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut()
            ),
            0
        );
        let manager: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void =
            std::mem::transmute(address(c"AAssetManager_fromJava"));
        let open: unsafe extern "C" fn(*mut c_void, *const c_char, i32) -> *mut c_void =
            std::mem::transmute(address(c"AAssetManager_open"));
        let close: extern "C" fn(*mut c_void) = std::mem::transmute(address(c"AAsset_close"));
        let read: unsafe extern "C" fn(*mut c_void, *mut c_void, usize) -> i32 =
            std::mem::transmute(address(c"AAsset_read"));
        let seek: extern "C" fn(*mut c_void, i64, i32) -> i64 =
            std::mem::transmute(address(c"AAsset_seek64"));
        let length: extern "C" fn(*mut c_void) -> i64 =
            std::mem::transmute(address(c"AAsset_getLength64"));
        let remaining: extern "C" fn(*mut c_void) -> i64 =
            std::mem::transmute(address(c"AAsset_getRemainingLength64"));
        let buffer: unsafe extern "C" fn(*mut c_void) -> *const u8 =
            std::mem::transmute(address(c"AAsset_getBuffer"));
        let fd: unsafe extern "C" fn(*mut c_void, *mut i64, *mut i64) -> i32 =
            std::mem::transmute(address(c"AAsset_openFileDescriptor64"));
        let mgr = manager(std::ptr::null_mut(), std::ptr::null_mut());
        assert!(!mgr.is_null());
        if boot_player.is_some() {
            let boot=open(mgr,c"bin/Data/boot.config".as_ptr(),2);assert!(!boot.is_null());
            let mut text=vec![0;PROJECTED_BOOT.len()];assert_eq!(length(boot),text.len() as i64);
            assert_eq!(read(boot,text.as_mut_ptr().cast(),text.len()),text.len() as i32);assert_eq!(text,PROJECTED_BOOT.as_bytes());
            close(boot);
            assert_eq!(std::fs::read_to_string(root.join("assets/bin/Data/boot.config")).unwrap(),ORIGINAL_BOOT);
        }
        assert!(open(mgr, c"../save".as_ptr(), 2).is_null());
        assert!(open(mgr, c"sub/b".as_ptr(), 99).is_null());
        let asset = open(mgr, c"sub/b".as_ptr(), 2);
        assert!(!asset.is_null());
        assert_eq!(length(asset), 10);
        let mut bytes = [0u8; 4];
        assert_eq!(read(asset, bytes.as_mut_ptr().cast(), bytes.len()), 4);
        assert_eq!(&bytes, b"0123");
        assert_eq!(remaining(asset), 6);
        assert_eq!(seek(asset, -2, 2 /* Linux SEEK_END */), 8);
        assert_eq!(read(asset, bytes.as_mut_ptr().cast(), bytes.len()), 2);
        assert_eq!(&bytes[..2], b"89");
        assert_eq!(read(asset, bytes.as_mut_ptr().cast(), bytes.len()), 0);
        assert_eq!(seek(asset, -11, 2), -1);
        assert_eq!(remaining(asset), 0);
        let mapped = buffer(asset);
        assert!(!mapped.is_null());
        assert_eq!(std::slice::from_raw_parts(mapped, 10), b"0123456789");
        assert_eq!(buffer(asset), mapped);
        let (mut start, mut size) = (-1, -1);
        let owned = fd(asset, &mut start, &mut size);
        assert!(owned >= 0);
        assert_eq!((start, size), (0, 10));
        let mut cloned = std::fs::File::from_raw_fd(owned);
        let mut contents = Vec::new();
        cloned.read_to_end(&mut contents).unwrap();
        assert_eq!(&contents, b"0123456789");
        drop(cloned);
        assert!(std::fs::read_to_string("/proc/self/maps")
            .unwrap()
            .contains(root.to_str().unwrap()));
        close(asset);
        close(asset);
        assert_eq!(length(asset), 0);
        assert_eq!(read(asset, bytes.as_mut_ptr().cast(), bytes.len()), -1);
        assert!(buffer(asset).is_null());
        assert!(!std::fs::read_to_string("/proc/self/maps")
            .unwrap()
            .contains(root.to_str().unwrap()));
        let dir_open: unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void =
            std::mem::transmute(address(c"AAssetManager_openDir"));
        let next: unsafe extern "C" fn(*mut c_void) -> *const c_char =
            std::mem::transmute(address(c"AAssetDir_getNextFileName"));
        let rewind: extern "C" fn(*mut c_void) = std::mem::transmute(address(c"AAssetDir_rewind"));
        let dir_close: extern "C" fn(*mut c_void) =
            std::mem::transmute(address(c"AAssetDir_close"));
        let dir = dir_open(mgr, c"sub".as_ptr());
        assert!(!dir.is_null());
        assert_eq!(CStr::from_ptr(next(dir)), c"a");
        assert_eq!(CStr::from_ptr(next(dir)), c"b");
        assert!(next(dir).is_null());
        rewind(dir);
        assert_eq!(CStr::from_ptr(next(dir)), c"a");
        dir_close(dir);
        assert!(next(dir).is_null());
        let explicit = host
            .asset_path(
                q::PATH,
                c"jar:file:/split_UnityDataAssetPack.apk!/assets/sub/b",
                None,
                0,
            )
            .unwrap()
            .unwrap();
        assert!(explicit
            .to_bytes()
            .ends_with(b"assetpacks/UnityDataAssetPack/assets/sub/b"));
        // Use the production VM/catalog/profile callbacks, without a fixture
        // VM, guest game or window. Each library remains process-pinned.
        let services: Vec<_> = [
            "libplatform_log_stderr.so",
            "libreply_catalog.so",
            "libandroid_jni_vm.so",
            "libandroid_base.so",
            "libunity_6.so",
        ]
        .into_iter()
        .map(|name| std::mem::ManuallyDrop::new(libloading::Library::new(kit.join(name)).unwrap()))
        .collect();
        for phase in [
            rudroid_unity_loader::phases::Phase::EarlyInit,
            rudroid_unity_loader::phases::Phase::RegisterJni,
            rudroid_unity_loader::phases::Phase::Init,
        ] {
            for library in &services {
                match host_api::run_phase_entry(phase, library) {
                    Ok(()) | Err(host_api::InitError::NotPresent) => {}
                    Err(error) => panic!("native service phase failed: {error:?}"),
                }
            }
        }
        // The real profile registers its Java services during prepare,
        // after the earlier phase registers the profile itself.
        let profile=rudroid_unity_loader::runtime::registered_profile().unwrap();
        assert!(!(profile.prepare.unwrap())(Some(host)).is_null());
        let vm = host.java_vm().unwrap();
        let env = vm
            .get_env(0x10006)
            .or_else(|| vm.attach_current_thread())
            .unwrap();
        for name in [
            c"com/unity3d/player/UnityPlayer",
            c"com/unity3d/player/UnityPlayerForActivityOrService",
        ] {
            let class = env.find_class(name);
            assert!(!class.is_null());
            let player = env.alloc_object(class);
            assert!(!player.is_null());
            let getter = env
                .method(class, c"getNetworkConnectivity", c"()I")
                .unwrap();
            assert!(!env.exception_check());
            assert_eq!(env.call_int(player, getter, &[]), 2);
            assert!(!env.exception_check());
            env.delete_local_ref(player);
            env.delete_local_ref(class);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
