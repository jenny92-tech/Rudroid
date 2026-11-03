//! INPUT: private game config, trusted ARM64 production kit and optional bounded diagnostic boot delay
//! OUTPUT: isolated comparison of NativeLoader-only versus the legacy C JNI startup sequence
//! POS: opt-in diagnostic; replay is never a production loader option
use rudroid_common::ModuleKind;
use rudroid_host_config::HostConfig;
use rudroid_unity_loader::{bring_up, guest, host_api, load_deploy_dir, runtime, GuestPlan, Selection};
use std::{ffi::c_void, path::PathBuf};

#[test]
#[ignore = "requires handheld, private RUDROID_GUEST_CONFIG, production RUDROID_RESOURCE_KIT; RUDROID_C_JNI_SEQUENCE=1 compares the C reference"]
fn compare_legacy_native_startup_sequence() {
    let path=PathBuf::from(std::env::var_os("RUDROID_GUEST_CONFIG").expect("private config required"));
    let kit=PathBuf::from(std::env::var_os("RUDROID_RESOURCE_KIT").expect("production kit required"));
    let mut config=HostConfig::from_path(&path).unwrap();
    let mut table=config.table().clone();
    table.get_mut("unity").unwrap().as_table_mut().unwrap().insert("smoke_frame_limit".into(),120i64.into());
    config=HostConfig::parse(&table.to_string()).unwrap();
    let root=config.paths().game_files.expect("game files required");
    let root=PathBuf::from(root).canonicalize().unwrap();
    assert!(config.set_string("paths.game_files",root.to_str().unwrap()));
    std::env::set_current_dir(root).unwrap();
    host_api::install_config(Some(config.clone())).unwrap();
    let selection=Selection::from_config(&config).unwrap();
    // SAFETY: explicit trusted production kit; handles remain process-pinned.
    let capabilities=unsafe { load_deploy_dir(&kit,&selection) }.unwrap();
    let capabilities=bring_up(capabilities,config.package().package_name.as_deref()).unwrap();
    runtime::bring_up_sdl_session(&capabilities,&config).unwrap();
    runtime::prepare_unity_runtime(&capabilities).unwrap();
    let plan=GuestPlan::from_config(&config).unwrap();
    // SAFETY: local licensed ARM64 guest, private paths, pinned native providers.
    let events=unsafe { guest::bring_up(&capabilities,&plan) }.unwrap();
    if let Ok(value)=std::env::var("RUDROID_DIAGNOSTIC_BOOT_DELAY_MS") {
        let delay:u64=value.parse().expect("diagnostic delay must be milliseconds");
        assert!(delay<=10_000,"diagnostic delay is bounded to ten seconds");
        eprintln!("[SEQUENCE-PROBE] mapped; diagnostic bootstrap delay {delay}ms");
        std::thread::sleep(std::time::Duration::from_millis(delay));
    }
    runtime::bootstrap_unity_runtime(&capabilities,"lib/arm64-v8a").unwrap();
    if std::env::var_os("RUDROID_C_JNI_SEQUENCE").is_some() {
        let host=host_api::table();
        let vm=host.java_vm().unwrap();
        for kind in [ModuleKind::Il2cpp,ModuleKind::Unity] {
            let event=events.iter().find(|e|e.kind==kind).unwrap();
            let address=host.guest_symbol(event.base,c"JNI_OnLoad").unwrap();
            // SAFETY: exact JNI entry in the pinned guest image. This opt-in
            // experiment repeats the C reference's post-bootstrap calls only.
            let entry:extern "C" fn(*mut c_void,*mut c_void)->i32=unsafe {std::mem::transmute(address)};
            let version=entry(vm.as_raw(),std::ptr::null_mut());
            eprintln!("[SEQUENCE-PROBE] C post-bootstrap {kind:?} JNI_OnLoad={version:#x}");
            assert_eq!(version,0x10006);
        }
    }
    let _completion = runtime::run_unity_runtime(&capabilities).unwrap();
    rudroid_unity_loader::terminate_completed_guest(&capabilities, 0);
}
