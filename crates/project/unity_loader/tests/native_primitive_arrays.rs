//! INPUT:  complete trusted production kit, common JNI helpers and real VM table entries
//! OUTPUT:  all eight primitive C ABIs, critical copy ownership, exact bits and query canaries
//! POS:    isolated handheld JNI acceptance; no original game files or saves are opened
use rudroid_common::{jni_abi::function_slot, jni_env::JniEnv};
use rudroid_unity_loader::host_api;
use std::{ffi::c_void, path::PathBuf};
#[test]
#[ignore = "run alone on the handheld with RUDROID_RESOURCE_KIT pointing to a complete production kit"]
fn all_primitive_arrays_work_through_the_production_jni_tables() {
    let kit =
        PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").expect("production kit required"));
    host_api::install_config(None).unwrap();
    let host = host_api::table();
    // SAFETY: same trusted production kit, kept loaded throughout every reference and callback.
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
    for phase in [
        rudroid_unity_loader::phases::Phase::EarlyInit,
        rudroid_unity_loader::phases::Phase::RegisterJni,
        rudroid_unity_loader::phases::Phase::Init,
    ] {
        for lib in &libraries {
            // SAFETY: exact pinned phase entry ABI; fixture uses no native guest or SDL producer.
            match unsafe { host_api::run_phase_entry(phase, lib) } {
                Ok(()) | Err(host_api::InitError::NotPresent) => {}
                Err(e) => panic!("phase failed {e:?}"),
            }
        }
    }
    let env = host.java_vm().unwrap().get_env(0x10006).unwrap();
    fn clear(env: JniEnv) {
        assert!(!env.exception_check());
    }
    macro_rules! check {
        ($index:expr,$ty:ty,$new:ident,$get:ident,$set:ident,$values:expr) => {{
            let values: [$ty; 3] = $values;
            let array = env.$new(3);
            assert!(!array.is_null());
            assert_eq!(env.get_array_length(array), 3);
            clear(env);
            env.$set(array, 0, &values);
            clear(env);
            let mut output = [0 as $ty; 4];
            output[3] = 13 as $ty;
            env.$get(array, 0, &mut output[..3]);
            clear(env);
            assert_eq!(output[3], 13 as $ty);
            // SAFETY: exact JNI 1.6 C prototypes, initialized POD values and bounded output.
            // All native copy accesses cease during release; globals preserve the real array.
            unsafe {
                assert_eq!(
                    std::slice::from_raw_parts(
                        output.as_ptr().cast::<u8>(),
                        std::mem::size_of_val(&values)
                    ),
                    std::slice::from_raw_parts(
                        values.as_ptr().cast::<u8>(),
                        std::mem::size_of_val(&values)
                    )
                );
                let borrow = function_slot::<
                    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut u8) -> *mut $ty,
                >(env.as_raw(), 183 + $index);
                let release = function_slot::<
                    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut $ty, i32),
                >(env.as_raw(), 191 + $index);
                let mut copy = [0u8, 99];
                let pointer = borrow(env.as_raw(), array, copy.as_mut_ptr());
                assert!(!pointer.is_null());
                assert_eq!(copy, [1, 99]);
                *pointer = 0 as $ty;
                release(env.as_raw(), array, pointer, 1);
                clear(env);
                *pointer = 1 as $ty;
                release(env.as_raw(), array, pointer, 2);
                clear(env);
                env.$get(array, 0, &mut output[..3]);
                clear(env);
                assert_eq!(output[0], 0 as $ty);
                let critical = function_slot::<
                    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut u8) -> *mut c_void,
                >(env.as_raw(), 222);
                let end = function_slot::<
                    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, i32),
                >(env.as_raw(), 223);
                let global = env.new_global_ref(array);
                clear(env);
                let pointer = critical(env.as_raw(), array, copy.as_mut_ptr()).cast::<$ty>();
                assert!(!pointer.is_null());
                *pointer = 1 as $ty;
                end(env.as_raw(), array, pointer.cast(), 0);
                clear(env);
                env.delete_local_ref(array);
                env.$get(global, 0, &mut output[..3]);
                clear(env);
                assert_eq!(output[0], 1 as $ty);
                assert_eq!(output[3], 13 as $ty);
                env.delete_global_ref(global);
                clear(env);
            }
        }};
    }
    check!(
        0,
        u8,
        new_boolean_array,
        get_boolean_array_region,
        set_boolean_array_region,
        [0, 1, 1]
    );
    check!(
        1,
        i8,
        new_byte_array,
        get_byte_array_region,
        set_byte_array_region,
        [i8::MIN, 0, i8::MAX]
    );
    check!(
        2,
        u16,
        new_char_array,
        get_char_array_region,
        set_char_array_region,
        [0, 0xd800, u16::MAX]
    );
    check!(
        3,
        i16,
        new_short_array,
        get_short_array_region,
        set_short_array_region,
        [i16::MIN, 0, i16::MAX]
    );
    check!(
        4,
        i32,
        new_int_array,
        get_int_array_region,
        set_int_array_region,
        [i32::MIN, 0, i32::MAX]
    );
    check!(
        5,
        i64,
        new_long_array,
        get_long_array_region,
        set_long_array_region,
        [i64::MIN, (1i64 << 53) + 1, i64::MAX]
    );
    check!(
        6,
        f32,
        new_float_array,
        get_float_array_region,
        set_float_array_region,
        [-0.0, f32::from_bits(0x7fc00001), f32::INFINITY]
    );
    check!(
        7,
        f64,
        new_double_array,
        get_double_array_region,
        set_double_array_region,
        [-0.0, f64::from_bits(0x7ff8000000000001), f64::NEG_INFINITY]
    );
}
