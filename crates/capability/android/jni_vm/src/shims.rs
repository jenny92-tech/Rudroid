//! INPUT: raw JNI arguments and the process session
//! OUTPUT: JavaVM and JNIEnv function table entries
//! POS: native ABI adapters for session operations
pub(crate) use super::scalar_calls::*;
pub(crate) use super::nonvirtual_calls::*;
pub(crate) use super::fields::shims::*;
pub(crate) use super::strings::shims::*;
pub(crate) use super::primitive_arrays::{new_boolean_array,get_boolean_array_region,set_boolean_array_region,get_boolean_array_elements,release_boolean_array_elements,new_byte_array,get_byte_array_region,set_byte_array_region,get_byte_array_elements,release_byte_array_elements,new_char_array,get_char_array_region,set_char_array_region,get_char_array_elements,release_char_array_elements,new_short_array,get_short_array_region,set_short_array_region,get_short_array_elements,release_short_array_elements,new_int_array,get_int_array_region,set_int_array_region,get_int_array_elements,release_int_array_elements,new_long_array,get_long_array_region,set_long_array_region,get_long_array_elements,release_long_array_elements,new_float_array,get_float_array_region,set_float_array_region,get_float_array_elements,release_float_array_elements,new_double_array,get_double_array_region,set_double_array_region,get_double_array_elements,release_double_array_elements,get_primitive_array_critical,release_primitive_array_critical};

pub use super::direct_buffers::shims::*;
pub(crate) use super::streams::shims::*;
use super::*;

pub extern "C" fn new_object_array(_env: *mut c_void, size: i32, class: *mut c_void, initial: *mut c_void) -> *mut c_void {
    session().map(|s| s.new_object_array(size, class, initial)).unwrap_or(std::ptr::null_mut())
}

pub extern "C" fn get_array_length(_env: *mut c_void, array: *mut c_void) -> i32 {
    session().map(|s| s.array_length(array)).unwrap_or(0)
}

pub extern "C" fn get_object_array_element(_env: *mut c_void, array: *mut c_void, index: i32) -> *mut c_void {
    session().map(|s| s.object_array_element(array, index)).unwrap_or(std::ptr::null_mut())
}

pub extern "C" fn set_object_array_element(_env: *mut c_void, array: *mut c_void, index: i32, value: *mut c_void) {
    if let Some(s) = session() { s.set_object_array_element(array, index, value); }
}

pub unsafe extern "C" fn new_object_a(env: *mut c_void, class: *mut c_void, method: *mut c_void, args: *const JValue) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    if s.blocked_by_pending("NewObjectA") { return std::ptr::null_mut(); }
    let Some(class_id) = s.resolve_class(class, "NewObjectA") else { return std::ptr::null_mut(); };
    let (tag, id) = handle_parts(method);
    let valid = tag == TAG_METHOD && s.methods.lock().unwrap().by_id.get(&id)
        .is_some_and(|m| m.name.as_ref() == "<init>" && m.class == class_id && m.sig.ends_with(")V"));
    if !valid {
        s.set_pending("java/lang/NoSuchMethodError", "not this class's constructor");
        return std::ptr::null_mut();
    }
    if s.class_name(class_id).as_ref()=="java/lang/String" && s.methods.lock().unwrap().by_id[&id].sig.as_ref()=="()V" {
        return s.new_string_units(&[]);
    }
    if s.class_name(class_id).as_ref()=="java/lang/String" && s.methods.lock().unwrap().by_id[&id].sig.as_ref()=="([BLjava/lang/String;)V" {
        if args.is_null(){s.set_pending("java/lang/IllegalArgumentException","missing String constructor arguments");return std::ptr::null_mut();}
        let Some(utf8)=s.with_string_bytes((*args.add(1)).l,|bytes|matches!(bytes,b"UTF-8"|b"UTF8")) else{return std::ptr::null_mut();};
        if !utf8{s.set_pending("java/io/UnsupportedEncodingException","String byte constructor supports UTF-8");return std::ptr::null_mut();}
        let Some(RefTarget::Array(array))=s.resolve((*args).l) else{s.set_pending("java/lang/NullPointerException","missing String byte array");return std::ptr::null_mut();};
        let units={let arrays=s.arrays.lock().unwrap();let Some(bytes)=arrays.get(&array).and_then(|a|a.primitive.as_ref().and_then(primitive_arrays::Values::bytes)) else{drop(arrays);s.set_pending("java/lang/IllegalArgumentException","String requires byte[]");return std::ptr::null_mut();};
            String::from_utf8_lossy(bytes::unsigned(bytes)).encode_utf16().collect::<Vec<_>>()};
        return s.new_string_units(&units);
    }
    let object = s.alloc_object(class);
    if object.is_null() { return object; }
    call_void_method_a(env, object, method, args);
    if s.with_thread(|t| t.pending.is_some()) {
        s.delete_ref(object, false, "NewObjectA failed constructor");
        std::ptr::null_mut()
    } else { object }
}

pub extern "C" fn immutable_string_constructor(_env:*mut c_void,_object:*mut c_void,_bytes:*mut c_void,_encoding:*mut c_void){if let Some(s)=session(){s.set_pending("java/lang/IllegalArgumentException","immutable String constructor requires NewObject");}}
fn with_boxed_field<R>(env: *mut c_void, object: *mut c_void, signature: &str, access: impl FnOnce(*mut c_void) -> R) -> R {
    let class = get_object_class(env, object);
    let field = session().map(|s| s.resolve_field(class, "value", signature, false)).unwrap_or(std::ptr::null_mut());
    let result = access(field);
    delete_local_ref(env, class);
    result
}

pub extern "C" fn long_init(env: *mut c_void, object: *mut c_void, value: i64) {
    with_boxed_field(env, object, "J", |field| set_long_field(env, object, field, value));
}
pub extern "C" fn integer_init(env: *mut c_void, object: *mut c_void, value: i32) {
    with_boxed_field(env, object, "I", |field| set_int_field(env, object, field, value));
}
pub extern "C" fn integer_parse(_env: *mut c_void, _class: *mut c_void, text: *mut c_void) -> i32 {
    let Some(s)=session() else { return 0; };
    if !text.is_null() {
        if let Some(result)=s.with_string_units(text,crate::integers::parse) {
            if let Some(value)=result { return value; }
        } else { return 0; }
    }
    s.set_pending("java/lang/NumberFormatException","invalid or out-of-range decimal integer");
    0
}
pub extern "C" fn integer_value(env: *mut c_void, object: *mut c_void) -> i32 {
    with_boxed_field(env, object, "I", |field| get_int_field(env, object, field))
}
pub extern "C" fn long_value(env: *mut c_void, object: *mut c_void) -> i64 {
    with_boxed_field(env, object, "J", |field| get_long_field(env, object, field))
}
pub extern "C" fn boolean_init(env: *mut c_void, object: *mut c_void, value: u8) {
    with_boxed_field(env, object, "Z", |field| set_boolean_field(env, object, field, value));
}
pub extern "C" fn boolean_value(env: *mut c_void, object: *mut c_void) -> u8 {
    with_boxed_field(env, object, "Z", |field| get_boolean_field(env, object, field))
}
pub extern "C" fn reflected_declaring_name(env:*mut c_void,object:*mut c_void)->*mut c_void{let class=reflected_declaring_class(env,object);let name=class_name_string(env,class);delete_local_ref(env,class);name}

pub extern "C" fn builder_init(_env: *mut c_void, object: *mut c_void) {
    let Some(s) = session() else { return; };
    if let Some(RefTarget::Object(id)) = s.resolve(object) { s.objects.lock().unwrap().get_mut(&id).unwrap().builder = Some(Vec::new()); }
}

pub unsafe extern "C" fn builder_append(env: *mut c_void, object: *mut c_void, string: *mut c_void) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    let Some(RefTarget::Object(id)) = s.resolve(object) else { return std::ptr::null_mut(); };
    let chars = if string.is_null() { std::ptr::null() } else {
        let chars = get_string_utf_chars(env, string, std::ptr::null_mut());
        if chars.is_null() { return std::ptr::null_mut(); }
        chars
    };
    let bytes = if chars.is_null() { b"null".as_slice() } else { CStr::from_ptr(chars).to_bytes() };
    let result = {
        let mut objects = s.objects.lock().unwrap();
        match objects.get_mut(&id).and_then(|o| o.builder.as_mut()) {
            None => Err(("java/lang/IllegalStateException", "uninitialized StringBuilder")),
            Some(buffer) => {
                if buffer.try_reserve(bytes.len()).is_err() { Err(("java/lang/OutOfMemoryError", "StringBuilder growth failed")) }
                else { buffer.extend_from_slice(bytes); Ok(()) }
            }
        }
    };
    if !chars.is_null() { release_string_utf_chars(env, string, chars); }
    if let Err((class, message)) = result {
        s.set_pending(class, message);
        return std::ptr::null_mut();
    }
    s.new_ref(RefTarget::Object(id), false)
}

pub extern "C" fn builder_string(_env: *mut c_void, object: *mut c_void) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    let Some(RefTarget::Object(id)) = s.resolve(object) else { return std::ptr::null_mut(); };
    let result = {
        let objects = s.objects.lock().unwrap();
        match objects.get(&id).and_then(|o| o.builder.as_ref()) {
            None => Err(("java/lang/IllegalStateException", "uninitialized StringBuilder")),
            Some(text) => {
                let mut bytes = Vec::new();
                if bytes.try_reserve_exact(text.len().saturating_add(1)).is_err() {
                    Err(("java/lang/OutOfMemoryError", "StringBuilder snapshot failed"))
                } else {
                    bytes.extend_from_slice(text); bytes.push(0); Ok(bytes.into_boxed_slice())
                }
            }
        }
    };
    match result {
        Ok(bytes) => s.new_string_owned(bytes),
        Err((class, message)) => { s.set_pending(class, message); std::ptr::null_mut() }
    }
}

#[no_mangle]
pub unsafe extern "C" fn rudroid_jni_argument_kinds(method: *mut c_void, out: *mut u8, capacity: usize) -> c_int {
    let Some(s) = session() else { return -1; };
    if s.blocked_by_pending("JNI variadic arguments") { return -1; }
    let (tag, id) = handle_parts(method);
    if tag != TAG_METHOD || out.is_null() {
        s.set_pending("java/lang/NoSuchMethodError", "invalid JNI variadic method ID");
        return -1;
    }
    let methods = s.methods.lock().unwrap();
    let Some(method) = methods.by_id.get(&id) else {
        drop(methods);
        s.set_pending("java/lang/NoSuchMethodError", "unknown JNI variadic method ID");
        return -1;
    };
    let bytes = method.sig.as_bytes();
    let mut cursor = 1;
    let mut count = 0;
    let result = (|| {
        if bytes.first() != Some(&b'(') { return None; }
        while *bytes.get(cursor)? != b')' {
            if count >= capacity { return None; }
            let mut kind = *bytes.get(cursor)?;
            while *bytes.get(cursor)? == b'[' { kind = b'L'; cursor += 1; }
            match *bytes.get(cursor)? {
                b'L' => {
                    kind = b'L';
                    cursor += 1;
                    let begin = cursor;
                    while *bytes.get(cursor)? != b';' { cursor += 1; }
                    if cursor == begin { return None; }
                }
                b'Z' | b'B' | b'C' | b'S' | b'I' | b'J' | b'F' | b'D' => {}
                _ => return None,
            }
            cursor += 1;
            // Caller provides capacity writable bytes; the method descriptor
            // is borrowed under the method lock, never copied or reallocated.
            *out.add(count) = kind;
            count += 1;
        }
        Some(count as c_int)
    })();
    drop(methods);
    result.unwrap_or_else(|| {
        s.set_pending("java/lang/IllegalArgumentException", "invalid or oversized JNI argument descriptor");
        -1
    })
}

pub extern "C" fn class_loader(_env: *mut c_void, _class: *mut c_void) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    if let Some(&handle) = s.class_loader.get() {
        return s.promote_reference(handle as *mut c_void, false);
    }
    let Some(class) = s.find_class("java/lang/ClassLoader") else { return std::ptr::null_mut() };
    let local = s.alloc_object(class);
    let global = s.promote_reference(local, true);
    s.delete_ref(local, false, "ClassLoader");
    s.delete_ref(class, false, "ClassLoader");
    if global.is_null() { return global; }
    let handle = *s.class_loader.get_or_init(|| global as usize) as *mut c_void;
    if handle != global { s.delete_ref(global, true, "ClassLoader"); }
    s.promote_reference(handle, false)
}

pub extern "C" fn class_name_string(_env: *mut c_void, class: *mut c_void) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    let Some(id) = s.resolve_class(class, "Class.getName") else { return std::ptr::null_mut(); };
    s.new_string(s.class_name(id).replace('/', ".").as_bytes())
}
pub extern "C" fn class_is_array(_env:*mut c_void,class:*mut c_void)->u8{
    let Some(s)=session() else{return 0;};let Some(id)=s.resolve_class(class,"Class.isArray") else{return 0;};u8::from(s.class_name(id).starts_with('['))
}

pub extern "C" fn load_class(env: *mut c_void, _loader: *mut c_void, name: *mut c_void) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    let _ = env;
    let Some(class_name) = s.with_string_bytes(name, |bytes| String::from_utf8_lossy(bytes).replace('.', "/")) else {
        return std::ptr::null_mut();
    };
    s.lookup_class(&class_name, true).unwrap_or(std::ptr::null_mut())
}

pub extern "C" fn class_for_name(env: *mut c_void, class: *mut c_void, name: *mut c_void,
    _initialize: u8, _loader: *mut c_void) -> *mut c_void {
    load_class(env, class, name)
}

pub extern "C" fn string_equals(_env:*mut c_void,string:*mut c_void,other:*mut c_void)->u8 {
    let Some(s)=session() else{return 0;};let(Some(RefTarget::Str(a)),Some(RefTarget::Str(b)))=(s.resolve(string),s.resolve(other))else{return 0;};
    let strings=s.strings.lock().unwrap();u8::from(strings.get(&a).zip(strings.get(&b)).is_some_and(|(a,b)|**a==**b))
}
pub extern "C" fn string_empty(_env:*mut c_void,string:*mut c_void)->u8 {session().and_then(|s|s.string_length(string)).map(|length|u8::from(length==0)).unwrap_or(0)}
pub extern "C" fn object_equals(env:*mut c_void,object:*mut c_void,other:*mut c_void)->u8 {is_same_object(env,object,other)}

pub extern "C" fn object_to_string(_env: *mut c_void, object: *mut c_void) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    let root = s.root_reference(object);
    match root.as_ref().map(|root| root.target) {
        Some(RefTarget::Str(id)) => s.new_ref(RefTarget::Str(id), false),
        Some(RefTarget::Throwable(id)) => {
            let text = s.throwables.lock().unwrap().get(&id).map(|e| format!("{}: {}", e.class, e.message));
            text.map(|t| s.new_string(t.as_bytes())).unwrap_or(std::ptr::null_mut())
        }
        Some(RefTarget::Class(id)) => s.new_string(s.class_name(id).as_bytes()),
        Some(RefTarget::ReflectedMethod(id)) => { let name=s.methods.lock().unwrap().by_id[&id].name.clone(); s.new_string(name.as_bytes()) },
        Some(RefTarget::ReflectedField(_))=>field_name(_env,object),
        Some(RefTarget::Object(id)) => {
            let class = s.objects.lock().unwrap().get(&id).unwrap().class;
            s.new_string(format!("{}@{id:x}", s.class_name(class)).as_bytes())
        }
        Some(RefTarget::Array(id)) => s.new_string(format!("array@{id:x}").as_bytes()),
        None => std::ptr::null_mut(),
    }
}

pub unsafe extern "C" fn call_object_method_a(env: *mut c_void, object: *mut c_void,
    method: *mut c_void, args: *const JValue) -> *mut c_void {
    call_object_a(env, object, method, args, Dispatch::Virtual)
}

pub unsafe extern "C" fn call_static_object_method_a(env: *mut c_void, class: *mut c_void,
    method: *mut c_void, args: *const JValue) -> *mut c_void {
    call_object_a(env, class, method, args, Dispatch::Static)
}

pub(super) unsafe fn call_object_a(env: *mut c_void, receiver: *mut c_void, method: *mut c_void,
    args: *const JValue, dispatch: Dispatch) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    match s.call_checked_dispatch(receiver, method, abi::MethodKind::Object, "CallObjectMethodA", dispatch) {
        Some(MethodValue::NativeObject(address, ObjectShape::NoArgs)) => {
            native_call!(address, fn(*mut c_void, *mut c_void) -> *mut c_void; env, receiver)
        }
        Some(MethodValue::NativeObject(address, ObjectShape::Int)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void, *mut c_void, i32) -> *mut c_void; env, receiver, (*args).i)
        }
        Some(MethodValue::NativeObject(address,ObjectShape::ThreeInts)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i32,i32,i32) ->*mut c_void; env,receiver,(*args).i,(*args.add(1)).i,(*args.add(2)).i)}
        Some(MethodValue::NativeObject(address,ObjectShape::TwoInts)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,i32,i32)->*mut c_void;env,receiver,(*args).i,(*args.add(1)).i)}
        Some(MethodValue::NativeObject(address,ObjectShape::ClassStringsBoolean)) if !args.is_null()=>{
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void,u8) ->*mut c_void; env,receiver,(*args).l,(*args.add(1)).l,(*args.add(2)).l,(*args.add(3)).z)
        }
        Some(MethodValue::NativeObject(address,ObjectShape::ObjectLongObject)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i64,*mut c_void) ->*mut c_void; env,receiver,(*args).l,(*args.add(1)).j,(*args.add(2)).l)}
        Some(MethodValue::NativeObject(address,ObjectShape::LongBoolean)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i64,u8) ->*mut c_void; env,receiver,(*args).j,(*args.add(1)).z)}
        Some(MethodValue::NativeObject(address,ObjectShape::LongTwoObjects)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i64,*mut c_void,*mut c_void) ->*mut c_void; env,receiver,(*args).j,(*args.add(1)).l,(*args.add(2)).l)}
        Some(MethodValue::NativeObject(address, ObjectShape::String)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void; env, receiver, (*args).l)
        }
        Some(MethodValue::NativeObject(address, ObjectShape::StringBoolObject)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, u8, *mut c_void) -> *mut c_void; env, receiver, (*args).l, (*args.add(1)).z, (*args.add(2)).l)
        }
        Some(MethodValue::NativeObject(address, ObjectShape::TwoObjects)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void) -> *mut c_void; env,receiver,(*args).l,(*args.add(1)).l)
        }
        Some(MethodValue::NativeObject(address,ObjectShape::ThreeObjects)) if !args.is_null()=>{
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void) ->*mut c_void; env,receiver,(*args).l,(*args.add(1)).l,(*args.add(2)).l)
        }
        Some(MethodValue::NativeObject(address,ObjectShape::StringInt)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32) ->*mut c_void; env,receiver,(*args).l,(*args.add(1)).i)
        }
        Some(MethodValue::NativeObject(address,ObjectShape::StringLong)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,i64)->*mut c_void;env,receiver,(*args).l,(*args.add(1)).j)}
        Some(MethodValue::NativeObject(address,ObjectShape::StringFloat)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,f32)->*mut c_void;env,receiver,(*args).l,(*args.add(1)).f)}
        Some(MethodValue::NativeObject(address,ObjectShape::StringBoolean)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,u8)->*mut c_void;env,receiver,(*args).l,(*args.add(1)).z)}
        Some(MethodValue::NativeObject(address, ObjectShape::LongThreeObjects)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void,*mut c_void,i64,*mut c_void,*mut c_void,*mut c_void) ->*mut c_void; env,receiver,(*args).j,(*args.add(1)).l,(*args.add(2)).l,(*args.add(3)).l)
        }
        Some(MethodValue::NativeObject(address, ObjectShape::LongObject)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void, *mut c_void, i64, *mut c_void) -> *mut c_void; env,receiver,(*args).j,(*args.add(1)).l)
        }
        Some(_) => {
            s.set_pending("java/lang/IllegalArgumentException", "missing native object argument array");
            std::ptr::null_mut()
        }
        None => std::ptr::null_mut(),
    }
}

pub unsafe extern "C" fn call_long_method_a(env:*mut c_void,receiver:*mut c_void,method:*mut c_void,args:*const JValue)->i64{call_long(env,receiver,method,args,Dispatch::Virtual)}
pub unsafe extern "C" fn call_static_long_method_a(env:*mut c_void,receiver:*mut c_void,method:*mut c_void,args:*const JValue)->i64{call_long(env,receiver,method,args,Dispatch::Static)}
pub(super) unsafe fn call_float(env: *mut c_void, receiver: *mut c_void, method: *mut c_void, args: *const JValue, dispatch: Dispatch) -> f32 {
    let Some(s) = session() else { return 0.0; };
    match s.call_checked_dispatch(receiver, method, abi::MethodKind::Float, "CallFloatMethodA", dispatch) {
        Some(MethodValue::NativeFloatNoArgs(address)) => {
            native_call!(address, fn(*mut c_void, *mut c_void) -> f32; env, receiver)
        },
        Some(MethodValue::NativeFloatInt(address)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void,*mut c_void,i32) ->f32; env,receiver,(*args).i)
        },
        Some(MethodValue::NativeFloatTwoInts(address)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void,*mut c_void,i32,i32) ->f32; env,receiver,(*args).i,(*args.add(1)).i)
        },
        Some(MethodValue::NativeFloatObjectFloat(address)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,f32)->f32; env,receiver,(*args).l,(*args.add(1)).f)
        },
        Some(MethodValue::NativeFloatInt(_) | MethodValue::NativeFloatTwoInts(_) | MethodValue::NativeFloatObjectFloat(_)) => {s.set_pending("java/lang/IllegalArgumentException","missing native float argument array");0.0},
        _ => 0.0
    }
}
pub unsafe extern "C" fn call_float_method_a(env: *mut c_void, receiver: *mut c_void, method: *mut c_void, args: *const JValue) -> f32 { call_float(env, receiver, method, args, Dispatch::Virtual) }
pub unsafe extern "C" fn call_static_float_method_a(env: *mut c_void, receiver: *mut c_void, method: *mut c_void, args: *const JValue) -> f32 { call_float(env, receiver, method, args, Dispatch::Static) }
pub(super) unsafe fn call_long(env:*mut c_void,receiver:*mut c_void,method:*mut c_void,args:*const JValue,dispatch:Dispatch)->i64{
    let Some(s)=session()else{return 0;};
    match s.call_checked_dispatch(receiver,method,abi::MethodKind::Long,"CallLongMethodA",dispatch){
        Some(MethodValue::NativeLongNoArgs(address))=>{native_call!(address,fn(*mut c_void,*mut c_void)->i64;env,receiver)},
        Some(MethodValue::NativeLongObjectLong(address)) if !args.is_null()=>{native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,i64)->i64;env,receiver,(*args).l,(*args.add(1)).j)},
        Some(_)=>{s.set_pending("java/lang/IllegalArgumentException","missing native long argument array");0},None=>0
    }
}
pub unsafe extern "C" fn call_int_method_a(env:*mut c_void,object:*mut c_void,method:*mut c_void,args:*const JValue)->i32 {call_int_a(env,object,method,args,Dispatch::Virtual)}
pub(super) unsafe fn call_int_a(env:*mut c_void,receiver:*mut c_void,method:*mut c_void,args:*const JValue,dispatch:Dispatch)->i32 {
    let Some(s)=session() else{return 0;};
    match s.call_checked_dispatch(receiver,method,abi::MethodKind::Int,"CallIntMethodA",dispatch) {
        Some(MethodValue::Int(value))=>value as i32,
        Some(MethodValue::NativeInt(address,IntShape::FourStringsStringArray)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void) ->i32; env,receiver,(*args).l,(*args.add(1)).l,(*args.add(2)).l,(*args.add(3)).l,(*args.add(4)).l)}
        Some(MethodValue::NativeInt(address,IntShape::ThreeReferences)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void) ->i32; env,receiver,(*args).l,(*args.add(1)).l,(*args.add(2)).l)}
        Some(MethodValue::NativeInt(address,IntShape::ThreeInts)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i32,i32,i32) ->i32; env,receiver,(*args).i,(*args.add(1)).i,(*args.add(2)).i)}
        Some(MethodValue::NativeInt(address,IntShape::TwoInts)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i32,i32) ->i32; env,receiver,(*args).i,(*args.add(1)).i)}
        Some(MethodValue::NativeInt(address,IntShape::Int)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,i32) ->i32; env,receiver,(*args).i)}
        Some(MethodValue::NativeInt(address,IntShape::NoArgs))=>{native_call!(address, fn(*mut c_void,*mut c_void) ->i32; env,receiver)}
        Some(MethodValue::NativeInt(address,IntShape::Reference)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void) ->i32; env,receiver,(*args).l)}
        Some(MethodValue::NativeInt(address,IntShape::ReferenceInt)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32) ->i32; env,receiver,(*args).l,(*args.add(1)).i)}
        Some(MethodValue::NativeInt(address,IntShape::ReferenceIntInt)) if !args.is_null()=>{native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32,i32) ->i32; env,receiver,(*args).l,(*args.add(1)).i,(*args.add(2)).i)}
        Some(_)=>{s.set_pending("java/lang/IllegalArgumentException","missing int method arguments");0},None=>0,
    }
}

pub(super) fn reflection_strings(s: &Session, name: *mut c_void, signature: *mut c_void) -> Option<(String, String)> {
    let name = s.with_string_bytes(name, |bytes| std::str::from_utf8(bytes).map(str::to_owned))?;
    let signature = s.with_string_bytes(signature, |bytes| std::str::from_utf8(bytes).map(str::to_owned))?;
    match (name, signature) {
        (Ok(name), Ok(signature)) => Some((name, signature)),
        _ => { s.set_pending("java/lang/IllegalArgumentException", "invalid reflection descriptor"); None }
    }
}
pub extern "C" fn identity_hash(_env: *mut c_void, _class: *mut c_void, object: *mut c_void) -> i32 {
    let Some(s) = session() else { return 0; };
    if object.is_null() { return 0; }
    let Some(target) = s.resolve(object) else { s.set_pending("java/lang/IllegalArgumentException", "invalid identityHashCode reference"); return 0; };
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    target.hash(&mut hash); let hash = hash.finish(); (hash ^ (hash >> 32)) as i32
}
pub extern "C" fn lookup_reflected_method(env: *mut c_void, _registry: *mut c_void, class: *mut c_void, name: *mut c_void, signature: *mut c_void, is_static: u8) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    if s.blocked_by_pending("reflectMethod") { return std::ptr::null_mut(); }
    let Some((name, signature)) = reflection_strings(s, name, signature) else { return std::ptr::null_mut(); };
    let Some(id) = s.reflected_method_id(class, &name, &signature) else { return std::ptr::null_mut(); };
    to_reflected_method(env, class, id, is_static)
}
pub extern "C" fn to_reflected_method(_env:*mut c_void,class:*mut c_void,method:*mut c_void,_static:u8)->*mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    if s.blocked_by_pending("ToReflectedMethod") { return std::ptr::null_mut(); }
    let Some(class) = s.resolve_class(class,"ToReflectedMethod") else { return std::ptr::null_mut(); };
    let (tag,id) = handle_parts(method);
    let declaring = s.methods.lock().unwrap().by_id.get(&id).filter(|_|tag==TAG_METHOD).map(|m|m.class);
    if !declaring.is_some_and(|declaring|s.is_subclass(class,declaring)) { s.set_pending("java/lang/IllegalArgumentException","invalid reflected method"); return std::ptr::null_mut(); }
    s.new_ref(RefTarget::ReflectedMethod(id),false)
}
pub extern "C" fn from_reflected_method(_env:*mut c_void,object:*mut c_void)->*mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    if s.blocked_by_pending("FromReflectedMethod") { return std::ptr::null_mut(); }
    match s.resolve(object) { Some(RefTarget::ReflectedMethod(id)) => make_handle(TAG_METHOD,id), _=>{ s.set_pending("java/lang/IllegalArgumentException","not a reflected method"); std::ptr::null_mut() } }
}
pub extern "C" fn reflected_method_name(_env:*mut c_void,object:*mut c_void)->*mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    let Some(RefTarget::ReflectedMethod(id)) = s.resolve(object) else { return std::ptr::null_mut(); };
    let name = s.methods.lock().unwrap().by_id[&id].name.clone(); s.new_string(name.as_bytes())
}
pub extern "C" fn reflected_declaring_class(_env:*mut c_void,object:*mut c_void)->*mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    let Some(RefTarget::ReflectedMethod(id)) = s.resolve(object) else { return std::ptr::null_mut(); };
    let class = s.methods.lock().unwrap().by_id[&id].class; s.new_ref(RefTarget::Class(class),false)
}

// ---- JNIInvokeInterface (JavaVM) ---------------------------------

pub extern "C" fn destroy_java_vm(_vm: *mut c_void) -> c_int {
    // Minimal session: nothing to tear down while guests hold handles;
    // the reference returns JNI_OK from its DestroyJavaVM too.
    abi::JNI_OK
}

pub unsafe extern "C" fn attach_current_thread(
    _vm: *mut c_void,
    penv: *mut *mut c_void,
    _args: *mut c_void,
) -> c_int {
    attach_shared(_vm, penv)
}

pub unsafe extern "C" fn attach_current_thread_as_daemon(
    _vm: *mut c_void,
    penv: *mut *mut c_void,
    _args: *mut c_void,
) -> c_int {
    attach_shared(_vm, penv)
}

pub extern "C" fn detach_current_thread(_vm: *mut c_void) -> c_int {
    // Ticket 88: drop the calling thread's locals and pending exception.
    let Some(session) = session() else {
        return abi::JNI_OK;
    };
    session.detach_current_thread()
}

pub unsafe extern "C" fn get_env(
    _vm: *mut c_void,
    penv: *mut *mut c_void,
    version: c_int,
) -> c_int {
    if penv.is_null() {
        return abi::JNI_ERR;
    }
    // SAFETY: the C ABI requires a writable output pointer. Clear it
    // before any failure, including a missing session.
    unsafe { *penv = std::ptr::null_mut() };
    let Some(session) = session() else {
        rudroid_common::error!(
            "[android_jni_vm] GetEnv: no session registered — guest \
             JNI_OnLoad cannot obtain a JNIEnv (ticket 52 minimum missing)"
        );
        return abi::JNI_ERR;
    };
    let mut env = std::ptr::null_mut();
    let result = if !matches!(
        version,
        abi::JNI_VERSION_1_1 | abi::JNI_VERSION_1_2 | abi::JNI_VERSION_1_4 | abi::JNI_VERSION_1_6
    ) {
        session.log(&format!(
            "GetEnv: unsupported JNI version {version:#x} — JNI_EVERSION"
        ));
        abi::JNI_EVERSION
    } else if !session.current_thread_is_attached() {
        abi::JNI_EDETACHED
    } else {
        env = abi::env_ptr();
        // SAFETY: non-null caller pointer, documented C ABI contract.
        unsafe { *penv = env };
        abi::JNI_OK
    };
    if session.trace_calls.load(Ordering::Relaxed) {
        session.log(&format!("JNI_VM op=GetEnv thread={:?} vm_same_shared={} rc={result} env_present={}",
            std::thread::current().id(), _vm == abi::java_vm_ptr(), !env.is_null()));
    }
    result
}

unsafe fn attach_shared(vm: *mut c_void, penv: *mut *mut c_void) -> c_int {
    if penv.is_null() {
        return abi::JNI_ERR;
    }
    // SAFETY: non-null caller output pointer per the C ABI.
    unsafe { *penv = std::ptr::null_mut() };
    let Some(session) = session() else {
        rudroid_common::error!("[android_jni_vm] AttachCurrentThread: no session registered");
        return abi::JNI_ERR;
    };
    let attached = session.attach_current_thread();
    let mut env = std::ptr::null_mut();
    let result = if attached {
        env = abi::env_ptr();
        // Oracle parity (non-GC jnivm): every thread shares the one env.
        // SAFETY: non-null caller output pointer.
        unsafe { *penv = env };
        abi::JNI_OK
    } else {
        abi::JNI_ERR
    };
    if session.trace_calls.load(Ordering::Relaxed) {
        session.log(&format!("JNI_VM op=attach_shared thread={:?} vm_same_shared={} rc={result} env_present={}",
            std::thread::current().id(), vm == abi::java_vm_ptr(), !env.is_null()));
    }
    result
}

// ---- JNINativeInterface (JNIEnv) ---------------------------------

pub extern "C" fn get_version(_env: *mut c_void) -> c_int {
    abi::JNI_VERSION_1_6
}

pub unsafe extern "C" fn find_class(env: *mut c_void, name: *const c_char) -> *mut c_void {
    let Some(session) = session() else {
        return std::ptr::null_mut();
    };
    let _ = env;
    let name = if name.is_null() { "" } else {
        match CStr::from_ptr(name).to_str() { Ok(name)=>name,Err(_)=>{session.set_pending("java/lang/NoClassDefFoundError","invalid class name encoding");return std::ptr::null_mut();} }
    };
    session.lookup_class(name, false).unwrap_or(std::ptr::null_mut())
}

pub extern "C" fn throw(env: *mut c_void, obj: *mut c_void) -> c_int {
    let Some(session) = session() else {
        return abi::JNI_ERR;
    };
    let _ = env;
    if obj.is_null() {
        session.log("Throw: null throwable");
        return abi::JNI_ERR;
    }
    session.throw(obj)
}

pub unsafe extern "C" fn throw_new(
    env: *mut c_void,
    class: *mut c_void,
    message: *const c_char,
) -> c_int {
    let Some(session) = session() else {
        return abi::JNI_ERR;
    };
    let _ = env;
    let Some(class_id) = session.resolve_class(class, "ThrowNew") else {
        return abi::JNI_ERR;
    };
    let message = if message.is_null() {
        std::borrow::Cow::Borrowed("")
    } else {
        // SAFETY: caller passes a NUL-terminated C string per the ABI.
        unsafe { CStr::from_ptr(message) }.to_string_lossy()
    };
    // ThrowNew names the requested exception class, not a generic Throwable:
    // callers may inspect its runtime class before clearing/rethrowing it.
    session.set_pending(&session.class_name(class_id), &message);
    abi::JNI_OK
}

pub extern "C" fn exception_occurred(env: *mut c_void) -> *mut c_void {
    let Some(session) = session() else {
        return std::ptr::null_mut();
    };
    let _ = env;
    // Ticket 88: the pending exception is per-thread; the throwable
    // handle is created from the calling thread's own pending state.
    session.exception_occurred()
}

pub extern "C" fn exception_describe(env: *mut c_void) {
    let Some(session) = session() else { return };
    let _ = env;
    if let Some(pending) = session.take_pending() {
        session.error(&format!(
            "ExceptionDescribe: {} ({}) — describing and clearing",
            pending.class, pending.message
        ));
    }
}

pub extern "C" fn exception_clear(env: *mut c_void) {
    let Some(session) = session() else { return };
    let _ = env;
    session.take_pending();
}

pub unsafe extern "C" fn fatal_error(_env: *mut c_void, message: *const c_char) {
    let message = if message.is_null() {
        String::new()
    } else {
        // SAFETY: caller passes a NUL-terminated C string per the ABI.
        unsafe { CStr::from_ptr(message) }.to_string_lossy().into_owned()
    };
    rudroid_common::error!("[android_jni_vm] FatalError: {message} — aborting (JNI spec)");
    std::process::abort();
}

pub extern "C" fn new_global_ref(env: *mut c_void, obj: *mut c_void) -> *mut c_void {
    let Some(session) = session() else {
        return std::ptr::null_mut();
    };
    let _ = env;
    session.promote_reference(obj,true)
}

pub extern "C" fn delete_global_ref(env: *mut c_void, obj: *mut c_void) {
    if let Some(session) = session() {
        let _ = env;
        session.delete_ref(obj, true, "DeleteGlobalRef");
    }
}

pub extern "C" fn delete_local_ref(env: *mut c_void, obj: *mut c_void) {
    if let Some(session) = session() {
        let _ = env;
        session.delete_ref(obj, false, "DeleteLocalRef");
    }
}

pub extern "C" fn is_same_object(
    env: *mut c_void,
    a: *mut c_void,
    b: *mut c_void,
) -> u8 {
    let Some(session) = session() else {
        return abi::JNI_FALSE;
    };
    let _ = env;
    if a.is_null() && b.is_null() {
        return abi::JNI_TRUE;
    }
    match (session.resolve(a), session.resolve(b)) {
        (Some(target_a), Some(target_b)) => {
            if target_a == target_b {
                abi::JNI_TRUE
            } else {
                abi::JNI_FALSE
            }
        }
        (None, None) => abi::JNI_TRUE, // two dead handles compare "same dead"
        _ => abi::JNI_FALSE,
    }
}

pub extern "C" fn new_local_ref(env: *mut c_void, obj: *mut c_void) -> *mut c_void {
    let Some(session) = session() else {
        return std::ptr::null_mut();
    };
    let _ = env;
    session.promote_reference(obj,false)
}

pub extern "C" fn new_weak_global_ref(_env:*mut c_void,object:*mut c_void)->*mut c_void {
    let Some(s)=session() else{return std::ptr::null_mut();};if object.is_null(){return std::ptr::null_mut();}
    let Some(target)=s.resolve(object) else {s.set_pending("java/lang/IllegalArgumentException","invalid weak reference source");return std::ptr::null_mut();};
    let Some(id)=s.next_serial() else{s.serial_exhausted("NewWeakGlobalRef");return std::ptr::null_mut();};
    s.weaks.lock().unwrap().insert(id,target);make_handle(TAG_WEAK,id)
}
pub extern "C" fn delete_weak_global_ref(_env:*mut c_void,object:*mut c_void) {
    let Some(s)=session() else{return;};if object.is_null(){return;}
    let (tag,id)=handle_parts(object);if tag!=TAG_WEAK {s.log("DeleteWeakGlobalRef: invalid weak handle");return;}
    let mut weaks=s.weaks.lock().unwrap();weaks.remove(&id);trim_reference_table(&mut weaks);
}
pub extern "C" fn get_object_ref_type(_env:*mut c_void,object:*mut c_void)->i32 {
    let Some(s)=session() else{return 0;};let(tag,id)=handle_parts(object);
    match tag {TAG_WEAK if s.weaks.lock().unwrap().contains_key(&id)=>3,TAG_GLOBAL if s.resolve(object).is_some()=>2,TAG_LOCAL if s.resolve(object).is_some()=>1,_=>0}
}

pub extern "C" fn push_local_frame(_env: *mut c_void, capacity: c_int) -> c_int {
    let Some(session) = session() else {
        return abi::JNI_ERR;
    };
    session.push_local_frame(capacity)
}

pub extern "C" fn pop_local_frame(_env: *mut c_void, result: *mut c_void) -> *mut c_void {
    let Some(session) = session() else {
        return std::ptr::null_mut();
    };
    session.pop_local_frame(result)
}

pub extern "C" fn ensure_local_capacity(_env: *mut c_void, capacity: c_int) -> c_int {
    let Some(session) = session() else {
        return abi::JNI_ERR;
    };
    session.ensure_local_capacity(capacity)
}

pub unsafe extern "C" fn register_natives(
    _env: *mut c_void,
    class: *mut c_void,
    methods: *const JniNativeMethod,
    count: c_int,
) -> c_int {
    let Some(session) = session() else {
        return abi::JNI_ERR;
    };
    // SAFETY: the guest passes `count` JNINativeMethod entries per the ABI.
    unsafe { session.register_natives(class, methods, count) }
}

pub extern "C" fn unregister_natives(_env: *mut c_void, class: *mut c_void) -> c_int {
    let Some(session) = session() else {
        return abi::JNI_ERR;
    };
    session.unregister_natives(class)
}

pub extern "C" fn monitor_enter(_env: *mut c_void, object: *mut c_void) -> c_int {
    let Some(session) = session() else {
        return abi::JNI_ERR;
    };
    session.monitor_enter(object)
}

pub extern "C" fn monitor_exit(_env: *mut c_void, object: *mut c_void) -> c_int {
    let Some(session) = session() else {
        return abi::JNI_ERR;
    };
    session.monitor_exit(object)
}

pub extern "C" fn alloc_object(_env: *mut c_void, class: *mut c_void) -> *mut c_void {
    session().map(|s| s.alloc_object(class)).unwrap_or(std::ptr::null_mut())
}

pub extern "C" fn set_superclass(_env: *mut c_void, _class: *mut c_void,
    child: *mut c_void, parent: *mut c_void) {
    if let Some(s) = session() { s.set_superclass(child, parent); }
}

pub extern "C" fn proxy_class(_env: *mut c_void, _registry: *mut c_void, base: *mut c_void, interfaces: *mut c_void) -> *mut c_void {
    session().and_then(|s| s.proxy_class(base, interfaces)).unwrap_or(std::ptr::null_mut())
}

pub extern "C" fn get_object_class(_env: *mut c_void, object: *mut c_void) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    if s.blocked_by_pending("GetObjectClass") { return std::ptr::null_mut(); }
    s.object_class(object, "GetObjectClass")
        .map(|id| s.new_ref(super::RefTarget::Class(id), false))
        .unwrap_or(std::ptr::null_mut())
}

pub extern "C" fn get_superclass(_env: *mut c_void, class: *mut c_void) -> *mut c_void {
    let Some(s) = session() else { return std::ptr::null_mut(); };
    if s.blocked_by_pending("GetSuperclass") { return std::ptr::null_mut(); }
    let Some(class) = s.resolve_class(class, "GetSuperclass") else { return std::ptr::null_mut(); };
    let parent = s.classes.read().unwrap().parents.get(&class).copied();
    parent.map(|id| s.new_ref(RefTarget::Class(id), false)).unwrap_or(std::ptr::null_mut())
}

pub extern "C" fn is_instance_of(_env: *mut c_void, object: *mut c_void, class: *mut c_void) -> u8 {
    let Some(s) = session() else { return abi::JNI_FALSE; };
    if s.blocked_by_pending("IsInstanceOf") { return abi::JNI_FALSE; }
    let Some(class) = s.resolve_class(class, "IsInstanceOf") else { return abi::JNI_FALSE; };
    // JNI defines null as assignable to any reference type.
    if object.is_null() { return abi::JNI_TRUE; }
    s.object_class(object, "IsInstanceOf").map(|id| u8::from(s.is_subclass(id, class))).unwrap_or(abi::JNI_FALSE)
}

pub extern "C" fn is_assignable_from(_env: *mut c_void, source: *mut c_void, target: *mut c_void) -> u8 {
    session().map_or(abi::JNI_FALSE, |s| s.is_assignable_from(source, target))
}

pub unsafe extern "C" fn call_void_method_a(env: *mut c_void, object: *mut c_void,
    method: *mut c_void, args: *const super::JValue) {
    call_void_a(env, object, method, args, Dispatch::Virtual);
}

pub unsafe extern "C" fn call_static_void_method_a(env: *mut c_void, class: *mut c_void,
    method: *mut c_void, args: *const super::JValue) {
    call_void_a(env, class, method, args, Dispatch::Static);
}

pub(super) unsafe fn call_void_a(env: *mut c_void, object: *mut c_void,
    method: *mut c_void, args: *const super::JValue, dispatch: Dispatch) {
    let Some(s) = session() else { return; };
    let Some(super::MethodValue::NativeVoid(address, shape)) = s.call_checked_dispatch(
        object, method, abi::MethodKind::Void, if dispatch != Dispatch::Static { "CallVoidMethodA" } else { "CallStaticVoidMethodA" }, dispatch) else { return; };
    if shape != super::VoidShape::NoArgs && args.is_null() {
        s.set_pending("java/lang/IllegalArgumentException", "missing native jvalue argument array");
        return;
    }
    // The descriptor is classified once during RegisterNatives. Each branch
    // uses its exact C prototype; the JNI caller supplies readable jvalues.
    match shape {
        super::VoidShape::NoArgs => {
            native_call!(address, fn(*mut c_void, *mut c_void); env, object);
        }
        super::VoidShape::Boolean => {
            native_call!(address, fn(*mut c_void, *mut c_void, u8); env, object, (*args).z);
        }
        super::VoidShape::Long => {native_call!(address, fn(*mut c_void,*mut c_void,i64); env,object,(*args).j)}
        super::VoidShape::Int => {
            native_call!(address, fn(*mut c_void, *mut c_void, i32); env, object, (*args).i);
        }
        super::VoidShape::Float => {
            native_call!(address, fn(*mut c_void, *mut c_void, f32); env, object, (*args).f);
        }
        super::VoidShape::IntInt => {
            native_call!(address, fn(*mut c_void, *mut c_void, i32, i32); env, object, (*args).i, (*args.add(1)).i);
        }
        super::VoidShape::Context => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void); env, object, (*args).l);
        }
        super::VoidShape::ContextInt => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, i32); env, object, (*args).l, (*args.add(1)).i);
        }
        super::VoidShape::IntSurface => {
            native_call!(address, fn(*mut c_void, *mut c_void, i32, *mut c_void); env, object, (*args).i, (*args.add(1)).l);
        }
        super::VoidShape::FieldDeclaration => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void, *mut c_void, u8); env, object, (*args).l, (*args.add(1)).l, (*args.add(2)).l, (*args.add(3)).z);
        }
        super::VoidShape::TwoReferences => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, *mut c_void); env, object, (*args).l, (*args.add(1)).l);
        }
        super::VoidShape::ThreeReferences => {
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,*mut c_void); env,object,(*args).l,(*args.add(1)).l,(*args.add(2)).l);
        }
        super::VoidShape::IntReference => {
            native_call!(address, fn(*mut c_void,*mut c_void,i32,*mut c_void); env,object,(*args).i,(*args.add(1)).l);
        }
        super::VoidShape::ReferenceTwoInts => {
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32,i32); env,object,(*args).l,(*args.add(1)).i,(*args.add(2)).i);
        }
        super::VoidShape::ReferenceIntReference => {
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32,*mut c_void); env,object,(*args).l,(*args.add(1)).i,(*args.add(2)).l);
        }
        super::VoidShape::ReferenceEightInts => {
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32,i32,i32,i32,i32,i32,i32,i32); env,object,(*args).l,(*args.add(1)).i,(*args.add(2)).i,(*args.add(3)).i,(*args.add(4)).i,(*args.add(5)).i,(*args.add(6)).i,(*args.add(7)).i,(*args.add(8)).i);
        }
        super::VoidShape::LongThreeReferences => {
            native_call!(address, fn(*mut c_void,*mut c_void,i64,*mut c_void,*mut c_void,*mut c_void); env,object,(*args).j,(*args.add(1)).l,(*args.add(2)).l,(*args.add(3)).l);
        }
        super::VoidShape::TwoReferencesTwoBooleansTwoReferencesBooleanReferenceBoolean => {
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,*mut c_void,u8,u8,*mut c_void,*mut c_void,u8,*mut c_void,u8); env,object,(*args).l,(*args.add(1)).l,(*args.add(2)).z,(*args.add(3)).z,(*args.add(4)).l,(*args.add(5)).l,(*args.add(6)).z,(*args.add(7)).l,(*args.add(8)).z);
        }
        super::VoidShape::ReferenceIntFourBooleansReferenceIntTwoBooleans => {
            native_call!(address, fn(*mut c_void,*mut c_void,*mut c_void,i32,u8,u8,u8,u8,*mut c_void,i32,u8,u8); env,object,(*args).l,(*args.add(1)).i,(*args.add(2)).z,(*args.add(3)).z,(*args.add(4)).z,(*args.add(5)).z,(*args.add(6)).l,(*args.add(7)).i,(*args.add(8)).z,(*args.add(9)).z);
        }
    }
}

pub unsafe extern "C" fn call_boolean_method_a(env: *mut c_void, object: *mut c_void,
    method: *mut c_void, args: *const super::JValue) -> u8 {
    call_boolean_a(env, object, method, args, Dispatch::Virtual)
}

pub(super) unsafe fn call_boolean_a(env: *mut c_void, receiver: *mut c_void,
    method: *mut c_void, args: *const super::JValue, dispatch: Dispatch) -> u8 {
    let Some(s) = session() else { return abi::JNI_FALSE; };
    let entry = if dispatch != Dispatch::Static { "CallBooleanMethodA" } else { "CallStaticBooleanMethodA" };
    match s.call_checked_dispatch(receiver, method, abi::MethodKind::Bool, entry, dispatch) {
        Some(super::MethodValue::Bool(value)) => u8::from(value),
        Some(super::MethodValue::NativeBoolNoArgs(address)) => {
            native_call!(address, fn(*mut c_void, *mut c_void) -> u8; env, receiver)
        }
        Some(super::MethodValue::NativeBoolInt(address)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void, *mut c_void, i32) -> u8; env, receiver, (*args).i)
        }
        Some(super::MethodValue::NativeBoolObjectInt(address)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, i32) -> u8; env, receiver, (*args).l, (*args.add(1)).i)
        }
        Some(super::MethodValue::NativeBoolString(address)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void) -> u8; env, receiver, (*args).l)
        }
        Some(super::MethodValue::NativeBoolObjectLong(address)) if !args.is_null() => {
            native_call!(address, fn(*mut c_void, *mut c_void, *mut c_void, i64) -> u8; env, receiver, (*args).l, (*args.add(1)).j)
        }
        Some(super::MethodValue::NativeBoolObjectBoolean(address)) if !args.is_null()=>{
            native_call!(address,fn(*mut c_void,*mut c_void,*mut c_void,u8)->u8;env,receiver,(*args).l,(*args.add(1)).z)
        }
        Some(_) => {
            s.set_pending("java/lang/IllegalArgumentException", "missing native jvalue arguments or unsupported boolean signature");
            abi::JNI_FALSE
        }
        None => abi::JNI_FALSE,
    }
}

pub unsafe extern "C" fn get_method_id(
    env: *mut c_void,
    class: *mut c_void,
    name: *const c_char,
    sig: *const c_char,
) -> *mut c_void {
    resolve_method_shim(env, class, name, sig, "GetMethodID")
}

pub unsafe extern "C" fn get_static_method_id(
    env: *mut c_void,
    class: *mut c_void,
    name: *const c_char,
    sig: *const c_char,
) -> *mut c_void {
    resolve_method_shim(env, class, name, sig, "GetStaticMethodID")
}

unsafe fn resolve_method_shim(
    env: *mut c_void,
    class: *mut c_void,
    name: *const c_char,
    sig: *const c_char,
    entry: &'static str,
) -> *mut c_void {
    let Some(session) = session() else {
        return std::ptr::null_mut();
    };
    let _ = env;
    let read = |p: *const c_char| {
        if p.is_null() {
            return None;
        }
        // SAFETY: caller passes a NUL-terminated C string per the ABI.
        Some(unsafe { CStr::from_ptr(p) }.to_string_lossy())
    };
    let (Some(name), Some(sig)) = (read(name), read(sig)) else {
        session.log(&format!("{entry}: method name or signature is null"));
        session.set_pending("java/lang/NoSuchMethodError", "null name or signature");
        return std::ptr::null_mut();
    };
    // This exact native handoff may stop before call dispatch. Observe only
    // the class identity and null/exception state, never proxy arguments.
    let proxy_lookup = (session.trace_calls.load(Ordering::Relaxed)
        && name == "newInterfaceProxy"
        && sig == "(J[Ljava/lang/Class;)Ljava/lang/Object;").then(|| {
            let owner = match session.resolve(class) {
                Some(RefTarget::Class(id)) => session.class_name(id),
                _ => Arc::from("<unresolved>"),
            };
            (owner, session.with_thread(|state| state.pending.is_some()))
        });
    let result = session
        .resolve_method(class, &name, &sig, entry)
        .unwrap_or(std::ptr::null_mut());
    if let Some((owner, pending_before)) = proxy_lookup {
        session.log(&format!("JNI_PROXY_LOOKUP entry={entry} owner={owner} class_present={} method_present={} pending_before={pending_before} pending_after={}",
            !class.is_null(), !result.is_null(), session.with_thread(|state| state.pending.is_some())));
    }
    result
}

pub unsafe extern "C" fn call_static_int_method_a(env:*mut c_void,class:*mut c_void,method:*mut c_void,args:*const JValue)->i32 {call_int_a(env,class,method,args,Dispatch::Static)}

pub unsafe extern "C" fn call_static_boolean_method_a(
    env: *mut c_void, class: *mut c_void, method: *mut c_void, args: *mut c_void,
) -> u8 {
    call_boolean_a(env, class, method, args.cast(), Dispatch::Static)
}

pub unsafe extern "C" fn new_string_utf(
    env: *mut c_void,
    bytes: *const c_char,
) -> *mut c_void {
    let Some(session) = session() else {
        return std::ptr::null_mut();
    };
    let _ = env;
    if bytes.is_null() {
        session.log("NewStringUTF: null input");
        session.set_pending("java/lang/NullPointerException", "null UTF-8 input");
        return std::ptr::null_mut();
    }
    // SAFETY: caller passes a NUL-terminated C string per the ABI.
    let bytes = unsafe { CStr::from_ptr(bytes) }.to_bytes();
    session.new_string(bytes)
}

pub extern "C" fn get_string_utf_length(env: *mut c_void, string: *mut c_void) -> c_int {
    let Some(session) = session() else {
        return 0;
    };
    let _ = env;
    session
        .string_length(string)
        .map(|len| len as c_int)
        .unwrap_or(0)
}

pub unsafe extern "C" fn get_string_utf_chars(
    env: *mut c_void,
    string: *mut c_void,
    is_copy: *mut u8,
) -> *const c_char {
    let Some(session) = session() else {
        return std::ptr::null();
    };
    let _ = env;
    let chars = session.borrow_string_chars(string);
    if !chars.is_null() && !is_copy.is_null() {
        // SAFETY: caller-provided out-param, non-null checked above.
        unsafe { *is_copy = abi::JNI_FALSE };
    }
    chars
}

/// # Safety
/// Release only an owned, outstanding JNI string borrow, with no users
/// of that borrow remaining. A matching pointer alone does not own a lease.
pub unsafe extern "C" fn release_string_utf_chars(
    env: *mut c_void,
    string: *mut c_void,
    chars: *const c_char,
) {
    let Some(session) = session() else { return };
    let _ = env;
    // A null string/chars pair is logged as a misuse.
    if string.is_null() || chars.is_null() {
        session.log("ReleaseStringUTFChars: null string or chars pointer");
        return;
    }
    session.release_string_chars(chars);
}

pub unsafe extern "C" fn get_java_vm(env: *mut c_void, vm: *mut *mut c_void) -> c_int {
    let Some(session) = session() else {
        return abi::JNI_ERR;
    };
    let _ = env;
    if vm.is_null() {
        session.log("GetJavaVM: vm out-param is null");
        return abi::JNI_ERR;
    }
    // SAFETY: non-null caller pointer.
    unsafe { *vm = abi::java_vm_ptr() };
    abi::JNI_OK
}

pub extern "C" fn exception_check(env: *mut c_void) -> u8 {
    let Some(session) = session() else {
        return abi::JNI_FALSE;
    };
    let _ = env;
    let pending = session.with_thread(|s| s.pending.is_some());
    if pending && session.trace_calls.load(Ordering::Relaxed) {
        session.log(&format!("JNI_EXCEPTION_CHECK thread={:?} pending=true", std::thread::current().id()));
    }
    if pending {
        abi::JNI_TRUE
    } else {
        abi::JNI_FALSE
    }
}
