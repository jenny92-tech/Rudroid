//! INPUT: shared JNI table, HostApi configuration and the StatFs registrar
//! OUTPUT: normalized Context directories, package/class-loader callbacks and Process compatibility
//! POS: Android mock behavior; cached global handles reference the shared VM's data
#![forbid(unsafe_code)]
use std::ffi::{c_void, CStr, CString};
use std::sync::OnceLock;
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native};

static PACKAGE: OnceLock<usize> = OnceLock::new();
static CODE_PATH: OnceLock<usize> = OnceLock::new();

/// Config string at `key` through a 4 KiB buffer, `fallback` when absent.
fn configured(host: &rudroid_common::host_api::HostApiV0, key: &CStr, fallback: &CStr) -> String {
    host.config_string(key, 4096).unwrap_or_else(|| fallback.to_string_lossy().into_owned())
}
pub(super) fn cached_string(env: JniEnv, cache: &OnceLock<usize>, key: &CStr, fallback: &CStr, canonical: bool) -> *mut c_void {
    let global = *cache.get_or_init(|| {
        let Some(host) = super::HOST.get() else { return 0; };
        let value = configured(host, key, fallback);
        let path = if canonical {
            std::path::Path::new(&value).canonicalize().ok()
                .map(|p| p.to_string_lossy().into_owned())
        } else { None };
        let value = path.as_deref().unwrap_or(&value);
        let local = super::preferences::string(env,value);
        let result = env.new_global_ref(local);
        env.delete_local_ref(local);
        result as usize
    });
    if global == 0 { return std::ptr::null_mut(); }
    env.new_local_ref(global as *mut c_void)
}

extern "C" fn package_name(env: JniEnv, _context: JObject) -> JObject {
    cached_string(env, &PACKAGE, c"package.packageName", c"package.name.not.defined", false)
}
extern "C" fn code_path(env: JniEnv, _context: JObject) -> JObject {
    if let Some(global)=CODE_PATH.get() {
        return if *global==0 {std::ptr::null_mut()}else{env.new_local_ref(*global as *mut c_void)};
    }
    // An explicitly supplied APK or extracted directory is authoritative. Tree detection
    // remains the compatibility fallback for existing handheld configurations.
    let configured=configured(super::HOST.get().unwrap(),c"paths.android_package_code",c"");
    if std::path::Path::new(&configured).is_file() || std::path::Path::new(&configured).is_dir() {
        return cached_string(env,&CODE_PATH,c"paths.android_package_code",c"",true);
    }
    for path in [".","gamedata"] {
        if std::path::Path::new(path).join("assets/bin/Data").is_dir() {
            let fallback = if path == "." { c"." } else { c"gamedata" };
            // A detected extracted Unity root overrides placeholder APK paths,
            // preserving the existing loader/config contract.
            return cached_string(env,&CODE_PATH,c"org.rudroid.detected_code_root",fallback,true);
        }
    }
    cached_string(env,&CODE_PATH,c"paths.android_package_code",c"./path_not_defined_code",true)
}
extern "C" fn class_loader(env: JniEnv, context: JObject) -> JObject {
    let class = env.get_object_class(context);
    let meta = env.get_object_class(class);
    let loader = env.method(meta, c"getClassLoader", c"()Ljava/lang/ClassLoader;")
        .map_or(std::ptr::null_mut(), |method| env.call_object(class, method, &[]));
    env.delete_local_ref(meta);
    env.delete_local_ref(class);
    loader
}

extern "C" fn application_context(env: JniEnv, _context: JObject) -> JObject {
    static APPLICATION: OnceLock<usize> = OnceLock::new();
    singleton(env, &APPLICATION, || allocate(env, c"android/app/Application"))
}

// The host owns scheduling policy; this matches the reference Android mock.
extern "C" fn thread_priority(_env: JniEnv, _class: JObject, _tid: i32, _priority: i32) {}

/// Registers on the shared session; plugin callbacks remain mapped for process life.
pub(super) fn register(env: JniEnv) -> i32 {
    let context = [
        Native::new0(c"getApplicationContext", c"()Landroid/content/Context;", application_context),
        Native::new0(c"getPackageCodePath", c"()Ljava/lang/String;", code_path),
        Native::new0(c"getPackageName", c"()Ljava/lang/String;", package_name),
        Native::new0(c"getClassLoader", c"()Ljava/lang/ClassLoader;", class_loader),
    ];
    if env.register_class(c"android/content/Context", &context) != 0 { return -1; }
    let process = [Native::new2(c"setThreadPriority", c"(II)V", thread_priority)];
    if env.register_class(c"android/os/Process", &process) != 0 { return -1; }
    let types = env.find_class(c"org/rudroid/runtime/TypeRegistry");
    let method = env.static_method(types, c"setSuperclass", c"(Ljava/lang/Class;Ljava/lang/Class;)V");
    let child = env.find_class(c"android/app/Activity");
    let parent = env.find_class(c"android/view/ContextThemeWrapper");
    let Some(method) = method.filter(|_| ![types, child, parent].iter().any(|p| p.is_null())) else { return -1; };
    env.call_static_void(types, method, &[Arg::object(child), Arg::object(parent)]);
    for handle in [parent, child, types] { env.delete_local_ref(handle); }
    rudroid_common::jni_abi::declare_superclass(env, c"android/app/Application", c"android/content/Context");
    if env.exception_check() { return -1; }
    let status=register_services(env);
    if status==0 { super::statfs::register(env) } else { status }
}

static APPLICATION_INFO: OnceLock<usize> = OnceLock::new();
static ASSETS: OnceLock<usize> = OnceLock::new();
static PACKAGE_MANAGER: OnceLock<usize> = OnceLock::new();
static INTENT: OnceLock<usize> = OnceLock::new();
static UI_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();
struct UiJob {handle:usize,deadline:Option<std::time::Instant>}
struct UiQueue {jobs:std::collections::VecDeque<UiJob>,closed:bool,finished:bool}
static UI_JOBS: std::sync::Mutex<UiQueue> = std::sync::Mutex::new(UiQueue{jobs:std::collections::VecDeque::new(),closed:false,finished:false});

pub(super) fn config_string(env: JniEnv, key: &CStr, fallback: &CStr, canonical: bool) -> *mut c_void {
    let Some(host) = super::HOST.get() else { return std::ptr::null_mut(); };
    let value = configured(host, key, fallback);
    let path = if canonical {
        Some(absolute(std::path::Path::new(&value)).to_string_lossy().into_owned())
    } else { None };
    super::preferences::string(env,path.as_deref().unwrap_or(&value))
}
pub(super) fn allocate(env: JniEnv, name: &CStr) -> *mut c_void {
    let class = env.find_class(name); let object = env.alloc_object(class); env.delete_local_ref(class); object
}
pub(super) fn set_string_field(env: JniEnv, object: *mut c_void, name: &CStr, string: *mut c_void) {
    let class = env.get_object_class(object); let field = env.get_field_id(class,name,c"Ljava/lang/String;");
    env.set_object_field(object,field,string); env.delete_local_ref(class); env.delete_local_ref(string);
}
pub(super) fn singleton(env: JniEnv, cache: &OnceLock<usize>, create: impl FnOnce()->*mut c_void) -> *mut c_void {
    let global = *cache.get_or_init(|| {
        let object = create();
        if object.is_null() { return 0; }
        let result = env.new_global_ref(object); env.delete_local_ref(object); result as usize
    });
    if global == 0 { std::ptr::null_mut() } else { env.new_local_ref(global as *mut c_void) }
}
extern "C" fn application_info(env: JniEnv, _context: JObject) -> JObject {
    singleton(env,&APPLICATION_INFO,|| {
        let object = allocate(env,c"android/content/pm/ApplicationInfo");
        for (name,key,default,canonical) in [
            (c"dataDir",c"paths.android_data",c".",true),
            (c"packageName",c"package.packageName",c"package.name.not.defined",false),
            (c"nativeLibraryDir",c"paths.library",c"lib/arm64-v8a",true),
        ] { set_string_field(env,object,name,config_string(env,key,default,canonical)); }
        set_string_field(env,object,c"sourceDir",base_package(env,std::ptr::null_mut()));
        set_string_field(env,object,c"publicSourceDir",base_package(env,std::ptr::null_mut()));
        let class=env.get_object_class(object);env.set_int_field(object,env.get_field_id(class,c"minSdkVersion",c"I"),23);env.set_int_field(object,env.get_field_id(class,c"targetSdkVersion",c"I"),26);
        let splits=split_packages(env);
        for name in [c"splitSourceDirs",c"splitPublicSourceDirs"] {
            env.set_object_field(object,env.get_field_id(class,name,c"[Ljava/lang/String;"),splits);
        }
        env.delete_local_ref(splits);env.delete_local_ref(class);
        object
    })
}
extern "C" fn assets(env:JniEnv,_context:JObject)->JObject {
    singleton(env,&ASSETS,||allocate(env,c"android/content/res/AssetManager"))
}
extern "C" fn package_manager(env:JniEnv,_context:JObject)->JObject {
    singleton(env,&PACKAGE_MANAGER,||allocate(env,c"android/content/pm/PackageManager"))
}
extern "C" fn intent(env: JniEnv, _activity: JObject) -> JObject {
    singleton(env,&INTENT,|| {
        let object = allocate(env,c"android/content/Intent");
        let action = env.new_string_utf(c"android.intent.action.MAIN");
        intent_init(env,object,action);
        env.delete_local_ref(action);
        object
    })
}
extern "C" fn intent_init(env:JniEnv,object:JObject,action:JObject) {
    let class=env.get_object_class(object);
    env.set_object_field(object,env.get_field_id(class,c"action",c"Ljava/lang/String;"),action);
    env.delete_local_ref(class);
}
extern "C" fn intent_action(env:JniEnv,object:JObject)->JObject {
    intent_string_field(env,object,c"action")
}
fn intent_string_field(env:JniEnv,object:JObject,name:&CStr)->JObject {
    let class=env.get_object_class(object);
    let value=env.get_object_field(object,env.get_field_id(class,name,c"Ljava/lang/String;"));
    env.delete_local_ref(class);
    value
}
extern "C" fn intent_set_package(env:JniEnv,object:JObject,package:JObject)->JObject {
    set_string_field(env,object,c"package",env.new_local_ref(package));
    env.new_local_ref(object)
}
extern "C" fn intent_package(env:JniEnv,object:JObject)->JObject {
    intent_string_field(env,object,c"package")
}
extern "C" fn package_application_info(env:JniEnv,_manager:JObject,_name:JObject,_flags:i32)->JObject {
    application_info(env,std::ptr::null_mut())
}
extern "C" fn intent_add_category(env:JniEnv,object:JObject,category:JObject)->JObject {
    if category.is_null() {
        let class=env.find_class(c"java/lang/NullPointerException");
        env.throw_new(class,c"Intent category is null");env.delete_local_ref(class);
        return std::ptr::null_mut();
    }
    let class=env.get_object_class(object);
    let field=env.get_field_id(class,c"categories",c"[Ljava/lang/String;");
    env.delete_local_ref(class);
    let old=env.get_object_field(object,field);
    let count=if old.is_null(){0}else{env.get_array_length(old)};
    let requested=env.string_utf16(category);
    for index in 0..count {
        let value=env.get_object_array_element(old,index);
        let duplicate=env.string_utf16(value)==requested;env.delete_local_ref(value);
        if duplicate {env.delete_local_ref(old);return env.new_local_ref(object);}
    }
    let Some(length)=count.checked_add(1) else {
        env.delete_local_ref(old);super::preferences::error(env,"too many Intent categories");
        return std::ptr::null_mut();
    };
    let string=env.find_class(c"java/lang/String");
    let array=env.new_object_array(length,string,std::ptr::null_mut());env.delete_local_ref(string);
    if !array.is_null() {
        for index in 0..count {
            let value=env.get_object_array_element(old,index);
            env.set_object_array_element(array,index,value);env.delete_local_ref(value);
        }
        env.set_object_array_element(array,count,category);
        if !env.exception_check(){env.set_object_field(object,field,array);}
    }
    env.delete_local_ref(old);env.delete_local_ref(array);
    if env.exception_check(){std::ptr::null_mut()}else{env.new_local_ref(object)}
}
extern "C" fn intent_categories(env:JniEnv,object:JObject)->JObject {
    let class=env.get_object_class(object);
    let array=env.get_object_field(object,env.get_field_id(class,c"categories",c"[Ljava/lang/String;"));
    env.delete_local_ref(class);
    let result=if array.is_null(){std::ptr::null_mut()}else{super::collections::set(env,array)};
    env.delete_local_ref(array);result
}
extern "C" fn intent_flags(env:JniEnv,object:JObject)->i32 {
    let class=env.get_object_class(object);
    let flags=env.get_int_field(object,env.get_field_id(class,c"flags",c"I"));
    env.delete_local_ref(class);flags
}
extern "C" fn intent_set_flags(env:JniEnv,object:JObject,flags:i32)->JObject {
    let class=env.get_object_class(object);
    env.set_int_field(object,env.get_field_id(class,c"flags",c"I"),flags);
    env.delete_local_ref(class);env.new_local_ref(object)
}
extern "C" fn intent_add_flags(env:JniEnv,object:JObject,flags:i32)->JObject {
    intent_set_flags(env,object,intent_flags(env,object)|flags)
}
extern "C" fn query_activities(env:JniEnv,_manager:JObject,_intent:JObject,_flags:i32)->JObject {
    // This host has no Android activity resolver. A real empty collection
    // lets Unity enumerate that absence without a missing-method exception.
    super::collections::empty_list(env)
}
extern "C" fn null_object(_env: JniEnv, _object: JObject) -> JObject { std::ptr::null_mut() }
extern "C" fn _null_string_argument(_env: JniEnv, _object: JObject, _key: JObject) -> JObject { std::ptr::null_mut() }
extern "C" fn obb_dirs(env: JniEnv, _context: JObject) -> JObject {
    let file = allocate(env,c"java/io/File");
    set_string_field(env,file,c"path",config_string(env,c"paths.android_external_files",c".",true));
    let class = env.find_class(c"java/io/File"); let result = env.new_object_array(1,class,file);
    env.delete_local_ref(file); env.delete_local_ref(class); result
}
extern "C" fn file_path(env: JniEnv, object: JObject) -> JObject {
    let class = env.get_object_class(object); let id = env.get_field_id(class,c"path",c"Ljava/lang/String;");
    let path = env.get_object_field(object,id); env.delete_local_ref(class); path
}
extern "C" fn file_parent(env:JniEnv,object:JObject)->JObject {
    let path=file_path(env,object);if path.is_null(){return path;}
    let units=env.string_utf16(path);env.delete_local_ref(path);
    let Some(Ok(text))=units.map(|units|String::from_utf16(&units)) else{return std::ptr::null_mut();};
    let Some(parent)=std::path::Path::new(&text).parent().filter(|p|!p.as_os_str().is_empty()) else{return std::ptr::null_mut();};
    let units:Vec<u16>=parent.to_string_lossy().encode_utf16().collect();env.new_string(&units)
}
fn run(env: JniEnv, runnable: *mut c_void) {
    let class = env.get_object_class(runnable);
    if let Some(id) = env.method(class,c"run",c"()V") { env.call_void(runnable,id,&[]); } env.delete_local_ref(class);
}
fn enqueue_ui(env:JniEnv,runnable:*mut c_void,deadline:Option<std::time::Instant>)->u8 {
    if runnable.is_null(){return 0;}let mut queue=UI_JOBS.lock().unwrap();if queue.closed{return 0;}
    let root=env.new_global_ref(runnable);if root.is_null(){return 0;}
    queue.jobs.push_back(UiJob{handle:root as usize,deadline});1
}
extern "C" fn run_on_ui(env: JniEnv, _activity: JObject, runnable: JObject) {
    if runnable.is_null(){return;}
    if UI_THREAD.get().is_some_and(|id| *id == std::thread::current().id()) { if !UI_JOBS.lock().unwrap().finished {run(env,runnable);} }
    else {enqueue_ui(env,runnable,Some(std::time::Instant::now()));}
}

extern "C" fn drain_ui(env: JniEnv, _class: JObject) {
    if !UI_THREAD.get().is_some_and(|id| *id == std::thread::current().id()) { return; }
    let budget=UI_JOBS.lock().unwrap().jobs.len();let now=std::time::Instant::now();
    for _ in 0..budget {
        let Some(job) = UI_JOBS.lock().unwrap().jobs.pop_front() else { break; };
        if !job.deadline.is_some_and(|deadline|deadline<=now) {UI_JOBS.lock().unwrap().jobs.push_back(job);continue;}
        run(env,job.handle as *mut c_void); env.delete_global_ref(job.handle as *mut c_void);
        if env.exception_check() { break; }
    }
}

pub(super) fn discard_ui_jobs(env:JniEnv) {
    let jobs={let mut queue=UI_JOBS.lock().unwrap();queue.closed=true;std::mem::take(&mut queue.jobs)};
    for job in jobs{env.delete_global_ref(job.handle as *mut c_void);}
}

pub(super) fn finish_ui_jobs(env:JniEnv){
    UI_JOBS.lock().unwrap().finished=true;discard_ui_jobs(env);
}

fn register_services(env: JniEnv) -> i32 {
    use rudroid_common::jni_abi::declare_field;
    UI_THREAD.get_or_init(|| std::thread::current().id());
    for (class, fields, is_static) in [
        (c"android/content/pm/ApplicationInfo", &[(c"dataDir",c"Ljava/lang/String;"),(c"nativeLibraryDir",c"Ljava/lang/String;"),(c"packageName",c"Ljava/lang/String;"),(c"sourceDir",c"Ljava/lang/String;"),(c"publicSourceDir",c"Ljava/lang/String;"),(c"minSdkVersion",c"I"),(c"targetSdkVersion",c"I"),(c"splitSourceDirs",c"[Ljava/lang/String;"),(c"splitPublicSourceDirs",c"[Ljava/lang/String;")] as &[( &CStr, &CStr)],false),
        (c"android/content/Context",&[(c"MODE_PRIVATE",c"I")],true),
        (c"android/content/pm/PackageItemInfo",&[(c"metaData",c"Landroid/os/Bundle;")],false),
        (c"android/content/Intent",&[(c"action",c"Ljava/lang/String;"),(c"package",c"Ljava/lang/String;"),(c"categories",c"[Ljava/lang/String;"),(c"flags",c"I")],false),
        (c"android/view/WindowManager$LayoutParams",&[(c"FLAG_KEEP_SCREEN_ON",c"I")],true),
        (c"android/content/pm/PackageInfo",&[(c"packageName",c"Ljava/lang/String;"),(c"versionName",c"Ljava/lang/String;"),(c"versionCode",c"I"),(c"applicationInfo",c"Landroid/content/pm/ApplicationInfo;")],false),
        (c"java/io/File",&[(c"path",c"Ljava/lang/String;")],false),
        (c"android/os/Environment",&[(c"MEDIA_MOUNTED",c"Ljava/lang/String;")],true),
        (c"android/os/Build$VERSION",&[(c"SDK_INT",c"I"),(c"RELEASE",c"Ljava/lang/String;"),(c"INCREMENTAL",c"Ljava/lang/String;")],true),
        (c"android/os/Build",&[(c"MANUFACTURER",c"Ljava/lang/String;"),(c"MODEL",c"Ljava/lang/String;"),(c"DEVICE",c"Ljava/lang/String;"),(c"ID",c"Ljava/lang/String;")],true),
    ] { for (name,signature) in fields {
        if declare_field(env,class,name,signature,is_static) != 0 { return -1; } } }
    if rudroid_common::jni_abi::declare_superclass(env,c"android/content/pm/ApplicationInfo",c"android/content/pm/PackageItemInfo") != 0 { return -1; }
    let version = env.find_class(c"android/os/Build$VERSION");
    env.set_static_int_field(version,env.get_static_field_id(version,c"SDK_INT",c"I"),26); env.delete_local_ref(version);
    let layout=env.find_class(c"android/view/WindowManager$LayoutParams");env.set_static_int_field(layout,env.get_static_field_id(layout,c"FLAG_KEEP_SCREEN_ON",c"I"),128);env.delete_local_ref(layout);
    for (class,name,key,default) in [
        (c"android/os/Environment",c"MEDIA_MOUNTED",c"android.storage_state",c"mounted"),
        (c"android/os/Build$VERSION",c"RELEASE",c"android.build.release",c"8.0"),
        (c"android/os/Build$VERSION",c"INCREMENTAL",c"android.build.incremental",c"Rudroid"),
        // Unity appends this identity to SystemInfo.deviceModel. "Linux" here
        // makes Android input libraries select incompatible desktop profiles.
        (c"android/os/Build",c"MANUFACTURER",c"android.build.manufacturer",c"Rudroid"),
        (c"android/os/Build",c"MODEL",c"android.build.model",c"Rudroid"),
        (c"android/os/Build",c"DEVICE",c"android.build.device",c"LinuxHandheld"),
        (c"android/os/Build",c"ID",c"android.build.id",c"Rudroid"),
    ] {
        let class = env.find_class(class); let id = env.get_static_field_id(class,name,c"Ljava/lang/String;");
        let value = config_string(env,key,default,false); env.set_static_object_field(class,id,value); env.delete_local_ref(value); env.delete_local_ref(class);
    }
    for (class,methods) in [
        (c"android/content/Context",vec![
            Native::new0(c"getFilesDir",c"()Ljava/io/File;",files_dir),
            Native::new0(c"getCacheDir",c"()Ljava/io/File;",cache_dir),
            Native::new0(c"getDataDir",c"()Ljava/io/File;",data_dir),
            Native::new1(c"getExternalFilesDir",c"(Ljava/lang/String;)Ljava/io/File;",external_dir),
            Native::new0(c"getExternalCacheDir",c"()Ljava/io/File;",cache_dir),
            Native::new0(c"getAssets",c"()Landroid/content/res/AssetManager;",assets),
            Native::new0(c"getPackageManager",c"()Landroid/content/pm/PackageManager;",package_manager),
            Native::new0(c"getApplicationInfo",c"()Landroid/content/pm/ApplicationInfo;",application_info),
            Native::new0(c"getObbDirs",c"()[Ljava/io/File;",obb_dirs),
        ]),
        (c"android/content/pm/PackageManager",vec![Native::new2(c"getPackageInfo",c"(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;",package_info),Native::new2(c"getApplicationInfo",c"(Ljava/lang/String;I)Landroid/content/pm/ApplicationInfo;",package_application_info),Native::new2(c"queryIntentActivities",c"(Landroid/content/Intent;I)Ljava/util/List;",query_activities)]),
        (c"android/content/res/AssetManager",vec![Native::new1(c"open",c"(Ljava/lang/String;)Ljava/io/InputStream;",asset_open)]),
        (c"android/os/Environment",vec![Native::new0(c"getExternalStorageState",c"()Ljava/lang/String;",storage_state)]),
        (c"android/os/Looper",vec![Native::new0(c"getMainLooper",c"()Landroid/os/Looper;",main_looper)]),
        (c"android/os/Handler",vec![Native::new1(c"<init>",c"(Landroid/os/Looper;)V",empty_constructor),Native::new2(c"postDelayed",c"(Ljava/lang/Runnable;J)Z",post_delayed),Native::new1(c"post",c"(Ljava/lang/Runnable;)Z",post)]),
        (c"java/lang/ClassLoader",vec![Native::new1(c"findLibrary",c"(Ljava/lang/String;)Ljava/lang/String;",find_library)]),
        (c"android/os/Bundle",vec![Native::new1(c"containsKey",c"(Ljava/lang/String;)Z",bundle_contains),Native::new2(c"getString",c"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;",bundle_string)]),
        (c"android/app/Activity",vec![
            Native::new0(c"getIntent",c"()Landroid/content/Intent;",intent),
            Native::new1(c"runOnUiThread",c"(Ljava/lang/Runnable;)V",run_on_ui),
        ]),
        (c"android/content/Intent",vec![
            Native::new1(c"<init>",c"(Ljava/lang/String;)V",intent_init),
            Native::new0(c"getAction",c"()Ljava/lang/String;",intent_action),
            Native::new1(c"setPackage",c"(Ljava/lang/String;)Landroid/content/Intent;",intent_set_package),
            Native::new0(c"getPackage",c"()Ljava/lang/String;",intent_package),
            Native::new1(c"addCategory",c"(Ljava/lang/String;)Landroid/content/Intent;",intent_add_category),
            Native::new0(c"getCategories",c"()Ljava/util/Set;",intent_categories),
            Native::new1(c"addFlags",c"(I)Landroid/content/Intent;",intent_add_flags),
            Native::new1(c"setFlags",c"(I)Landroid/content/Intent;",intent_set_flags),
            Native::new0(c"getFlags",c"()I",intent_flags),
            Native::new0(c"getData",c"()Landroid/net/Uri;",null_object),
            Native::new0(c"getExtras",c"()Landroid/os/Bundle;",extras),
            Native::new1(c"getStringExtra",c"(Ljava/lang/String;)Ljava/lang/String;",intent_string),
        ]),
        (c"java/io/File",vec![
            Native::new0(c"getAbsolutePath",c"()Ljava/lang/String;",file_path),
            Native::new0(c"getPath",c"()Ljava/lang/String;",file_path),
            Native::new0(c"getParent",c"()Ljava/lang/String;",file_parent),
        ]),
        (c"org/rudroid/runtime/UiThread",vec![Native::new0(c"drain",c"()V",drain_ui)]),
    ] {
        if env.register_class(class,&methods) != 0 { return -1; } }
    if env.exception_check() { -1 } else { if super::preferences::register(env)!=0{-1}else{super::services::register(env)} }
}

static PACKAGE_INFO: OnceLock<usize> = OnceLock::new();
static MAIN_LOOPER: OnceLock<usize> = OnceLock::new();
static EXTRAS: OnceLock<usize> = OnceLock::new();
static STORAGE_STATE: OnceLock<usize> = OnceLock::new();
static BASE_PACKAGE: OnceLock<usize> = OnceLock::new();
fn absolute(path:&std::path::Path)->std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_|if path.is_absolute(){path.to_owned()}else{std::env::current_dir().unwrap_or_else(|_|"/".into()).join(path)})
}
extern "C" fn base_package(env:JniEnv,_context:JObject)->JObject {
    singleton(env,&BASE_PACKAGE,|| {
        let root=code_path(env,std::ptr::null_mut());let text=super::preferences::text(env,root);env.delete_local_ref(root);
        let Some(text)=text else{return std::ptr::null_mut();};
        let path=std::path::PathBuf::from(text);
        let configured=if path.is_dir(){path.join("base.apk")}else{path.clone()};
        let chosen=[configured,std::path::PathBuf::from("base.apk"),"gamedata/base.apk".into()].into_iter().find(|p|p.is_file()).unwrap_or(path);
        super::preferences::string(env,&absolute(&chosen).to_string_lossy())
    })
}

fn split_packages(env:JniEnv)->*mut c_void {
    let mut paths=Vec::new();
    for index in 0..rudroid_common::android_assets::MAX_PACKS {
        match super::HOST.get().unwrap().asset_path(rudroid_common::android_assets::SPLIT,c"",None,index as u32) {
            Ok(Some(path))=>paths.push(path),Ok(None)=>break,Err(_)=>return std::ptr::null_mut(),
        }
    }
    if paths.is_empty(){return std::ptr::null_mut();}
    let class=env.find_class(c"java/lang/String");let array=env.new_object_array(paths.len() as i32,class,std::ptr::null_mut());
    for (index,path) in paths.iter().enumerate(){
        let value=super::preferences::string(env,&path.to_string_lossy());env.set_object_array_element(array,index as i32,value);super::preferences::delete(env,value);
    }
    super::preferences::delete(env,class);array
}
fn directory(env:JniEnv,key:&CStr,default:&CStr)->*mut c_void {
    let Some(host)=super::HOST.get() else{return std::ptr::null_mut();};
    let requested=configured(host,key,default);
    if std::fs::create_dir_all(&requested).is_err(){return std::ptr::null_mut();}
    // Resolve after creation so the first and later calls return the same
    // path. A retained trailing slash changes consumers' parent-directory logic.
    let Ok(path)=std::path::Path::new(&requested).canonicalize() else{return std::ptr::null_mut();};
    if !path.is_dir(){return std::ptr::null_mut();}
    let file=allocate(env,c"java/io/File");
    set_string_field(env,file,c"path",super::preferences::string(env,&path.to_string_lossy()));file
}
extern "C" fn files_dir(env:JniEnv,_context:JObject)->JObject {
    directory(env,c"paths.android_files",c".")
}
extern "C" fn data_dir(env:JniEnv,_context:JObject)->JObject {
    directory(env,c"paths.android_data",c".")
}
extern "C" fn cache_dir(env:JniEnv,_context:JObject)->JObject {
    directory(env,c"paths.android_cache",c".")
}
extern "C" fn external_dir(env:JniEnv,_context:JObject,_kind:JObject)->JObject {
    directory(env,c"paths.android_external_files",c".")
}
extern "C" fn package_info(env:JniEnv,_manager:JObject,_package:JObject,_flags:i32)->JObject {
    singleton(env,&PACKAGE_INFO,||{
        let object=allocate(env,c"android/content/pm/PackageInfo");
        set_string_field(env,object,c"packageName",package_name(env,std::ptr::null_mut()));
        set_string_field(env,object,c"versionName",config_string(env,c"package.versionName",c"0.1",false));
        let class=env.get_object_class(object);let info=application_info(env,std::ptr::null_mut());
        env.set_int_field(object,env.get_field_id(class,c"versionCode",c"I"),super::HOST.get().unwrap().config_i64(c"package.versionCode",1) as i32);
        env.set_object_field(object,env.get_field_id(class,c"applicationInfo",c"Landroid/content/pm/ApplicationInfo;"),info);env.delete_local_ref(info);env.delete_local_ref(class);object
    })
}
extern "C" fn asset_open(env:JniEnv,_assets:JObject,name:JObject)->JObject {
    let Some(text)=super::preferences::text(env,name) else{return std::ptr::null_mut();};
    let path=CString::new(text).ok().and_then(|name|super::HOST.get().unwrap().asset_path(rudroid_common::android_assets::MERGED,&name,None,0).ok().flatten());
    let Some(path)=path else {
        let error=env.find_class(c"java/io/FileNotFoundException");env.throw_new(error,c"asset missing or path invalid");env.delete_local_ref(error);return std::ptr::null_mut();
    };
    let name=super::preferences::string(env,&path.to_string_lossy());
    let class=env.find_class(c"java/io/FileInputStream");let ctor=env.method(class,c"<init>",c"(Ljava/lang/String;)V");
    let stream=ctor.map_or(std::ptr::null_mut(),|ctor|env.new_object(class,ctor,&[Arg::object(name)]));env.delete_local_ref(name);env.delete_local_ref(class);stream
}
extern "C" fn main_looper(env:JniEnv,_class:JObject)->JObject {
    singleton(env,&MAIN_LOOPER,||allocate(env,c"android/os/Looper"))
}
pub(super) fn current_ui_looper(env:JniEnv)->JObject {
    if UI_THREAD.get().is_some_and(|id| *id == std::thread::current().id()) {
        main_looper(env,std::ptr::null_mut())
    } else { std::ptr::null_mut() }
}
extern "C" fn empty_constructor(_env:JniEnv,_object:JObject,_argument:JObject) {}
pub(super) extern "C" fn post_delayed(env:JniEnv,_handler:JObject,runnable:JObject,milliseconds:i64)->u8 {
    if let Some(result)=super::threads::post(env,_handler,runnable,milliseconds){return result;}
    // The closed flag and global acquisition share the same queue lock.
    enqueue_ui(env,runnable,std::time::Instant::now().checked_add(std::time::Duration::from_millis(milliseconds.max(0) as u64)))

}
extern "C" fn post(env:JniEnv,handler:JObject,runnable:JObject)->u8 {post_delayed(env,handler,runnable,0)}
extern "C" fn find_library(env:JniEnv,_loader:JObject,name:JObject)->JObject {
    let Some(text)=env.string_utf(name) else{return std::ptr::null_mut();};
    let requested=text.to_string_lossy().into_owned();
    if requested.is_empty(){return std::ptr::null_mut();}
    let filename=if requested.contains('/'){requested.clone()}else{format!("{}{}{}",if requested.starts_with("lib"){""}else{"lib"},requested,if requested.ends_with(".so"){""}else{".so"})};
    for candidate in [std::path::PathBuf::from("lib/arm64-v8a").join(&filename),filename.into()] {
        if candidate.is_file(){let path=CString::new(absolute(&candidate).to_string_lossy().as_bytes()).unwrap();return env.new_string_utf(&path);}
    }
    env.new_local_ref(name)
}
extern "C" fn extras(env:JniEnv,_intent:JObject)->JObject {
    singleton(env,&EXTRAS,||allocate(env,c"android/os/Bundle"))
}
/// `package.mainIntentBundle.<key>` for the Java string `key`; `None` when unreadable.
fn bundle_key(env:JniEnv,key:*mut c_void)->Option<CString> {
    let text=env.string_utf(key)?;
    Some(CString::new(format!("package.mainIntentBundle.{}",text.to_string_lossy())).unwrap())
}
extern "C" fn bundle_contains(env:JniEnv,_bundle:JObject,key:JObject)->u8 {
    let Some(key)=bundle_key(env,key) else{return 0;};
    let host=super::HOST.get().unwrap();u8::from(host.config_string(&key,4096).is_some())
}
extern "C" fn bundle_string(env:JniEnv,bundle:JObject,key:JObject,default:JObject)->JObject {
    if bundle_contains(env,bundle,key)==0 {return env.new_local_ref(default);}
    let Some(config)=bundle_key(env,key) else{return std::ptr::null_mut();};config_string(env,&config,c"",false)
}
extern "C" fn intent_string(env:JniEnv,intent:JObject,key:JObject)->JObject {bundle_string(env,intent,key,std::ptr::null_mut())}

extern "C" fn storage_state(env:JniEnv,_class:JObject)->JObject {
    cached_string(env,&STORAGE_STATE,c"android.storage_state",c"mounted",false)
}
