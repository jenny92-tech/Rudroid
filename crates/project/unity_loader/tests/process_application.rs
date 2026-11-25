//! INPUT: real separately loaded framework/VM libraries and a synthetic Application subclass
//! OUTPUT: process identity, rooting, idempotence and conflicting-owner checks
//! POS: isolated process cases; no game SDK implementation or original input
#![cfg(all(target_os = "linux", target_arch = "aarch64"))]

use std::ffi::CStr;
use rudroid_common::jni_env::{Arg, JniEnv, JObject, Native};
use rudroid_unity_loader::{bring_up, host_api, load_deploy_dir, Selection};

fn call(env: JniEnv, object: JObject, name: &CStr, signature: &CStr) -> JObject {
    let class = env.get_object_class(object);
    let method = env.method(class, name, signature).unwrap();
    let result = env.call_object(object, method, &[]);
    env.delete_local_ref(class);
    result
}

fn current(env: JniEnv) -> JObject {
    let class = env.find_class(c"android/app/ActivityThread");
    let method = env.static_method(class, c"currentApplication", c"()Landroid/app/Application;").unwrap();
    let result = env.call_static_object(class, method, &[]);
    env.delete_local_ref(class);
    result
}

fn publish(env: JniEnv, application: JObject) -> bool {
    let class = env.find_class(c"org/rudroid/runtime/ApplicationOwner");
    let method = env.static_method(class, c"publish", c"(Landroid/app/Application;)Z").unwrap();
    let result = env.call_static_boolean(class, method, &[Arg::object(application)]);
    env.delete_local_ref(class);
    result
}

fn reserve(env: JniEnv) -> bool {
    let class = env.find_class(c"org/rudroid/runtime/ApplicationOwner");
    let method = env.static_method(class, c"reserve", c"()Z").unwrap();
    let result = env.call_static_boolean(class, method, &[]);
    env.delete_local_ref(class);
    result
}

fn assert_exception(env: JniEnv, name: &CStr) {
    assert!(env.exception_check());
    let exception = env.exception_occurred();
    env.exception_clear();
    let class = env.find_class(name);
    assert!(env.is_instance_of(exception, class));
    env.delete_local_ref(exception);
    env.delete_local_ref(class);
}

extern "C" fn attaching_before_publication(env:JniEnv,application:JObject)->i32 {
    let from_thread=current(env);
    let from_context=call(env,application,c"getApplicationContext",c"()Landroid/content/Context;");
    let failed=!from_thread.is_null() || !from_context.is_null() || env.exception_check();
    env.delete_local_ref(from_thread);env.delete_local_ref(from_context);
    i32::from(failed)
}

#[test]
fn process_application_identity_and_ownership() {
    let Ok(case) = std::env::var("RUDROID_TEST_APPLICATION_OWNER_CASE") else {
        for case in ["published-first", "fallback-first", "reserved-first", "sdk-negative", "sdk-overflow"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "process_application_identity_and_ownership", "--nocapture"])
                .env("RUDROID_TEST_APPLICATION_OWNER_CASE", case).output().unwrap();
            assert!(output.status.success(), "{case}: {} {}",
                String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    assert!(matches!(case.as_str(), "published-first" | "fallback-first" | "reserved-first" | "sdk-negative" | "sdk-overflow"));
    let deploy = std::env::temp_dir().join(format!("rudroid-application-owner-{}", std::process::id()));
    std::fs::create_dir(&deploy).unwrap();
    for (package, artifact) in [
        ("rudroid-android-elf-load", "android_elf_load"),
        ("rudroid-android-jni-vm", "android_jni_vm"),
        ("rudroid-android-framework", "android_base"),
        ("rudroid-android-reply-catalog", "rudroid_android_reply_catalog"),
    ] {
        let library = rudroid_test_support::cargo::ensure_workspace_cdylib(package, artifact);
        std::fs::copy(&library, deploy.join(library.file_name().unwrap())).unwrap();
    }
    let declarations=match case.as_str() {
        "sdk-negative" => "minSdkVersion=0\ntargetSdkVersion=35\n",
        "sdk-overflow" => "minSdkVersion=23\ntargetSdkVersion=2147483648\n",
        _ => "",
    };
    let config = rudroid_host_config::HostConfig::parse(&format!("[package]\npackageName='org.rudroid.fixture'\n{declarations}")).unwrap();
    host_api::install_config(Some(config)).unwrap();
    // SAFETY: only the same-kit production workspace libraries built above.
    let capabilities = unsafe { load_deploy_dir(&deploy, &Selection::All) }.unwrap();
    let capabilities = bring_up(capabilities, None).unwrap();
    let env = host_api::table().java_vm().unwrap().get_env(0x10006).unwrap();
    let activity=env.find_class(c"android/app/Activity");
    let context=env.alloc_object(activity);
    let info=call(env,context,c"getApplicationInfo",c"()Landroid/content/pm/ApplicationInfo;");
    if case.starts_with("sdk-") {
        assert!(info.is_null());
        assert_exception(env,c"java/lang/IllegalArgumentException");
        assert!(call(env,context,c"getApplicationInfo",c"()Landroid/content/pm/ApplicationInfo;").is_null());
        assert_exception(env,c"java/lang/IllegalArgumentException");
        let manager=call(env,context,c"getPackageManager",c"()Landroid/content/pm/PackageManager;");
        let manager_class=env.get_object_class(manager);
        let get=env.method(manager_class,c"getPackageInfo",c"(Ljava/lang/String;I)Landroid/content/pm/PackageInfo;").unwrap();
        let name=env.new_string_utf(c"org.rudroid.fixture");
        for _ in 0..2 {
            assert!(env.call_object(manager,get,&[Arg::object(name),Arg::int(0)]).is_null());
            assert_exception(env,c"java/lang/IllegalArgumentException");
        }
        for local in [name,manager_class,manager] {env.delete_local_ref(local);}
        env.delete_local_ref(context);env.delete_local_ref(activity);
        std::mem::forget(capabilities);
        std::fs::remove_dir_all(deploy).unwrap();
        return;
    }
    let info_class=env.get_object_class(info);
    assert_eq!(env.get_int_field(info,env.get_field_id(info_class,c"minSdkVersion",c"I")),23);
    assert_eq!(env.get_int_field(info,env.get_field_id(info_class,c"targetSdkVersion",c"I")),26);
    for local in [info_class,info,context,activity] {env.delete_local_ref(local);}
    assert!(!env.exception_check());
    if case == "reserved-first" {
        assert!(reserve(env) && reserve(env), "pending reservation is idempotent");
        assert!(current(env).is_null() && !env.exception_check(), "no Application exists yet");
    }
    assert_eq!(rudroid_common::jni_abi::declare_superclass(env,
        c"org/rudroid/fixture/NativeApplication", c"android/app/Application"), 0);
    assert_eq!(rudroid_common::jni_abi::declare_superclass(env,
        c"org/rudroid/fixture/ActualApplication", c"org/rudroid/fixture/NativeApplication"), 0);
    let class = env.find_class(c"org/rudroid/fixture/ActualApplication");
    let application = env.alloc_object(class);
    assert!(!application.is_null() && !env.exception_check());

    if case == "fallback-first" {
        let fallback = current(env);
        assert!(!fallback.is_null());
        assert!(!reserve(env));
        assert_exception(env, c"java/lang/IllegalStateException");
        assert!(!publish(env, application));
        assert_exception(env, c"java/lang/IllegalStateException");
        let again = current(env);
        assert!(env.is_same_object(fallback, again));
        assert!(!env.is_same_object(application, again));
        for local in [fallback, again, application, class] { env.delete_local_ref(local); }
    } else {
        assert!(!publish(env, std::ptr::null_mut()));
        assert_exception(env, c"java/lang/NullPointerException");
        let invalid = env.new_string_utf(c"not an Application");
        assert!(!publish(env, invalid));
        assert_exception(env, c"java/lang/ClassCastException");
        env.delete_local_ref(invalid);
        if case == "reserved-first" {
            assert!(current(env).is_null() && !env.exception_check(), "invalid publication cannot release the reservation");
            assert_eq!(env.register_class(c"org/rudroid/fixture/NativeApplication",&[
                Native::new0(c"attachOwnerProbe",c"()I",attaching_before_publication),
            ]),0);
            let attach=env.method(class,c"attachOwnerProbe",c"()I").unwrap();
            assert_eq!(env.call_int(application,attach,&[]),0,
                "native attach receives the actual object while process application queries remain null");
            assert!(!env.exception_check());
        }
        let weak = env.new_weak_global_ref(application);
        assert!(publish(env, application));
        assert!(publish(env, application), "same owner is idempotent");
        env.delete_local_ref(application);
        // The weak comparison is not an extra strong root: only the framework
        // keeps the actual Application alive after the caller local disappears.
        let actual = current(env);
        assert!(!actual.is_null() && !env.is_same_object(weak, std::ptr::null_mut()));
        assert!(env.is_instance_of(actual, class));
        assert!(env.is_same_object(weak, actual));
        let runtime_class = env.get_object_class(actual);
        assert!(env.is_same_object(runtime_class, class), "concrete class must survive publication");
        let parent = call(env, runtime_class, c"getSuperclass", c"()Ljava/lang/Class;");
        let native_class = env.find_class(c"org/rudroid/fixture/NativeApplication");
        assert!(env.is_same_object(parent, native_class));
        let grandparent = call(env, parent, c"getSuperclass", c"()Ljava/lang/Class;");
        let application_class = env.find_class(c"android/app/Application");
        assert!(env.is_same_object(grandparent, application_class));
        let context = call(env, actual, c"getApplicationContext", c"()Landroid/content/Context;");
        let thread_class = env.find_class(c"android/app/ActivityThread");
        let get_thread = env.static_method(thread_class, c"currentActivityThread", c"()Landroid/app/ActivityThread;").unwrap();
        let thread = env.call_static_object(thread_class, get_thread, &[]);
        let from_thread = call(env, thread, c"getApplication", c"()Landroid/app/Application;");
        assert!(env.is_same_object(actual, context) && env.is_same_object(actual, from_thread));
        assert!(!reserve(env), "an actual owner cannot return to a pending state");
        assert_exception(env, c"java/lang/IllegalStateException");
        let conflicting = env.alloc_object(class);
        assert!(!publish(env, conflicting));
        assert_exception(env, c"java/lang/IllegalStateException");
        let still = current(env);
        assert!(env.is_same_object(actual, still));
        env.delete_weak_global_ref(weak);
        for local in [actual, runtime_class, parent, native_class, grandparent, application_class, context, thread_class, thread, from_thread, conflicting, still, class] { env.delete_local_ref(local); }
    }
    assert!(!env.exception_check());
    std::mem::forget(capabilities); // production loader pins successful code
    std::fs::remove_dir_all(deploy).unwrap();
}
