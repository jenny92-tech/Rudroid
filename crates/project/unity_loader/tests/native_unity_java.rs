//! INPUT: trusted production core/profile DSOs, shared Host API and actual JNI tables
//! OUTPUT: both profiles' Java/PAD ABIs, presentation routing, storage/device ownership and native dispatch
//! POS: isolated handheld acceptance; one profile per process, no game or saves opened
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native};
use rudroid_unity_loader::{host_api, phases::Phase, runtime};
use std::{
    ffi::CStr,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static CALLS: AtomicUsize = AtomicUsize::new(0);
const HANDLE: i64 = 0x1234_5678_9abc_def0;
extern "C" fn invoke(
    env: JniEnv,
    _: JObject,
    handle: i64,
    class: JObject,
    method: JObject,
    args: JObject,
) -> JObject {
    assert_eq!(handle, HANDLE);
    assert!(!class.is_null() && !method.is_null());
    assert_eq!(env.get_array_length(args), 0);
    CALLS.fetch_add(1, Ordering::SeqCst);
    std::ptr::null_mut()
}
extern "C" fn reflect_invoke(
    env: JniEnv,
    _: JObject,
    handle: i64,
    name: JObject,
    args: JObject,
) -> JObject {
    assert_eq!(handle, HANDLE);
    assert_eq!(env.string_utf(name).unwrap().as_c_str(), c"run");
    assert_eq!(env.get_array_length(args), 0);
    CALLS.fetch_add(1, Ordering::SeqCst);
    std::ptr::null_mut()
}
fn statfs_contract(env: JniEnv) {
    let class=env.find_class(c"android/os/StatFs");
    let init=env.method(class,c"<init>",c"(Ljava/lang/String;)V").unwrap();
    let restat=env.method(class,c"restat",c"(Ljava/lang/String;)V").unwrap();
    let object=env.alloc_object(class);
    let requested=std::env::var("RUDROID_STATFS_PATH").unwrap_or_else(|_|"/".into());
    let path=env.new_string(&requested.encode_utf16().collect::<Vec<_>>());
    env.call_void(object,init,&[Arg::object(path)]);clean(env);
    let long=|name:&CStr|env.call_long(object,env.method(class,name,c"()J").unwrap(),&[]);
    let int=|name:&CStr|env.call_int(object,env.method(class,name,c"()I").unwrap(),&[]);
    let first=[long(c"getBlockSizeLong"),long(c"getBlockCountLong"),long(c"getFreeBlocksLong"),long(c"getAvailableBlocksLong")];
    assert!(first[0]>0 && first[1]>0 && 0<=first[3] && first[3]<=first[2] && first[2]<=first[1]);
    for (name,value) in [(c"getBlockSize",first[0]),(c"getBlockCount",first[1]),(c"getFreeBlocks",first[2]),(c"getAvailableBlocks",first[3])] {assert_eq!(int(name),value as i32);}
    for (name,value) in [(c"getTotalBytes",first[1]),(c"getFreeBytes",first[2]),(c"getAvailableBytes",first[3])] {assert_eq!(long(name),first[0].wrapping_mul(value));}
    let field=env.get_field_id(class,c"_rudroidStat",c"[J");
    let array=env.get_object_field(object,field);let old_weak=env.new_weak_global_ref(array);env.delete_local_ref(array);
    let missing=std::ffi::CString::new(format!("/rudroid-statfs-missing-{}",std::process::id())).unwrap();
    let missing=env.new_string_utf(&missing);
    for bad in [missing,std::ptr::null_mut()] {
        env.call_void(object,restat,&[Arg::object(bad)]);assert!(env.exception_check());env.exception_clear();
        assert_eq!(long(c"getAvailableBlocksLong"),first[3],"failed restat must preserve the complete prior snapshot");
        assert!(!env.is_same_object(old_weak,std::ptr::null_mut()));
    }
    env.delete_local_ref(missing);
    env.call_void(object,restat,&[Arg::object(path)]);clean(env);
    assert!(env.is_same_object(old_weak,std::ptr::null_mut()),"restat must release the retired snapshot");env.delete_weak_global_ref(old_weak);
    let wide=env.new_long_array(4);env.set_long_array_region(wide,0,&[4096,i64::MAX,(1i64<<32)+1,1i64<<32]);
    env.set_object_field(object,field,wide);env.delete_local_ref(wide);
    assert_eq!(int(c"getBlockCount"),-1);assert_eq!(int(c"getFreeBlocks"),1);assert_eq!(int(c"getAvailableBlocks"),0);
    assert_eq!(long(c"getTotalBytes"),-4096,"byte queries preserve Java long multiplication");
    env.call_void(object,restat,&[Arg::object(path)]);clean(env);
    let array=env.get_object_field(object,field);let final_weak=env.new_weak_global_ref(array);env.delete_local_ref(array);
    for local in [object,path,class]{env.delete_local_ref(local);}
    assert!(env.is_same_object(final_weak,std::ptr::null_mut()),"no process-global StatFs snapshot cache may retain the object");env.delete_weak_global_ref(final_weak);clean(env);
    println!("StatFs path={requested} block_size={} blocks={} free={} available={}: native capacity, retry failure, wide values and snapshot ownership passed",first[0],first[1],first[2],first[3]);
}

fn input_device_identity_contract(env: JniEnv) {
    let class = env.find_class(c"android/view/InputDevice");
    let device = env.alloc_object(class);
    let vendor = env.get_field_id(class, c"vendorId", c"I");
    let product = env.get_field_id(class, c"productId", c"I");
    let name_field = env.get_field_id(class, c"name", c"Ljava/lang/String;");
    assert!(!vendor.is_null() && !product.is_null() && !name_field.is_null());
    let get_vendor = env.method(class, c"getVendorId", c"()I").unwrap();
    let get_product = env.method(class, c"getProductId", c"()I").unwrap();
    assert_eq!(env.call_int(device, get_vendor, &[]), 0);
    assert_eq!(env.call_int(device, get_product, &[]), 0);
    env.set_int_field(device, vendor, 0x045e);
    env.set_int_field(device, product, 0x028e);
    assert_eq!(env.call_int(device, get_vendor, &[]), 0x045e);
    assert_eq!(env.call_int(device, get_product, &[]), 0x028e);
    let other = env.alloc_object(class);
    assert_eq!(env.call_int(other, get_vendor, &[]), 0, "identity belongs to each device");
    let name = env.new_string_utf(c"Device-owned controller identity");
    let weak = env.new_weak_global_ref(name);
    env.set_object_field(device, name_field, name);
    env.delete_local_ref(name);
    for method_name in [c"getName", c"getDescriptor"] {
        let method = env.method(class, method_name, c"()Ljava/lang/String;").unwrap();
        let returned = env.call_object(device, method, &[]);
        assert!(env.is_same_object(returned, weak));
        env.delete_local_ref(returned);
    }
    assert!(!env.is_same_object(weak, std::ptr::null_mut()));
    for local in [other, device, class] { env.delete_local_ref(local); }
    assert!(env.is_same_object(weak, std::ptr::null_mut()), "no global identity string cache");
    env.delete_weak_global_ref(weak);
    clean(env);
}

fn clean(env: JniEnv) {
    assert!(!env.exception_check(), "JNI call left an exception");
}

fn accessibility_delegate_contract(env: JniEnv, player: JObject) {
    let class = env.find_class(c"com/unity3d/player/UnityAccessibilityDelegate");
    let init = env.static_method(class, c"init",
        c"(Lcom/unity3d/player/UnityPlayer;)Lcom/unity3d/player/UnityAccessibilityDelegate;").unwrap();
    let cleanup = env.method(class, c"cleanup", c"()V").unwrap();
    let delegate = env.call_static_object(class, init, &[Arg::object(player)]);
    clean(env);
    assert!(!delegate.is_null() && env.is_instance_of(delegate, class));
    let weak = env.new_weak_global_ref(delegate);
    let owner = env.new_global_ref(delegate);
    env.delete_local_ref(delegate);
    // Match Unity's global owner through nativeDone; cleanup is idempotent,
    // and a failed second initialization must not retire the first owner.
    for _ in 0..2 { env.call_void(owner, cleanup, &[]); clean(env); }
    assert!(env.call_static_object(class, init, &[Arg::object(std::ptr::null_mut())]).is_null());
    assert!(env.exception_check());
    env.exception_clear();
    assert!(!env.is_same_object(weak, std::ptr::null_mut()));
    env.delete_global_ref(owner);
    assert!(env.is_same_object(weak, std::ptr::null_mut()), "no process-global delegate cache");
    env.delete_weak_global_ref(weak);
    env.delete_local_ref(class);
    clean(env);
}

fn presentation_route_contract(env: JniEnv, activity: JObject) {
    let context = env.find_class(c"android/content/Context");
    let service = env.method(context, c"getSystemService", c"(Ljava/lang/String;)Ljava/lang/Object;").unwrap();
    let service_object = |name: &CStr| {
        let key = env.new_string_utf(name);
        let object = env.call_object(activity, service, &[Arg::object(key)]);
        env.delete_local_ref(key);
        assert!(!object.is_null());
        object
    };
    let router = service_object(c"media_router");
    let router_class = env.get_object_class(router);
    let selected = env.method(router_class, c"getSelectedRoute", c"(I)Landroid/media/MediaRouter$RouteInfo;").unwrap();
    let video = env.get_static_int_field(router_class, env.get_static_field_id(router_class, c"ROUTE_TYPE_LIVE_VIDEO", c"I"));
    assert_eq!(video, 2);
    assert!(env.call_object(router, selected, &[Arg::int(video)]).is_null(),
        "single-display default must not force an unrelated Unity audio reset");
    let configure = env.static_method(router_class, c"_rudroidSetHostPresentationRoute", c"(Z)V").unwrap();
    env.call_static_void(router_class, configure, &[Arg::bool(true)]);
    let route = env.call_object(router, selected, &[Arg::int(video)]);
    clean(env);
    assert!(!route.is_null(), "host presentation route must be available to Unity");
    let route_class = env.find_class(c"android/media/MediaRouter$RouteInfo");
    assert!(env.is_instance_of(route, route_class));
    let presentation = env.method(route_class, c"getPresentationDisplay", c"()Landroid/view/Display;").unwrap();
    let routed = env.call_object(route, presentation, &[]);
    let window = service_object(c"window");
    let window_class = env.get_object_class(window);
    let primary = env.call_object(window, env.method(window_class, c"getDefaultDisplay", c"()Landroid/view/Display;").unwrap(), &[]);
    assert!(!routed.is_null() && env.is_same_object(routed, primary), "media route reuses the existing host display");
    let display_class = env.get_object_class(primary);
    let id = env.call_int(primary, env.method(display_class, c"getDisplayId", c"()I").unwrap(), &[]);
    assert_eq!(id, 1, "preserve the reference host presentation identity");
    let manager = service_object(c"display");
    let manager_class = env.get_object_class(manager);
    let lookup = env.method(manager_class, c"getDisplay", c"(I)Landroid/view/Display;").unwrap();
    for value in [0, id] {
        let found = env.call_object(manager, lookup, &[Arg::int(value)]);
        assert!(env.is_same_object(found, primary));
        env.delete_local_ref(found);
    }
    assert!(env.call_object(manager, lookup, &[Arg::int(99)]).is_null());
    for _ in 0..32 {
        let next = env.call_object(router, selected, &[Arg::int(video)]);
        let next_display = env.call_object(next, presentation, &[]);
        assert!(env.is_same_object(next, route) && env.is_same_object(next_display, primary), "repeated queries must not create route or display objects");
        env.delete_local_ref(next_display);
        env.delete_local_ref(next);
    }
    assert!(env.call_object(router, selected, &[Arg::int(0)]).is_null());
    env.call_static_void(router_class, configure, &[Arg::bool(false)]);
    assert!(env.call_object(router, selected, &[Arg::int(video)]).is_null());
    assert!(env.call_object(route, presentation, &[]).is_null(),
        "a previously retained route must not advertise a disabled presentation");
    env.call_static_void(router_class, configure, &[Arg::bool(true)]);
    let again = env.call_object(router, selected, &[Arg::int(video)]);
    assert!(env.is_same_object(again, route), "reconfiguration reuses the bounded VM-owned facade");
    env.delete_local_ref(again);
    env.call_static_void(router_class, configure, &[Arg::bool(false)]);
    let view_class = env.find_class(c"android/view/View");
    let view = env.alloc_object(view_class);
    assert_eq!(env.call_int(view, env.method(view_class, c"getSystemUiVisibility", c"()I").unwrap(), &[]), 0, "display identity must not change UI visibility flags");
    for local in [view, view_class, manager_class, manager, display_class, primary, window_class, window, routed, route_class, route, router_class, router, context] {
        env.delete_local_ref(local);
    }
    clean(env);
    println!("host presentation route: absent by default, explicit enable/disable, coherent identity and reused ownership passed");
}

fn storage_contract(env: JniEnv, activity: JObject, root: &std::path::Path) {
    let class = env.get_object_class(activity);
    let file_class = env.find_class(c"java/io/File");
    let absolute = env.method(file_class, c"getAbsolutePath", c"()Ljava/lang/String;").unwrap();
    let get_path = env.method(file_class, c"getPath", c"()Ljava/lang/String;").unwrap();
    let parent = env.method(file_class, c"getParent", c"()Ljava/lang/String;").unwrap();
    let helper = env.find_class(c"com/unity3d/player/ReflectionHelper");
    let lookup = env.static_method(helper, c"getMethodID", c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Method;").unwrap();
    let object_result = env.new_string_utf(c"()Ljava/lang/Object;");
    for (name, dir, external) in [
        (c"getFilesDir", "files 数据", false),
        (c"getExternalFilesDir", "external 数据", true),
        (c"getCacheDir", "cache 数据", false),
        (c"getExternalCacheDir", "cache 数据", false),
        (c"getDataDir", "data 数据", false),
        (c"getFilesDir", "files 数据", false), // Re-query the now-existing directory.
    ] {
        let signature = if external { c"(Ljava/lang/String;)Ljava/io/File;" } else { c"()Ljava/io/File;" };
        let args = [Arg::object(std::ptr::null_mut())];
        let file = env.call_object(activity, env.method(class, name, signature).unwrap(), if external { &args } else { &[] });
        clean(env);
        assert!(!file.is_null() && env.is_instance_of(file, file_class));
        let path = env.call_object(file, absolute, &[]);
        let text = env.string(path).unwrap();
        let actual = PathBuf::from(text.as_str());
        let expected = root.join(dir).canonicalize().unwrap();
        assert!(actual.is_absolute());
        assert_eq!(actual, expected, "new and existing directories must return the same normalized path, without dot components or a trailing separator");
        assert_eq!(actual.canonicalize().unwrap(), expected, "{name:?} must honor the configured directory");
        let path_again = env.call_object(file, get_path, &[]);
        assert_eq!(env.string(path_again).unwrap().as_str(), text.as_str());
        let parent_path = env.call_object(file, parent, &[]);
        assert_eq!(PathBuf::from(env.string(parent_path).unwrap().as_str()).canonicalize().unwrap(), root);
        // Unity reflection requests Object for these inherited File methods.
        let method_name = env.new_string_utf(name);
        let requested = if external { env.new_string_utf(c"(Ljava/lang/String;)Ljava/lang/Object;") } else { env.new_local_ref(object_result) };
        let reflected = env.call_static_object(helper, lookup, &[Arg::object(class), Arg::object(method_name), Arg::object(requested), Arg::bool(false)]);
        clean(env);
        assert!(!reflected.is_null(), "Unity must resolve inherited {name:?} without a pending exception");
        let file_weak = env.new_weak_global_ref(file);
        let path_weak = env.new_weak_global_ref(path);
        for local in [path, path_again, parent_path, method_name, requested, reflected, file] { env.delete_local_ref(local); }
        assert!(env.is_same_object(file_weak, std::ptr::null_mut()), "Context must not retain each returned File");
        assert!(env.is_same_object(path_weak, std::ptr::null_mut()), "a retired File must release its path String");
        for weak in [file_weak, path_weak] { env.delete_weak_global_ref(weak); }
        println!("configured {name:?}: {}", actual.display());
    }
    let info = env.call_object(activity, env.method(class, c"getApplicationInfo", c"()Landroid/content/pm/ApplicationInfo;").unwrap(), &[]);
    let info_class = env.get_object_class(info);
    let data = env.get_object_field(info, env.get_field_id(info_class, c"dataDir", c"Ljava/lang/String;"));
    assert_eq!(PathBuf::from(env.string(data).unwrap().as_str()).canonicalize().unwrap(), root.join("data 数据").canonicalize().unwrap());
    for local in [data, info_class, info, object_result, helper, file_class, class] { env.delete_local_ref(local); }
    clean(env);
}

#[test]
#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
fn configured_storage_paths_and_owners_survive_both_profile_bridges() {
    use rudroid_test_support::cargo::ensure_workspace_cdylib;
    // Each VM/profile lives in a fresh child process. Build through the same
    // fixed-entry cargo helpers; no production game or save is opened.
    let root = std::env::temp_dir().join(format!("rudroid-storage-contract-{}", std::process::id()));
    std::fs::create_dir(&root).expect("never reuse an existing fixture directory");
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }
    let _owned = Fixture(root.clone());
    let kit = root.join("kit");
    std::fs::create_dir(&kit).unwrap();
    for (package, artifact, name) in [
        ("rudroid-linux-log-stderr", "platform_log_stderr", "platform_log_stderr"),
        ("rudroid-android-reply-catalog", "rudroid_android_reply_catalog", "reply_catalog"),
        ("rudroid-android-jni-vm", "android_jni_vm", "android_jni_vm"),
        ("rudroid-android-framework", "android_base", "android_base"),
        ("rudroid-android-elf-load", "android_elf_load", "android_elf_load"),
        ("rudroid-unity-profile-2021-3", "unity_2021_3", "unity_2021_3"),
        ("rudroid-unity-profile-6", "unity_6", "unity_6"),
    ] {
        std::fs::copy(ensure_workspace_cdylib(package, artifact), kit.join(format!("lib{name}.so"))).unwrap();
    }
    for profile in ["unity_2021_3", "unity_6"] {
        let fixture = root.join(profile);
        std::fs::create_dir(&fixture).unwrap();
        std::fs::create_dir(fixture.join("gamedata")).unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["production_profiles_share_java_services_with_their_own_signatures", "--exact", "--ignored", "--nocapture", "--test-threads=1"])
            .env("RUDROID_RESOURCE_KIT", &kit).env("RUDROID_UNITY_PROFILE", profile)
            .env("RUDROID_STORAGE_PROBE_ROOT", &fixture).env("RUDROID_STATFS_PATH", &fixture)
            .current_dir(fixture.join("gamedata")).output().unwrap();
        assert!(output.status.success(), "{profile}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        println!("{profile}: {}", String::from_utf8_lossy(&output.stdout));
    }
}

#[test]
#[ignore = "run alone on handheld with RUDROID_RESOURCE_KIT and RUDROID_UNITY_PROFILE=unity_6 or unity_2021_3"]
fn production_profiles_share_java_services_with_their_own_signatures() {
    let kit =
        PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").expect("production kit required"));
    let profile = std::env::var("RUDROID_UNITY_PROFILE").expect("profile required");
    assert!(matches!(profile.as_str(), "unity_6" | "unity_2021_3"));
    let legacy = profile == "unity_2021_3";
    let storage_root = std::env::var_os("RUDROID_STORAGE_PROBE_ROOT").map(PathBuf::from);
    let config = storage_root.as_ref().map(|_| rudroid_host_config::HostConfig::parse(
        "[paths]\nandroid_files='../files 数据/'\nandroid_external_files='../external 数据/'\nandroid_cache='../cache 数据/'\nandroid_data='../data 数据/'\n"
    ).unwrap());
    host_api::install_config(config).unwrap();
    let host = host_api::table();
    let names = [
        "libplatform_log_stderr.so".to_owned(),
        "libreply_catalog.so".to_owned(),
        "libandroid_jni_vm.so".to_owned(),
        "libandroid_base.so".to_owned(),
        "libandroid_elf_load.so".to_owned(),
        format!("lib{profile}.so"),
    ];
    // SAFETY: exact trusted production DSOs; every handle and registered callback stays mapped.
    let libraries: Vec<_> = unsafe {
        names
            .iter()
            .map(|name| {
                std::mem::ManuallyDrop::new(libloading::Library::new(kit.join(name)).unwrap())
            })
            .collect()
    };
    for phase in [Phase::EarlyInit, Phase::RegisterJni, Phase::Init] {
        for lib in &libraries {
            // SAFETY: pinned same-kit phase entry ABI; no guest/native producers are started.
            match unsafe { host_api::run_phase_entry(phase, lib) } {
                Ok(()) | Err(host_api::InitError::NotPresent) => {}
                Err(error) => panic!("phase failed: {error:?}"),
            }
        }
    }
    let registered = runtime::registered_profile().unwrap();
    assert!(!(registered.prepare.unwrap()(Some(host))).is_null());
    let env = host.java_vm().unwrap().get_env(0x10006).unwrap();
    clean(env);
    if !legacy {
        let class = env.find_class(c"com/unity3d/player/UnityPlayer");
        let player = env.alloc_object(class);
        accessibility_delegate_contract(env, player);
        env.delete_local_ref(player);
        env.delete_local_ref(class);
    }
    statfs_contract(env);
    let build = env.find_class(c"android/os/Build");
    let manufacturer = env.get_static_object_field(build,
        env.get_static_field_id(build, c"MANUFACTURER", c"Ljava/lang/String;"));
    assert_eq!(env.string_utf(manufacturer).unwrap().as_c_str(), c"Rudroid",
        "the Android device identity must not select a desktop Linux input profile");
    env.delete_local_ref(manufacturer);
    env.delete_local_ref(build);
    clean(env);
    input_device_identity_contract(env);
    let system=env.find_class(c"java/lang/System");
    let missing=env.new_string_utf(c"rudroid-intentionally-not-in-load-plan");
    for name in [c"load",c"loadLibrary"] {
        let load=env.static_method(system,name,c"(Ljava/lang/String;)V").unwrap();
        env.call_static_void(system,load,&[Arg::object(missing)]);
        assert!(env.exception_check(),"an unloaded library must not report success");
        env.exception_clear();
    }
    for (name,request) in [(c"load",c"libdl"),(c"loadLibrary",c"dl")] {
        let request=env.new_string_utf(request);
        env.call_static_void(system,env.static_method(system,name,c"(Ljava/lang/String;)V").unwrap(),&[Arg::object(request)]);
        clean(env);env.delete_local_ref(request);
    }
    for local in [missing,system] {env.delete_local_ref(local);}
    let player_class = env.find_class(c"com/unity3d/player/UnityPlayer");
    let activity = env.get_static_object_field(
        player_class,
        env.get_static_field_id(player_class, c"currentActivity", c"Landroid/app/Activity;"),
    );
    assert!(!activity.is_null());
    if let Some(root) = storage_root { storage_contract(env, activity, &root); }
    let activity_type=env.find_class(c"com/unity3d/player/UnityPlayerActivity");
    assert!(env.is_instance_of(activity,activity_type));
    let owned_player=env.get_object_field(activity,env.get_field_id(activity_type,c"mUnityPlayer",c"Lcom/unity3d/player/UnityPlayer;"));
    assert!(!owned_player.is_null() && env.is_instance_of(owned_player,player_class));
    for local in [owned_player,activity_type]{env.delete_local_ref(local);}
    let context_class = env.find_class(c"android/content/Context");
    assert!(env.is_instance_of(activity, context_class));
    let package=env.call_object(activity,env.method(context_class,c"getPackageName",c"()Ljava/lang/String;").unwrap(),&[]);
    assert_eq!(env.string_utf(package).unwrap().as_c_str(),c"package.name.not.defined");env.delete_local_ref(package);
    // Older Unity reads these before its first frame. Missing static String
    // values must not become null native C strings or pending JNI failures.
    let service = env.method(context_class, c"getSystemService", c"(Ljava/lang/String;)Ljava/lang/Object;").unwrap();
    for (name, expected, available) in [
        (c"LOCATION_SERVICE", c"location", false),
        (c"INPUT_SERVICE", c"input", true),
        (c"AUDIO_SERVICE", c"audio", true),
        (c"POWER_SERVICE", c"power", true),
        (c"MEDIA_ROUTER_SERVICE", c"media_router", true),
    ] {
        let field = env.get_static_field_id(context_class, name, c"Ljava/lang/String;");
        let value = env.get_static_object_field(context_class, field);
        assert_eq!(env.string_utf(value).unwrap().as_c_str(), expected);
        let object = env.call_object(activity, service, &[Arg::object(value)]);
        assert_eq!(!object.is_null(), available);
        clean(env);
        for local in [object, value] { if !local.is_null() { env.delete_local_ref(local); } }
    }
    let intent = env.find_class(c"android/content/Intent");
    let audio_name=env.new_string_utf(c"audio");
    let audio=env.call_object(activity,service,&[Arg::object(audio_name)]);
    let audio_class=env.find_class(c"android/media/AudioManager");
    let get_property=env.method(audio_class,c"getProperty",c"(Ljava/lang/String;)Ljava/lang/String;").unwrap();
    for (field,expected) in [(c"PROPERTY_OUTPUT_SAMPLE_RATE",c"48000"),(c"PROPERTY_OUTPUT_FRAMES_PER_BUFFER",c"256")] {
        let key=env.get_static_object_field(audio_class,env.get_static_field_id(audio_class,field,c"Ljava/lang/String;"));
        let value=env.call_object(audio,get_property,&[Arg::object(key)]);
        assert_eq!(env.string_utf(value).unwrap().as_c_str(),expected);
        for local in [key,value]{env.delete_local_ref(local);}
    }
    let unknown=env.new_string_utf(c"rudroid.unsupported.audio.property");
    assert!(env.call_object(audio,get_property,&[Arg::object(unknown)]).is_null());
    for local in [unknown,audio_class,audio,audio_name]{env.delete_local_ref(local);}
    clean(env);
    presentation_route_contract(env, activity);
    let action = env.get_static_object_field(intent, env.get_static_field_id(intent, c"ACTION_MAIN", c"Ljava/lang/String;"));
    assert_eq!(env.string_utf(action).unwrap().as_c_str(), c"android.intent.action.MAIN");
    let constructed = env.new_object(intent, env.method(intent,c"<init>",c"(Ljava/lang/String;)V").unwrap(), &[Arg::object(action)]);
    let get_package=env.method(intent,c"getPackage",c"()Ljava/lang/String;").unwrap();
    let set_package=env.method(intent,c"setPackage",c"(Ljava/lang/String;)Landroid/content/Intent;").unwrap();
    assert!(env.call_object(constructed,get_package,&[]).is_null());
    let target=env.new_string_utf(c"package.test.target");
    let returned=env.call_object(constructed,set_package,&[Arg::object(target)]);
    assert!(env.is_same_object(returned,constructed));
    for local in [target,returned]{env.delete_local_ref(local);}
    let stored=env.call_object(constructed,get_package,&[]);
    assert_eq!(env.string_utf(stored).unwrap().as_c_str(),c"package.test.target");env.delete_local_ref(stored);
    let returned=env.call_object(constructed,set_package,&[Arg::object(std::ptr::null_mut())]);env.delete_local_ref(returned);
    assert!(env.call_object(constructed,get_package,&[]).is_null());
    let read_action = env.call_object(constructed,env.method(intent,c"getAction",c"()Ljava/lang/String;").unwrap(),&[]);
    assert!(env.is_same_object(action,read_action));
    let add_category=env.method(intent,c"addCategory",c"(Ljava/lang/String;)Landroid/content/Intent;").unwrap();
    for _ in 0..2 {
        let category=env.new_string_utf(c"android.intent.category.LAUNCHER");
        let returned=env.call_object(constructed,add_category,&[Arg::object(category)]);
        assert!(env.is_same_object(returned,constructed));
        for local in [returned,category]{env.delete_local_ref(local);}
    }
    let categories=env.call_object(constructed,env.method(intent,c"getCategories",c"()Ljava/util/Set;").unwrap(),&[]);
    let set=env.find_class(c"java/util/Set");
    assert_eq!(env.call_int(categories,env.method(set,c"size",c"()I").unwrap(),&[]),1);
    for local in [set,categories]{env.delete_local_ref(local);}
    let add_flags=env.method(intent,c"addFlags",c"(I)Landroid/content/Intent;").unwrap();
    for value in [0x10000000,0x20] {
        let returned=env.call_object(constructed,add_flags,&[Arg::int(value)]);
        assert!(env.is_same_object(returned,constructed));env.delete_local_ref(returned);
    }
    assert_eq!(env.call_int(constructed,env.method(intent,c"getFlags",c"()I").unwrap(),&[]),0x10000020);
    for local in [read_action,constructed] {env.delete_local_ref(local);}
    let manager = env.find_class(c"android/content/pm/PackageManager");
    assert_eq!(env.get_static_int_field(manager, env.get_static_field_id(manager, c"GET_META_DATA", c"I")), 128);
    let package_manager = env.call_object(activity,env.method(context_class,c"getPackageManager",c"()Landroid/content/pm/PackageManager;").unwrap(),&[]);
    let app_info = env.call_object(package_manager,env.method(manager,c"getApplicationInfo",c"(Ljava/lang/String;I)Landroid/content/pm/ApplicationInfo;").unwrap(),&[Arg::object(env.new_string_utf(c"package.name.not.defined")),Arg::int(128)]);
    let context_info = env.call_object(activity,env.method(context_class,c"getApplicationInfo",c"()Landroid/content/pm/ApplicationInfo;").unwrap(),&[]);
    assert!(!app_info.is_null() && env.is_same_object(app_info,context_info));
    let item_info=env.find_class(c"android/content/pm/PackageItemInfo");
    assert!(env.is_instance_of(app_info,item_info));
    assert!(env.get_object_field(app_info,env.get_field_id(item_info,c"metaData",c"Landroid/os/Bundle;")).is_null());
    env.delete_local_ref(item_info);
    for local in [context_info,app_info,package_manager] {env.delete_local_ref(local);}
    for local in [manager, action, intent] { env.delete_local_ref(local); }
    clean(env);
    let looper=env.find_class(c"android/os/Looper");
    let integer=env.find_class(c"java/lang/Integer");
    let parse=env.static_method(integer,c"parseInt",c"(Ljava/lang/String;)I").unwrap();
    for (text,expected) in [("48000",48000),("+256",256),("-2147483648",i32::MIN),("2147483647",i32::MAX),("４８０００",48000)] {
        let text=env.new_string(&text.encode_utf16().collect::<Vec<_>>());
        assert_eq!(env.call_static_int(integer,parse,&[Arg::object(text)]),expected);
        env.delete_local_ref(text);clean(env);
    }
    let number_error=env.find_class(c"java/lang/NumberFormatException");
    for text in [Some(""),Some("2147483648"),Some("48000x"),None] {
        let value=text.map_or(std::ptr::null_mut(),|s|env.new_string(&s.encode_utf16().collect::<Vec<_>>()));
        assert_eq!(env.call_static_int(integer,parse,&[Arg::object(value)]),0);
        let error=env.exception_occurred();assert!(!error.is_null());env.exception_clear();
        assert!(env.is_instance_of(error,number_error));
        for local in [value,error]{env.delete_local_ref(local);}
    }
    for local in [number_error,integer]{env.delete_local_ref(local);}
    clean(env);
    let main=env.call_static_object(looper,env.static_method(looper,c"getMainLooper",c"()Landroid/os/Looper;").unwrap(),&[]);
    let current=env.call_static_object(looper,env.static_method(looper,c"myLooper",c"()Landroid/os/Looper;").unwrap(),&[]);
    assert!(!main.is_null() && env.is_same_object(main,current));
    let handler_class=env.find_class(c"android/os/Handler");
    let handler=env.new_object(handler_class,env.method(handler_class,c"<init>",c"()V").unwrap(),&[]);
    let bound=env.call_object(handler,env.method(handler_class,c"getLooper",c"()Landroid/os/Looper;").unwrap(),&[]);
    assert!(env.is_same_object(main,bound));
    for local in [bound,handler,handler_class,current,main,looper]{env.delete_local_ref(local);}
    clean(env);
    let vm=host.java_vm().unwrap();
    std::thread::spawn(move || {
        let env=vm.attach_current_thread().unwrap();
        let looper=env.find_class(c"android/os/Looper");
        assert!(env.call_static_object(looper,env.static_method(looper,c"myLooper",c"()Landroid/os/Looper;").unwrap(),&[]).is_null());
        let class=env.find_class(c"android/os/Handler");
        let object=env.new_object(class,env.method(class,c"<init>",c"()V").unwrap(),&[]);
        assert!(env.exception_check(),"foreign thread must not silently acquire the UI queue");env.exception_clear();
        for local in [object,class,looper]{env.delete_local_ref(local);}
        assert_eq!(vm.detach_current_thread(),0);
    }).join().unwrap();
    // Enum metadata lets native startup reach its explicit unavailable-feature
    // branch without a missing-field exception. No orientation sensor is hosted.
    let info=env.find_class(c"android/content/pm/ActivityInfo");
    for (name,value) in [(c"SCREEN_ORIENTATION_SENSOR",4),(c"SCREEN_ORIENTATION_USER_PORTRAIT",12),(c"SCREEN_ORIENTATION_USER_LANDSCAPE",11)] {
        let field=env.get_static_field_id(info,name,c"I");
        clean(env);
        assert!(!field.is_null(), "required startup metadata {name:?} must be registered");
        assert_eq!(env.get_static_int_field(info,field),value);
    }
    env.delete_local_ref(info);clean(env);
    let actual_player_class = if legacy { player_class } else {
        env.find_class(c"com/unity3d/player/UnityPlayerForActivityOrService")
    };
    let player = env.alloc_object(actual_player_class);
    let launch = env
        .method(player_class, c"getLaunchURL", c"()Ljava/lang/String;")
        .unwrap();
    let url = env.call_object(player, launch, &[]);
    if legacy {
        assert_eq!(env.string_utf(url).unwrap().as_c_str(), c"");
    } else {
        assert!(url.is_null());
    }
    let ar = env
        .method(actual_player_class, c"initializeGoogleAr", c"()Z")
        .unwrap();
    assert!(!env.call_boolean(player, ar, &[]));
    if legacy {
        let start=env.method(player_class,c"startOrientationListener",c"(I)Z").unwrap();
        assert!(!env.call_boolean(player,start,&[Arg::int(0)]));
        for name in [c"stopOrientationListener",c"isShouldShowLaunchScreenAds"] {
            assert!(!env.call_boolean(player,env.method(player_class,name,c"()Z").unwrap(),&[]));
        }
    }
    clean(env);

    let pad = env.find_class(c"com/unity3d/player/PlayAssetDeliveryUnityWrapper");
    let signature: &CStr = if legacy {
        c"(Landroid/content/Context;)Lcom/unity3d/player/PlayAssetDeliveryUnityWrapper;"
    } else {
        c"(Lcom/unity3d/player/UnityPlayer;Landroid/content/Context;)Lcom/unity3d/player/PlayAssetDeliveryUnityWrapper;"
    };
    let init = env.static_method(pad, c"init", signature).unwrap();
    let values = if legacy {
        vec![Arg::object(activity)]
    } else {
        vec![Arg::object(player), Arg::object(activity)]
    };
    let wrapper = env.call_static_object(pad, init, &values);
    assert!(!wrapper.is_null());
    let instance = env.call_static_object(
        pad,
        env.static_method(
            pad,
            c"getInstance",
            c"()Lcom/unity3d/player/PlayAssetDeliveryUnityWrapper;",
        )
        .unwrap(),
        &[],
    );
    assert!(env.is_same_object(wrapper, instance));
    let missing = env.method(pad, c"playCoreApiMissing", c"()Z").unwrap();
    assert_eq!(env.call_boolean(wrapper, missing, &[]), legacy);
    clean(env);

    let reflection = env.find_class(c"com/unity3d/player/ReflectionHelper");
    let field = env.call_static_object(
        reflection,
        env.static_method(
            reflection,
            c"getFieldID",
            c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Field;",
        )
        .unwrap(),
        &[
            Arg::object(player_class),
            Arg::object(env.new_string_utf(c"currentActivity")),
            Arg::object(env.new_string_utf(c"Landroid/app/Activity;")),
            Arg::bool(true),
        ],
    );
    assert!(!field.is_null());
    let signature = env.call_static_object(
        reflection,
        env.static_method(
            reflection,
            c"getFieldSignature",
            c"(Ljava/lang/reflect/Field;)Ljava/lang/String;",
        )
        .unwrap(),
        &[Arg::object(field)],
    );
    assert_eq!(
        env.string_utf(signature).unwrap().as_c_str(),
        c"Landroid/app/Activity;"
    );
    clean(env);

    assert_eq!(env.register_class(c"bitter/jnibridge/JNIBridge", &[Native::new4(c"invoke",
        c"(JLjava/lang/Class;Ljava/lang/reflect/Method;[Ljava/lang/Object;)Ljava/lang/Object;", invoke)]), 0);
    assert_eq!(
        env.register_class(
            c"com/unity3d/player/ReflectionHelper",
            &[Native::new3(
                c"nativeProxyInvoke",
                c"(JLjava/lang/String;[Ljava/lang/Object;)Ljava/lang/Object;",
                reflect_invoke
            )]
        ),
        0
    );
    let runnable = env.find_class(c"java/lang/Runnable");
    let class_class = env.find_class(c"java/lang/Class");
    let interfaces = env.new_object_array(1, class_class, runnable);
    let bridge = env.find_class(c"bitter/jnibridge/JNIBridge");
    let ordinary = env.call_static_object(
        bridge,
        env.static_method(
            bridge,
            c"newInterfaceProxy",
            c"(J[Ljava/lang/Class;)Ljava/lang/Object;",
        )
        .unwrap(),
        &[Arg::long(HANDLE), Arg::object(interfaces)],
    );
    let reflected = env.call_static_object(
        reflection,
        env.static_method(
            reflection,
            c"newProxyInstance",
            c"(Lcom/unity3d/player/UnityPlayer;JLjava/lang/Class;)Ljava/lang/Object;",
        )
        .unwrap(),
        &[
            Arg::object(player),
            Arg::long(HANDLE),
            Arg::object(runnable),
        ],
    );
    for object in [ordinary, reflected] {
        assert!(!object.is_null());
        assert!(env.is_instance_of(object, runnable));
        let class = env.get_object_class(object);
        env.call_void(object, env.method(class, c"run", c"()V").unwrap(), &[]);
        clean(env);
        env.call_static_void(
            bridge,
            env.static_method(bridge, c"disableInterfaceProxy", c"(Ljava/lang/Object;)V")
                .unwrap(),
            &[Arg::object(object)],
        );
        env.call_void(object, env.method(class, c"run", c"()V").unwrap(), &[]);
        clean(env);
    }
    assert_eq!(
        CALLS.load(Ordering::SeqCst),
        2,
        "disabled proxies must not call stale native handles"
    );
    println!("{profile}: Activity, reflection, PAD and both native proxy routes passed");
}
