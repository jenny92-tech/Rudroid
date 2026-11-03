//! INPUT: trusted production core/legacy-profile/Terraria DSOs and shared JNI tables
//! OUTPUT: Title field isolation, array ownership, reflection and float callback checks
//! POS: isolated handheld acceptance without native game libraries or real saves
use rudroid_common::{jni_env::Arg,ModuleKind};
use rudroid_host_config::HostConfig;
use rudroid_unity_loader::{host_api,phases::Phase,runtime};
use std::path::PathBuf;

#[test]
#[ignore = "requires production RUDROID_RESOURCE_KIT; RUDROID_TERRARIA_FOREIGN=1 checks package isolation in a fresh process"]
fn production_terraria_activity_owns_state_and_isolates_other_titles() {
    let kit=PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").unwrap());
    let foreign=std::env::var_os("RUDROID_TERRARIA_FOREIGN").is_some();
    let package=if foreign{"org.rudroid.other_title"}else{"com.and.games505.TerrariaPaid"};
    host_api::install_config(Some(HostConfig::parse(&format!("[package]\npackageName = {package:?}\n")).unwrap())).unwrap();
    let host=host_api::table();
    // SAFETY: explicit trusted same-kit production DSOs, pinned for all callbacks.
    let libraries:Vec<_>=unsafe {[
        "libplatform_log_stderr.so","libreply_catalog.so","libandroid_jni_vm.so",
        "libandroid_base.so","libunity_2021_3.so","libterraria_android.so",
    ].iter().map(|name|std::mem::ManuallyDrop::new(libloading::Library::new(kit.join(name)).unwrap())).collect()};
    for phase in [Phase::EarlyInit,Phase::RegisterJni,Phase::Init] {
        for lib in &libraries {
            // SAFETY: trusted phase entry with the common pinned Host API ABI.
            match unsafe{host_api::run_phase_entry(phase,lib)} {
                Ok(())|Err(host_api::InitError::NotPresent)=>(),
                Err(e)=>panic!("phase: {e:?}"),
            }
        }
    }
    assert!(!(runtime::registered_profile().unwrap().prepare.unwrap()(Some(host))).is_null());
    let env=host.java_vm().unwrap().get_env(0x10006).unwrap();
    let unity=env.find_class(c"com/unity3d/player/UnityPlayer");
    let activity=env.get_static_object_field(unity,env.get_static_field_id(unity,c"currentActivity",c"Landroid/app/Activity;"));
    let class=env.get_object_class(activity);
    let field=env.get_field_id(class,c"PressedStates",c"[Z");
    // Real native loads are loader-owned events; capability fire_event rejects them.
    let main=rudroid_common::registry::RudroidModuleEvent {
        kind:ModuleKind::Main as u32,base:0,
        name:rudroid_common::registry::HostStr::new(c"lib/arm64-v8a/libmain.so"),
    };
    if foreign {
        assert!(field.is_null() && env.exception_check(),"Title fields must not appear on other games");env.exception_clear();
    } else {
        assert!(!field.is_null());
        assert!(env.get_object_field(activity,field).is_null());
        runtime::fire_module_event(&main);
        let keys=env.get_object_field(activity,field);assert_eq!(env.get_array_length(keys),330);
        let mut values=[1u8;330];env.get_boolean_array_region(keys,0,&mut values);assert_eq!(values,[0u8;330]);
        assert_eq!(env.get_int_field(activity,env.get_field_id(class,c"MouseMode",c"I")),-1);
        assert_ne!(env.get_boolean_field(activity,env.get_field_id(class,c"MouseInside",c"Z")),0);
        env.set_boolean_array_region(keys,329,&[1]);env.delete_local_ref(keys);
        runtime::fire_module_event(&main);
        let retained=env.get_object_field(activity,field);let mut last=[0];env.get_boolean_array_region(retained,329,&mut last);assert_eq!(last,[1]);
        let reflection=env.find_class(c"com/unity3d/player/ReflectionHelper");
        let name=env.new_string_utf(c"PressedStates");let hint=env.new_string_utf(c"Ljava/lang/Object;");
        let reflected=env.call_static_object(reflection,env.static_method(reflection,c"getFieldID",c"(Ljava/lang/Class;Ljava/lang/String;Ljava/lang/String;Z)Ljava/lang/reflect/Field;").unwrap(),&[Arg::object(class),Arg::object(name),Arg::object(hint),Arg::bool(false)]);
        assert!(!reflected.is_null());
        let signature=env.call_static_object(reflection,env.static_method(reflection,c"getFieldSignature",c"(Ljava/lang/reflect/Field;)Ljava/lang/String;").unwrap(),&[Arg::object(reflected)]);
        assert_eq!(env.string_utf(signature).unwrap().as_c_str(),c"[Z");
        let request=env.method(class,c"SetSurfaceFrameRate",c"(F)V").unwrap();
        for rate in [30.0,60.0,-0.0,f32::NAN]{env.call_void(activity,request,&[Arg::float(rate)]);assert!(!env.exception_check());}
        for local in [signature,reflected,hint,name,reflection,retained]{env.delete_local_ref(local);}
    }
    assert!(!env.exception_check());
    for local in [class,activity,unity]{env.delete_local_ref(local);}
}
