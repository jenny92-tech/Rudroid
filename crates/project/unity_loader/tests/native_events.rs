//! INPUT:  production ELF/framework/Bionic/event cdylibs and an independent LP64 C guest
//! OUTPUT:  fd/reentry/wake/TLS/rollback assertions and repeated ready-fd poll cost samples
//! POS:    validates thread-owned native NDK events and explicit unavailable-sensor policy
#[cfg(not(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu")))]
#[test]
#[ignore = "requires Linux ARM64 GNU; use docker-rust.sh test"]
fn native_events_guest() { panic!("requires the ARM64 guest execution backend"); }

#[cfg(all(target_os = "linux", target_arch = "aarch64", target_env = "gnu"))]
#[test]
fn native_events_guest() {
    use rudroid_common::{symbols, ModuleKind};
    use rudroid_test_support::cargo::ensure_workspace_cdylib;
    use rudroid_unity_loader::{guest, load_deploy_dir, runtime, Selection};
    use std::{ffi::{c_char,c_void,CString},fs,process::Command};
    let root = std::env::temp_dir().join(format!("rudroid-native-events-guest-{}",std::process::id()));
    let kit = root.join("kit"); fs::create_dir_all(&kit).unwrap();
    for (package,artifact) in [
        ("rudroid-android-framework","android_base"),
        ("rudroid-android-elf-load","android_elf_load"),
        ("rudroid-android-bionic","android_bionic"),
        ("rudroid-android-native-events","android_native_events"),
    ] {
        let path=ensure_workspace_cdylib(package,artifact);
        fs::copy(&path,kit.join(path.file_name().unwrap())).unwrap();
    }
    let compile = |source: &str, output: &str| {
        let result=Command::new("gcc").args(["-shared","-fPIC","-nostdlib","-fno-builtin",
            "-fno-stack-protector","-O2",source,"-o",output]).current_dir(&root).output().unwrap();
        assert!(result.status.success(),"gcc failed: {}",String::from_utf8_lossy(&result.stderr));
    };
    fs::write(root.join("events.c"),include_str!("fixtures/native_events.c")).unwrap();
    compile("events.c","guest.so");
    fs::write(root.join("bad.c"),"extern void *ALooper_forThread(void); extern void events_not_supported(void); void probe(void) { if (ALooper_forThread()) events_not_supported(); }\n").unwrap();
    compile("bad.c","bad.so");
    let capabilities=unsafe { load_deploy_dir(&kit,&Selection::All) }.unwrap();
    let plan=guest::GuestPlan { game_files:root.clone(), entries:vec![guest::GuestEntry {
        kind:Some(ModuleKind::Main),relative_path:"guest.so",required:true,gates:&[],
    }] };
    let mut bad=plan.clone(); bad.entries[0].relative_path="bad.so";
    let error=unsafe { guest::bring_up(&capabilities,&bad) }.unwrap_err().to_string();
    assert!(error.contains("events_not_supported"),"{error}");
    let elf=capabilities.iter().find(|c| c.descriptor.id=="android_elf_load").unwrap();
    unsafe {
        let load:libloading::Symbol<unsafe extern "C" fn(*const c_char)->*mut c_void>=
            elf.library().get(symbols::elf_exec::EXEC_LOAD.as_bytes()).unwrap();
        let path=CString::new(root.join("guest.so").to_str().unwrap()).unwrap();
        assert!(load(path.as_ptr()).is_null(),"failure must revoke the event source");
    }
    let events=unsafe { guest::bring_up(&capabilities,&plan) }.unwrap();
    let before=fs::read_dir("/proc/self/fd").unwrap().count();
    unsafe {
        let address=runtime::guest_exec_symbol(events[0].base,"rudroid_native_events_probe").unwrap();
        let probe:unsafe extern "C" fn(*mut u64)->i32=std::mem::transmute(address);
        let mut iterations=0;
        assert_eq!(probe(&mut iterations),0,"guest NDK events/sensors ABI (failure is a C source line)");
        assert_eq!(iterations,2000);
        println!("GUEST_NATIVE_EVENTS threads={iterations} fd_before={before}");
        let address=runtime::guest_exec_symbol(events[0].base,"rudroid_native_events_cost").unwrap();
        let cost:unsafe extern "C" fn(*mut u64)->i32=std::mem::transmute(address);
        let mut samples=[0u64;20];
        assert_eq!(cost(samples.as_mut_ptr()),0,"ready-fd callback/cost probe");
        for (n,raw) in [1,16,64,256].into_iter().zip(samples.chunks_exact(5)) {
            let mut sorted=raw.to_vec(); sorted.sort_unstable();
            println!("NATIVE_EVENTS_COST fds={n} polls=32768 samples_ns={raw:?} median_ns_per_poll={}",sorted[2]/32768);
        }
    }
    assert_eq!(fs::read_dir("/proc/self/fd").unwrap().count(),before,"TLS exit/last release must recover epoll/eventfd");
    fs::remove_dir_all(root).unwrap();
}
