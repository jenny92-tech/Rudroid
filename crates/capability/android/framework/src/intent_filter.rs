//! INPUT: IntentFilter constructors and public action-list operations
//! OUTPUT: per-object, insertion-ordered UTF-16 action sets rooted by the shared VM
//! POS: action data only; no broadcast subscription, MIME/data/category matching or service success
#![forbid(unsafe_code)]
use std::ffi::CStr;
use rudroid_common::jni_abi::declare_field;
use rudroid_common::jni_env::{JFieldId, JObject, JniEnv, Native};

const CLASS: &CStr=c"android/content/IntentFilter";
const ACTIONS: &CStr=c"rudroidIntentFilterActions";
const ARRAY: &CStr=c"[Ljava/lang/String;";

fn field(env:JniEnv,filter:JObject)->JFieldId {
    let class=env.get_object_class(filter);
    let id=env.get_field_id(class,ACTIONS,ARRAY);
    env.delete_local_ref(class);id
}
fn error(env:JniEnv,class:&CStr,message:&CStr) {
    let class=env.find_class(class);env.throw_new(class,message);env.delete_local_ref(class);
}
fn actions(env:JniEnv,filter:JObject)->JObject {
    let value=env.get_object_field(filter,field(env,filter));
    if value.is_null() && !env.exception_check() {
        error(env,c"java/lang/NullPointerException",c"IntentFilter constructor has not initialized actions");
    }
    value
}
fn array(env:JniEnv,length:i32)->JObject {
    let class=env.find_class(c"java/lang/String");
    let value=env.new_object_array(length,class,std::ptr::null_mut());
    env.delete_local_ref(class);value
}
fn contains(env:JniEnv,values:JObject,action:JObject)->bool {
    let Some(units)=env.string_utf16(action) else{return false;};
    for index in 0..env.get_array_length(values) {
        let candidate=env.get_object_array_element(values,index);
        let equal=env.string_utf16(candidate).is_some_and(|candidate|candidate==units);
        env.delete_local_ref(candidate);
        if equal {return true;}
        if env.exception_check() {return false;}
    }
    false
}
// Android's public action-list contracts are backed by real object state.
// AOSP API26 reference: frameworks/base android-8.0.0_r1/core/java/android/content/IntentFilter.java
extern "C" fn empty(env:JniEnv,filter:JObject) {
    let values=array(env,0);
    if !values.is_null() {env.set_object_field(filter,field(env,filter),values);}
    env.delete_local_ref(values);
}
extern "C" fn add(env:JniEnv,filter:JObject,action:JObject) {
    if action.is_null() {error(env,c"java/lang/NullPointerException",c"IntentFilter action is null");return;}
    let values=actions(env,filter);
    if values.is_null() {return;}
    if !contains(env,values,action) && !env.exception_check() {
        let count=env.get_array_length(values);
        if let Some(length)=count.checked_add(1) {
            let copy=array(env,length);
            if !copy.is_null() {
                for index in 0..count {
                    let value=env.get_object_array_element(values,index);
                    env.set_object_array_element(copy,index,value);env.delete_local_ref(value);
                    if env.exception_check() {break;}
                }
                if !env.exception_check() {
                    env.set_object_array_element(copy,count,action);
                    if !env.exception_check() {env.set_object_field(filter,field(env,filter),copy);}
                }
                env.delete_local_ref(copy);
            }
        } else {error(env,c"java/lang/OutOfMemoryError",c"IntentFilter actions exceed Java array length");}
    }
    env.delete_local_ref(values);
}
extern "C" fn with_action(env:JniEnv,filter:JObject,action:JObject) {
    empty(env,filter);if !env.exception_check() {add(env,filter,action);}
}
extern "C" fn count(env:JniEnv,filter:JObject)->i32 {
    let values=actions(env,filter);
    let count=if values.is_null(){0}else{env.get_array_length(values)};
    env.delete_local_ref(values);count
}
extern "C" fn has(env:JniEnv,filter:JObject,action:JObject)->u8 {
    if action.is_null() {return 0;}
    let values=actions(env,filter);
    let found=!values.is_null() && contains(env,values,action);
    env.delete_local_ref(values);u8::from(found)
}
extern "C" fn get(env:JniEnv,filter:JObject,index:i32)->JObject {
    let values=actions(env,filter);
    if values.is_null() {return values;}
    let result=if index<0 || index>=env.get_array_length(values) {
        error(env,c"java/lang/IndexOutOfBoundsException",c"IntentFilter action index is outside its list");
        std::ptr::null_mut()
    } else {env.get_object_array_element(values,index)};
    env.delete_local_ref(values);result
}

pub(super) fn register(env:JniEnv)->i32 {
    if declare_field(env,CLASS,ACTIONS,ARRAY,false)!=0 {return -1;}
    env.register_class(CLASS,&[
        Native::new0(c"<init>",c"()V",empty),
        Native::new1(c"<init>",c"(Ljava/lang/String;)V",with_action),
        Native::new1(c"addAction",c"(Ljava/lang/String;)V",add),
        Native::new0(c"countActions",c"()I",count),
        Native::new1(c"getAction",c"(I)Ljava/lang/String;",get),
        Native::new1(c"hasAction",c"(Ljava/lang/String;)Z",has),
        Native::new1(c"matchAction",c"(Ljava/lang/String;)Z",has),
    ])
}
