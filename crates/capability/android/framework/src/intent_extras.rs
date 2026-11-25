//! INPUT: string Intent/Bundle calls and immutable launcher bundle configuration
//! OUTPUT: per-object string extras and detached getExtras snapshots on the shared VM
//! POS: VM fields own every reference; ordinary Intents never read launcher configuration
#![forbid(unsafe_code)]
use rudroid_common::jni_abi::declare_field;
use rudroid_common::jni_env::{JFieldId, JObject, JniEnv, Native};
use std::ffi::{CStr, CString};

// Flat key/value pairs; null keys/values remain distinguishable from absence.
const ENTRIES: &CStr = c"rudroidStringEntries";
const LAUNCHER: &CStr = c"rudroidLauncher";
const EXTRAS: &CStr = c"rudroidExtras";
const ARRAY: &CStr = c"[Ljava/lang/Object;";
const BUNDLE: &CStr = c"Landroid/os/Bundle;";

fn field(env: JniEnv, object: JObject, name: &CStr, signature: &CStr) -> JFieldId {
    let class = env.get_object_class(object);
    let id = env.get_field_id(class, name, signature);
    env.delete_local_ref(class);
    id
}
fn entries(env: JniEnv, bundle: JObject) -> JObject {
    env.get_object_field(bundle, field(env, bundle, ENTRIES, ARRAY))
}
fn extras(env: JniEnv, intent: JObject) -> JObject {
    env.get_object_field(intent, field(env, intent, EXTRAS, BUNDLE))
}
fn copy_entries(env: JniEnv, source: JObject, length: i32) -> JObject {
    let class = env.find_class(c"java/lang/Object");
    let copy = env.new_object_array(length, class, std::ptr::null_mut());
    env.delete_local_ref(class);
    if copy.is_null() { return copy; }
    if !source.is_null() {
        for index in 0..env.get_array_length(source) {
            let value = env.get_object_array_element(source, index);
            env.set_object_array_element(copy, index, value);
            env.delete_local_ref(value);
        }
    }
    if env.exception_check() {
        env.delete_local_ref(copy);
        std::ptr::null_mut()
    } else { copy }
}
fn find(env: JniEnv, array: JObject, key: JObject) -> Option<i32> {
    if array.is_null() { return None; }
    let units = if key.is_null() { None } else { env.string_utf16(key) };
    if !key.is_null() && units.is_none() { return None; }
    for index in (0..env.get_array_length(array)).step_by(2) {
        let candidate = env.get_object_array_element(array, index);
        let equal = env.is_same_object(candidate, key) ||
            (!candidate.is_null() && !key.is_null() && env.string_utf16(candidate) == units);
        env.delete_local_ref(candidate);
        if equal { return Some(index); }
    }
    None
}
fn configured(env: JniEnv, bundle: JObject, key: JObject) -> Option<String> {
    if key.is_null() || env.get_boolean_field(bundle, field(env, bundle, LAUNCHER, c"Z")) == 0 {
        return None;
    }
    let text = String::from_utf16(&env.string_utf16(key)?).ok()?;
    let key = CString::new(format!("package.mainIntentBundle.{text}")).ok()?;
    super::HOST.get()?.config_string(&key, 4096)
}

/// Only Activity.getIntent's process-owned launcher object gets this view.
/// Configuration stays immutable; local writes override it, including null.
pub(super) fn initialize_launcher(env: JniEnv, intent: JObject) {
    let bundle = super::java::allocate(env, c"android/os/Bundle");
    if bundle.is_null() { return; }
    env.set_boolean_field(bundle, field(env, bundle, LAUNCHER, c"Z"), 1);
    env.set_object_field(intent, field(env, intent, EXTRAS, BUNDLE), bundle);
    env.delete_local_ref(bundle);
}
extern "C" fn empty(_env: JniEnv, _bundle: JObject) {}
extern "C" fn put_string(env: JniEnv, bundle: JObject, key: JObject, value: JObject) {
    let array = entries(env, bundle);
    if let Some(index) = find(env, array, key) {
        env.set_object_array_element(array, index + 1, value);
    } else if !env.exception_check() {
        let length = if array.is_null() { 0 } else { env.get_array_length(array) };
        if let Some(length) = length.checked_add(2) {
            let copy = copy_entries(env, array, length);
            if !copy.is_null() {
                env.set_object_array_element(copy, length - 2, key);
                env.set_object_array_element(copy, length - 1, value);
                if !env.exception_check() {
                    env.set_object_field(bundle, field(env, bundle, ENTRIES, ARRAY), copy);
                }
                env.delete_local_ref(copy);
            }
        } else {
            let class = env.find_class(c"java/lang/OutOfMemoryError");
            env.throw_new(class, c"string extras exceed Java array length");
            env.delete_local_ref(class);
        }
    }
    env.delete_local_ref(array);
}
extern "C" fn contains(env: JniEnv, bundle: JObject, key: JObject) -> u8 {
    let array = entries(env, bundle);
    let found = find(env, array, key).is_some();
    env.delete_local_ref(array);
    u8::from(found || configured(env, bundle, key).is_some())
}
extern "C" fn get_string(env: JniEnv, bundle: JObject, key: JObject, default: JObject) -> JObject {
    let array = entries(env, bundle);
    let index = find(env, array, key);
    let value = if let Some(index) = index {
        env.get_object_array_element(array, index + 1)
    } else {
        configured(env, bundle, key).map_or(std::ptr::null_mut(), |text| super::preferences::string(env, &text))
    };
    env.delete_local_ref(array);
    // BaseBundle.getString(key, default) uses the default for explicit null too.
    if value.is_null() { env.new_local_ref(default) } else { value }
}
extern "C" fn get_string_or_null(env: JniEnv, bundle: JObject, key: JObject) -> JObject {
    get_string(env, bundle, key, std::ptr::null_mut())
}
extern "C" fn put_extra(env: JniEnv, intent: JObject, key: JObject, value: JObject) -> JObject {
    let mut bundle = extras(env, intent);
    if bundle.is_null() && !env.exception_check() {
        bundle = super::java::allocate(env, c"android/os/Bundle");
        if !bundle.is_null() { env.set_object_field(intent, field(env, intent, EXTRAS, BUNDLE), bundle); }
    }
    if !bundle.is_null() { put_string(env, bundle, key, value); }
    env.delete_local_ref(bundle);
    if env.exception_check() { std::ptr::null_mut() } else { env.new_local_ref(intent) }
}
extern "C" fn get_extra(env: JniEnv, intent: JObject, key: JObject) -> JObject {
    let bundle = extras(env, intent);
    let value = if bundle.is_null() { std::ptr::null_mut() } else { get_string_or_null(env, bundle, key) };
    env.delete_local_ref(bundle);
    value
}
extern "C" fn get_extras(env: JniEnv, intent: JObject) -> JObject {
    let bundle = extras(env, intent);
    if bundle.is_null() { return bundle; }
    // Intent.getExtras returns a Bundle copy, not its mutable backing map.
    // https://android.googlesource.com/platform/frameworks/base/+/92b424749346b860fb043dd831746a0b0d3a3e4b/core/java/android/content/Intent.java
    let copy = super::java::allocate(env, c"android/os/Bundle");
    if copy.is_null() { env.delete_local_ref(bundle); return copy; }
    let array = entries(env, bundle);
    let copied = if array.is_null() { array } else { copy_entries(env, array, env.get_array_length(array)) };
    env.set_object_field(copy, field(env, copy, ENTRIES, ARRAY), copied);
    let launcher = env.get_boolean_field(bundle, field(env, bundle, LAUNCHER, c"Z"));
    env.set_boolean_field(copy, field(env, copy, LAUNCHER, c"Z"), launcher);
    for reference in [array, copied, bundle] { env.delete_local_ref(reference); }
    if env.exception_check() {
        env.delete_local_ref(copy);
        std::ptr::null_mut()
    } else { copy }
}

pub(super) fn register(env: JniEnv) -> i32 {
    for (class, name, signature) in [
        (c"android/content/Intent", EXTRAS, BUNDLE),
        (c"android/os/Bundle", ENTRIES, ARRAY),
        (c"android/os/Bundle", LAUNCHER, c"Z"),
    ] {
        if declare_field(env, class, name, signature, false) != 0 { return -1; }
    }
    if env.register_class(c"android/os/Bundle", &[
        Native::new0(c"<init>", c"()V", empty),
        Native::new2(c"putString", c"(Ljava/lang/String;Ljava/lang/String;)V", put_string),
        Native::new1(c"containsKey", c"(Ljava/lang/String;)Z", contains),
        Native::new1(c"getString", c"(Ljava/lang/String;)Ljava/lang/String;", get_string_or_null),
        Native::new2(c"getString", c"(Ljava/lang/String;Ljava/lang/String;)Ljava/lang/String;", get_string),
    ]) != 0 { return -1; }
    env.register_class(c"android/content/Intent", &[
        Native::new2(c"putExtra", c"(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;", put_extra),
        Native::new0(c"getExtras", c"()Landroid/os/Bundle;", get_extras),
        Native::new1(c"getStringExtra", c"(Ljava/lang/String;)Ljava/lang/String;", get_extra),
    ])
}
