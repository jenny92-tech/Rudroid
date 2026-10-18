//! INPUT:  complete production kit and a dedicated writable preference-test fixture directory
//! OUTPUT:  real JNI primitive persistence, isolated editors and fresh-process reload acceptance
//! POS:    handheld ABI test; never opens an original game's preferences or saves
use rudroid_common::{host_api::HostApiV0,jni_env::{Arg,JniEnv,JObject,Method}};
use rudroid_unity_loader::host_api;
use std::{ffi::CStr,path::PathBuf};

fn method(env:JniEnv,object:JObject,name:&CStr,signature:&CStr)->Method {
    let class=env.get_object_class(object);let method=env.method(class,name,signature).unwrap();
    env.delete_local_ref(class);assert!(!env.exception_check());method
}
fn string(env:JniEnv,value:&str)->JObject {env.new_string(&value.encode_utf16().collect::<Vec<_>>())}
fn put(env:JniEnv,editor:JObject,name:&CStr,sig:&CStr,key:&str,value:Arg) {
    let key=string(env,key);let returned=env.call_object(editor,method(env,editor,name,sig),&[Arg::object(key),value]);
    assert!(!env.exception_check());assert!(env.is_same_object(editor,returned));
    env.delete_local_ref(key);env.delete_local_ref(returned);
}
fn read_long(env:JniEnv,prefs:JObject,key:&str,default:i64)->i64 {
    let key=string(env,key);let result=env.call_long(prefs,method(env,prefs,c"getLong",c"(Ljava/lang/String;J)J"),&[Arg::object(key),Arg::long(default)]);env.delete_local_ref(key);assert!(!env.exception_check());result
}
fn read_float(env:JniEnv,prefs:JObject,key:&str,default:f32)->f32 {
    let key=string(env,key);let result=env.call_float(prefs,method(env,prefs,c"getFloat",c"(Ljava/lang/String;F)F"),&[Arg::object(key),Arg::float(default)]);env.delete_local_ref(key);assert!(!env.exception_check());result
}
fn read_bool(env:JniEnv,prefs:JObject,key:&str,default:bool)->bool {
    let key=string(env,key);let result=env.call_boolean(prefs,method(env,prefs,c"getBoolean",c"(Ljava/lang/String;Z)Z"),&[Arg::object(key),Arg::bool(default)]);env.delete_local_ref(key);assert!(!env.exception_check());result
}
#[test]
#[ignore = "run on handheld twice with RUDROID_RESOURCE_KIT, RUDROID_PREFS_FIXTURE and RUDROID_PREFS_MODE=write/read"]
fn primitive_preferences_survive_real_jni_and_fresh_process_reload() {
    let kit=PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").expect("production kit required"));
    let root=PathBuf::from(std::env::var_os("RUDROID_PREFS_FIXTURE").expect("isolated fixture required"));
    let mode=std::env::var("RUDROID_PREFS_MODE").expect("write/read mode required");
    assert!(matches!(mode.as_str(),"write"|"read"));
    let marker=root.join(".rudroid-preferences-test");
    if mode=="write" {assert!(!root.exists(),"never overwrite an existing fixture");std::fs::create_dir_all(&root).unwrap();std::fs::write(&marker,b"isolated-primitive-preferences-v1").unwrap();}
    assert_eq!(std::fs::read(&marker).unwrap(),b"isolated-primitive-preferences-v1");
    let config=rudroid_host_config::HostConfig::parse(&format!("[paths]\nandroid_files={:?}\n",root.to_str().unwrap())).unwrap();
    host_api::install_config(Some(config)).unwrap();let host=host_api::table();
    // SAFETY: trusted same production kit and exact phase ABI; DSO code is process-pinned.
    let libraries:Vec<_>=unsafe {["libplatform_log_stderr.so","libreply_catalog.so","libandroid_jni_vm.so","libandroid_base.so"].into_iter().map(|name|std::mem::ManuallyDrop::new(libloading::Library::new(kit.join(name)).unwrap())).collect()};
    for phase in [rudroid_unity_loader::phases::Phase::EarlyInit,rudroid_unity_loader::phases::Phase::RegisterJni,rudroid_unity_loader::phases::Phase::Init] {
        for library in &libraries {
            // SAFETY: library remains loaded for all registered callbacks and the shared VM.
            match unsafe{host_api::run_phase_entry(phase,library)} {Ok(())|Err(host_api::InitError::NotPresent)=>{},Err(e)=>panic!("phase failed: {e:?}")}
        }
    }
    let env=host.java_vm().unwrap().get_env(0x10006).unwrap();let class=env.find_class(c"android/content/Context");let context=env.alloc_object(class);env.delete_local_ref(class);
    let name=string(env,"primitive-fixture");let prefs=env.call_object(context,method(env,context,c"getSharedPreferences",c"(Ljava/lang/String;I)Landroid/content/SharedPreferences;"),&[Arg::object(name),Arg::int(0)]);
    env.delete_local_ref(name);assert!(!prefs.is_null());assert!(!env.exception_check());
    let big=9_007_199_254_740_993i64;
    if mode=="write" {
        assert_eq!(read_long(env,prefs,"missing",i64::MIN),i64::MIN);assert!(read_bool(env,prefs,"missing",true));assert!(!read_bool(env,prefs,"missing",false));
        let editor=env.call_object(prefs,method(env,prefs,c"edit",c"()Landroid/content/SharedPreferences$Editor;"),&[]);
        put(env,editor,c"putLong",c"(Ljava/lang/String;J)Landroid/content/SharedPreferences$Editor;","large",Arg::long(big));
        put(env,editor,c"putLong",c"(Ljava/lang/String;J)Landroid/content/SharedPreferences$Editor;","minimum",Arg::long(i64::MIN));
        put(env,editor,c"putFloat",c"(Ljava/lang/String;F)Landroid/content/SharedPreferences$Editor;","fraction",Arg::float(-0.375));
        put(env,editor,c"putFloat",c"(Ljava/lang/String;F)Landroid/content/SharedPreferences$Editor;","negative-zero",Arg::float(-0.0));
        put(env,editor,c"putBoolean",c"(Ljava/lang/String;Z)Landroid/content/SharedPreferences$Editor;","yes",Arg::bool(true));
        put(env,editor,c"putBoolean",c"(Ljava/lang/String;Z)Landroid/content/SharedPreferences$Editor;","no",Arg::bool(false));
        assert_eq!(read_long(env,prefs,"large",99),99,"edits stay isolated before commit");
        assert!(env.call_boolean(editor,method(env,editor,c"commit",c"()Z"),&[]));assert!(!env.exception_check());env.delete_local_ref(editor);
        let path=root.join("shared_prefs/primitive-fixture.json");let saved=std::fs::read_to_string(path).unwrap();assert!(saved.contains("9007199254740993"));
    }
    assert_eq!(read_long(env,prefs,"large",0),big);assert_eq!(read_long(env,prefs,"minimum",0),i64::MIN);
    assert_eq!(read_float(env,prefs,"fraction",9.0),-0.375);assert_eq!(read_float(env,prefs,"negative-zero",1.0).to_bits(),(-0.0f32).to_bits());
    assert!(read_bool(env,prefs,"yes",false));assert!(!read_bool(env,prefs,"no",true));
    // getAll must retain exact boxed Long/Boolean identities and accessors.
    let map=env.call_object(prefs,method(env,prefs,c"getAll",c"()Ljava/util/Map;"),&[]);assert!(!map.is_null());assert!(!env.exception_check());
    let entries=env.call_object(map,method(env,map,c"entrySet",c"()Ljava/util/Set;"),&[]);
    let iter=env.call_object(entries,method(env,entries,c"iterator",c"()Ljava/util/Iterator;"),&[]);
    let has=method(env,iter,c"hasNext",c"()Z");let next=method(env,iter,c"next",c"()Ljava/lang/Object;");let mut seen=0;
    while env.call_boolean(iter,has,&[]) {
        let entry=env.call_object(iter,next,&[]);let key=env.call_object(entry,method(env,entry,c"getKey",c"()Ljava/lang/Object;"),&[]);let value=env.call_object(entry,method(env,entry,c"getValue",c"()Ljava/lang/Object;"),&[]);
        match env.string(key).unwrap().as_str(){"large"=>{assert_eq!(env.call_long(value,method(env,value,c"longValue",c"()J"),&[]),big);seen+=1;},"yes"=>{assert!(env.call_boolean(value,method(env,value,c"booleanValue",c"()Z"),&[]));seen+=1;},_=>{}}
        assert!(!env.exception_check());for object in [value,key,entry]{env.delete_local_ref(object);}
    }
    assert_eq!(seen,2);for object in [iter,entries,map,prefs,context]{env.delete_local_ref(object);}
    let _contract: &HostApiV0=host;
}
