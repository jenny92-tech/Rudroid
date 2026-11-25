//! INPUT: real VM/framework DSOs, caller-owned queue and registered Runnable
//! OUTPUT: same-owner dispatch, delayed/reposted jobs, exception and close checks
//! POS: no game inputs; native player queue must not borrow the creator UI queue
#![cfg(all(target_os="linux",target_arch="aarch64"))]
use std::{sync::{Mutex,atomic::{AtomicBool,AtomicI32,AtomicUsize,Ordering}},time::{Duration,Instant}};
use rudroid_common::{jni_abi,jni_env::{Arg,JObject,JniEnv,Native}};
use rudroid_unity_loader::{bring_up,host_api,load_deploy_dir,Selection};

static CALLS:AtomicI32=AtomicI32::new(0);
static MODE:AtomicI32=AtomicI32::new(0);
static WRONG:AtomicBool=AtomicBool::new(false);
static HANDLER:AtomicUsize=AtomicUsize::new(0);
static JOB:AtomicUsize=AtomicUsize::new(0);
static OWNER:Mutex<Option<std::thread::ThreadId>>=Mutex::new(None);
static CALLBACK:Mutex<Option<std::thread::ThreadId>>=Mutex::new(None);
static MESSAGE:AtomicI32=AtomicI32::new(0);
extern "C" fn handle_message(env:JniEnv,_callback:JObject,message:JObject)->u8 {
    let class=env.get_object_class(message);let field=env.get_field_id(class,c"what",c"I");
    MESSAGE.store(env.get_int_field(message,field),Ordering::SeqCst);env.delete_local_ref(class);1
}

extern "C" fn run(env:JniEnv,_object:JObject) {
    let current=std::thread::current().id();
    *CALLBACK.lock().unwrap()=Some(current);
    let mode=MODE.load(Ordering::Relaxed);
    if mode!=2 && OWNER.lock().unwrap().as_ref()!=Some(&current) {WRONG.store(true,Ordering::Relaxed);}
    let count=CALLS.fetch_add(1,Ordering::SeqCst)+1;
    if mode==1 {
        let error=env.find_class(c"java/lang/IllegalStateException");
        env.throw_new(error,c"caller Looper callback failure");env.delete_local_ref(error);
    } else if mode==0 && count==1 {
        let class=env.find_class(c"android/os/Handler");
        let post=env.method(class,c"post",c"(Ljava/lang/Runnable;)Z").unwrap();
        if !env.call_boolean(HANDLER.load(Ordering::Acquire) as JObject,post,&[Arg::object(JOB.load(Ordering::Acquire) as JObject)]) {
            WRONG.store(true,Ordering::Relaxed);
        }
        env.delete_local_ref(class);
    }
}

#[test]
fn caller_looper_preserves_owner_queue_exceptions_and_handler_threads() {
    let deploy=std::env::temp_dir().join(format!("rudroid-caller-looper-{}",std::process::id()));
    std::fs::create_dir(&deploy).unwrap();
    for (package,artifact) in [
        ("rudroid-android-elf-load","android_elf_load"),
        ("rudroid-android-jni-vm","android_jni_vm"),
        ("rudroid-android-framework","android_base"),
        ("rudroid-android-reply-catalog","rudroid_android_reply_catalog"),
    ] {
        let file=rudroid_test_support::cargo::ensure_workspace_cdylib(package,artifact);
        std::fs::copy(&file,deploy.join(file.file_name().unwrap())).unwrap();
    }
    // SAFETY: same-kit production DSOs built above, pinned by the loader.
    let caps=bring_up(unsafe{load_deploy_dir(&deploy,&Selection::All)}.unwrap(),None).unwrap();
    let vm=host_api::table().java_vm().unwrap();let env=vm.get_env(0x10006).unwrap();
    assert_eq!(jni_abi::declare_superclass(env,c"test/LoopJob",c"java/lang/Runnable"),0);
    assert_eq!(env.register_class(c"test/LoopJob",&[Native::new0(c"run",c"()V",run)]),0);
    assert_eq!(jni_abi::declare_superclass(env,c"test/MessageCallback",c"android/os/Handler$Callback"),0);
    assert_eq!(env.register_class(c"test/MessageCallback",&[Native::new1(c"handleMessage",c"(Landroid/os/Message;)Z",handle_message)]),0);
    let job_class=env.find_class(c"test/LoopJob");let local_job=env.alloc_object(job_class);
    let weak_job=env.new_weak_global_ref(local_job);
    let job=env.new_global_ref(local_job);env.delete_local_ref(local_job);JOB.store(job as usize,Ordering::Release);
    let service=env.find_class(c"org/rudroid/runtime/CallerLooper");
    let begin=env.static_method(service,c"begin",c"()Z").unwrap();
    assert!(!env.call_static_boolean(service,begin,&[]),"creator UI Looper cannot be replaced");
    let (send,receive)=std::sync::mpsc::sync_channel(0);
    let (go,ready)=std::sync::mpsc::sync_channel(0);
    let owner=std::thread::spawn(move || {
        let env=vm.attach_current_thread().unwrap();*OWNER.lock().unwrap()=Some(std::thread::current().id());
        let service=env.find_class(c"org/rudroid/runtime/CallerLooper");
        let begin=env.static_method(service,c"begin",c"()Z").unwrap();
        let drain=env.static_method(service,c"drain",c"()V").unwrap();
        let finish=env.static_method(service,c"finish",c"()V").unwrap();
        let looper_class=env.find_class(c"android/os/Looper");
        let mine=env.static_method(looper_class,c"myLooper",c"()Landroid/os/Looper;").unwrap();
        let main=env.static_method(looper_class,c"getMainLooper",c"()Landroid/os/Looper;").unwrap();
        assert!(env.call_static_object(looper_class,mine,&[]).is_null());
        assert!(env.call_static_boolean(service,begin,&[]));
        assert!(!env.call_static_boolean(service,begin,&[]),"begin must not replace its own queue");
        let looper=env.call_static_object(looper_class,mine,&[]);
        let ui=env.call_static_object(looper_class,main,&[]);
        assert!(!looper.is_null() && !env.is_same_object(looper,ui));
        let class=env.find_class(c"android/os/Handler");let handler=env.alloc_object(class);
        let init=env.method(class,c"<init>",c"()V").unwrap();env.call_void(handler,init,&[]);
        assert!(!env.exception_check(),"Handler() must bind the native owner's Looper");
        let get=env.method(class,c"getLooper",c"()Landroid/os/Looper;").unwrap();
        let bound=env.call_object(handler,get,&[]);assert!(env.is_same_object(bound,looper));
        let handler=env.new_global_ref(handler);HANDLER.store(handler as usize,Ordering::Release);
        send.send(handler as usize).unwrap();ready.recv_timeout(Duration::from_secs(3)).unwrap();
        env.call_static_void(service,drain,&[]);assert_eq!(CALLS.load(Ordering::SeqCst),1);
        env.call_static_void(service,drain,&[]);assert_eq!(CALLS.load(Ordering::SeqCst),2);
        let callback_class=env.find_class(c"test/MessageCallback");let callback=env.alloc_object(callback_class);
        let message_handler=env.alloc_object(class);
        let init=env.method(class,c"<init>",c"(Landroid/os/Looper;Landroid/os/Handler$Callback;)V").unwrap();
        env.call_void(message_handler,init,&[Arg::object(looper),Arg::object(callback)]);
        let obtain=env.method(class,c"obtainMessage",c"(I)Landroid/os/Message;").unwrap();
        let message=env.call_object(message_handler,obtain,&[Arg::int(17)]);
        let send_message=env.method(class,c"sendMessage",c"(Landroid/os/Message;)Z").unwrap();
        assert!(env.call_boolean(message_handler,send_message,&[Arg::object(message)]));
        env.call_static_void(service,drain,&[]);assert_eq!(MESSAGE.load(Ordering::SeqCst),17);
        assert!(!env.exception_check(),"a real Handler.Callback must survive constructor and message dispatch");
        let message_class=env.get_object_class(message);
        let runnable_field=env.get_field_id(message_class,c"callback",c"Ljava/lang/Runnable;");
        let payload_field=env.get_field_id(message_class,c"obj",c"Ljava/lang/Object;");
        env.set_object_field(message,runnable_field,JOB.load(Ordering::Acquire) as JObject);
        MESSAGE.store(0,Ordering::SeqCst);
        assert!(env.call_boolean(message_handler,send_message,&[Arg::object(message)]));
        env.call_static_void(service,drain,&[]);
        assert_eq!(CALLS.load(Ordering::SeqCst),3);assert_eq!(MESSAGE.load(Ordering::SeqCst),0,"message Runnable precedes Handler.Callback");
        env.set_object_field(message,runnable_field,std::ptr::null_mut());
        env.set_object_field(message,payload_field,JOB.load(Ordering::Acquire) as JObject);
        assert!(env.call_boolean(handler,send_message,&[Arg::object(message)]));
        env.call_static_void(service,drain,&[]);
        assert_eq!(CALLS.load(Ordering::SeqCst),3,"Message.obj must not be treated as a Runnable");
        MODE.store(1,Ordering::Relaxed);
        let post=env.method(class,c"post",c"(Ljava/lang/Runnable;)Z").unwrap();
        env.set_object_field(message,runnable_field,JOB.load(Ordering::Acquire) as JObject);
        assert!(env.call_boolean(message_handler,send_message,&[Arg::object(message)]));
        env.call_static_void(service,drain,&[]);assert!(env.exception_check());
        assert_eq!(MESSAGE.load(Ordering::SeqCst),0,"throwing message Runnable cannot fall through to Handler.Callback");
        let error=env.exception_occurred();env.exception_clear();
        for local in [callback_class,callback,message_handler,message,message_class] {env.delete_local_ref(local);}
        env.call_static_void(service,finish,&[]);env.call_static_void(service,finish,&[]);
        assert!(env.call_static_object(looper_class,mine,&[]).is_null());
        assert!(!env.call_boolean(handler,post,&[Arg::object(JOB.load(Ordering::Acquire) as JObject)]));
        env.throw(error);assert!(env.exception_check());env.exception_clear();
        for local in [service,looper_class,looper,ui,class,bound,error] {env.delete_local_ref(local);}
        env.delete_global_ref(handler);vm.detach_current_thread();
    });
    let handler=receive.recv_timeout(Duration::from_secs(3)).unwrap() as JObject;
    let handler=env.new_global_ref(handler);
    let class=env.find_class(c"android/os/Handler");
    let post=env.method(class,c"post",c"(Ljava/lang/Runnable;)Z").unwrap();
    let delayed=env.method(class,c"postDelayed",c"(Ljava/lang/Runnable;J)Z").unwrap();
    assert!(env.call_boolean(handler,post,&[Arg::object(job)]));
    assert!(env.call_boolean(handler,delayed,&[Arg::object(job),Arg::long(60000)]));
    assert_eq!(CALLS.load(Ordering::SeqCst),0,"posting cannot execute on the producer/UI thread");
    go.send(()).unwrap();owner.join().unwrap();
    assert_eq!(CALLS.load(Ordering::SeqCst),4);assert!(!WRONG.load(Ordering::Relaxed));
    assert!(!env.call_boolean(handler,post,&[Arg::object(job)]));
    // Existing independently running HandlerThread queues retain their lifecycle.
    MODE.store(2,Ordering::Relaxed);
    let thread_class=env.find_class(c"android/os/HandlerThread");let thread=env.alloc_object(thread_class);
    let init=env.method(thread_class,c"<init>",c"(Ljava/lang/String;)V").unwrap();
    let name=env.new_string_utf(c"compatibility HandlerThread");env.call_void(thread,init,&[Arg::object(name)]);
    let start=env.method(thread_class,c"start",c"()V").unwrap();env.call_void(thread,start,&[]);
    let get=env.method(thread_class,c"getLooper",c"()Landroid/os/Looper;").unwrap();
    let looper=env.call_object(thread,get,&[]);assert!(!looper.is_null());
    let worker_handler=env.alloc_object(class);let init=env.method(class,c"<init>",c"(Landroid/os/Looper;)V").unwrap();
    env.call_void(worker_handler,init,&[Arg::object(looper)]);assert!(env.call_boolean(worker_handler,post,&[Arg::object(job)]));
    let deadline=Instant::now()+Duration::from_secs(2);
    while CALLS.load(Ordering::SeqCst)<5 && Instant::now()<deadline {std::thread::sleep(Duration::from_millis(1));}
    assert_eq!(CALLS.load(Ordering::SeqCst),5);assert_ne!(*CALLBACK.lock().unwrap(),Some(std::thread::current().id()));
    let quit=env.method(thread_class,c"quitSafely",c"()Z").unwrap();assert!(env.call_boolean(thread,quit,&[]));
    let join=env.method(thread_class,c"join",c"()V").unwrap();env.call_void(thread,join,&[]);
    assert!(!env.call_boolean(worker_handler,post,&[Arg::object(job)]));assert!(!env.exception_check());
    env.delete_global_ref(handler);env.delete_global_ref(job);
    assert!(env.is_same_object(weak_job,std::ptr::null_mut()),"closed future jobs must release their global roots");
    env.delete_weak_global_ref(weak_job);
    for local in [service,job_class,class,thread_class,thread,name,looper,worker_handler] {env.delete_local_ref(local);}
    std::mem::forget(caps);
    std::fs::remove_dir_all(deploy).unwrap();
}
