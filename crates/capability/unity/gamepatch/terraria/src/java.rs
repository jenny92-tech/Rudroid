//! INPUT: the shared VM's standard UnityPlayerActivity and Terraria JNI requests
//! OUTPUT: VM-owned 330-key snapshot, mouse defaults and exact float callback
//! POS: safe Title-only Java glue, without native allocations or polling
#![forbid(unsafe_code)]
use rudroid_common::{jni_abi::declare_field,jni_env::{JObject,JniEnv,Native}};
const ACTIVITY:&std::ffi::CStr=c"com/unity3d/player/UnityPlayerActivity";

// The compositor owns the fixed handheld refresh rate. Match the C facade;
// do not create a second display/vsync owner for a game-specific request.
extern "C" fn frame_rate(_env:JniEnv,_activity:JObject,_rate:f32) {}

pub(super) fn register(env:JniEnv)->i32 {
    for (name,sig) in [(c"PressedStates",c"[Z"),(c"MouseMode",c"I"),(c"MouseInside",c"Z")] {
        if declare_field(env,ACTIVITY,name,sig,false)!=0{return -1;}
    }
    env.register_class(ACTIVITY,&[Native::new1(c"SetSurfaceFrameRate",c"(F)V",frame_rate)])
}
pub(super) fn initialize(env:JniEnv)->bool {
    let unity=env.find_class(c"com/unity3d/player/UnityPlayer");
    let activity=env.get_static_object_field(unity,env.get_static_field_id(unity,c"currentActivity",c"Landroid/app/Activity;"));
    env.delete_local_ref(unity);
    if activity.is_null(){return false;}
    let class=env.get_object_class(activity);
    let field=env.get_field_id(class,c"PressedStates",c"[Z");
    let current=env.get_object_field(activity,field);
    if current.is_null() {
        // The reference initializes a zeroed snapshot and never polls it.
        // The field, rather than a Rust/global cache, owns the array.
        let keys=env.new_boolean_array(330);env.set_object_field(activity,field,keys);env.delete_local_ref(keys);
        env.set_int_field(activity,env.get_field_id(class,c"MouseMode",c"I"),-1);
        env.set_boolean_field(activity,env.get_field_id(class,c"MouseInside",c"Z"),1);
    }
    for local in [current,class,activity]{env.delete_local_ref(local);}
    !env.exception_check()
}
