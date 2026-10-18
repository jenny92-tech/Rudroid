//! INPUT: Java Thread/HandlerThread lifecycle and Handler jobs using shared JNI globals
//! OUTPUT: owned host workers, deadline queues, join/quit and paired JNI attachment/references
//! POS: core scheduling service; integer IDs identify Rust owners without exposing Rust pointers
#![forbid(unsafe_code)]
use rudroid_common::jni_abi::{declare_field, declare_superclass};
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};
struct Job {
    runnable: usize,
    deadline: Option<Instant>,
}
struct Worker {
    jobs: Mutex<VecDeque<Job>>,
    wake: Condvar,
    stop: AtomicBool,
    safe_cutoff: Mutex<Option<Instant>>,
    alive: AtomicBool,
    looper: Mutex<usize>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}
static WORKERS: OnceLock<Mutex<HashMap<u64, Arc<Worker>>>> = OnceLock::new();
static START: Mutex<()> = Mutex::new(());
static CLOSING: AtomicBool = AtomicBool::new(false);
static NEXT: AtomicU64 = AtomicU64::new(1);
static THREAD_ID: OnceLock<usize> = OnceLock::new();
static LOOP_ID: OnceLock<usize> = OnceLock::new();
static HANDLER_ID: OnceLock<usize> = OnceLock::new();
static TRACE_MESSAGES: OnceLock<bool> = OnceLock::new();
fn trace_messages() -> bool {
    *TRACE_MESSAGES.get_or_init(|| {
        super::HOST.get().unwrap().config_bool(c"jni.trace_messages", false)
    })
}
thread_local! {static CURRENT:std::cell::Cell<u64>=const{std::cell::Cell::new(0)};}
fn workers() -> &'static Mutex<HashMap<u64, Arc<Worker>>> {
    WORKERS.get_or_init(|| Mutex::new(HashMap::new()))
}

// The player invokes this after nativeDone has stopped native Thread targets.
// Handler queues stop under their own lock before joining, without holding
// START across native reentrant calls or a worker's final JNI detach.
pub(super) fn shutdown(env: JniEnv) {
    let all: Vec<_> = {
        let _start = START.lock().unwrap();
        CLOSING.store(true, Ordering::Release);
        workers()
            .lock()
            .unwrap()
            .iter()
            .map(|(&id, w)| (id, Arc::clone(w)))
            .collect()
    };
    for (_, worker) in &all {
        close_queue(env, worker);
    }
    let current = CURRENT.with(|id| id.get());
    for (id, worker) in all {
        if id == current {
            continue;
        }
        let mut handle = worker.handle.lock().unwrap();
        if let Some(thread) = handle.take() {
            let _ = thread.join();
        }
        drop(handle);
        workers().lock().unwrap().remove(&id);
    }
}
// Close under the same queue lock used by post. No task can acquire a new
// global after the final drain, including native spawn/attach failure paths.
fn close_queue(env: JniEnv, worker: &Worker) {
    let jobs = {
        let mut jobs = worker.jobs.lock().unwrap();
        worker.stop.store(true, Ordering::Release);
        std::mem::take(&mut *jobs)
    };
    worker.wake.notify_all();
    for job in jobs {
        delete_global(env, job.runnable);
    }
}
fn get_id(env: JniEnv, object: *mut c_void, field: &OnceLock<usize>) -> u64 {
    env.get_long_field(object, *field.get().unwrap() as *mut c_void) as u64
}
fn set_id(env: JniEnv, object: *mut c_void, field: &OnceLock<usize>, id: u64) {
    env.set_long_field(object, *field.get().unwrap() as *mut c_void, id as i64)
}
fn global(env: JniEnv, object: *mut c_void) -> usize {
    env.new_global_ref(object) as usize
}
fn delete_global(env: JniEnv, reference: usize) {
    if reference != 0 {
        env.delete_global_ref(reference as *mut c_void)
    }
}
fn run(env: JniEnv, object: usize) {
    let class = env.get_object_class(object as *mut c_void);
    if let Some(method) = env.method(class, c"run", c"()V") {
        env.call_void(object as *mut c_void, method, &[]);
    }
    super::preferences::delete(env, class);
}
extern "C" fn init_name(env: JniEnv, object: JObject, name: JObject) {
    super::java::set_string_field(env, object, c"name", env.new_local_ref(name));
}
extern "C" fn init_target(env: JniEnv, object: JObject, target: JObject) {
    let class = env.get_object_class(object);
    env.set_object_field(
        object,
        env.get_field_id(class, c"target", c"Ljava/lang/Runnable;"),
        target,
    );
    super::preferences::delete(env, class);
}
extern "C" fn thread_run(env: JniEnv, object: JObject) {
    let class = env.get_object_class(object);
    let target = env.get_object_field(
        object,
        env.get_field_id(class, c"target", c"Ljava/lang/Runnable;"),
    );
    super::preferences::delete(env, class);
    if !target.is_null() {
        run(env, target as usize);
        super::preferences::delete(env, target);
    }
}
fn prune_finished() {
    let done: Vec<_> = workers()
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, worker)| !worker.alive.load(Ordering::Acquire))
        .map(|(&id, worker)| (id, Arc::clone(worker)))
        .collect();
    for (id, worker) in done {
        let mut handle = worker.handle.lock().unwrap();
        if let Some(thread) = handle.take() {
            let _ = thread.join();
        }
        drop(handle);
        workers().lock().unwrap().remove(&id);
    }
}
extern "C" fn start(env: JniEnv, object: JObject) {
    let _start = START.lock().unwrap();
    if CLOSING.load(Ordering::Acquire) {
        super::preferences::error(env, "thread service is closed");
        return;
    }
    prune_finished();
    if get_id(env, object, &THREAD_ID) != 0 {
        super::preferences::error(env, "Java Thread cannot be started twice");
        return;
    }
    let id = NEXT.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        super::preferences::error(env, "thread ID exhausted");
        return;
    }
    let looper = super::java::allocate(env, c"android/os/Looper");
    set_id(env, looper, &LOOP_ID, id);
    let looper_global = global(env, looper);
    super::preferences::delete(env, looper);
    let owner = global(env, object);
    if owner == 0 || looper_global == 0 {
        delete_global(env, owner);
        delete_global(env, looper_global);
        return;
    }
    let class = env.find_class(c"android/os/HandlerThread");
    let handler = env.is_instance_of(object, class);
    super::preferences::delete(env, class);
    let worker = Arc::new(Worker {
        jobs: Mutex::new(VecDeque::new()),
        wake: Condvar::new(),
        stop: AtomicBool::new(false),
        safe_cutoff: Mutex::new(None),
        alive: AtomicBool::new(true),
        looper: Mutex::new(looper_global),
        handle: Mutex::new(None),
    });
    workers().lock().unwrap().insert(id, Arc::clone(&worker));
    set_id(env, object, &THREAD_ID, id);
    let vm = super::HOST.get().unwrap().java_vm();
    let task = Arc::clone(&worker);
    let (send, ready) = std::sync::mpsc::sync_channel(0);
    let spawned = std::thread::Builder::new()
        .name(format!("rudroid-java-{id}"))
        .spawn(move || {
            let vm = vm.expect("JNI handle is null — the session handed out a broken JavaVM/JNIEnv");
            let Some(env) = vm.attach_current_thread() else {
                let _ = send.send(false);
                task.alive.store(false, Ordering::Release);
                return;
            };
            CURRENT.with(|current| current.set(id));
            let _ = send.send(true);
            if handler {
                loop {
                    let job = {
                        let mut jobs = task.jobs.lock().unwrap();
                        loop {
                            let stopping = task.stop.load(Ordering::Acquire);
                            let cutoff = *task.safe_cutoff.lock().unwrap();
                            if stopping && cutoff.is_none() {
                                break None;
                            }
                            let now = if stopping {
                                cutoff.unwrap()
                            } else {
                                Instant::now()
                            };
                            if let Some(index) = jobs
                                .iter()
                                .enumerate()
                                .filter_map(|(index, job)| {
                                    job.deadline.filter(|at| *at <= now).map(|at| (index, at))
                                })
                                .min_by_key(|(_, at)| *at)
                                .map(|(index, _)| index)
                            {
                                break jobs.remove(index);
                            }
                            if stopping {
                                break None;
                            }
                            if let Some(deadline) = jobs.iter().filter_map(|job| job.deadline).min()
                            {
                                jobs = task
                                    .wake
                                    .wait_timeout(jobs, deadline.saturating_duration_since(now))
                                    .unwrap()
                                    .0;
                            } else {
                                jobs = task.wake.wait(jobs).unwrap();
                            }
                        }
                    };
                    let Some(job) = job else {
                        break;
                    };
                    run(env, job.runnable);
                    delete_global(env, job.runnable);
                    if env.exception_check() {
                        env.exception_describe();
                        break;
                    }
                }
            } else {
                run(env, owner);
            }
            close_queue(env, &task);
            let looper = std::mem::take(&mut *task.looper.lock().unwrap());
            delete_global(env, looper);
            delete_global(env, owner);
            CURRENT.with(|current| current.set(0));
            vm.detach_current_thread();
            task.alive.store(false, Ordering::Release);
        });
    match spawned {
        Ok(handle) => {
            *worker.handle.lock().unwrap() = Some(handle);
            if ready.recv() != Ok(true) {
                close_queue(env, &worker);
                worker.alive.store(false, Ordering::Release);
                delete_global(env, owner);
                let looper = std::mem::take(&mut *worker.looper.lock().unwrap());
                delete_global(env, looper);
                set_id(env, object, &THREAD_ID, 0);
                super::preferences::error(env, "worker could not attach to shared VM");
            }
        }
        Err(error) => {
            close_queue(env, &worker);
            worker.alive.store(false, Ordering::Release);
            workers().lock().unwrap().remove(&id);
            delete_global(env, owner);
            let looper = std::mem::take(&mut *worker.looper.lock().unwrap());
            delete_global(env, looper);
            set_id(env, object, &THREAD_ID, 0);
            super::preferences::error(env, &error.to_string());
        }
    }
}
extern "C" fn looper(env: JniEnv, object: JObject) -> JObject {
    let id = get_id(env, object, &THREAD_ID);
    let worker = workers().lock().unwrap().get(&id).cloned();
    let Some(worker) = worker else {
        return std::ptr::null_mut();
    };
    if !worker.alive.load(Ordering::Acquire) {
        return std::ptr::null_mut();
    }
    let looper = worker.looper.lock().unwrap();
    env.new_local_ref(*looper as *mut c_void)
}
extern "C" fn alive(env: JniEnv, object: JObject) -> u8 {
    let id = get_id(env, object, &THREAD_ID);
    u8::from(
        workers()
            .lock()
            .unwrap()
            .get(&id)
            .is_some_and(|worker| worker.alive.load(Ordering::Acquire)),
    )
}
extern "C" fn quit(env: JniEnv, object: JObject) -> u8 {
    let id = get_id(env, object, &THREAD_ID);
    let worker = workers().lock().unwrap().get(&id).cloned();
    let Some(worker) = worker else {
        return 0;
    };
    let _queue = worker.jobs.lock().unwrap();
    if !worker.alive.load(Ordering::Acquire) {
        return 0;
    }
    *worker.safe_cutoff.lock().unwrap() = None;
    worker.stop.store(true, Ordering::Release);
    worker.wake.notify_all();
    1
}
extern "C" fn looper_quit(env: JniEnv, object: JObject) {
    let id = get_id(env, object, &LOOP_ID);
    let worker = workers().lock().unwrap().get(&id).cloned();
    if let Some(worker) = worker {
        close_queue(env, &worker);
    }
}
extern "C" fn quit_safely(env: JniEnv, object: JObject) -> u8 {
    let id = get_id(env, object, &THREAD_ID);
    let worker = workers().lock().unwrap().get(&id).cloned();
    let Some(worker) = worker else {
        return 0;
    };
    let _queue = worker.jobs.lock().unwrap();
    if !worker.alive.load(Ordering::Acquire) {
        return 0;
    }
    if !worker.stop.load(Ordering::Acquire) {
        *worker.safe_cutoff.lock().unwrap() = Some(Instant::now());
        worker.stop.store(true, Ordering::Release);
    }
    worker.wake.notify_all();
    1
}
extern "C" fn join(env: JniEnv, object: JObject) {
    let id = get_id(env, object, &THREAD_ID);
    if id == 0 {
        return;
    }
    if CURRENT.with(|current| current.get()) == id {
        super::preferences::error(env, "thread cannot join itself");
        return;
    }
    let worker = {
        let _start = START.lock().unwrap();
        workers().lock().unwrap().get(&id).cloned()
    };
    if let Some(worker) = worker {
        let mut handle = worker.handle.lock().unwrap();
        if let Some(thread) = handle.take() {
            let _ = thread.join();
        }
        drop(handle);
        workers().lock().unwrap().remove(&id);
    }
}
extern "C" fn handler_init(env: JniEnv, handler: JObject, looper: JObject) {
    if looper.is_null() {
        super::preferences::error(env, "Handler requires a Looper");
        return;
    }
    let id = get_id(env, looper, &LOOP_ID);
    set_id(env, handler, &HANDLER_ID, id);
    set_object(env, handler, c"looper", c"Landroid/os/Looper;", looper)
}
extern "C" fn current_looper(env: JniEnv, _class: JObject) -> JObject {
    let id = current_id();
    if id == 0 {
        return super::java::current_ui_looper(env);
    }
    let worker = workers().lock().unwrap().get(&id).cloned();
    let Some(worker) = worker else { return std::ptr::null_mut(); };
    let looper = worker.looper.lock().unwrap();
    env.new_local_ref(*looper as *mut c_void)
}
extern "C" fn handler_init_current(env: JniEnv, handler: JObject) {
    // No implicit worker: bind only to this thread's existing owned queue.
    let looper = current_looper(env, std::ptr::null_mut());
    handler_init(env, handler, looper);
    super::preferences::delete(env, looper);
}
fn object_field(
    env: JniEnv,
    object: *mut c_void,
    name: &std::ffi::CStr,
    sig: &std::ffi::CStr,
) -> *mut c_void {
    let class = env.get_object_class(object);
    let id = env.get_field_id(class, name, sig);
    super::preferences::delete(env, class);
    id
}
fn get_object(
    env: JniEnv,
    object: *mut c_void,
    name: &std::ffi::CStr,
    sig: &std::ffi::CStr,
) -> *mut c_void {
    env.get_object_field(object, object_field(env, object, name, sig))
}
fn set_object(
    env: JniEnv,
    object: *mut c_void,
    name: &std::ffi::CStr,
    sig: &std::ffi::CStr,
    value: *mut c_void,
) {
    env.set_object_field(object, object_field(env, object, name, sig), value)
}
extern "C" fn handler_looper(env: JniEnv, handler: JObject) -> JObject {
    get_object(env, handler, c"looper", c"Landroid/os/Looper;")
}
extern "C" fn handler_callback(
    env: JniEnv,
    handler: JObject,
    looper: JObject,
    callback: JObject,
) {
    handler_init(env, handler, looper);
    set_object(
        env,
        handler,
        c"callback",
        c"Landroid/os/Handler$Callback;",
        callback,
    )
}
extern "C" fn message_obtain(env: JniEnv, _class: JObject) -> JObject {
    super::java::allocate(env, c"android/os/Message")
}
extern "C" fn obtain_message(
    env: JniEnv,
    handler: JObject,
    what: i32,
) -> JObject {
    if trace_messages() {
        eprintln!(
            "[JavaMessage] obtain worker={} what={what}",
            get_id(env, handler, &HANDLER_ID)
        );
    }
    let message = message_obtain(env, std::ptr::null_mut());
    if message.is_null() {
        return message;
    }
    env.set_int_field(message, object_field(env, message, c"what", c"I"), what);
    set_object(env, message, c"target", c"Landroid/os/Handler;", handler);
    message
}
extern "C" fn get_target(env: JniEnv, message: JObject) -> JObject {
    get_object(env, message, c"target", c"Landroid/os/Handler;")
}
extern "C" fn message_run(env: JniEnv, adapter: JObject) {
    let handler = get_object(env, adapter, c"handler", c"Landroid/os/Handler;");
    let message = get_object(env, adapter, c"message", c"Landroid/os/Message;");
    if trace_messages() {
        let what = env.get_int_field(message, object_field(env, message, c"what", c"I"));
        eprintln!(
            "[JavaMessage] dispatch worker={} what={what}",
            get_id(env, handler, &HANDLER_ID)
        );
    }
    let class = env.get_object_class(handler);
    if let Some(method) = env.method(class, c"dispatchMessage", c"(Landroid/os/Message;)V") {
        env.call_void(handler, method, &[Arg::object(message)]);
    }
    super::preferences::delete(env, class);
    super::preferences::delete(env, message);
    super::preferences::delete(env, handler)
}
extern "C" fn handle_message(
    _env: JniEnv,
    _handler: JObject,
    _message: JObject,
) {
}
extern "C" fn default_callback(
    _env: JniEnv,
    _callback: JObject,
    _message: JObject,
) -> u8 {
    0
}
extern "C" fn dispatch(env: JniEnv, handler: JObject, message: JObject) {
    let callback = get_object(env, handler, c"callback", c"Landroid/os/Handler$Callback;");
    let mut consumed = false;
    if !callback.is_null() {
        let class = env.get_object_class(callback);
        if let Some(method) = env.method(class, c"handleMessage", c"(Landroid/os/Message;)Z") {
            consumed = env.call_boolean(callback, method, &[Arg::object(message)]);
        }
        super::preferences::delete(env, class);
        super::preferences::delete(env, callback);
    }
    if !consumed && !env.exception_check() {
        let class = env.get_object_class(handler);
        if let Some(method) = env.method(class, c"handleMessage", c"(Landroid/os/Message;)V") {
            env.call_void(handler, method, &[Arg::object(message)]);
        }
        super::preferences::delete(env, class);
    }
}
extern "C" fn send_delayed(
    env: JniEnv,
    handler: JObject,
    message: JObject,
    milliseconds: i64,
) -> u8 {
    if message.is_null() {
        return 0;
    }
    set_object(env, message, c"target", c"Landroid/os/Handler;", handler);
    // The queued global owns this VM graph, including Handler and Message;
    // worker and main-thread scheduling share the same Runnable path.
    let adapter = super::java::allocate(env, c"org/rudroid/runtime/MessageTask");
    if adapter.is_null() {
        return 0;
    }
    set_object(env, adapter, c"handler", c"Landroid/os/Handler;", handler);
    set_object(env, adapter, c"message", c"Landroid/os/Message;", message);
    let result = super::java::post_delayed(env, handler, adapter, milliseconds);
    super::preferences::delete(env, adapter);
    result
}
extern "C" fn send_message(
    env: JniEnv,
    handler: JObject,
    message: JObject,
) -> u8 {
    send_delayed(env, handler, message, 0)
}
extern "C" fn send_to_target(env: JniEnv, message: JObject) {
    let target = get_target(env, message);
    if !target.is_null() {
        send_message(env, target, message);
        super::preferences::delete(env, target);
    }
}
pub(super) fn post(
    env: JniEnv,
    handler: *mut c_void,
    runnable: *mut c_void,
    milliseconds: i64,
) -> Option<u8> {
    let id = get_id(env, handler, &HANDLER_ID);
    if id == 0 {
        return None;
    }
    let worker = workers().lock().unwrap().get(&id).cloned();
    let Some(worker) = worker else {
        return Some(0);
    };
    let global = global(env, runnable);
    if global == 0 {
        return Some(0);
    }
    let mut jobs = worker.jobs.lock().unwrap();
    if worker.stop.load(Ordering::Acquire)
        || !worker.alive.load(Ordering::Acquire)
        || jobs.try_reserve(1).is_err()
    {
        drop(jobs);
        delete_global(env, global);
        return Some(0);
    }
    jobs.push_back(Job {
        runnable: global,
        deadline: Instant::now().checked_add(Duration::from_millis(milliseconds.max(0) as u64)),
    });
    worker.wake.notify_one();
    Some(1)
}

pub(super) fn current_id() -> u64 {
    CURRENT.with(|current| current.get())
}
pub(super) fn current_handler(env: JniEnv) -> *mut c_void {
    let handler = super::java::allocate(env, c"android/os/Handler");
    if !handler.is_null() {
        set_id(env, handler, &HANDLER_ID, current_id());
    }
    handler
}
pub(super) fn register(env: JniEnv) -> i32 {
    if declare_superclass(env, c"java/lang/Thread", c"java/lang/Runnable") != 0
        || declare_superclass(env, c"android/os/HandlerThread", c"java/lang/Thread") != 0
        || declare_superclass(
            env,
            c"org/rudroid/runtime/MessageTask",
            c"java/lang/Runnable",
        ) != 0
    {
        return -1;
    }
    for (class, fields) in [
        (
            c"android/os/Handler",
            &[
                (c"callback", c"Landroid/os/Handler$Callback;"),
                (c"looper", c"Landroid/os/Looper;"),
            ] as &[(&std::ffi::CStr, &std::ffi::CStr)],
        ),
        (
            c"android/os/Message",
            &[
                (c"what", c"I"),
                (c"arg1", c"I"),
                (c"arg2", c"I"),
                (c"obj", c"Ljava/lang/Object;"),
                (c"target", c"Landroid/os/Handler;"),
            ],
        ),
        (
            c"org/rudroid/runtime/MessageTask",
            &[
                (c"handler", c"Landroid/os/Handler;"),
                (c"message", c"Landroid/os/Message;"),
            ],
        ),
    ] {
        for (name, sig) in fields {
            if declare_field(env, class, name, sig, false) != 0 {
                return -1;
            }
        }
    }
    for (class, cache) in [
        (c"java/lang/Thread", &THREAD_ID),
        (c"android/os/Looper", &LOOP_ID),
        (c"android/os/Handler", &HANDLER_ID),
    ] {
        if declare_field(env, class, c"workerId", c"J", false) != 0 {
            return -1;
        }
        let class = env.find_class(class);
        let field = env.get_field_id(class, c"workerId", c"J");
        super::preferences::delete(env, class);
        if field.is_null() {
            return -1;
        }
        cache.get_or_init(|| field as usize);
    }
    for (name, sig) in [
        (c"name", c"Ljava/lang/String;"),
        (c"target", c"Ljava/lang/Runnable;"),
    ] {
        if declare_field(env, c"java/lang/Thread", name, sig, false) != 0 {
            return -1;
        }
    }
    for (class, methods) in [
        (
            c"android/os/Looper",
            vec![
                Native::new0(c"quit", c"()V", looper_quit),
                Native::new0(c"myLooper", c"()Landroid/os/Looper;", current_looper),
            ],
        ),
        (
            c"java/lang/Thread",
            vec![
                Native::new1(c"<init>", c"(Ljava/lang/String;)V", init_name),
                Native::new1(c"<init>", c"(Ljava/lang/Runnable;)V", init_target),
                Native::new0(c"start", c"()V", start),
                Native::new0(c"run", c"()V", thread_run),
                Native::new0(c"join", c"()V", join),
                Native::new0(c"isAlive", c"()Z", alive),
            ],
        ),
        (
            c"android/os/HandlerThread",
            vec![
                Native::new1(c"<init>", c"(Ljava/lang/String;)V", init_name),
                Native::new0(c"getLooper", c"()Landroid/os/Looper;", looper),
                Native::new0(c"quit", c"()Z", quit),
                Native::new0(c"quitSafely", c"()Z", quit_safely),
            ],
        ),
        (
            c"android/os/Handler",
            vec![
                Native::new0(c"<init>", c"()V", handler_init_current),
                Native::new0(c"getLooper", c"()Landroid/os/Looper;", handler_looper),
                Native::new1(c"<init>", c"(Landroid/os/Looper;)V", handler_init),
                Native::new2(
                    c"<init>",
                    c"(Landroid/os/Looper;Landroid/os/Handler$Callback;)V",
                    handler_callback
                ),
                Native::new1(c"obtainMessage", c"(I)Landroid/os/Message;", obtain_message),
                Native::new1(c"sendMessage", c"(Landroid/os/Message;)Z", send_message),
                Native::new2(
                    c"sendMessageDelayed",
                    c"(Landroid/os/Message;J)Z",
                    send_delayed
                ),
                Native::new1(c"dispatchMessage", c"(Landroid/os/Message;)V", dispatch),
                Native::new1(c"handleMessage", c"(Landroid/os/Message;)V", handle_message),
            ],
        ),
        (
            c"android/os/Handler$Callback",
            vec![Native::new1(
                c"handleMessage",
                c"(Landroid/os/Message;)Z",
                default_callback
            )],
        ),
        (
            c"android/os/Message",
            vec![
                Native::new0(c"obtain", c"()Landroid/os/Message;", message_obtain),
                Native::new0(c"getTarget", c"()Landroid/os/Handler;", get_target),
                Native::new0(c"sendToTarget", c"()V", send_to_target),
            ],
        ),
        (
            c"org/rudroid/runtime/MessageTask",
            vec![Native::new0(c"run", c"()V", message_run)],
        ),
    ] {
        if env.register_class(class, &methods) != 0 {
            return -1;
        }
    }
    0
}
