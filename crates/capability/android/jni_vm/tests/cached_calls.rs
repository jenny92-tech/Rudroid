//! INPUT: production JNI session/shims and a fixed host provider
//! OUTPUT: allocation/query regressions for attachment, methods, fields, UTF data, frames and exceptions
//! POS: measures the complete JNI entry path, including borrowed C keys

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::ffi::c_int;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use android_jni_vm::{abi, session};
use rudroid_common::host_api::HostApiV0;
use rudroid_common::registry::HostStr;

struct Allocator;
thread_local! { static TRACK: Cell<bool> = const { Cell::new(false) }; }
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static QUERIES: AtomicUsize = AtomicUsize::new(0);
static SERIAL: Mutex<()> = Mutex::new(());

unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(layout.size(), Ordering::Relaxed);
        }
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) { unsafe { System.dealloc(ptr, layout) } }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        if TRACK.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(size, Ordering::Relaxed);
        }
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Allocator = Allocator;

extern "C" fn config_bool(_: HostStr<'_>,fallback:i32)->i32 {fallback}
extern "C" fn log(_: HostStr<'_>, _: HostStr<'_>) {}
extern "C" fn query_bool(_: HostStr<'_>, _: HostStr<'_>, _: HostStr<'_>, _: Option<&mut c_int>) -> c_int {
    QUERIES.fetch_add(1, Ordering::Relaxed);
    0
}
extern "C" fn query_int(_: HostStr<'_>, _: HostStr<'_>, _: HostStr<'_>, out: Option<&mut i64>) -> c_int {
    QUERIES.fetch_add(1, Ordering::Relaxed);
    *out.unwrap() = 42;
    1
}
fn host() -> HostApiV0 {
    HostApiV0 {
        config_get_bool: config_bool, log, catalog_query_int: query_int, catalog_query_bool: query_bool,
        ..HostApiV0::stubbed()
    }
}

#[test]
fn env_queries_and_repeated_attachment_do_not_allocate() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    let vm = abi::java_vm_ptr();
    let mut output = std::ptr::null_mut();
    ALLOCS.store(0, Ordering::Relaxed);
    TRACK.with(|track| track.set(true));
    for _ in 0..10_000 {
        assert_eq!(unsafe { session::shims::get_env(vm, &mut output, abi::JNI_VERSION_1_6) }, abi::JNI_OK);
        assert_eq!(output, abi::env_ptr());
        assert_eq!(unsafe { session::shims::attach_current_thread(vm, &mut output, std::ptr::null_mut()) }, abi::JNI_OK);
    }
    session::shims::detach_current_thread(vm);
    for _ in 0..10_000 {
        output = abi::env_ptr();
        assert_eq!(unsafe { session::shims::get_env(vm, &mut output, abi::JNI_VERSION_1_6) }, abi::JNI_EDETACHED);
        assert!(output.is_null());
    }
    TRACK.with(|track| track.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "env negotiation must not allocate thread state on queries");
}

#[test]
fn cached_method_lookup_and_call_are_allocation_free() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    let env = abi::env_ptr();
    unsafe {
        let class = session::shims::find_class(env, c"cost/Cache".as_ptr());
        let method = session::shims::get_static_method_id(env, class, c"read".as_ptr(), c"()I".as_ptr());
        assert!(!method.is_null());
        QUERIES.store(0, Ordering::Relaxed);
        ALLOCS.store(0, Ordering::Relaxed);
        TRACK.with(|track| track.set(true));
        for _ in 0..10_000 {
            assert_eq!(session::shims::get_static_method_id(env, class, c"read".as_ptr(), c"()I".as_ptr()), method);
            assert_eq!(session::shims::call_static_int_method_a(env, class, method, std::ptr::null_mut()), 42);
        }
        TRACK.with(|track| track.set(false));
        assert_eq!(QUERIES.load(Ordering::Relaxed), 0, "cached method must bypass provider resolution");
        assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "warm JNI lookup and calls must not copy metadata");
        session::shims::delete_local_ref(env, class);
        session::shims::detach_current_thread(abi::java_vm_ptr());
    }
}

#[test]
fn declared_field_lookup_avoids_scans_and_temporary_allocations() {
    use rudroid_common::{jni_abi::declare_field, jni_env::JniEnv};
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    // SAFETY: the production in-process VM is pinned and this thread is attached.
    let env = unsafe { JniEnv::from_raw(abi::env_ptr()) };
    for index in 0..256 {
        let name = std::ffi::CString::new(format!("noise{index}")).unwrap();
        assert_eq!(declare_field(env, c"cost/Fields", &name, c"I", false), 0);
    }
    assert_eq!(declare_field(env, c"cost/Fields", c"value", c"J", false), 0);
    assert_eq!(declare_field(env, c"cost/Fields", c"value", c"I", true), 0);
    let class = env.find_class(c"cost/Fields");
    let instance = env.get_field_id(class, c"value", c"J");
    let static_id = env.get_static_field_id(class, c"value", c"I");
    assert!(!instance.is_null() && !static_id.is_null() && instance != static_id);
    ALLOCS.store(0, Ordering::Relaxed);
    BYTES.store(0, Ordering::Relaxed);
    TRACK.with(|track| track.set(true));
    for _ in 0..10_000 {
        assert_eq!(env.get_field_id(class, c"value", c"J"), instance);
        assert_eq!(env.get_static_field_id(class, c"value", c"I"), static_id);
    }
    TRACK.with(|track| track.set(false));
    let allocations = ALLOCS.load(Ordering::Relaxed);
    let bytes = BYTES.load(Ordering::Relaxed);
    // Time the same real JNI entries without allocation-counter overhead.
    let start = std::time::Instant::now();
    for _ in 0..100_000 {
        std::hint::black_box(env.get_field_id(class, c"value", c"J"));
        std::hint::black_box(env.get_static_field_id(class, c"value", c"I"));
    }
    println!("JNI_FIELD_COST lookups=200000 elapsed_ns={} tracked_lookups=20000 allocations={allocations} requested_bytes={bytes}", start.elapsed().as_nanos());
    env.delete_local_ref(class);
    session::shims::detach_current_thread(abi::java_vm_ptr());
    assert_eq!(allocations, 0, "declared field lookup must not allocate an inheritance walk");
    assert_eq!(bytes, 0);
}

#[test]
fn registered_native_lookup_and_calls_share_the_allocation_free_method_index() {
    let _serial=SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    let env=abi::env_ptr();
    unsafe extern "C" fn body(_env:*mut std::ffi::c_void,_class:*mut std::ffi::c_void,string:*mut std::ffi::c_void)->u8 {
        u8::from(!string.is_null())
    }
    unsafe {
        let class=session::shims::find_class(env,c"cost/NativeCache".as_ptr());
        let string=session::shims::new_string_utf(env,c"shared-argument".as_ptr());
        let binding=session::JniNativeMethod { name:c"load".as_ptr(),signature:c"(Ljava/lang/String;)Z".as_ptr(),fn_ptr:body as *mut std::ffi::c_void };
        assert_eq!(session::shims::register_natives(env,class,&binding,1),abi::JNI_OK);
        let method=session::shims::get_static_method_id(env,class,binding.name,binding.signature);
        assert!(!method.is_null());
        let mut args=[string];
        // Warm native-entry local capacity before measuring the hot path.
        assert_eq!(session::shims::call_static_boolean_method_a(env,class,method,args.as_mut_ptr().cast()),abi::JNI_TRUE);
        QUERIES.store(0,Ordering::Relaxed);
        ALLOCS.store(0,Ordering::Relaxed);
        BYTES.store(0,Ordering::Relaxed);
        TRACK.with(|track| track.set(true));
        for _ in 0..10_000 {
            assert_eq!(session::shims::get_static_method_id(env,class,binding.name,binding.signature),method);
            assert_eq!(session::shims::call_static_boolean_method_a(env,class,method,args.as_mut_ptr().cast()),abi::JNI_TRUE);
            assert_eq!(session::session().unwrap().registered_native("cost/NativeCache","load","(Ljava/lang/String;)Z"),Some(body as usize));
        }
        TRACK.with(|track| track.set(false));
        assert_eq!(QUERIES.load(Ordering::Relaxed),0);
        assert_eq!(ALLOCS.load(Ordering::Relaxed),0);
        assert_eq!(BYTES.load(Ordering::Relaxed),0);
        println!("JNI_NATIVE_COST iterations=10000 allocations=0 requested_bytes=0 provider_queries=0");
        assert_eq!(session::shims::unregister_natives(env,class),abi::JNI_OK);
        session::shims::delete_local_ref(env,string);
        session::shims::delete_local_ref(env,class);
        session::shims::detach_current_thread(abi::java_vm_ptr());
    }
}

#[test]
fn warm_nonvirtual_calls_reuse_method_metadata_without_string_copies() {
    use rudroid_common::jni_abi::function_slot;
    use std::ffi::c_void;
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    let env = abi::env_ptr();
    // SAFETY: shared stable table, a valid zero-argument Int catalog method
    // and a live receiver of the supplied class; no guest threads use it.
    unsafe {
        let class = session::shims::find_class(env, c"cost/NonvirtualCache".as_ptr());
        let method = session::shims::get_method_id(env, class, c"read".as_ptr(), c"()I".as_ptr());
        let object = session::shims::alloc_object(env, class);
        assert!(!method.is_null() && !object.is_null());
        let call = function_slot::<unsafe extern "C" fn(*mut c_void, *mut c_void,
            *mut c_void, *mut c_void, *const rudroid_common::jni_abi::JValue) -> i32>
            (env, abi::env_slot::CallNonvirtualIntMethodA);
        assert_eq!(call(env,object,class,method,std::ptr::null()),42);
        QUERIES.store(0, Ordering::Relaxed);
        ALLOCS.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        TRACK.with(|track| track.set(true));
        for _ in 0..20_000 {
            assert_eq!(call(env,object,class,method,std::ptr::null()),42);
        }
        TRACK.with(|track| track.set(false));
        assert_eq!(ALLOCS.load(Ordering::Relaxed),0);
        assert_eq!(BYTES.load(Ordering::Relaxed),0);
        assert_eq!(QUERIES.load(Ordering::Relaxed),0);
        println!("JNI_NONVIRTUAL_COST iterations=20000 allocations=0 requested_bytes=0 provider_queries=0");
        session::shims::delete_local_ref(env,object);
        session::shims::delete_local_ref(env,class);
        session::shims::detach_current_thread(abi::java_vm_ptr());
    }
}

#[test]
fn string_length_and_warm_utf_borrows_do_not_copy_the_string() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    let text = std::ffi::CString::new(vec![b'a'; 1 << 20]).unwrap();
    let env = abi::env_ptr();
    unsafe {
        let string = session::shims::new_string_utf(env, text.as_ptr());
        assert!(!string.is_null());
        let pointer = session::shims::get_string_utf_chars(env, string, std::ptr::null_mut());
        assert!(!pointer.is_null());
        session::shims::release_string_utf_chars(env, string, pointer);
        ALLOCS.store(0, Ordering::Relaxed);
        TRACK.with(|track| track.set(true));
        for _ in 0..100 {
            assert_eq!(session::shims::get_string_utf_length(env, string), 1 << 20);
            let mut copied = abi::JNI_TRUE;
            let chars = session::shims::get_string_utf_chars(env, string, &mut copied);
            assert_eq!(chars, pointer);
            assert_eq!(copied, abi::JNI_FALSE);
            session::shims::release_string_utf_chars(env, string, chars);
        }
        TRACK.with(|track| track.set(false));
        assert_eq!(ALLOCS.load(Ordering::Relaxed), 0, "length and warmed UTF borrows share data");
        session::shims::delete_local_ref(env, string);
        session::shims::detach_current_thread(abi::java_vm_ptr());
    }
}

#[test]
fn scoped_string_views_are_allocation_free_and_release_roots_on_every_exit() {
    use rudroid_common::jni_env::JniEnv;
    let _serial=SERIAL.lock().unwrap_or_else(|p|p.into_inner());
    session::init_with_host(host());
    // SAFETY: the in-process production VM table remains pinned and this thread is attached.
    let env=unsafe{JniEnv::from_raw(abi::env_ptr())};
    let text=std::ffi::CString::new(vec![b'a';256]).unwrap();
    let string=env.new_string_utf(&text);
    assert_eq!(env.with_string_utf(string,|s|s.to_bytes().len()),Some(256));
    assert_eq!(env.with_string_utf16(string,|s|s.len()),Some(256));
    ALLOCS.store(0,Ordering::Relaxed);BYTES.store(0,Ordering::Relaxed);
    TRACK.with(|track|track.set(true));
    for _ in 0..10_000 {
        assert_eq!(env.with_string_utf(string,|s|s.to_bytes().len()),Some(256));
        assert_eq!(env.with_string_utf16(string,|s|s.len()),Some(256));
    }
    TRACK.with(|track|track.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed),0);assert_eq!(BYTES.load(Ordering::Relaxed),0);
    ALLOCS.store(0,Ordering::Relaxed);BYTES.store(0,Ordering::Relaxed);
    TRACK.with(|track|track.set(true));
    for _ in 0..1_000 {
        std::hint::black_box(env.string_utf(string).unwrap());
        std::hint::black_box(env.string_utf16(string).unwrap());
    }
    TRACK.with(|track|track.set(false));
    assert_eq!(ALLOCS.load(Ordering::Relaxed),2000);
    assert_eq!(BYTES.load(Ordering::Relaxed),769_000);
    println!("JNI_STRING_COST scoped_reads=20000 allocations=0 bytes=0; explicit_owned_copies=2000 allocations=2000 bytes=769000");
    env.delete_local_ref(string);

    for utf16 in [false,true] {
        let string=env.new_string_utf(c"owned until callback finishes");
        let weak=env.new_weak_global_ref(string);
        if utf16 {
            assert_eq!(env.with_string_utf16(string,|view|{env.delete_local_ref(string);view.len()}),Some(29));
        }else{
            assert_eq!(env.with_string_utf(string,|view|{env.delete_local_ref(string);view.to_bytes().len()}),Some(29));
        }
        assert!(env.new_local_ref(weak).is_null(),"callback exits release both string pin and hidden root");
        env.delete_weak_global_ref(weak);
        let string=env.new_string_utf(c"panic cleanup");let weak=env.new_weak_global_ref(string);
        let failed=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if utf16 {env.with_string_utf16(string,|_|panic!("expected scoped UTF-16 unwind"));}
            else {env.with_string_utf(string,|_|panic!("expected scoped UTF-8 unwind"));}
        }));
        assert!(failed.is_err());env.delete_local_ref(string);
        assert!(env.new_local_ref(weak).is_null(),"unwind must not leave a pin or root");
        env.delete_weak_global_ref(weak);assert!(!env.exception_check());
    }
    session::shims::detach_current_thread(abi::java_vm_ptr());
}

#[cfg(target_os="linux")]
#[test]
fn repeated_string_encoding_bursts_reclaim_owners_and_reach_a_resident_plateau() {
    use rudroid_common::jni_env::JniEnv;
    fn rss_kib()->usize {
        std::fs::read_to_string("/proc/self/status").unwrap().lines()
            .find_map(|line|line.strip_prefix("VmRSS:").map(|value|value.split_whitespace().next().unwrap().parse().unwrap())).unwrap()
    }
    let _serial=SERIAL.lock().unwrap_or_else(|p|p.into_inner());session::init_with_host(host());
    // SAFETY: attached current thread and pinned in-process production VM table.
    let env=unsafe{JniEnv::from_raw(abi::env_ptr())};
    let text=std::ffi::CString::new(vec![b'x';2048]).unwrap();
    let mut steady=Vec::new();
    for round in 0..12 {
        let started=std::time::Instant::now();
        let mut handles=Vec::with_capacity(2000);let mut weak=Vec::with_capacity(40);
        for index in 0..2000 {
            let string=env.new_string_utf(&text);assert!(!string.is_null());
            assert_eq!(env.with_string_utf(string,|s|s.to_bytes().len()),Some(2048));
            assert_eq!(env.with_string_utf16(string,|s|s.len()),Some(2048));
            if index%50==0 {weak.push(env.new_weak_global_ref(string));}
            handles.push(string);
        }
        let peak=rss_kib();
        for string in handles {env.delete_local_ref(string);}
        for weak in weak {
            assert!(env.new_local_ref(weak).is_null(),"last reference/pin must reclaim the sampled owner");
            env.delete_weak_global_ref(weak);
        }
        assert!(!env.exception_check());let after=rss_kib();steady.push(after);
        println!("JNI_STRING_RSS round={round} owners=2000 utf8_bytes=4098000 utf16_bytes=8196000 peak_kib={peak} released_kib={after} ns={}",started.elapsed().as_nanos());
    }
    let baseline=*steady[2..5].iter().min().unwrap();
    assert!(steady[11]<=baseline+8*1024,"resident memory kept growing across reclaimed encoding bursts: {steady:?}");
    session::shims::detach_current_thread(abi::java_vm_ptr());
}

#[test]
fn references_after_a_large_burst_only_allocate_new_string_payloads() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    let env = abi::env_ptr();
    unsafe {
        let handles: Vec<_> = (0..16_000).map(|_| {
            let string = session::shims::new_string_utf(env, c"burst".as_ptr());
            let global = session::shims::new_global_ref(env, string);
            let chars = session::shims::get_string_utf_chars(env, string, std::ptr::null_mut());
            assert!(!string.is_null() && !global.is_null() && !chars.is_null());
            (string, global, chars)
        }).collect();
        for (string, global, chars) in handles {
            session::shims::delete_global_ref(env, global);
            session::shims::release_string_utf_chars(env, string, chars);
            session::shims::delete_local_ref(env, string);
        }
        ALLOCS.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        TRACK.with(|track| track.set(true));
        for _ in 0..1_000 {
            let string = session::shims::new_string_utf(env, c"reuse".as_ptr());
            let global = session::shims::new_global_ref(env, string);
            let chars = session::shims::get_string_utf_chars(env, string, std::ptr::null_mut());
            assert!(!string.is_null() && !global.is_null() && !chars.is_null());
            assert_eq!(session::shims::is_same_object(env, string, global), abi::JNI_TRUE);
            assert_eq!(std::ffi::CStr::from_ptr(chars), c"reuse");
            session::shims::delete_global_ref(env, global);
            session::shims::release_string_utf_chars(env, string, chars);
            session::shims::delete_local_ref(env, string);
        }
        TRACK.with(|track| track.set(false));
        let allocations = ALLOCS.load(Ordering::Relaxed);
        let bytes = BYTES.load(Ordering::Relaxed);
        session::shims::detach_current_thread(abi::java_vm_ptr());
        println!("JNI_MEMORY_COST {{\"kind\":\"post_burst_reuse\",\"burst\":16000,\"cycles\":1000,\"allocations\":{allocations},\"requested_bytes\":{bytes}}}");
        assert_eq!(allocations, 1_000, "only each new NUL-terminated string payload allocates; reference tables stay warm");
        assert_eq!(bytes, 6_000, "each string owns five bytes plus its trailing NUL exactly once");
    }
}

#[test]
fn warm_local_frame_cycles_do_not_allocate_a_reclamation_list() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    let env = abi::env_ptr();
    unsafe {
        let class = session::shims::find_class(env, c"cost/LocalFrames".as_ptr());
        assert!(!class.is_null());
        // Warm the local table; the base class survives every pushed frame.
        for _ in 0..128 {
            assert_eq!(session::shims::push_local_frame(env, 32), abi::JNI_OK);
            for _ in 0..32 { assert!(!session::shims::new_local_ref(env, class).is_null()); }
            assert!(session::shims::pop_local_frame(env, std::ptr::null_mut()).is_null());
        }
        ALLOCS.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        TRACK.with(|track| track.set(true));
        let started = std::time::Instant::now();
        for _ in 0..1_000 {
            assert_eq!(session::shims::push_local_frame(env, 32), abi::JNI_OK);
            let mut kept = std::ptr::null_mut();
            for _ in 0..32 { kept = session::shims::new_local_ref(env, class); assert!(!kept.is_null()); }
            let result = session::shims::pop_local_frame(env, kept);
            assert!(!result.is_null());
            assert_eq!(session::shims::is_same_object(env, class, result), abi::JNI_TRUE);
            session::shims::delete_local_ref(env, result);
        }
        let elapsed = started.elapsed().as_nanos();
        TRACK.with(|track| track.set(false));
        let allocations = ALLOCS.load(Ordering::Relaxed);
        let bytes = BYTES.load(Ordering::Relaxed);
        session::shims::delete_local_ref(env, class);
        session::shims::detach_current_thread(abi::java_vm_ptr());
        println!("JNI_MEMORY_COST {{\"kind\":\"local_frames\",\"cycles\":1000,\"locals_per_frame\":32,\"ns\":{elapsed},\"allocations\":{allocations},\"requested_bytes\":{bytes}}}");
        assert_eq!(allocations, 0, "popping a warmed local frame must not allocate a temporary list");
    }
}

#[test]
fn observed_exception_rethrows_share_the_payload() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    let env = abi::env_ptr();
    let message = std::ffi::CString::new(vec![b'e'; 16_384]).unwrap();
    unsafe {
        let class = session::shims::find_class(env, c"java/lang/Throwable".as_ptr());
        assert_eq!(session::shims::throw_new(env, class, message.as_ptr()), abi::JNI_OK);
        let local = session::shims::exception_occurred(env);
        let global = session::shims::new_global_ref(env, local);
        assert!(!global.is_null());
        session::shims::exception_clear(env);
        session::shims::delete_local_ref(env, local);
        ALLOCS.store(0, Ordering::Relaxed);
        BYTES.store(0, Ordering::Relaxed);
        TRACK.with(|track| track.set(true));
        let started = std::time::Instant::now();
        for _ in 0..1_000 {
            assert_eq!(session::shims::throw(env, global), abi::JNI_OK);
            let observed = session::shims::exception_occurred(env);
            assert_eq!(session::shims::is_same_object(env, observed, global), abi::JNI_TRUE);
            session::shims::exception_clear(env);
            session::shims::delete_local_ref(env, observed);
        }
        let elapsed = started.elapsed().as_nanos();
        TRACK.with(|track| track.set(false));
        let allocations = ALLOCS.load(Ordering::Relaxed);
        let bytes = BYTES.load(Ordering::Relaxed);
        session::shims::delete_global_ref(env, global);
        session::shims::delete_local_ref(env, class);
        session::shims::detach_current_thread(abi::java_vm_ptr());
        println!("JNI_MEMORY_COST {{\"kind\":\"exception_rethrows\",\"cycles\":1000,\"message_bytes\":16384,\"ns\":{elapsed},\"allocations\":{allocations},\"requested_bytes\":{bytes}}}");
        assert_eq!(allocations, 0, "observing and rethrowing the same exception must share its payload");
    }
}

#[test]
fn parallel_jni_query_cost() {
    let _serial = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
    session::init_with_host(host());
    const ITERATIONS: usize = 100_000;
    for kind in ["env_exception", "cached_method"] {
        for workers in [1, 2] {
            for round in 0..7 {
                let barrier = std::sync::Arc::new(std::sync::Barrier::new(workers + 1));
                let mut threads = Vec::new();
                for _ in 0..workers {
                    let barrier = std::sync::Arc::clone(&barrier);
                    threads.push(std::thread::spawn(move || {
                        let vm = abi::java_vm_ptr();
                        let env = abi::env_ptr();
                        let mut output = std::ptr::null_mut();
                        unsafe {
                            assert_eq!(session::shims::attach_current_thread(vm, &mut output, std::ptr::null_mut()), abi::JNI_OK);
                            let class = session::shims::find_class(env, c"cost/ThreadCache".as_ptr());
                            let method = session::shims::get_static_method_id(env, class, c"read".as_ptr(), c"()I".as_ptr());
                            assert!(!method.is_null());
                            barrier.wait();
                            barrier.wait();
                            TRACK.with(|track| track.set(true));
                            let started = std::time::Instant::now();
                            for _ in 0..ITERATIONS {
                                if kind == "env_exception" {
                                    assert_eq!(session::shims::get_env(vm, &mut output, abi::JNI_VERSION_1_6), abi::JNI_OK);
                                    assert_eq!(session::shims::exception_check(env), abi::JNI_FALSE);
                                } else {
                                    assert_eq!(session::shims::get_static_method_id(env, class, c"read".as_ptr(), c"()I".as_ptr()), method);
                                    assert_eq!(session::shims::call_static_int_method_a(env, class, method, std::ptr::null_mut()), 42);
                                }
                            }
                            let elapsed = started.elapsed().as_nanos();
                            TRACK.with(|track| track.set(false));
                            session::shims::delete_local_ref(env, class);
                            session::shims::detach_current_thread(vm);
                            elapsed
                        }
                    }));
                }
                barrier.wait();
                ALLOCS.store(0, Ordering::Relaxed);
                QUERIES.store(0, Ordering::Relaxed);
                barrier.wait();
                let elapsed = threads.into_iter().map(|thread| thread.join().unwrap()).max().unwrap();
                let allocations = ALLOCS.load(Ordering::Relaxed);
                let queries = QUERIES.load(Ordering::Relaxed);
                println!("JNI_COST {{\"kind\":\"{kind}\",\"workers\":{workers},\"round\":{round},\"iterations_per_worker\":{ITERATIONS},\"max_worker_ns\":{elapsed},\"allocations\":{allocations},\"provider_queries\":{queries}}}");
                assert_eq!(allocations, 0, "parallel warm calls must not allocate");
                assert_eq!(queries, 0, "parallel warm calls must not resolve providers again");
            }
        }
    }
    session::shims::detach_current_thread(abi::java_vm_ptr());
}
