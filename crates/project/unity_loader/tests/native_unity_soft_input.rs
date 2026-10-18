//! INPUT:  trusted production shared VM/framework/legacy Unity profile DSOs
//! OUTPUT:  text ownership, exact keyboard ABI, input routing and reentrant close checks
//! POS:    isolated handheld keyboard acceptance without native game data or saves
use rudroid_common::jni_env::{Arg,JObject,JniEnv,Native};
use rudroid_unity_loader::{host_api,phases::Phase,runtime};
use std::{path::PathBuf,sync::{Mutex,atomic::{AtomicBool,AtomicI32,AtomicUsize,Ordering}}};
const PLAYER:&std::ffi::CStr=c"com/unity3d/player/UnityPlayer";
const SHOW:&std::ffi::CStr=c"(Ljava/lang/String;IZZZZLjava/lang/String;IZZ)V";
static TEXT:Mutex<Vec<u16>>=Mutex::new(Vec::new());
static VISIBLE:AtomicBool=AtomicBool::new(false);
static CLOSED:AtomicUsize=AtomicUsize::new(0);
static CANCELED:AtomicUsize=AtomicUsize::new(0);
static INJECTED:AtomicUsize=AtomicUsize::new(0);
static REOPEN:AtomicBool=AtomicBool::new(false);
static SELECTION:AtomicI32=AtomicI32::new(0);
static QUEUED:AtomicUsize=AtomicUsize::new(0);
extern "C" fn text(env:JniEnv,_player:JObject,value:JObject){*TEXT.lock().unwrap()=env.string_utf16(value).unwrap();}
extern "C" fn selection(_env:JniEnv,_player:JObject,start:i32,end:i32){SELECTION.store(if start==end{start}else{-1},Ordering::SeqCst);}
extern "C" fn visible(_env:JniEnv,_player:JObject,value:u8){VISIBLE.store(value!=0,Ordering::SeqCst);}
extern "C" fn canceled(_env:JniEnv,_player:JObject){CANCELED.fetch_add(1,Ordering::SeqCst);}
extern "C" fn injected(_env:JniEnv,_player:JObject,_event:JObject)->u8{INJECTED.fetch_add(1,Ordering::SeqCst);1}
extern "C" fn closed(env:JniEnv,player:JObject){
    CLOSED.fetch_add(1,Ordering::SeqCst);
    if REOPEN.swap(false,Ordering::SeqCst){open(env,player,"new",0);}
}
extern "C" fn poll(env:JniEnv,_class:JObject)->JObject{
    let global=QUEUED.swap(0,Ordering::SeqCst) as JObject;
    if global.is_null(){return global;}
    let local=env.new_local_ref(global);env.delete_global_ref(global);local
}
extern "C" fn no_quit(_env:JniEnv,_class:JObject)->u8{0}
fn open(env:JniEnv,player:JObject,value:&str,limit:i32){
    let class=env.get_object_class(player);let text=env.new_string(&value.encode_utf16().collect::<Vec<_>>());
    let hint=env.new_string_utf(c"hint");
    env.call_void(player,env.method(class,c"showSoftInput",SHOW).unwrap(),&[
        Arg::object(text),Arg::int(1),Arg::bool(true),Arg::bool(false),Arg::bool(true),Arg::bool(false),
        Arg::object(hint),Arg::int(limit),Arg::bool(true),Arg::bool(false)]);
    for local in [hint,text,class]{env.delete_local_ref(local);}
}
fn event(env:JniEnv,action:i32,code:i32)->JObject{
    let factory=env.find_class(c"org/rudroid/runtime/InputEvents");
    let event=env.call_static_object(factory,env.static_method(factory,c"key",c"()Ljava/lang/Object;").unwrap(),&[]);env.delete_local_ref(factory);
    let class=env.get_object_class(event);
    for (name,value) in [(c"action",action),(c"keyCode",code)]{env.set_int_field(event,env.get_field_id(class,name,c"I"),value);}
    env.delete_local_ref(class);event
}
fn key(env:JniEnv,player:JObject,action:i32,code:i32){
    let event=event(env,action,code);let class=env.get_object_class(player);
    assert!(env.call_boolean(player,env.method(class,c"injectEvent",c"(Landroid/view/InputEvent;)Z").unwrap(),&[Arg::object(event)]));
    for local in [class,event]{env.delete_local_ref(local);}
    assert!(!env.exception_check());
}
fn units(value:&str)->Vec<u16>{value.encode_utf16().collect()}

#[test]
#[ignore = "requires production RUDROID_RESOURCE_KIT; one isolated legacy-profile VM process"]
fn production_soft_input_owns_text_routes_keys_and_preserves_reentrant_keyboard(){
    std::env::remove_var("BD_SOFT_INPUT_DEFAULT");std::env::remove_var("BD_SOFT_INPUT_FORCE_DEFAULT");
    std::env::remove_var("BD_SOFT_INPUT_CONFIRM_KEYS");std::env::remove_var("BD_SOFT_INPUT_CANCEL_KEYS");
    let kit=PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").unwrap());host_api::install_config(None).unwrap();let host=host_api::table();
    // SAFETY: trusted same-kit production libraries, process-pinned for every callback.
    let libraries:Vec<_>=unsafe {[
        "libplatform_log_stderr.so","libreply_catalog.so","libandroid_jni_vm.so","libandroid_base.so","libunity_2021_3.so",
    ].iter().map(|name|std::mem::ManuallyDrop::new(libloading::Library::new(kit.join(name)).unwrap())).collect()};
    for phase in [Phase::EarlyInit,Phase::RegisterJni,Phase::Init]{for library in &libraries {
        // SAFETY: pinned production phase entries share this Host API's exact ABI.
        match unsafe{host_api::run_phase_entry(phase,library)}{Ok(())|Err(host_api::InitError::NotPresent)=>(),Err(e)=>panic!("{e:?}")}
    }}
    assert!(!(runtime::registered_profile().unwrap().prepare.unwrap()(Some(host))).is_null());let env=host.java_vm().unwrap().get_env(0x10006).unwrap();
    let class=env.find_class(PLAYER);let activity=env.get_static_object_field(class,env.get_static_field_id(class,c"currentActivity",c"Landroid/app/Activity;"));
    let activity_class=env.get_object_class(activity);let player=env.get_object_field(activity,env.get_field_id(activity_class,c"mUnityPlayer",c"Lcom/unity3d/player/UnityPlayer;"));
    assert_eq!(env.register_class(PLAYER,&[
        Native::new1(c"nativeSetInputString",c"(Ljava/lang/String;)V",text),Native::new2(c"nativeSetInputSelection",c"(II)V",selection),
        Native::new1(c"nativeSetKeyboardIsVisible",c"(Z)V",visible),Native::new0(c"nativeSoftInputClosed",c"()V",closed),
        Native::new0(c"nativeSoftInputCanceled",c"()V",canceled),Native::new1(c"nativeInjectEvent",c"(Landroid/view/InputEvent;)Z",injected),
    ]),0);
    let text_field=env.get_field_id(class,c"_rudroidSoftInputText",c"Ljava/lang/String;");
    open(env,player,"a🎮z",3);assert!(!env.exception_check());assert_eq!(*TEXT.lock().unwrap(),units("a🎮"));
    assert_eq!(SELECTION.load(Ordering::SeqCst),3);assert!(VISIBLE.load(Ordering::SeqCst));
    let retained=env.get_object_field(player,text_field);assert_eq!(env.string_utf16(retained).unwrap(),units("a🎮"));env.delete_local_ref(retained);
    key(env,player,1,100);assert_eq!(*TEXT.lock().unwrap(),units("a🎮"));
    key(env,player,1,67);assert_eq!(*TEXT.lock().unwrap(),units("a"));
    key(env,player,1,99);key(env,player,1,100);assert_eq!(*TEXT.lock().unwrap(),units("ayx"));
    key(env,player,0,96);assert_eq!(CLOSED.load(Ordering::SeqCst),0);
    key(env,player,1,96);assert_eq!(CLOSED.load(Ordering::SeqCst),1);assert!(!VISIBLE.load(Ordering::SeqCst));
    assert!(env.get_object_field(player,text_field).is_null());assert_eq!(INJECTED.load(Ordering::SeqCst),0);
    key(env,player,1,97);assert_eq!(INJECTED.load(Ordering::SeqCst),1,"ordinary input resumes after close");
    open(env,player,"cancel",0);key(env,player,1,97);assert_eq!(CANCELED.load(Ordering::SeqCst),1);
    assert!(env.get_object_field(player,text_field).is_null());
    open(env,player,"old",0);REOPEN.store(true,Ordering::SeqCst);key(env,player,1,96);
    assert_eq!(*TEXT.lock().unwrap(),units("new"));assert!(VISIBLE.load(Ordering::SeqCst));
    let retained=env.get_object_field(player,text_field);assert_eq!(env.string_utf16(retained).unwrap(),units("new"));env.delete_local_ref(retained);
    env.call_void(player,env.method(class,c"hideSoftInput",c"()V").unwrap(),&[]);assert!(env.get_object_field(player,text_field).is_null());
    let supplied=env.new_string_utf(c"second");env.call_void(player,env.method(class,c"setSoftInputStr",c"(Ljava/lang/String;I)V").unwrap(),&[Arg::object(supplied),Arg::int(0)]);env.delete_local_ref(supplied);
    assert_eq!(*TEXT.lock().unwrap(),units("second"));assert!(!VISIBLE.load(Ordering::SeqCst));
    // The real input adapter invokes no consumer on an idle poll, keeps
    // consumed events out of the engine, and recovers after consumer refusal.
    assert_eq!(env.register_class(c"org/rudroid/runtime/InputSource",&[Native::new0(c"poll",c"()Ljava/lang/Object;",poll),Native::new0(c"quitRequested",c"()Z",no_quit)]),0);
    let mut input=rudroid_common::jni_abi::PlayerInput::new(env,false).unwrap();
    assert_eq!(input.pump_with(player,class,|_|panic!("idle consumer must not run")),Ok(false));
    let enqueue=||{let event=event(env,1,96);let weak=env.new_weak_global_ref(event);QUEUED.store(env.new_global_ref(event) as usize,Ordering::SeqCst);env.delete_local_ref(event);weak};
    let released=|weak|{assert!(env.is_same_object(weak,std::ptr::null_mut()),"last event owner must be released");env.delete_weak_global_ref(weak);};
    let count=INJECTED.load(Ordering::SeqCst);let weak=enqueue();assert_eq!(input.pump_with(player,class,|_|Ok(true)),Ok(false));released(weak);
    let weak=enqueue();assert_eq!(input.pump_with(player,class,|_|Err(())),Err(()));released(weak);assert_eq!(INJECTED.load(Ordering::SeqCst),count);
    let weak=enqueue();assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(||input.pump_with(player,class,|_|panic!("consumer unwind")))).is_err());released(weak);
    let weak=enqueue();assert_eq!(input.pump(player,class),Ok(false));released(weak);assert_eq!(INJECTED.load(Ordering::SeqCst),count+1);
    assert!(!env.exception_check());
    for local in [player,activity_class,activity,class]{env.delete_local_ref(local);}
}
