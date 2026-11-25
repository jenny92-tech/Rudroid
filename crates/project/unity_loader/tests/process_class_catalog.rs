//! INPUT: real production framework/VM DSOs and synthetic class metadata
//! OUTPUT: typed class misses, query identity and provider declarations after sealing
//! POS: isolated process regression, no game inputs or SDK result substitutes
#![cfg(all(target_os="linux",target_arch="aarch64"))]

use std::{ffi::CStr, sync::atomic::{AtomicI32,Ordering}};
use rudroid_common::{jni_abi, jni_env::{Arg,JniEnv,JObject,Native}};
use rudroid_unity_common::PlayerInputDispatch;
use rudroid_unity_loader::{bring_up,host_api,load_deploy_dir,runtime,Selection};

static CALLS:AtomicI32=AtomicI32::new(0);
static INPUT_MODE:AtomicI32=AtomicI32::new(-1);
extern "C" fn counted(_env:JniEnv,_object:JObject)->i32 {CALLS.fetch_add(1,Ordering::Relaxed)+1}
extern "C" fn input(_env:JniEnv,_player:JObject,event:JObject,mode:i32)->u8 {
    INPUT_MODE.store(mode,Ordering::Relaxed);u8::from(!event.is_null())
}

fn exception(env:JniEnv,expected:&CStr) {
    assert!(env.exception_check());
    let value=env.exception_occurred();env.exception_clear();
    let class=env.get_object_class(value);
    let throwable=env.find_class(c"java/lang/Throwable");assert!(env.is_instance_of(value,throwable));
    let class_class=env.get_object_class(class);
    let get_name=env.method(class_class,c"getName",c"()Ljava/lang/String;").unwrap();
    let name=env.call_object(class,get_name,&[]);
    assert_eq!(env.string_utf(name).unwrap().as_c_str(),expected);
    for local in [name,class_class,class,value,throwable] {env.delete_local_ref(local);}
}

#[test]
fn class_catalog_queries_and_trusted_declarations_use_the_same_real_vm() {
    let deploy=std::env::temp_dir().join(format!("rudroid-class-catalog-{}",std::process::id()));
    std::fs::create_dir(&deploy).unwrap();
    for (package,artifact) in [
        ("rudroid-android-elf-load","android_elf_load"),
        ("rudroid-android-jni-vm","android_jni_vm"),
        ("rudroid-android-framework","android_base"),
        ("rudroid-android-reply-catalog","rudroid_android_reply_catalog"),
        ("rudroid-unity-profile-2022-3","unity_2022_3"),
    ] {
        let file=rudroid_test_support::cargo::ensure_workspace_cdylib(package,artifact);
        std::fs::copy(&file,deploy.join(file.file_name().unwrap())).unwrap();
    }
    // SAFETY: explicit same-kit production DSOs built above, pinned by loader.
    let caps=unsafe{load_deploy_dir(&deploy,&Selection::All)}.unwrap();
    let _caps=bring_up(caps,None).unwrap();
    let env=host_api::table().java_vm().unwrap().get_env(0x10006).unwrap();
    assert!(!jni_abi::install_class_catalog(env,&["catalog/Defined".into(),"bad.name".into()]));
    // Do not inspect/create an exception object before the cold bootstrap test.
    assert!(env.exception_check());env.exception_clear();
    let legacy=env.find_class(c"legacy/BeforeCatalog");assert!(!legacy.is_null());env.delete_local_ref(legacy);
    assert!(jni_abi::install_class_catalog(env,&["catalog/Defined".into()]));
    for name in [c"java/lang/NoSuchMethodError",c"java/lang/NoClassDefFoundError",c"java/lang/ClassNotFoundException"] {
        let class=env.find_class(name);assert!(!class.is_null());env.delete_local_ref(class);
    }
    for _ in 0..2 {
        assert!(env.find_class(c"catalog/Absent").is_null());
        exception(env,c"java.lang.NoClassDefFoundError");
    }
    let class=env.find_class(c"java/lang/Class");
    let for_name=env.static_method(class,c"forName",c"(Ljava/lang/String;ZLjava/lang/ClassLoader;)Ljava/lang/Class;").unwrap();
    let absent=env.new_string_utf(c"catalog.Absent");
    assert!(env.call_static_object(class,for_name,&[Arg::object(absent),Arg::bool(false),Arg::object(std::ptr::null_mut())]).is_null());
    exception(env,c"java.lang.ClassNotFoundException");
    let defined=env.find_class(c"catalog/Defined");assert!(!defined.is_null());
    let name=env.new_string_utf(c"catalog.Defined");
    let reflected=env.call_static_object(class,for_name,&[Arg::object(name),Arg::bool(false),Arg::object(std::ptr::null_mut())]);
    assert!(env.is_same_object(defined,reflected));
    let get_loader=env.method(class,c"getClassLoader",c"()Ljava/lang/ClassLoader;").unwrap();
    let loader=env.call_object(defined,get_loader,&[]);
    let loader_class=env.get_object_class(loader);
    let load=env.method(loader_class,c"loadClass",c"(Ljava/lang/String;)Ljava/lang/Class;").unwrap();
    assert!(env.call_object(loader,load,&[Arg::object(absent)]).is_null());exception(env,c"java.lang.ClassNotFoundException");
    let from_loader=env.call_object(loader,load,&[Arg::object(name)]);
    assert!(env.is_same_object(defined,from_loader));
    assert!(env.find_class(c"[[Lcatalog/Absent;").is_null());exception(env,c"java.lang.NoClassDefFoundError");
    let array=env.find_class(c"[[Lcatalog/Defined;");assert!(!array.is_null());

    // The declaration path must work after lookup is sealed; it supplies actual
    // metadata and a callable native body, without making unknown queries succeed.
    assert_eq!(env.register_class(c"provider/Late",&[Native::new0(c"counted",c"()I",counted)]),0);
    assert_eq!(jni_abi::declare_superclass(env,c"provider/Late",c"android/app/Application"),0);
    assert_eq!(jni_abi::declare_field(env,c"provider/Late",c"value",c"I",false),0);
    let late=env.find_class(c"provider/Late");let object=env.alloc_object(late);
    let app=env.find_class(c"android/app/Application");assert!(env.is_instance_of(object,app));
    let field=env.get_field_id(late,c"value",c"I");env.set_int_field(object,field,73);assert_eq!(env.get_int_field(object,field),73);
    let method=env.method(late,c"counted",c"()I").unwrap();
    assert_eq!(env.call_int(object,method,&[]),1);assert_eq!(env.call_int(object,method,&[]),2);
    assert!(jni_abi::install_class_catalog(env,&["catalog/Defined".into()]));
    assert!(!jni_abi::install_class_catalog(env,&["catalog/Different".into()]));exception(env,c"java.lang.IllegalStateException");
    assert!(env.find_class(c"catalog/Different").is_null());exception(env,c"java.lang.NoClassDefFoundError");
    let context_class=env.find_class(c"android/content/Context");let context=env.alloc_object(context_class);
    let get_resolver=env.method(context_class,c"getContentResolver",c"()Landroid/content/ContentResolver;").unwrap();
    let resolver=env.call_object(context,get_resolver,&[]);assert!(!resolver.is_null());
    let resolver_again=env.call_object(context,get_resolver,&[]);
    assert!(env.is_same_object(resolver,resolver_again));
    let resolver_class=env.find_class(c"android/content/ContentResolver");
    assert!(env.is_instance_of(resolver,resolver_class));
    for local in [context_class,context,resolver,resolver_again,resolver_class] {env.delete_local_ref(local);}
    for local in [class,absent,defined,name,reflected,array,late,object,app,loader,loader_class,from_loader] {env.delete_local_ref(local);}
    assert!(!env.exception_check());
    runtime::prepare_engine_runtime(&_caps).expect("profile-owned Activity/Surface stay declared under catalog lookup");
    // Synthetic registered native, real shared VM and real dispatch resolver:
    // verify its first cold NoSuchMethodError lookup and the 2022 mode0 ABI.
    assert_eq!(env.register_class(c"com/unity3d/player/UnityPlayer",&[
        Native::new2(c"nativeInjectEvent",c"(Landroid/view/InputEvent;I)Z",input)]),0);
    let unity=env.find_class(c"com/unity3d/player/UnityPlayer");
    let dispatch=PlayerInputDispatch::resolve(env,unity).expect("first input resolves after strict catalog");
    assert!(matches!(dispatch,PlayerInputDispatch::Unity2022(_)));
    let activity=env.get_static_object_field(unity,env.get_static_field_id(unity,c"currentActivity",c"Landroid/app/Activity;"));
    let activity_class=env.get_object_class(activity);
    let player=env.get_object_field(activity,env.get_field_id(activity_class,c"mUnityPlayer",c"Lcom/unity3d/player/UnityPlayer;"));
    let event_class=env.find_class(c"android/view/KeyEvent");let event=env.alloc_object(event_class);
    assert!(dispatch.call(env,player,event).unwrap());assert_eq!(INPUT_MODE.load(Ordering::Relaxed),0);
    for local in [unity,activity,activity_class,player,event_class,event] {env.delete_local_ref(local);}
    assert!(!env.exception_check());
    std::fs::remove_dir_all(deploy).unwrap();
}
