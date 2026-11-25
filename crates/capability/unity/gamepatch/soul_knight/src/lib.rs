//! INPUT:  exact Soul Knight files, shared VM, post-Main-JNI loader operation
//! OUTPUT: original anogs/anort JNI registration and AnoApplication.initialize
//! POS:    Title-only compatibility adapter; other packages remain unaffected
#![deny(unsafe_op_in_unsafe_fn)]

mod identity;
mod android_id;
mod protocol;
mod sha256;
mod offline;
mod signature;

use std::ffi::{c_int, c_void, CStr, CString};

use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::jni_env::{Arg, JObject};
use rudroid_common::registry::{HostStr, RudroidAfterMainJniContext};
use rudroid_common::{ModuleKind, Kind};

use identity::{BootstrapStep, PACKAGE, Selection, VerifiedBuild};

rudroid_common::declare_descriptor!(
    "soul_knight",
    Kind::GamePatch,
    package = Some(PACKAGE),
    requires = Some("android_base,android_jni_vm,android_elf_load,unity_2022_3")
);

static BUILDS: rudroid_common::handles::HandleTable<VerifiedBuild> = rudroid_common::handles::HandleTable::new();

const TAG: &CStr = c"SOUL_KNIGHT";
const MAIN_MODULE: &str = "lib/arm64-v8a/libmain.so";
const JNI_VERSION_1_6: i32 = 0x0001_0006;

pub fn applies_to(api: &HostApiV0) -> bool {
    api.config_string(c"package.packageName", 256).as_deref() == Some(PACKAGE)
}

fn current_identity(api: &HostApiV0) -> Result<VerifiedBuild, String> {
    let package = api.config_string(c"package.packageName", 256);
    let version_name = api.config_string(c"package.versionName", 64);
    let version_code = api.config_i64(c"package.versionCode", 0);
    match identity::selection(package.as_deref(), version_name.as_deref(), version_code) {
        Selection::Other => Err("package gate unexpectedly did not select Soul Knight".to_owned()),
        Selection::Reject(reason) => Err(reason.to_owned()),
        Selection::Apply => identity::verify(api),
    }
}

#[no_mangle]
pub extern "C" fn rudroid_plugin_init(api: Option<&HostApiV0>) -> c_int {
    let Some(api) = api.filter(|api| api.abi_version == HOST_API_VERSION) else {
        return -2;
    };
    if !applies_to(api) {
        return 0;
    }
    let mut verified = match current_identity(api) {
        Ok(verified) => verified,
        Err(reason) => {
            api.log_error(TAG, &format!("refusing bootstrap identity: {reason}"));
            return -1;
        }
    };
    if let Err(reason) = offline::arm(api) {
        api.log_error(TAG, &format!("refusing explicit offline candidate: {reason}"));
        return -1;
    }
    // Original SDK class initializers may query ActivityThread before the
    // Application instance exists. Reserve its owner BEFORE guest loading:
    // those early queries truthfully return null instead of a generic object
    // that would conflict with the real AnoApplication created after SDK JNI.
    let Some(env) = api.java_vm().and_then(|vm| vm.get_env(JNI_VERSION_1_6)) else {
        api.log_error(TAG, "shared JNI VM is unavailable at Application reservation");
        return -1;
    };
    // Actual APK definitions constrain guest class queries before either SDK
    // JNI_OnLoad. A lookup is not a declaration and cannot manufacture an
    // optional dependency. Explicit capability declarations remain possible.
    let class_names = std::mem::take(&mut verified.class_names);
    if !rudroid_common::jni_abi::install_class_catalog(env, &class_names) {
        if env.exception_check() { env.exception_describe();env.exception_clear(); }
        api.log_error(TAG, "could not install the verified default application class catalog");
        return -1;
    }
    api.log_line(TAG, &format!("verified default DEX class catalog installed: {} classes", class_names.len()));
    drop(class_names);
    if signature::register(env) != 0 || env.exception_check() {
        if env.exception_check() { env.exception_describe(); env.exception_clear(); }
        api.log_error(TAG, "could not register the verified APK signature source");
        return -1;
    }
    let owner = env.find_class(c"org/rudroid/runtime/ApplicationOwner");
    let reserve = env.static_method(owner, c"reserve", c"()Z");
    let reserved = reserve.is_some_and(|method| env.call_static_boolean(owner, method, &[]));
    env.delete_local_ref(owner);
    if !reserved || env.exception_check() {
        if env.exception_check() { env.exception_describe(); env.exception_clear(); }
        api.log_error(TAG, "could not reserve the actual Application before guest loading");
        return -1;
    }
    // Publish only verified class metadata before SDK JNI can inspect it.
    // Neither class definition allocates an instance or executes DEX/SDK code.
    if rudroid_common::jni_abi::declare_superclass(
        env, protocol::APPLICATION_CLASS, c"android/app/Application",
    ) != 0 {
        env.exception_clear();
        api.log_error(TAG, "AnoApplication could not be linked to android.app.Application");
        return -1;
    }
    if rudroid_common::jni_abi::declare_superclass(
        env, protocol::APPLICATION_INSTANCE_CLASS, protocol::APPLICATION_CLASS,
    ) != 0 {
        env.exception_clear();
        api.log_error(TAG, "manifest Application could not be linked to AnoApplication");
        return -1;
    }
    let state = BUILDS.insert(verified);
    if !api.on_after_main_jni(after_main_jni, state) {
        BUILDS.remove(state);
        api.log_error(TAG, "loader refused the post-Main-JNI bootstrap registration");
        return -1;
    }
    api.log_line(TAG, "8.7.0 original container and libraries verified; bootstrap armed");
    0
}

extern "C" fn after_main_jni(
    context: Option<&RudroidAfterMainJniContext>,
    user: *mut c_void,
) -> c_int {
    // The post-Main callback is one-shot. Hold the verified inputs locally
    // through all guest calls and release them even on a rejected bootstrap.
    let Some(verified) = BUILDS.remove(user) else { return -1 };
    let api = &verified.host;
    let Some(context) = context else {
        api.log_error(TAG, "loader supplied no post-Main-JNI context");
        return -1;
    };
    if context.main.kind != ModuleKind::Main as u32
        || context.main.name.to_cstr().and_then(|name| name.to_str().ok()) != Some(MAIN_MODULE)
    {
        api.log_error(TAG, "post-Main-JNI hook received an unexpected Main image identity");
        return -1;
    }
    let Some(load) = context.load_and_jni_on_load else {
        api.log_error(TAG, "loader has no explicit load-and-JNI operation");
        return -1;
    };
    let mut load_library = |library: &'static str| -> Result<(), c_int> {
        let name = CString::new(library).expect("static library path has no NUL");
        let mut base = 0;
        // SAFETY: loader_user is the live bridge for this synchronous callback
        // and `name` is valid for the call.
        let result = unsafe { load(context.loader_user, HostStr::new(&name), &mut base) };
        if result == 0 {
            api.log_line(TAG, &format!("original JNI_OnLoad completed: {library} base={base:#x}"));
        }
        (result == 0).then_some(()).ok_or(result)
    };
    match execute_bootstrap(&mut load_library, &mut || run_ano_initialize(&verified)) {
        Ok(()) => {
            api.log_line(TAG, "original Application bootstrap and libu8t loading completed; continuing before il2cpp");
            0
        }
        Err(BootstrapFailure::Load { library, code }) => {
            api.log_error(
                TAG,
                &format!(
                    "original load step `{library}` failed with {code}; stopping before later bootstrap and il2cpp"
                ),
            );
            -1
        }
        Err(BootstrapFailure::Initialize) => {
            api.log_error(TAG, "original Application initialize failed; stopping before libu8t and il2cpp");
            -1
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BootstrapFailure {
    Load { library: &'static str, code: c_int },
    Initialize,
}

fn execute_bootstrap<L, I>(mut load: L, mut initialize: I) -> Result<(), BootstrapFailure>
where
    L: FnMut(&'static str) -> Result<(), c_int>,
    I: FnMut() -> Result<(), ()>,
{
    for step in identity::BOOTSTRAP_STEPS {
        match step {
            BootstrapStep::Load(library) => {
                if let Err(code) = load(library) {
                    return Err(BootstrapFailure::Load { library, code });
                }
            }
            BootstrapStep::InitializeAnoApplication => {
                if initialize().is_err() {
                    return Err(BootstrapFailure::Initialize);
                }
            }
        }
    }
    Ok(())
}

fn run_ano_initialize(verified: &VerifiedBuild) -> Result<(), ()> {
    let api = &verified.host;
    let Some(env) = api.java_vm().and_then(|vm| vm.get_env(JNI_VERSION_1_6)) else {
        api.log_error(TAG, "shared JNI VM is unavailable at AnoApplication.initialize");
        return Err(());
    };
    if env.exception_check() {
        env.exception_clear();
        api.log_error(TAG, "a JNI exception was pending before AnoApplication.initialize");
        return Err(());
    }
    // Both original classes have an evidenced empty constructor layer. The
    // concrete manifest class must be visible to getClass/getSuperclass; a bare
    // AnoApplication has the wrong runtime identity. The VM does not execute
    // DEX, attachBaseContext, MultiDex/module discovery or onCreate here, and
    // never seeds fields to imply that those nontrivial callbacks completed.
    let class = env.find_class(protocol::APPLICATION_CLASS);
    if class.is_null() || env.exception_check() {
        env.exception_clear();
        api.log_error(TAG, "com.ano.gshell.AnoApplication could not be resolved");
        return Err(());
    }
    let instance_class = env.find_class(protocol::APPLICATION_INSTANCE_CLASS);
    let application = env.alloc_object(instance_class);
    env.delete_local_ref(instance_class);
    if application.is_null() || env.exception_check() {
        env.exception_clear();
        env.delete_local_ref(class);
        api.log_error(TAG, "manifest Application default instance allocation failed");
        return Err(());
    }
    // Android Instrumentation.newApplication calls app.attach before
    // LoadedApk.mApplication / ActivityThread.mInitialApplication are set.
    // Ano.initialize runs inside that attach. Keep the explicit reservation
    // pending while it executes; only its receiver exists at this point.
    // API26 references: frameworks/base android-8.0.0_r1 core/java/android/app/
    // Instrumentation.java, LoadedApk.java, ContextImpl.java, ActivityThread.java.
    api.log_line(TAG, "original Application instance=BasePlayerApplication parent=AnoApplication; native attach initialization precedes process publication");
    let Some(method) = env.method(
        class,
        protocol::INITIALIZE,
        protocol::INITIALIZE_SIGNATURE,
    ) else {
        env.exception_clear();
        env.delete_local_ref(application);
        env.delete_local_ref(class);
        api.log_error(TAG, "AnoApplication.initialize exact descriptor is unavailable");
        return Err(());
    };
    let package = java_string(&env, PACKAGE);
    let files = java_string(&env, &verified.files_dir.to_string_lossy());
    let native_dir = java_string(&env, &verified.native_library_dir.to_string_lossy());
    let source_apk = java_string(&env, &verified.base_apk.to_string_lossy());
    let split_paths = [
        java_string(&env, &verified.split_apks[0].to_string_lossy()),
        java_string(&env, &verified.split_apks[1].to_string_lossy()),
    ];
    let string_class = env.find_class(c"java/lang/String");
    let splits = env.new_object_array(2, string_class, std::ptr::null_mut());
    for (index, split) in split_paths.iter().enumerate() {
        env.set_object_array_element(splits, index as i32, *split);
    }
    env.delete_local_ref(string_class);
    if [package, files, native_dir, source_apk, splits].iter().any(|object| object.is_null())
        || env.exception_check()
    {
        env.exception_clear();
        for local in [package, files, native_dir, source_apk, split_paths[0], split_paths[1]] {
            env.delete_local_ref(local);
        }
        env.delete_local_ref(splits);
        env.delete_local_ref(application);
        env.delete_local_ref(class);
        api.log_error(TAG, "AnoApplication path arguments could not be constructed");
        return Err(());
    }
    let result = env.call_int(
        application,
        method,
        &[
            Arg::object(package),
            Arg::object(files),
            Arg::object(native_dir),
            Arg::object(source_apk),
            Arg::object(splits),
        ],
    );
    let exception = env.exception_check();
    if exception {
        env.exception_describe();
        env.exception_clear();
    }
    for local in [package, files, native_dir, source_apk, split_paths[0], split_paths[1]] {
        env.delete_local_ref(local);
    }
    env.delete_local_ref(splits);
    env.delete_local_ref(class);
    let status=publish_after_initialize(result,exception,|| {
        api.log_line(TAG, "original AnoApplication.initialize returned 0");
        let owner=env.find_class(c"org/rudroid/runtime/ApplicationOwner");
        let publisher=env.static_method(owner,c"publish",c"(Landroid/app/Application;)Z");
        let published=publisher.is_some_and(|method|
            env.call_static_boolean(owner,method,&[Arg::object(application)]));
        env.delete_local_ref(owner);
        if !published || env.exception_check() {
            if env.exception_check() {env.exception_describe();env.exception_clear();}
            false
        } else {
            api.log_line(TAG,"manifest Application published after original attach initialization");
            true
        }
    });
    env.delete_local_ref(application);
    status.map_err(|reason|api.log_error(TAG,&reason.describe(result)))
}

fn java_string(env: &rudroid_common::jni_env::JniEnv, value: &str) -> JObject {
    let Ok(value) = CString::new(value) else {
        return std::ptr::null_mut();
    };
    env.new_string_utf(&value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InitializeFailure {
    ReturnedNonZero,
    JavaException,
    PublicationRejected,
}

impl InitializeFailure {
    fn describe(self, code: i32) -> String {
        match self {
            InitializeFailure::ReturnedNonZero => format!(
                "original AnoApplication.initialize returned {code}; stopping before il2cpp"
            ),
            InitializeFailure::JavaException => "original AnoApplication.initialize raised a JNI exception; stopping before il2cpp".to_owned(),
            InitializeFailure::PublicationRejected => "manifest Application could not become the process Application after initialize; refusing identity replacement".to_owned(),
        }
    }
}

fn initialize_outcome(code: i32, exception: bool) -> Result<(), InitializeFailure> {
    if exception {
        Err(InitializeFailure::JavaException)
    } else if code != 0 {
        Err(InitializeFailure::ReturnedNonZero)
    } else {
        Ok(())
    }
}

/// Adapter fail-stop policy: publication requires the real native attach result.
/// Failure never publishes/releases reservation; original Ano Java instead
/// catches its initialization exception, which is not this adapter's policy.
fn publish_after_initialize(code:i32,exception:bool,publish:impl FnOnce()->bool)->Result<(),InitializeFailure> {
    initialize_outcome(code,exception)?;
    if publish() {Ok(())} else {Err(InitializeFailure::PublicationRejected)}
}

#[cfg(test)]
#[path = "../tests/unit/lib.rs"]
mod tests;
