//! INPUT: Unity 2021/6 Java calls, copied Host API and engine-registered JNI bridges
//! OUTPUT: JNI-owned interface proxies, host connectivity/proxy queries and local asset-delivery availability
//! POS: Unity-specific mock semantics; all objects and fields live in the shared VM
#![forbid(unsafe_code)]
use rudroid_common::jni_abi::declare_field;
use rudroid_common::jni_env::{Arg, JObject, JniEnv, Native};
use std::ffi::{c_void, CStr};
use std::sync::OnceLock;

const PROXY: &std::ffi::CStr = c"org/rudroid/unity/InterfaceProxy";
static HOST: OnceLock<rudroid_common::host_api::HostApiV0> = OnceLock::new();
static WRAPPER: OnceLock<usize> = OnceLock::new();
extern "C" fn empty(_env: JniEnv, _object: JObject) {}
extern "C" fn no_url(_env: JniEnv, _player: JObject) -> JObject {
    std::ptr::null_mut()
}
extern "C" fn unavailable(_env: JniEnv, _object: JObject) -> u8 {
    eprintln!("[PAD] local installed-pack API queried");
    // This wrapper implements installed-file queries locally. Network delivery
    // remains unavailable; no download or ownership success is synthesized.
    0
}
fn allocate(env: JniEnv, name: &std::ffi::CStr) -> *mut c_void {
    let class = env.find_class(name);
    let object = env.alloc_object(class);
    env.delete_local_ref(class);
    object
}
extern "C" fn proxy(
    env: JniEnv,
    _bridge: JObject,
    handle: i64,
    interfaces: JObject,
) -> JObject {
    let base = env.find_class(PROXY);
    let registry = env.find_class(c"org/rudroid/runtime/TypeRegistry");
    let method = env.static_method(
        registry,
        c"proxyClass",
        c"(Ljava/lang/Class;[Ljava/lang/Class;)Ljava/lang/Class;",
    );
    let class = method.map_or(std::ptr::null_mut(), |method| {
        env.call_static_object(registry, method, &[Arg::object(base), Arg::object(interfaces)])
    });
    env.delete_local_ref(base);
    env.delete_local_ref(registry);
    if class.is_null() {
        return std::ptr::null_mut();
    }
    let object = env.alloc_object(class);
    env.set_long_field(object, env.get_field_id(class, c"nativeHandle", c"J"), handle);
    env.set_object_field(
        object,
        env.get_field_id(class, c"interfaces", c"[Ljava/lang/Class;"),
        interfaces,
    );
    env.delete_local_ref(class);
    object
}
fn invoke_proxy(
    env: JniEnv,
    object: *mut c_void,
    interface_name: &std::ffi::CStr,
    name: &std::ffi::CStr,
    signature: &std::ffi::CStr,
    argument: Option<*mut c_void>,
) -> *mut c_void {
    let class = env.find_class(PROXY);
    let id = env.get_field_id(class, c"nativeHandle", c"J");
    let handle = env.get_long_field(object, id);
    let reflection = env.get_int_field(object, env.get_field_id(class, c"invokeKind", c"I")) != 0;
    let interface = env.find_class(interface_name);
    if handle == 0 || !env.is_instance_of(object, interface) {
        env.delete_local_ref(class);
        env.delete_local_ref(interface);
        return std::ptr::null_mut();
    }
    let reflected = if reflection {
        std::ptr::null_mut()
    } else {
        let method = env.get_method_id(interface, name, signature);
        env.to_reflected_method(interface, method, false)
    };
    let base = env.find_class(c"java/lang/Object");
    let arguments = env.new_object_array(
        i32::from(argument.is_some()),
        base,
        argument.unwrap_or(std::ptr::null_mut()),
    );
    if reflection {
        let bridge = env.find_class(c"com/unity3d/player/ReflectionHelper");
        let name = env.new_string_utf(name);
        let invoke = env.static_method(
            bridge,
            c"nativeProxyInvoke",
            c"(JLjava/lang/String;[Ljava/lang/Object;)Ljava/lang/Object;",
        );
        let result = invoke.map_or(std::ptr::null_mut(), |invoke| {
            env.call_static_object(
                bridge,
                invoke,
                &[Arg::long(handle), Arg::object(name), Arg::object(arguments)],
            )
        });
        for local in [class, interface, base, arguments, bridge, name] {
            env.delete_local_ref(local);
        }
        return result;
    }
    let bridge = env.find_class(c"bitter/jnibridge/JNIBridge");
    let invoke = env.static_method(
        bridge,
        c"invoke",
        c"(JLjava/lang/Class;Ljava/lang/reflect/Method;[Ljava/lang/Object;)Ljava/lang/Object;",
    );
    let result = match invoke {
        Some(invoke) if !reflected.is_null() => env.call_static_object(
            bridge,
            invoke,
            &[
                Arg::long(handle),
                Arg::object(interface),
                Arg::object(reflected),
                Arg::object(arguments),
            ],
        ),
        _ => std::ptr::null_mut(),
    };
    for local in [class, interface, reflected, base, arguments, bridge] {
        if !local.is_null() {
            env.delete_local_ref(local);
        }
    }
    result
}
extern "C" fn proxy_run(env: JniEnv, object: JObject) {
    let result = invoke_proxy(env, object, c"java/lang/Runnable", c"run", c"()V", None);
    if !result.is_null() {
        env.delete_local_ref(result);
    }
}
extern "C" fn proxy_message(
    env: JniEnv,
    object: JObject,
    message: JObject,
) -> u8 {
    let result = invoke_proxy(
        env,
        object,
        c"android/os/Handler$Callback",
        c"handleMessage",
        c"(Landroid/os/Message;)Z",
        Some(message),
    );
    if result.is_null() {
        return 0;
    }
    let class = env.get_object_class(result);
    let value = env
        .method(class, c"booleanValue", c"()Z")
        .map_or(0, |method| u8::from(env.call_boolean(result, method, &[])));
    env.delete_local_ref(class);
    env.delete_local_ref(result);
    value
}
extern "C" fn disable_proxy(env: JniEnv, _class: JObject, object: JObject) {
    if object.is_null() {
        return;
    }
    let class = env.find_class(PROXY);
    if env.is_instance_of(object, class) {
        env.set_long_field(object, env.get_field_id(class, c"nativeHandle", c"J"), 0);
    }
    env.delete_local_ref(class);
}
extern "C" fn constructor(
    env: JniEnv,
    _helper: JObject,
    class: JObject,
    signature: JObject,
) -> JObject {
    let Some(method) = env.with_string_utf(signature, |descriptor| env.get_method_id(class, c"<init>", descriptor)) else {
        return std::ptr::null_mut();
    };
    if method.is_null() {
        std::ptr::null_mut()
    } else {
        env.to_reflected_method(class, method, false)
    }
}
extern "C" fn reflected_method(
    env: JniEnv,
    _helper: JObject,
    class: JObject,
    name: JObject,
    signature: JObject,
    is_static: u8,
) -> JObject {
    reflect_lookup(env, c"reflectMethod", c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Method;", class, name, signature, is_static)
}
/// `descriptor` takes (Class, String, String, boolean), the four arguments passed.
fn reflect_lookup(env: JniEnv, name_key: &CStr, descriptor: &CStr, class: JObject, name: JObject, signature: JObject, is_static: u8) -> JObject {
    let registry = env.find_class(c"org/rudroid/runtime/TypeRegistry");
    let result = env.static_method(registry, name_key, descriptor).map_or(std::ptr::null_mut(), |method| env.call_static_object(registry, method, &[Arg::object(class), Arg::object(name), Arg::object(signature), Arg::bool(is_static != 0)]));
    env.delete_local_ref(registry); result
}

extern "C" fn proxy_frame(env: JniEnv, object: JObject, nanos: i64) {
    let class = env.find_class(c"java/lang/Long");
    let boxed = env
        .method(class, c"<init>", c"(J)V")
        .map_or(std::ptr::null_mut(), |ctor| env.new_object(class, ctor, &[Arg::long(nanos)]));
    if !boxed.is_null() {
        let result = invoke_proxy(
            env,
            object,
            c"android/view/Choreographer$FrameCallback",
            c"doFrame",
            c"(J)V",
            Some(boxed),
        );
        if !result.is_null() {
            env.delete_local_ref(result);
        }
        env.delete_local_ref(boxed);
    }
    env.delete_local_ref(class);
}
extern "C" fn reflected_field(env:JniEnv,_helper:JObject,class:JObject,name:JObject,signature:JObject,is_static:u8)->JObject{
    reflect_lookup(env, c"reflectField", c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Field;", class, name, signature, is_static)
}

extern "C" fn field_signature(env: JniEnv, _helper: JObject, field: JObject) -> JObject {
    let registry = env.find_class(c"org/rudroid/runtime/TypeRegistry");
    let result = env
        .static_method(registry, c"fieldSignature", c"(Ljava/lang/reflect/Field;)Ljava/lang/String;")
        .map_or(std::ptr::null_mut(), |method| env.call_static_object(registry, method, &[Arg::object(field)]));
    env.delete_local_ref(registry);
    result
}
extern "C" fn reflection_proxy_array(env:JniEnv,_helper:JObject,_player:JObject,handle:i64,interfaces:JObject)->JObject{
    let result = proxy(env, std::ptr::null_mut(), handle, interfaces);
    if result.is_null() {
        return result;
    }
    let class = env.find_class(PROXY);
    env.set_int_field(result, env.get_field_id(class, c"invokeKind", c"I"), 1);
    env.delete_local_ref(class);
    result
}
extern "C" fn reflection_proxy(env:JniEnv,helper:JObject,player:JObject,handle:i64,interface:JObject)->JObject{
    if interface.is_null() {
        return std::ptr::null_mut();
    }
    let class = env.find_class(c"java/lang/Class");
    let interfaces = env.new_object_array(1, class, interface);
    let result = reflection_proxy_array(env, helper, player, handle, interfaces);
    env.delete_local_ref(interfaces);
    env.delete_local_ref(class);
    result
}
extern "C" fn invocation_error(env:JniEnv,_helper:JObject,handle:i64,native_exception:u8)->JObject{
    let object = allocate(env, c"com/unity3d/player/ReflectionHelper$InvocationError");
    let class = env.find_class(c"com/unity3d/player/ReflectionHelper$InvocationError");
    env.set_long_field(object, env.get_field_id(class, c"nativeHandle", c"J"), handle);
    env.set_boolean_field(object, env.get_field_id(class, c"nativeException", c"Z"), native_exception);
    env.delete_local_ref(class);
    object
}
extern "C" fn wrapper(
    env: JniEnv,
    _class: JObject,
    _player: JObject,
    _context: JObject,
) -> JObject {
    eprintln!("[PAD] wrapper init");
    let global = *WRAPPER.get_or_init(|| {
        let object = allocate(env, c"com/unity3d/player/PlayAssetDeliveryUnityWrapper");
        if object.is_null() {
            return 0;
        }
        let result = env.new_global_ref(object);
        env.delete_local_ref(object);
        result as usize
    });
    if global == 0 {
        std::ptr::null_mut()
    } else {
        env.new_local_ref(global as *mut c_void)
    }
}

extern "C" fn wrapper_instance(env:JniEnv,class:JObject)->JObject{
    wrapper(env,class,std::ptr::null_mut(),std::ptr::null_mut())
}

extern "C" fn listener_token(env:JniEnv,_object:JObject,listener:JObject)->JObject{
    env.new_local_ref(listener)
}
extern "C" fn discard_listener(_env:JniEnv,_object:JObject,_listener:JObject){}
// Extracted local packs are immutable port inputs. Android's uninstall
// bookkeeping has nothing to remove here, matching the reference host.
extern "C" fn retain_local_pack(_env:JniEnv,_object:JObject,_pack:JObject){}

// Unity's own status listener forwards one native result per pack. This is
// an installed-file query; it neither grants DLC ownership nor downloads data.
extern "C" fn core_pack_status(env:JniEnv,callback:JObject,_bytes:i64,names:JObject,states:JObject,errors:JObject){
    eprintln!("[PAD] core status delivery");
    let class = env.find_class(c"com/unity3d/player/UnityCoreAssetPacksStatusCallbacks");
    let method = env.method(class, c"nativeStatusQueryResult", c"(Ljava/lang/String;II)V");
    let count = env.get_array_length(names).min(env.get_array_length(states)).min(env.get_array_length(errors));
    for i in 0..count {
        let name = env.get_object_array_element(names, i);
        let mut state = 0;
        let mut error = 0;
        env.get_int_array_region(states, i, std::slice::from_mut(&mut state));
        env.get_int_array_region(errors, i, std::slice::from_mut(&mut error));
        if let Some(method) = method {
            env.call_void(callback, method, &[Arg::object(name), Arg::int(state), Arg::int(error)]);
        }
        env.delete_local_ref(name);
    }
    env.delete_local_ref(class);
}
fn pack_states_now(env:JniEnv,wrapper:JObject,names:JObject,callback:JObject){
    use rudroid_common::jni_abi::invoke_proxy_arguments;
    let count = env.get_array_length(names);
    let states = env.new_int_array(count);
    let errors = env.new_int_array(count);
    for i in 0..count {
        let name = env.get_object_array_element(names, i);
        let path = asset_pack_path(env, wrapper, name);
        let installed = java_text(env, path).is_some_and(|p| !p.is_empty());
        let state = if installed { 4 } else { 8 };
        let error = if installed { 0 } else { -2 };
        env.set_int_array_region(states, i, &[state]);
        env.set_int_array_region(errors, i, &[error]);
        env.delete_local_ref(path);
        env.delete_local_ref(name);
    }
    let class = env.find_class(c"java/lang/Long");
    let number = env
        .method(class, c"<init>", c"(J)V")
        .map_or(std::ptr::null_mut(), |ctor| env.new_object(class, ctor, &[Arg::long(0)]));
    invoke_proxy_arguments(env,callback,c"com/unity3d/player/IAssetPackManagerStatusQueryCallback",c"onStatusResult",c"(J[Ljava/lang/String;[I[I)V",&[Arg::long(0),Arg::object(names),Arg::object(states),Arg::object(errors)],&[number,names,states,errors]);
    for h in [number, class, states, errors] {
        env.delete_local_ref(h);
    }
}
const PACK_QUERY:&CStr=c"org/rudroid/unity/AssetPackQuery";
fn pack_query_field(env:JniEnv,name:&CStr)->*mut c_void{
    let class = env.find_class(PACK_QUERY);
    let result = env.get_field_id(class, name, c"Ljava/lang/Object;");
    env.delete_local_ref(class);
    result
}
extern "C" fn pack_query_run(env:JniEnv,job:JObject){
    eprintln!("[PAD] query run");
    let names = env.get_object_field(job, pack_query_field(env, c"names"));
    let callback = env.get_object_field(job, pack_query_field(env, c"callback"));
    pack_states_now(env, std::ptr::null_mut(), names, callback);
    env.delete_local_ref(names);
    env.delete_local_ref(callback);
}
extern "C" fn pack_states(env:JniEnv,_wrapper:JObject,names:JObject,callback:JObject){
    eprintln!("[PAD] query enqueue");
    let job = allocate(env, PACK_QUERY);
    env.set_object_field(job, pack_query_field(env, c"names"), names);
    env.set_object_field(job, pack_query_field(env, c"callback"), callback);
    rudroid_common::jni_abi::post_main_runnable(env, job);
    env.delete_local_ref(job);
}
extern "C" fn pack_state(env:JniEnv,wrapper:JObject,name:JObject,callback:JObject){
    let class = env.find_class(c"java/lang/String");
    let names = env.new_object_array(1, class, name);
    pack_states(env, wrapper, names, callback);
    env.delete_local_ref(names);
    env.delete_local_ref(class);
}

/// Strict UTF-16 decode: an unpaired surrogate yields `None`.
fn java_text(env:JniEnv,string:JObject)->Option<String>{
    env.string_utf16(string).and_then(|units| String::from_utf16(&units).ok())
}
extern "C" fn asset_pack_path(env:JniEnv,_wrapper:JObject,pack:JObject)->JObject{
    eprintln!("[PAD] path query");
    let Some(pack)=java_text(env,pack) else{return std::ptr::null_mut();};
    let path=std::ffi::CString::new(pack.as_bytes()).ok().and_then(|name|
        HOST.get()?.asset_path(rudroid_common::android_assets::PACK_ROOT,c"",Some(&name),0).ok().flatten()
    );
    let value=path.as_ref().map(|p|p.to_string_lossy()).unwrap_or_default();
    eprintln!("[PAD] path result: {pack}: {value}");
    let value: Vec<u16> = value.encode_utf16().collect();
    env.new_string(&value)
}

extern "C" fn generic_proxy_invoke(env:JniEnv,_bridge:JObject,object:JObject,reflected:JObject,arguments:JObject)->JObject{
    let class = env.find_class(PROXY);
    if !env.is_instance_of(object, class) {
        env.delete_local_ref(class);
        return std::ptr::null_mut();
    }
    let handle = env.get_long_field(object, env.get_field_id(class, c"nativeHandle", c"J"));
    let reflection = env.get_int_field(object, env.get_field_id(class, c"invokeKind", c"I")) != 0;
    env.delete_local_ref(class);
    if handle == 0 {
        return std::ptr::null_mut();
    }
    let method_class = env.find_class(c"java/lang/reflect/Method");
    let (bridge, method, locals, values) = if reflection {
        let name = env.method(method_class, c"getName", c"()Ljava/lang/String;").map_or(std::ptr::null_mut(), |get_name| env.call_object(reflected, get_name, &[]));
        let bridge = env.find_class(c"com/unity3d/player/ReflectionHelper");
        let method = env.static_method(bridge, c"nativeProxyInvoke", c"(JLjava/lang/String;[Ljava/lang/Object;)Ljava/lang/Object;");
        (bridge,method,name,vec![Arg::long(handle),Arg::object(name),Arg::object(arguments)])
    } else {
        let interface = env.method(method_class, c"getDeclaringClass", c"()Ljava/lang/Class;").map_or(std::ptr::null_mut(), |get_declaring| env.call_object(reflected, get_declaring, &[]));
        let bridge = env.find_class(c"bitter/jnibridge/JNIBridge");
        let method = env.static_method(bridge, c"invoke", c"(JLjava/lang/Class;Ljava/lang/reflect/Method;[Ljava/lang/Object;)Ljava/lang/Object;");
        (bridge,method,interface,vec![Arg::long(handle),Arg::object(interface),Arg::object(reflected),Arg::object(arguments)])
    };
    let result = method.map_or(std::ptr::null_mut(), |method| env.call_static_object(bridge, method, &values));
    for local in [method_class, bridge, locals] {
        env.delete_local_ref(local);
    }
    result
}

pub fn register_6(env:JniEnv,api:&rudroid_common::host_api::HostApiV0)->i32 {register(env,api,false)}
pub fn register_2021(env:JniEnv,api:&rudroid_common::host_api::HostApiV0)->i32 {register(env,api,true)}
fn register(env: JniEnv,api:&rudroid_common::host_api::HostApiV0,legacy:bool) -> i32 {
    let _=HOST.set(*api);
    if rudroid_common::jni_abi::declare_superclass(env,c"com/unity3d/player/UnityCoreAssetPacksStatusCallbacks",c"com/unity3d/player/IAssetPackManagerStatusQueryCallback")!=0{return -1;}
    if rudroid_common::jni_abi::declare_superclass(env,PACK_QUERY,c"java/lang/Runnable")!=0{return -1;}
    for name in [c"names",c"callback"]{if declare_field(env,PACK_QUERY,name,c"Ljava/lang/Object;",false)!=0{return -1;}}
    if rudroid_common::jni_abi::declare_superclass(env,PROXY,c"org/rudroid/runtime/JavaProxy")!=0{return -1;}
    for (name, signature) in [
        (c"nativeHandle", c"J"),
        (c"interfaces", c"[Ljava/lang/Class;"),
        (c"invokeKind",c"I"),
    ] {
        if declare_field(env, PROXY, name, signature, false) != 0 {
            return -1;
        }
    }
    for (name,sig) in [(c"nativeHandle",c"J"),(c"nativeException",c"Z")]{if declare_field(env,c"com/unity3d/player/ReflectionHelper$InvocationError",name,sig,false)!=0{return -1;}}
    for (class,natives) in natives(legacy) {
        if env.register_class(class,&natives)!=0{return -1;}
    }
    if legacy&&super::soft_input::register(env,api)!=0{return -1;}
    0
}

// The host has no Android accessibility view/service. Unity 6 nevertheless
// owns a delegate and calls cleanup during nativeDone, even when accessibility
// is disabled. Keep that owner in the VM rather than returning a null receiver.
extern "C" fn accessibility_delegate(env: JniEnv, class: JObject, player: JObject) -> JObject {
    if player.is_null() {
        let error = env.find_class(c"java/lang/NullPointerException");
        env.throw_new(error, c"Unity accessibility delegate needs a player");
        env.delete_local_ref(error);
        return std::ptr::null_mut();
    }
    env.alloc_object(class)
}

/// Every native this profile registers, per class; each entry is checked against its descriptor.
fn natives(legacy:bool) -> Vec<(&'static CStr, Vec<Native>)> {
    let mut classes=vec![
        (c"org/rudroid/runtime/JavaProxy",vec![Native::new3(c"invoke",c"(Ljava/lang/Object;Ljava/lang/reflect/Method;[Ljava/lang/Object;)Ljava/lang/Object;",generic_proxy_invoke)]),
        (PACK_QUERY,vec![Native::new0(c"run",c"()V",pack_query_run)]),
        (c"com/unity3d/player/UnityPlayer",vec![
            Native::new0(c"getNetworkConnectivity",c"()I",super::network_connectivity::query),
            Native::new1(c"getNetworkProxySettings",c"(Ljava/lang/String;)Ljava/lang/String;",super::network_proxy::settings),
            Native::new0(c"getLaunchURL",c"()Ljava/lang/String;",no_url),
            Native::new0(c"hideSoftInput",c"()V",empty),
            Native::new0(c"hidePreservedContent",c"()V",empty),
            Native::new0(c"addPhoneCallListener",c"()V",empty),
        ]),
        (c"com/unity3d/player/UnityPlayerForActivityOrService",vec![
            Native::new0(c"getNetworkConnectivity",c"()I",super::network_connectivity::query),
        ]),
        (c"bitter/jnibridge/JNIBridge",vec![Native::new2(c"newInterfaceProxy",c"(J[Ljava/lang/Class;)Ljava/lang/Object;",proxy),Native::new1(c"disableInterfaceProxy",c"(Ljava/lang/Object;)V",disable_proxy)]),
        (c"com/unity3d/player/ReflectionHelper",vec![Native::new0(c"endUnityLaunch",c"()V",empty),Native::new2(c"getConstructorID",c"(Ljava/lang/Class;Ljava/lang/String;)Ljava/lang/reflect/Constructor;",constructor),Native::new4(c"getMethodID",c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Method;",reflected_method),Native::new4(c"getFieldID",c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Field;",reflected_field),Native::new1(c"getFieldSignature",c"(Ljava/lang/reflect/Field;)Ljava/lang/String;",field_signature),Native::new3(c"newProxyInstance",c"(Lcom/unity3d/player/UnityPlayer;JLjava/lang/Class;)Ljava/lang/Object;",reflection_proxy),Native::new3(c"newProxyInstance",c"(Lcom/unity3d/player/UnityPlayer;J[Ljava/lang/Class;)Ljava/lang/Object;",reflection_proxy_array),Native::new2(c"createInvocationError",c"(JZ)Ljava/lang/Object;",invocation_error)]),
        (c"java/lang/Runnable",vec![Native::new0(c"run",c"()V",empty)]),
        (PROXY,vec![Native::new0(c"run",c"()V",proxy_run),Native::new1(c"handleMessage",c"(Landroid/os/Message;)Z",proxy_message),Native::new1(c"doFrame",c"(J)V",proxy_frame)]),
        (c"com/unity3d/player/UnityCoreAssetPacksStatusCallbacks",vec![Native::new0(c"<init>",c"()V",empty),Native::new4(c"onStatusResult",c"(J[Ljava/lang/String;[I[I)V",core_pack_status)]),
        (c"com/unity3d/player/IAssetPackManagerStatusQueryCallback",vec![Native::new4(c"onStatusResult",c"(J[Ljava/lang/String;[I[I)V",core_pack_status)]),
        (c"com/unity3d/player/PlayAssetDeliveryUnityWrapper",vec![
            Native::new2(c"init",c"(Lcom/unity3d/player/UnityPlayer;Landroid/content/Context;)Lcom/unity3d/player/PlayAssetDeliveryUnityWrapper;",wrapper),
            Native::new0(c"getInstance",c"()Lcom/unity3d/player/PlayAssetDeliveryUnityWrapper;",wrapper_instance),
            Native::new1(c"getAssetPackPath",c"(Ljava/lang/String;)Ljava/lang/String;",asset_pack_path),
            Native::new1(c"removeAssetPack",c"(Ljava/lang/String;)V",retain_local_pack),
            Native::new1(c"registerDownloadStatusListener",c"(Lcom/unity3d/player/IAssetPackManagerDownloadStatusCallback;)Ljava/lang/Object;",listener_token),
            Native::new1(c"unregisterDownloadStatusListener",c"(Ljava/lang/Object;)V",discard_listener),
            Native::new2(c"getAssetPackState",c"(Ljava/lang/String;Lcom/unity3d/player/IAssetPackManagerStatusQueryCallback;)V",pack_state),
            Native::new2(c"getAssetPackStates",c"([Ljava/lang/String;Lcom/unity3d/player/IAssetPackManagerStatusQueryCallback;)V",pack_states),
            Native::new0(c"playCoreApiMissing",c"()Z",unavailable),
        ]),
    ];
    if !legacy {
        classes.push((c"com/unity3d/player/UnityAccessibilityDelegate", vec![
            Native::new1(c"init", c"(Lcom/unity3d/player/UnityPlayer;)Lcom/unity3d/player/UnityAccessibilityDelegate;", accessibility_delegate),
            Native::new0(c"cleanup", c"()V", empty),
        ]));
    }
    if legacy {
        let player=&mut classes.iter_mut().find(|(c,_)|*c==c"com/unity3d/player/UnityPlayer").unwrap().1;
        player.retain(|m|m.name()!=c"getLaunchURL");
        player.push(Native::new0(c"getLaunchURL",c"()Ljava/lang/String;",legacy_launch_url));
        player.push(Native::new0(c"initializeGoogleAr",c"()Z",legacy_feature_missing));
        player.push(Native::new1(c"startOrientationListener",c"(I)Z",legacy_orientation_missing));
        player.push(Native::new0(c"stopOrientationListener",c"()Z",legacy_feature_missing));
        player.push(Native::new0(c"isShouldShowLaunchScreenAds",c"()Z",legacy_feature_missing));
        let pad=&mut classes.iter_mut().find(|(c,_)|*c==c"com/unity3d/player/PlayAssetDeliveryUnityWrapper").unwrap().1;
        pad.retain(|m|m.name()!=c"init" && m.name()!=c"playCoreApiMissing");
        pad.push(Native::new1(c"init",c"(Landroid/content/Context;)Lcom/unity3d/player/PlayAssetDeliveryUnityWrapper;",legacy_wrapper));
        pad.push(Native::new0(c"playCoreApiMissing",c"()Z",legacy_play_core_missing));
    }
    classes
}
extern "C" fn legacy_launch_url(env:JniEnv,_player:JObject)->JObject {env.new_string_utf(c"")}
// No Android orientation sensor or China Player launch-ad Activity is hosted.
// Keep these optional feature queries explicit and worker-free.
extern "C" fn legacy_feature_missing(_env:JniEnv,_player:JObject)->u8 {0}
extern "C" fn legacy_orientation_missing(_env:JniEnv,_player:JObject,_mode:i32)->u8 {0}
extern "C" fn legacy_play_core_missing(_env:JniEnv,_wrapper:JObject)->u8 {1}
extern "C" fn legacy_wrapper(env:JniEnv,class:JObject,context:JObject)->JObject {wrapper(env,class,std::ptr::null_mut(),context)}

#[cfg(test)]
#[path = "../tests/unit/java.rs"]
mod tests;
