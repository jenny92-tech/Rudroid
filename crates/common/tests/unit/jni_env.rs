//! INPUT:  private items of `src/jni_env.rs` via `super`, a fake JNI function table
//! OUTPUT:  slot routing, descriptor parsing, checked calls/natives and string copy checks for `JavaVm` / `JniEnv`
//! POS:    proves each wrapper reaches its named slot; jni_vm separately pins the indices to its real table
use super::*;
use std::sync::Mutex;

static CALLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

unsafe extern "C" fn get_version(_env: *mut c_void) -> i32 {
    0x0001_0006
}
unsafe extern "C" fn find_class(_env: *mut c_void, name: *const c_char) -> JObject {
    let name = unsafe { CStr::from_ptr(name) }
        .to_string_lossy()
        .into_owned();
    CALLS.lock().unwrap().push(format!("find_class {name}"));
    0x31 as JObject
}
unsafe extern "C" fn delete_local_ref(_env: *mut c_void, object: JObject) {
    CALLS
        .lock()
        .unwrap()
        .push(format!("delete_local_ref {:#x}", object as usize));
}
unsafe extern "C" fn exception_check(_env: *mut c_void) -> u8 {
    1
}
unsafe extern "C" fn get_static_int(_env: *mut c_void, class: JObject, field: JFieldId) -> i32 {
    assert_eq!((class as usize, field as usize), (0x31, 0x72));
    96
}
unsafe extern "C" fn new_byte_array(_env: *mut c_void, length: i32) -> JObject {
    assert_eq!(length, 3);
    0x41 as JObject
}
unsafe extern "C" fn set_byte_region(
    _env: *mut c_void,
    array: JObject,
    start: i32,
    length: i32,
    values: *const i8,
) {
    assert_eq!((array as usize, start, length), (0x41, 0, 3));
    // SAFETY: the tested wrapper borrows exactly length readable elements.
    assert_eq!(
        unsafe { std::slice::from_raw_parts(values, length as usize) },
        [-128, 0, 127]
    );
}
unsafe extern "C" fn get_byte_region(
    _env: *mut c_void,
    array: JObject,
    start: i32,
    length: i32,
    out: *mut i8,
) {
    assert_eq!((array as usize, start, length), (0x41, 0, 3));
    // SAFETY: the tested wrapper lends a writable three-element slice.
    unsafe { std::ptr::copy_nonoverlapping([-1i8, 0, 127].as_ptr(), out, 3) };
}
unsafe extern "C" fn set_int_array_region(
    _env: *mut c_void,
    _array: JObject,
    start: i32,
    len: i32,
    values: *const i32,
) {
    let values = unsafe { std::slice::from_raw_parts(values, len as usize) };
    CALLS
        .lock()
        .unwrap()
        .push(format!("set_int_array_region {start} {values:?}"));
}
unsafe extern "C" fn vm_get_env(_vm: *mut c_void, env: *mut *mut c_void, version: i32) -> i32 {
    unsafe { *env = 0x1234 as *mut c_void };
    if version == JNI_VERSION_1_6 {
        JNI_OK
    } else {
        -3
    }
}

/// A handle whose first word points at `table`, the JNI double indirection.
fn handle(table: &'static [usize]) -> *mut c_void {
    Box::leak(Box::new(table.as_ptr())) as *mut *const usize as *mut c_void
}

#[test]
fn env_methods_reach_their_named_slots() {
    let mut table = vec![0usize; 233];
    table[slot::env::GET_VERSION] = get_version as usize;
    table[slot::env::FIND_CLASS] = find_class as usize;
    table[slot::env::DELETE_LOCAL_REF] = delete_local_ref as usize;
    table[slot::env::EXCEPTION_CHECK] = exception_check as usize;
    table[slot::env::SET_INT_ARRAY_REGION] = set_int_array_region as usize;
    table[slot::env::GET_STATIC_INT_FIELD] = get_static_int as usize;
    table[slot::env::NEW_BYTE_ARRAY] = new_byte_array as usize;
    table[slot::env::SET_BYTE_ARRAY_REGION] = set_byte_region as usize;
    table[slot::env::GET_BYTE_ARRAY_REGION] = get_byte_region as usize;
    let env = unsafe { JniEnv::from_raw(handle(Box::leak(table.into_boxed_slice()))) };
    CALLS.lock().unwrap().clear();

    assert_eq!(env.get_version(), 0x0001_0006);
    assert_eq!(slot::env::GET_STATIC_INT_FIELD, 150);
    assert_eq!(
        env.get_static_int_field(0x31 as JObject, 0x72 as JFieldId),
        96
    );
    assert_eq!(
        (
            slot::env::NEW_BYTE_ARRAY,
            slot::env::GET_BYTE_ARRAY_REGION,
            slot::env::SET_BYTE_ARRAY_REGION
        ),
        (176, 200, 208)
    );
    let bytes = env.new_byte_array(3);
    env.set_byte_array_region(bytes, 0, &[-128, 0, 127]);
    let mut output = [0; 3];
    env.get_byte_array_region(bytes, 0, &mut output);
    assert_eq!(output, [-1, 0, 127]);
    assert_eq!(env.find_class(c"java/lang/String") as usize, 0x31);
    env.delete_local_ref(0x31 as JObject);
    assert!(env.exception_check());
    env.set_int_array_region(std::ptr::null_mut(), 2, &[7, 8, 9]);
    assert_eq!(
        *CALLS.lock().unwrap(),
        [
            "find_class java/lang/String",
            "delete_local_ref 0x31",
            "set_int_array_region 2 [7, 8, 9]"
        ]
    );
}

#[test]
fn vm_get_env_requires_ok_and_a_non_null_env() {
    let mut table = vec![0usize; 8];
    table[slot::vm::GET_ENV] = vm_get_env as usize;
    let vm = unsafe { JavaVm::from_raw(handle(Box::leak(table.into_boxed_slice()))) }.unwrap();
    assert_eq!(
        vm.get_env(JNI_VERSION_1_6).map(JniEnv::as_raw),
        Some(0x1234 as *mut c_void)
    );
    assert_eq!(vm.get_env(0x0001_0004), None);
    assert!(unsafe { JavaVm::from_raw(std::ptr::null_mut()) }.is_none());
}

#[test]
fn signatures_parse_into_parameter_and_return_types() {
    use JniType::*;
    assert_eq!(parse_signature(c"()V"), Some((vec![], Void)));
    assert_eq!(
        parse_signature(c"(I[BLjava/lang/String;[[Ljava/lang/Object;ZJ)Ljava/lang/Object;"),
        Some((vec![Int, Object, Object, Object, Boolean, Long], Object))
    );
    assert_eq!(
        parse_signature(c"(FDCS)[I"),
        Some((vec![Float, Double, Char, Short], Object))
    );
    for bad in [
        c"",
        c"V",
        c"(V)V",
        c"(L;)V",
        c"(Ljava/lang/String)V",
        c"(I",
        c"()",
        c"()VV",
        c"([V)V",
        c"(Q)V",
    ] {
        assert_eq!(parse_signature(bad), None, "{bad:?}");
    }
}

#[test]
fn args_initialise_all_eight_bytes() {
    for arg in [
        Arg::bool(true),
        Arg::byte(-1),
        Arg::char(0xffff),
        Arg::short(-1),
        Arg::int(-1),
        Arg::float(-1.0),
    ] {
        // SAFETY: every constructor writes all eight bytes first.
        let bits = unsafe { arg.0.j } as u64;
        assert_eq!(bits >> 32, 0, "upper half stays zero: {bits:#x}");
    }
    // SAFETY: as above.
    assert_eq!(unsafe { Arg::long(-2).0.j }, -2);
    assert_eq!(unsafe { Arg::object(0x35 as JObject).0.l } as usize, 0x35);
}

extern "C" fn native_object_int(_env: JniEnv, _this: JObject, _name: JObject, _value: i32) -> u8 {
    1
}
extern "C" fn native_void(_env: JniEnv, _this: JObject) {}

#[test]
fn natives_match_their_descriptor() {
    let natives = [
        Native::new2(c"put", c"(Ljava/lang/String;I)Z", native_object_int),
        Native::new0(c"run", c"()V", native_void),
    ];
    assert_eq!(natives[0].name(), c"put");
    assert_eq!(natives[1].0.fn_ptr as usize, native_void as usize);
}

#[test]
#[should_panic(expected = "does not match its Rust prototype")]
fn a_native_with_the_wrong_parameter_type_is_rejected() {
    // jboolean is u8: an i32 parameter would read undefined upper bits.
    Native::new2(c"put", c"(Ljava/lang/String;Z)Z", native_object_int);
}

#[test]
#[should_panic(expected = "does not match its Rust prototype")]
fn a_native_with_the_wrong_return_type_is_rejected() {
    Native::new0(c"run", c"()I", native_void);
}

static ARGS: Mutex<Vec<i64>> = Mutex::new(Vec::new());

unsafe extern "C" fn get_static_method_id(
    _env: *mut c_void,
    _class: JObject,
    _name: *const c_char,
    _sig: *const c_char,
) -> JMethodId {
    0x33 as JMethodId
}
unsafe extern "C" fn call_static_int_a(
    _env: *mut c_void,
    _class: JObject,
    method: JMethodId,
    args: *const JValue,
) -> i32 {
    assert_eq!(method as usize, 0x33);
    // Two parameters: the descriptor below is `(IJ)I`.
    let args = unsafe { std::slice::from_raw_parts(args, 2) };
    ARGS.lock()
        .unwrap()
        .extend(args.iter().map(|a| unsafe { a.j }));
    7
}
unsafe extern "C" fn get_string_length(_env: *mut c_void, _s: JObject) -> i32 {
    2
}
static CHARS: [u16; 2] = [0x4f60, 0x597d];
static RELEASED: Mutex<Vec<String>> = Mutex::new(Vec::new());
thread_local! {static STRING_ROOTED:std::cell::Cell<bool>=const{std::cell::Cell::new(false)};
static STRING_ROOT_RELEASES:std::cell::Cell<usize>=const{std::cell::Cell::new(0)};}
unsafe extern "C" fn retain_string(_: *mut c_void, string: JObject) -> JObject {
    assert_eq!(string as usize, 0x32);
    STRING_ROOTED.with(|root| assert!(!root.replace(true)));
    0x88 as JObject
}
unsafe extern "C" fn delete_string(_: *mut c_void, string: JObject) {
    assert_eq!(string as usize, 0x88);
    STRING_ROOTED.with(|root| assert!(root.replace(false)));
    STRING_ROOT_RELEASES.with(|n| n.set(n.get() + 1));
}

unsafe extern "C" fn get_string_chars(
    _env: *mut c_void,
    _s: JObject,
    _copy: *mut u8,
) -> *const u16 {
    assert_eq!(_s as usize, 0x88);
    STRING_ROOTED.with(|root| assert!(root.get()));
    CHARS.as_ptr()
}
unsafe extern "C" fn release_string_chars(_env: *mut c_void, _s: JObject, chars: *const u16) {
    assert_eq!(_s as usize, 0x88);
    STRING_ROOTED.with(|root| assert!(root.get()));
    RELEASED
        .lock()
        .unwrap()
        .push(format!("release {:x}", unsafe { *chars }));
}

fn call_env() -> JniEnv {
    let mut table = vec![0usize; 233];
    table[slot::env::GET_STATIC_METHOD_ID] = get_static_method_id as usize;
    table[slot::env::CALL_STATIC_INT_METHOD_A] = call_static_int_a as usize;
    table[slot::env::GET_STRING_LENGTH] = get_string_length as usize;
    table[slot::env::GET_STRING_CHARS] = get_string_chars as usize;
    table[slot::env::RELEASE_STRING_CHARS] = release_string_chars as usize;
    table[slot::env::NEW_LOCAL_REF] = retain_string as usize;
    table[slot::env::DELETE_LOCAL_REF] = delete_string as usize;
    unsafe { JniEnv::from_raw(handle(Box::leak(table.into_boxed_slice()))) }
}

#[test]
fn checked_calls_pass_exactly_the_descriptor_arguments() {
    let env = call_env();
    let method = env
        .static_method(std::ptr::null_mut(), c"f", c"(IJ)I")
        .unwrap();
    assert_eq!(method.arity(), 2);
    ARGS.lock().unwrap().clear();
    assert_eq!(
        env.call_static_int(std::ptr::null_mut(), method, &[Arg::int(3), Arg::long(-4)]),
        7
    );
    assert_eq!(*ARGS.lock().unwrap(), [3, -4]);
    assert_eq!(
        env.static_method(std::ptr::null_mut(), c"f", c"(IJ"),
        None,
        "malformed descriptor"
    );
}

#[test]
#[should_panic(expected = "passes 1 arguments to a 2-parameter method")]
fn a_call_with_the_wrong_argument_count_panics_before_the_vm() {
    let env = call_env();
    let method = env
        .static_method(std::ptr::null_mut(), c"f", c"(IJ)I")
        .unwrap();
    env.call_static_int(std::ptr::null_mut(), method, &[Arg::int(3)]);
}

#[test]
fn strings_are_copied_and_released() {
    RELEASED.lock().unwrap().clear();
    STRING_ROOT_RELEASES.with(|n| n.set(0));
    let env = call_env();
    assert_eq!(env.string(0x32 as JObject).as_deref(), Some("你好"));
    assert_eq!(*RELEASED.lock().unwrap(), ["release 4f60"]);
    assert_eq!(env.string(std::ptr::null_mut()), None);
    STRING_ROOTED.with(|root| assert!(!root.get()));
    STRING_ROOT_RELEASES.with(|n| assert_eq!(n.get(), 1));
}

thread_local! {
    static BUFFER_ROOTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static BUFFER_CAPACITY: std::cell::Cell<i64> = const { std::cell::Cell::new(4) };
    static BUFFER_RELEASES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}
static BUFFER_BYTES: [u8; 4] = [1, 2, 3, 4];
unsafe extern "C" fn retain_buffer(_: *mut c_void, _: JObject) -> JObject {
    BUFFER_ROOTED.with(|root| assert!(!root.replace(true)));
    0x77 as JObject
}
unsafe extern "C" fn release_buffer(_: *mut c_void, object: JObject) {
    assert_eq!(object as usize, 0x77);
    BUFFER_ROOTED.with(|root| assert!(root.replace(false)));
    BUFFER_RELEASES.with(|n| n.set(n.get() + 1));
}
unsafe extern "C" fn buffer_address(_: *mut c_void, object: JObject) -> *mut c_void {
    assert_eq!(object as usize, 0x77);
    BUFFER_ROOTED.with(|root| assert!(root.get(), "read addressed unretained storage"));
    BUFFER_BYTES.as_ptr().cast_mut().cast()
}
unsafe extern "C" fn buffer_capacity(_: *mut c_void, _: JObject) -> i64 {
    BUFFER_CAPACITY.with(|n| n.get())
}
#[test]
fn direct_buffer_copy_retains_before_address_lookup_and_releases_on_failure() {
    let mut table = vec![0usize; 233];
    table[slot::env::NEW_LOCAL_REF] = retain_buffer as usize;
    table[slot::env::DELETE_LOCAL_REF] = release_buffer as usize;
    table[slot::env::GET_DIRECT_BUFFER_ADDRESS] = buffer_address as usize;
    table[slot::env::GET_DIRECT_BUFFER_CAPACITY] = buffer_capacity as usize;
    let env = unsafe { JniEnv::from_raw(handle(Box::leak(table.into_boxed_slice()))) };
    BUFFER_RELEASES.with(|n| n.set(0));
    let mut out = [0; 4];
    assert!(env.read_direct_buffer(0x10 as JObject, &mut out));
    assert_eq!(out, BUFFER_BYTES);
    BUFFER_CAPACITY.with(|n| n.set(2));
    assert!(!env.read_direct_buffer(0x10 as JObject, &mut out));
    BUFFER_CAPACITY.with(|n| n.set(-1));
    assert!(!env.read_direct_buffer(0x10 as JObject, &mut out));
    BUFFER_ROOTED.with(|root| assert!(!root.get()));
    BUFFER_RELEASES.with(|n| assert_eq!(n.get(), 3));
}

#[test]
fn optional_vm_preserves_the_nullable_pointer_c_abi() {
    use std::mem::{align_of, size_of};
    assert_eq!(size_of::<Option<JavaVm>>(), size_of::<*mut c_void>());
    assert_eq!(align_of::<Option<JavaVm>>(), align_of::<*mut c_void>());
    // SAFETY: Rust guarantees NonNull's nullable-pointer representation.
    let absent: *mut c_void = unsafe { std::mem::transmute(None::<JavaVm>) };
    assert!(absent.is_null());
}
