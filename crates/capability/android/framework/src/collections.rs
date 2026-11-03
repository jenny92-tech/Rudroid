//! INPUT: SharedPreferences snapshots, shared object arrays and ordinary JNI calls
//! OUTPUT: typed Map entries and array-backed Lists/Sets/iterators on the shared heap
//! POS: Android preference snapshot façade; no second persistent store or native owner handles
#![forbid(unsafe_code)]
use rudroid_common::jni_abi::{declare_field, declare_superclass};
use rudroid_common::jni_env::{JObject, JniEnv, Native};
use serde_json::Value;
use std::ffi::{c_void, CStr};

const MAP: &CStr = c"org/rudroid/runtime/PreferenceMap";
const SET: &CStr = c"org/rudroid/runtime/PreferenceEntrySet";
const ITER: &CStr = c"org/rudroid/runtime/PreferenceIterator";
const ENTRY: &CStr = c"org/rudroid/runtime/PreferenceEntry";
const EMPTY_LIST: &CStr = c"org/rudroid/runtime/EmptyList";
pub(super) fn list(env:JniEnv,array:JObject)->JObject {
    let object=super::java::allocate(env,EMPTY_LIST);bind_entries(env,object,array);object
}
pub(super) fn set(env:JniEnv,array:JObject)->JObject {
    let object=super::java::allocate(env,SET);bind_entries(env,object,array);object
}
extern "C" fn indexed(env:JniEnv,list:JObject,index:i32)->JObject {
    let array=entries(env,list);let value=env.get_object_array_element(array,index);super::preferences::delete(env,array);value
}
pub(super) fn empty_list(env: JniEnv) -> *mut c_void {
    let class = env.find_class(c"java/util/Map$Entry");
    let array = env.new_object_array(0, class, std::ptr::null_mut());
    super::preferences::delete(env, class);
    let list = super::java::allocate(env, EMPTY_LIST);
    bind_entries(env, list, array);
    super::preferences::delete(env, array);
    list
}
fn field(env: JniEnv, object: *mut c_void, name: &CStr, descriptor: &CStr) -> *mut c_void {
    let class = env.get_object_class(object);
    let result = env.get_field_id(class, name, descriptor);
    super::preferences::delete(env, class);
    result
}
fn entries(env: JniEnv, object: *mut c_void) -> *mut c_void {
    env.get_object_field(
        object,
        field(env, object, c"entries", c"[Ljava/lang/Object;"),
    )
}
fn bind_entries(env: JniEnv, object: *mut c_void, array: *mut c_void) {
    env.set_object_field(
        object,
        field(env, object, c"entries", c"[Ljava/lang/Object;"),
        array,
    )
}
extern "C" fn size(env: JniEnv, object: JObject) -> i32 {
    let array = entries(env, object);
    let size = env.get_array_length(array);
    super::preferences::delete(env, array);
    size
}
extern "C" fn empty(env: JniEnv, object: JObject) -> u8 {
    u8::from(size(env, object) == 0)
}
extern "C" fn entry_set(env: JniEnv, map: JObject) -> JObject {
    let array = entries(env, map);
    let set = super::java::allocate(env, SET);
    bind_entries(env, set, array);
    super::preferences::delete(env, array);
    set
}
extern "C" fn iterator(env: JniEnv, set: JObject) -> JObject {
    let array = entries(env, set);
    let iter = super::java::allocate(env, ITER);
    bind_entries(env, iter, array);
    super::preferences::delete(env, array);
    iter
}
fn index(env: JniEnv, iter: *mut c_void) -> i32 {
    env.get_int_field(iter, field(env, iter, c"index", c"I"))
}
extern "C" fn has_next(env: JniEnv, iter: JObject) -> u8 {
    u8::from(index(env, iter) < size(env, iter))
}
extern "C" fn next(env: JniEnv, iter: JObject) -> JObject {
    let at = index(env, iter);
    if at >= size(env, iter) {
        let class = env.find_class(c"java/util/NoSuchElementException");
        env.throw_new(class, c"iterator exhausted");
        super::preferences::delete(env, class);
        return std::ptr::null_mut();
    }
    let array = entries(env, iter);
    let entry = env.get_object_array_element(array, at);
    super::preferences::delete(env, array);
    env.set_int_field(iter, field(env, iter, c"index", c"I"), at + 1);
    entry
}
extern "C" fn key(env: JniEnv, entry: JObject) -> JObject {
    env.get_object_field(entry, field(env, entry, c"key", c"Ljava/lang/String;"))
}
extern "C" fn value(env: JniEnv, entry: JObject) -> JObject {
    env.get_object_field(entry, field(env, entry, c"value", c"Ljava/lang/Object;"))
}
extern "C" fn int_value(env: JniEnv, object: JObject) -> i32 {
    env.get_int_field(object, field(env, object, c"value", c"I"))
}
extern "C" fn float_value(env: JniEnv, object: JObject) -> f32 {
    env.get_float_field(object, field(env, object, c"value", c"F"))
}
extern "C" fn long_value(env:JniEnv,object:JObject)->i64 {env.get_long_field(object,field(env,object,c"value",c"J"))}
extern "C" fn boolean_value(env:JniEnv,object:JObject)->u8 {env.get_boolean_field(object,field(env,object,c"value",c"Z"))}
fn boxed(env: JniEnv, section: &str, value: &Value) -> *mut c_void {
    let (class, descriptor) = match section {
        "ints" => (c"java/lang/Integer", c"I"),
        "longs" => (c"java/lang/Long", c"J"),
        "floats" => (c"java/lang/Float", c"F"),
        "bools" => (c"java/lang/Boolean", c"Z"),
        _ => {
            return value
                .as_str()
                .map(|v| super::preferences::string(env, v))
                .unwrap_or(std::ptr::null_mut())
        }
    };
    let object = super::java::allocate(env, class);
    let id = field(env, object, c"value", descriptor);
    match section {
        "ints" => env.set_int_field(object, id, value.as_i64().unwrap() as i32),
        "longs" => env.set_long_field(object, id, value.as_i64().unwrap()),
        "floats" => env.set_float_field(object, id, value.as_f64().unwrap() as f32),
        _ => env.set_boolean_field(object, id, value.as_bool().unwrap() as u8),
    }
    object
}
pub(super) fn snapshot(env: JniEnv, data: Value) -> *mut c_void {
    let count: usize = ["strings", "ints", "longs", "floats", "bools"]
        .iter()
        .map(|k| {
            data.get(k)
                .and_then(Value::as_object)
                .map_or(0, |v| v.len())
        })
        .sum();
    let Ok(count) = i32::try_from(count) else {
        super::preferences::error(env, "preferences snapshot too large");
        return std::ptr::null_mut();
    };
    let class = env.find_class(c"java/util/Map$Entry");
    let entries = env.new_object_array(count, class, std::ptr::null_mut());
    super::preferences::delete(env, class);
    let mut at = 0;
    for section in ["strings", "ints", "longs", "floats", "bools"] {
        if let Some(values) = data.get(section).and_then(Value::as_object) {
            for (name, v) in values {
                let valid = match section {
                    "strings" => v.is_string(),
                    "ints" => v.as_i64().is_some_and(|v| i32::try_from(v).is_ok()),
                    "longs" => v.as_i64().is_some(),
                    "floats" => v.as_f64().is_some_and(|v| (v as f32).is_finite()),
                    _ => v.is_boolean(),
                };
                if !valid {
                    super::preferences::delete(env, entries);
                    super::preferences::error(env, "invalid preferences value type");
                    return std::ptr::null_mut();
                }
                let entry = super::java::allocate(env, ENTRY);
                let key = super::preferences::string(env, name);
                let value = boxed(env, section, v);
                env.set_object_field(
                    entry,
                    field(env, entry, c"key", c"Ljava/lang/String;"),
                    key,
                );
                env.set_object_field(
                    entry,
                    field(env, entry, c"value", c"Ljava/lang/Object;"),
                    value,
                );
                env.set_object_array_element(entries, at, entry);
                at += 1;
                for reference in [key, value, entry] {
                    super::preferences::delete(env, reference);
                }
            }
        }
    }
    let map = super::java::allocate(env, MAP);
    bind_entries(env, map, entries);
    super::preferences::delete(env, entries);
    map
}
pub(super) fn register(env: JniEnv) -> i32 {
    for (child, parent) in [
        (EMPTY_LIST, c"java/util/List"),
        (MAP, c"java/util/Map"),
        (SET, c"java/util/Set"),
        (ITER, c"java/util/Iterator"),
        (ENTRY, c"java/util/Map$Entry"),
        (c"java/lang/Integer", c"java/lang/Number"),
        (c"java/lang/Float", c"java/lang/Number"),
        (c"java/lang/Long", c"java/lang/Number"),
    ] {
        if declare_superclass(env, child, parent) != 0 {
            return -1;
        }
    }
    for class in [EMPTY_LIST, MAP, SET, ITER] {
        if declare_field(env, class, c"entries", c"[Ljava/lang/Object;", false) != 0 {
            return -1;
        }
    }
    for (class, name, sig) in [
        (ITER, c"index", c"I"),
        (ENTRY, c"key", c"Ljava/lang/String;"),
        (ENTRY, c"value", c"Ljava/lang/Object;"),
        (c"java/lang/Integer", c"value", c"I"),
        (c"java/lang/Float", c"value", c"F"),
        (c"java/lang/Long", c"value", c"J"),
        (c"java/lang/Boolean", c"value", c"Z"),
    ] {
        if declare_field(env, class, name, sig, false) != 0 {
            return -1;
        }
    }
    for (class, methods) in [
        (
            c"java/util/List",
            vec![
                Native::new0(c"size", c"()I", size),
                Native::new0(c"isEmpty", c"()Z", empty),
                Native::new0(c"iterator", c"()Ljava/util/Iterator;", iterator),
                Native::new1(c"get",c"(I)Ljava/lang/Object;",indexed),
            ],
        ),
        (
            c"java/util/Map",
            vec![
                Native::new0(c"size", c"()I", size),
                Native::new0(c"isEmpty", c"()Z", empty),
                Native::new0(c"entrySet", c"()Ljava/util/Set;", entry_set),
            ],
        ),
        (
            c"java/util/Set",
            vec![
                Native::new0(c"size", c"()I", size),
                Native::new0(c"isEmpty", c"()Z", empty),
                Native::new0(c"iterator", c"()Ljava/util/Iterator;", iterator),
            ],
        ),
        (
            c"java/util/Iterator",
            vec![
                Native::new0(c"hasNext", c"()Z", has_next),
                Native::new0(c"next", c"()Ljava/lang/Object;", next),
            ],
        ),
        (
            c"java/util/Map$Entry",
            vec![
                Native::new0(c"getKey", c"()Ljava/lang/Object;", key),
                Native::new0(c"getValue", c"()Ljava/lang/Object;", value),
            ],
        ),
        (
            c"java/lang/Integer",
            vec![Native::new0(c"intValue", c"()I", int_value)],
        ),
        (
            c"java/lang/Float",
            vec![Native::new0(c"floatValue", c"()F", float_value)],
        ),
        (c"java/lang/Long",vec![Native::new0(c"longValue",c"()J",long_value)]),
        (c"java/lang/Boolean",vec![Native::new0(c"booleanValue",c"()Z",boolean_value)]),
    ] {
        if env.register_class(class, &methods) != 0 {
            return -1;
        }
    }
    0
}
