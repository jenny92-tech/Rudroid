//! INPUT: trusted production core DSOs, shared JNI tables and immutable UTF-16 owners
//! OUTPUT: casts, critical strings and exact scalar/nonvirtual JNI ABI checks
//! POS: isolated handheld JNI type/string acceptance; never opens games or saves
use rudroid_common::jni_env::JniEnv;
use rudroid_unity_loader::{host_api, phases::Phase};
use std::path::PathBuf;

fn clean(env: JniEnv) {
    assert!(!env.exception_check());
}
fn environment() -> JniEnv {
    let kit = PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").expect("kit required"));
    host_api::install_config(None).unwrap();
    let host = host_api::table();
    // SAFETY: trusted same-kit native libraries and callbacks remain process-pinned.
    let libraries: Vec<_> = unsafe {
        [
            "libplatform_log_stderr.so",
            "libreply_catalog.so",
            "libandroid_jni_vm.so",
            "libandroid_base.so",
        ]
        .into_iter()
        .map(|name| std::mem::ManuallyDrop::new(libloading::Library::new(kit.join(name)).unwrap()))
        .collect()
    };
    for phase in [Phase::EarlyInit, Phase::RegisterJni, Phase::Init] {
        for lib in &libraries {
            // SAFETY: pinned native phase entry uses the exact same-kit Host API ABI.
            match unsafe { host_api::run_phase_entry(phase, lib) } {
                Ok(()) | Err(host_api::InitError::NotPresent) => {}
                Err(error) => panic!("phase failed {error:?}"),
            }
        }
    }
    host.java_vm().unwrap().get_env(0x10006).unwrap()
}

#[test]
#[ignore = "run alone on handheld with RUDROID_RESOURCE_KIT pointing to production"]
fn production_jni_class_assignability_uses_the_shared_type_authority() {
    let env = environment();
    let activity = env.find_class(c"android/app/Activity");
    let context = env.find_class(c"android/content/Context");
    let object = env.find_class(c"java/lang/Object");
    assert!(env.is_assignable_from(activity, context));
    assert!(!env.is_assignable_from(context, activity));
    assert!(env.is_assignable_from(activity, activity));
    let ints = env.find_class(c"[I");
    let longs = env.find_class(c"[J");
    let objects = env.find_class(c"[Ljava/lang/Object;");
    let strings = env.find_class(c"[Ljava/lang/String;");
    let cloneable = env.find_class(c"java/lang/Cloneable");
    assert!(env.is_assignable_from(ints, object));
    assert!(env.is_assignable_from(ints, cloneable));
    assert!(!env.is_assignable_from(ints, longs));
    assert!(env.is_assignable_from(strings, objects));
    assert!(!env.is_assignable_from(objects, strings));
    clean(env);
    assert!(!env.is_assignable_from(std::ptr::null_mut(), context));
    assert!(env.exception_check());
    env.exception_clear();
    println!("production JNI casts preserve class/interface/array direction and invalid-input exceptions");
}

#[test]
#[ignore = "run alone on handheld with RUDROID_RESOURCE_KIT pointing to production"]
fn production_critical_strings_preserve_exact_utf16_and_nested_borrows() {
    use rudroid_common::jni_abi::function_slot;
    use std::ffi::c_void;
    let env = environment();
    let units = [65u16, 0, 0x4e2d, 0xd83d, 0xde00, 0xd800];
    let string = env.new_string(&units);
    assert!(!string.is_null());
    assert_eq!(env.get_string_length(string), 6);
    let weak = env.new_weak_global_ref(string);
    clean(env);
    // SAFETY: exact JNI 1.6 prototypes in pinned tables. No unrelated JNI,
    // allocation or blocking operation occurs inside these nested critical
    // pairs; each bounded read ends before its matching release.
    unsafe {
        let borrow = function_slot::<
            unsafe extern "C" fn(*mut c_void, *mut c_void, *mut u8) -> *const u16,
        >(env.as_raw(), 224);
        let release = function_slot::<unsafe extern "C" fn(*mut c_void, *mut c_void, *const u16)>(
            env.as_raw(),
            225,
        );
        let mut copy = [7u8, 99];
        let first = borrow(env.as_raw(), string, copy.as_mut_ptr());
        let second = borrow(env.as_raw(), string, std::ptr::null_mut());
        let mut output = [0u16; 6];
        let valid = !first.is_null() && first == second;
        if valid {
            std::ptr::copy_nonoverlapping(first, output.as_mut_ptr(), 6);
        }
        if !first.is_null() {
            release(env.as_raw(), string, first);
        }
        // The second lease still owns the immutable allocation after release one.
        let mut after = [0u16; 6];
        if !second.is_null() {
            std::ptr::copy_nonoverlapping(second, after.as_mut_ptr(), 6);
            release(env.as_raw(), string, second);
        }
        assert!(valid);
        assert_eq!(copy, [0, 99]);
        assert_eq!(output, units);
        assert_eq!(after, units);
        let empty = env.new_string(&[]);
        let pointer = borrow(env.as_raw(), empty, std::ptr::null_mut());
        if !pointer.is_null() {
            release(env.as_raw(), empty, pointer);
        }
        assert!(!pointer.is_null());
        env.delete_local_ref(empty);
    }
    clean(env);
    env.delete_local_ref(string);
    assert!(
        env.new_local_ref(weak).is_null(),
        "all critical pins were released"
    );
    clean(env);
    env.delete_weak_global_ref(weak);
    println!("production critical UTF-16: exact NUL/surrogates, copy canary, nested lifetime and empty string passed");
}

#[test]
#[ignore = "run alone on handheld with RUDROID_RESOURCE_KIT pointing to production"]
fn production_scalar_calls_preserve_narrow_widths_and_double_bits() {
    use rudroid_common::{
        jni_abi::{function_slot, JValue},
        jni_env::{JObject, Native},
    };
    use std::{
        ffi::c_void,
        sync::atomic::{AtomicUsize, Ordering},
    };
    static CALLS: AtomicUsize = AtomicUsize::new(0);
    extern "C" fn byte(_: JniEnv, _: JObject, value: i8) -> i8 {
        CALLS.fetch_add(1, Ordering::SeqCst);
        value
    }
    extern "C" fn char(_: JniEnv, _: JObject, value: u16) -> u16 {
        CALLS.fetch_add(1, Ordering::SeqCst);
        value
    }
    extern "C" fn short(_: JniEnv, _: JObject, value: i16) -> i16 {
        CALLS.fetch_add(1, Ordering::SeqCst);
        value
    }
    extern "C" fn double(_: JniEnv, _: JObject, value: f64) -> f64 {
        CALLS.fetch_add(1, Ordering::SeqCst);
        value
    }
    let env = environment();
    let class = env.find_class(c"org/rudroid/tests/ScalarWidths");
    assert_eq!(
        env.register_class(
            c"org/rudroid/tests/ScalarWidths",
            &[
                Native::new1(c"byte", c"(B)B", byte),
                Native::new1(c"char", c"(C)C", char),
                Native::new1(c"short", c"(S)S", short),
                Native::new1(c"double", c"(D)D", double),
            ]
        ),
        0
    );
    let object = env.alloc_object(class);
    macro_rules! check {
        ($name:expr, $signature:expr, $ty:ty, $promoted:ty, $field:ident, $a:expr, $static_a:expr, $variadic:expr, $static_variadic:expr, $values:expr) => {
            let method = env.get_method_id(class, $name, $signature);
            assert!(!method.is_null());
            for value in $values {
                let argument = JValue { $field: value };
                // SAFETY: exact JNI prototypes and descriptor-matched POD values.
                // Narrow varargs use C integer promotions; double stays 64-bit.
                unsafe {
                    for (slot, receiver) in [($a, object), ($static_a, class)] {
                        let call = function_slot::<
                            unsafe extern "C" fn(
                                *mut c_void,
                                *mut c_void,
                                *mut c_void,
                                *const JValue,
                            ) -> $ty,
                        >(env.as_raw(), slot);
                        assert_eq!(
                            call(env.as_raw(), receiver, method, &argument).to_ne_bytes(),
                            value.to_ne_bytes()
                        );
                        clean(env);
                    }
                    for (slot, receiver) in [($variadic, object), ($static_variadic, class)] {
                        let call = function_slot::<
                            unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, ...) -> $ty,
                        >(env.as_raw(), slot);
                        assert_eq!(
                            call(env.as_raw(), receiver, method, value as $promoted).to_ne_bytes(),
                            value.to_ne_bytes()
                        );
                        clean(env);
                    }
                }
            }
        };
    }
    check!(
        c"byte",
        c"(B)B",
        i8,
        i32,
        b,
        42,
        122,
        40,
        120,
        [i8::MIN, i8::MAX]
    );
    check!(
        c"char",
        c"(C)C",
        u16,
        i32,
        c,
        45,
        125,
        43,
        123,
        [0u16, u16::MAX]
    );
    check!(
        c"short",
        c"(S)S",
        i16,
        i32,
        s,
        48,
        128,
        46,
        126,
        [i16::MIN, i16::MAX]
    );
    check!(
        c"double",
        c"(D)D",
        f64,
        f64,
        d,
        60,
        140,
        58,
        138,
        [
            -0.0f64,
            1_234_567.125,
            f64::from_bits(0x7ff8_1234_5678_9abc)
        ]
    );
    assert_eq!(CALLS.load(Ordering::SeqCst), 36);
    let method = env.get_method_id(class, c"byte", c"(B)B");
    let arg = JValue { b: 5 };
    // SAFETY: valid ID/POD argument; intentionally wrong return accessor must
    // be rejected by the VM before any registered native address is called.
    let result = unsafe {
        function_slot::<
            unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, *const JValue) -> i32,
        >(env.as_raw(), 51)(env.as_raw(), object, method, &arg)
    };
    assert_eq!(result, 0);
    assert!(env.exception_check());
    env.exception_clear();
    assert_eq!(CALLS.load(Ordering::SeqCst), 36);
    println!("production A/variadic scalar returns preserve signed widths, char65535, double precision/signed-zero/NaN and accessor rejection");
}

#[test]
#[ignore = "run alone on handheld with RUDROID_RESOURCE_KIT pointing to production"]
fn production_scoped_string_views_keep_storage_live_and_release_on_unwind() {
    let env=environment();
    for utf16 in [false,true] {
        let string=env.new_string_utf(c"scope");let weak=env.new_weak_global_ref(string);
        if utf16 {
            assert_eq!(env.with_string_utf16(string,|view| {
                env.delete_local_ref(string);assert_eq!(view,[115,99,111,112,101]);view.len()
            }),Some(5));
        }else{
            assert_eq!(env.with_string_utf(string,|view| {
                env.delete_local_ref(string);assert_eq!(view,c"scope");view.to_bytes().len()
            }),Some(5));
        }
        assert!(env.new_local_ref(weak).is_null());env.delete_weak_global_ref(weak);clean(env);
        let string=env.new_string_utf(c"cleanup during unwind");let weak=env.new_weak_global_ref(string);
        let failed=std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if utf16 {env.with_string_utf16(string,|_|panic!("expected scoped UTF-16 unwind"));}
            else {env.with_string_utf(string,|_|panic!("expected scoped UTF-8 unwind"));}
        }));
        assert!(failed.is_err());env.delete_local_ref(string);
        assert!(env.new_local_ref(weak).is_null());env.delete_weak_global_ref(weak);clean(env);
    }
    let units=[65u16,0,0xd800,0xdc00,0x4e2d];let string=env.new_string(&units);
    assert_eq!(env.with_string_utf16(string,|view|view==units),Some(true));
    assert_eq!(env.with_string_utf(string,|view|view.to_bytes().windows(2).any(|pair|pair==[0xc0,0x80])),Some(true));
    env.delete_local_ref(string);clean(env);
    println!("production scoped strings: retained contents after caller deletion, exact UTF-16/modified UTF-8 and normal/unwind cleanup passed");
}

#[test]
#[ignore = "run alone on handheld with RUDROID_RESOURCE_KIT pointing to production"]
fn production_nonvirtual_calls_preserve_class_selection_and_exact_return_abis() {
    use rudroid_common::jni_abi::{declare_superclass, function_slot, JValue};
    use rudroid_common::jni_env::{JObject, Native};
    use std::ffi::c_void;
    use std::sync::atomic::{AtomicUsize, Ordering};
    static VOIDS: AtomicUsize = AtomicUsize::new(0);
    macro_rules! constant {
        ($name:ident,$ty:ty,$value:expr) => { extern "C" fn $name(_:JniEnv,_:JObject)->$ty {$value} };
    }
    extern "C" fn object(_:JniEnv,receiver:JObject)->JObject {receiver}
    constant!(boolean,u8,1);constant!(byte,i8,-127);constant!(character,u16,65535);
    constant!(short,i16,-32767);constant!(int,i32,0x1234567);
    constant!(long,i64,0x1122334455667788);constant!(float,f32,1.25);
    constant!(double,f64,1234567.125);constant!(child_int,i32,22);
    extern "C" fn void(_:JniEnv,_:JObject) {VOIDS.fetch_add(1,Ordering::Relaxed);}
    let env=environment();
    assert_eq!(declare_superclass(env,c"org/rudroid/tests/NvChild",c"org/rudroid/tests/NvBase"),0);
    assert_eq!(env.register_class(c"org/rudroid/tests/NvBase",&[
        Native::new0(c"object",c"()Ljava/lang/Object;",object),
        Native::new0(c"boolean",c"()Z",boolean),Native::new0(c"byte",c"()B",byte),
        Native::new0(c"char",c"()C",character),Native::new0(c"short",c"()S",short),
        Native::new0(c"int",c"()I",int),Native::new0(c"long",c"()J",long),
        Native::new0(c"float",c"()F",float),Native::new0(c"double",c"()D",double),
        Native::new0(c"void",c"()V",void),
    ]),0);
    assert_eq!(env.register_class(c"org/rudroid/tests/NvChild",&[Native::new0(c"int",c"()I",child_int)]),0);
    let base=env.find_class(c"org/rudroid/tests/NvBase");
    let child=env.find_class(c"org/rudroid/tests/NvChild");
    let receiver=env.alloc_object(child);
    macro_rules! check {
        ($name:expr,$signature:expr,$ty:ty,$slot:expr,$expected:expr) => {{
            let method=env.get_method_id(base,$name,$signature);assert!(!method.is_null());
            // SAFETY: fixed JNI signatures, live classes/receiver/method and
            // exact zero-argument native prototypes, called through production.
            unsafe {
                let a=function_slot::<unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*const JValue)->$ty>(env.as_raw(),$slot);
                let variadic=function_slot::<unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,...)->$ty>(env.as_raw(),$slot-2);
                assert_eq!(a(env.as_raw(),receiver,base,method,std::ptr::null()),$expected);
                clean(env);
                assert_eq!(variadic(env.as_raw(),receiver,base,method),$expected);
                clean(env);
            }
        }};
    }
    let object_id=env.get_method_id(base,c"object",c"()Ljava/lang/Object;");
    // SAFETY: exact reference-returning A/variadic ABIs. JNI guarantees
    // object identity, not equality of distinct local reference tokens.
    unsafe {
        let a=function_slot::<unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*const JValue)->*mut c_void>(env.as_raw(),66);
        let regular=function_slot::<unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,...)->*mut c_void>(env.as_raw(),64);
        for result in [a(env.as_raw(),receiver,base,object_id,std::ptr::null()),regular(env.as_raw(),receiver,base,object_id)] {
            assert!(env.is_same_object(result,receiver));env.delete_local_ref(result);
        }
    }
    check!(c"boolean",c"()Z",u8,69,1);check!(c"byte",c"()B",i8,72,-127);
    check!(c"char",c"()C",u16,75,65535);check!(c"short",c"()S",i16,78,-32767);
    check!(c"int",c"()I",i32,81,0x1234567);check!(c"long",c"()J",i64,84,0x1122334455667788);
    check!(c"float",c"()F",f32,87,1.25);check!(c"double",c"()D",f64,90,1234567.125);
    check!(c"void",c"()V",(),93,());assert_eq!(VOIDS.load(Ordering::Relaxed),2);
    let base_id=env.get_method_id(base,c"int",c"()I");
    let child_id=env.get_method_id(child,c"int",c"()I");
    assert_eq!(env.call_int(receiver,env.method(base,c"int",c"()I").unwrap(),&[]),22);
    // SAFETY: exact typed nonvirtual Int entry, zero arguments and retained IDs.
    unsafe {
        let call=function_slot::<unsafe extern "C" fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*const JValue)->i32>(env.as_raw(),81);
        assert_eq!(call(env.as_raw(),receiver,base,base_id,std::ptr::null()),0x1234567);
        assert_eq!(call(env.as_raw(),receiver,child,child_id,std::ptr::null()),22);
        assert_eq!(call(env.as_raw(),receiver,base,child_id,std::ptr::null()),0);
        assert!(env.exception_check());env.exception_clear();
        let unrelated=env.find_class(c"org/rudroid/tests/NvUnrelated");
        assert_eq!(call(env.as_raw(),receiver,unrelated,base_id,std::ptr::null()),0);
        assert!(env.exception_check());env.exception_clear();env.delete_local_ref(unrelated);
        let define=function_slot::<unsafe extern "C" fn(*mut c_void,*const std::ffi::c_char,*mut c_void,*const i8,i32)->*mut c_void>(env.as_raw(),5);
        let bytes=[0i8;4];assert!(define(env.as_raw(),c"unsupported".as_ptr(),std::ptr::null_mut(),bytes.as_ptr(),4).is_null());
        assert!(env.exception_check());env.exception_clear();
    }
    env.delete_local_ref(receiver);env.delete_local_ref(child);env.delete_local_ref(base);
    println!("production all ten nonvirtual A/variadic return ABIs and override/provenance rejection passed; DefineClass remains explicit unavailable");
}

#[test]
#[ignore = "run alone on handheld with RUDROID_RESOURCE_KIT pointing to production"]
fn production_native_returns_reclaim_temporary_locals_and_preserve_caller_roots() {
    use rudroid_common::jni_env::{JObject,Native};
    use std::sync::atomic::{AtomicUsize,Ordering};
    static TEMP:AtomicUsize=AtomicUsize::new(0);
    static RETURNED:AtomicUsize=AtomicUsize::new(0);
    extern "C" fn create(env:JniEnv,receiver:JObject)->JObject {
        env.delete_local_ref(receiver);
        let temp=env.new_string_utf(c"native temporary");
        TEMP.store(env.new_weak_global_ref(temp) as usize,Ordering::Relaxed);
        env.new_byte_array(2048);
        let result=env.new_string_utf(c"native returned string");
        RETURNED.store(env.new_weak_global_ref(result) as usize,Ordering::Relaxed);
        result
    }
    let env=environment();
    assert_eq!(env.register_class(c"org/rudroid/tests/AutomaticNativeFrame",&[Native::new0(c"create",c"()Ljava/lang/String;",create)]),0);
    let class=env.find_class(c"org/rudroid/tests/AutomaticNativeFrame");
    let receiver=env.alloc_object(class);let method=env.method(class,c"create",c"()Ljava/lang/String;").unwrap();
    for _ in 0..200 {
        let result=env.call_object(receiver,method,&[]);
        let temp=TEMP.swap(0,Ordering::Relaxed) as JObject;
        let returned=RETURNED.swap(0,Ordering::Relaxed) as JObject;
        assert!(env.is_same_object(temp,std::ptr::null_mut()),"unreturned native local must die");
        assert!(env.is_instance_of(receiver,class),"native argument deletion cannot delete its caller");
        assert_eq!(env.with_string_utf(result,|text|text.to_bytes()==b"native returned string"),Some(true));
        assert!(!env.is_same_object(returned,std::ptr::null_mut()));
        env.delete_local_ref(result);assert!(env.is_same_object(returned,std::ptr::null_mut()));
        env.delete_weak_global_ref(temp);env.delete_weak_global_ref(returned);clean(env);
    }
    env.delete_local_ref(receiver);env.delete_local_ref(class);
    println!("production 200 native returns reclaimed temporary strings/arrays, independently owned arguments and preserved transferred result roots");
}
