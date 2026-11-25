//! Native bridge alias and P/Invoke round trip against real ARM64 DSOs.

use super::*;
use std::fs;
use std::process::Command;

fn compile(dir: &Path, file: &str, source: &str, guest: bool) {
    let c_file = dir.join("fixture.c");
    fs::write(&c_file, source).unwrap();
    let mut cc = Command::new("cc");
    cc.args(["-shared", "-fPIC", "-Wall", "-Wextra", "-Werror"]);
    if guest {
        cc.arg("-nostdlib");
    }
    let result = cc
        .arg("-o")
        .arg(dir.join(file))
        .arg(c_file)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}

#[test]
fn native_alias_and_scoped_pinvoke() {
    let root = std::env::temp_dir().join(format!("rudroid-mono-{}", std::process::id()));
    let guest = root.join("lib/arm64-v8a");
    let native = root.join("native");
    fs::create_dir_all(&guest).unwrap();
    fs::create_dir_all(&native).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(root.clone());
    compile(
        &native,
        "libmonobdwgc-2.0.so",
        r#"
void mono_jit_init_version(void) {}
void mono_runtime_invoke(void) {}
void mono_add_internal_call(void) {}
void mono_gc_collect(void) {}
void mono_gc_get_used_size(void) {}
void mono_gc_get_heap_size(void) {}
"#,
        false,
    );
    compile(
        &native,
        "libil2cpp.so",
        r#"
#include <stdbool.h>
#include <string.h>
struct host {
    int size;
    void *(*open)(const char *, int);
    void *(*lookup)(void *, const char *);
    const char *libdir, *unity_version, *managed;
    int slice;
};
static struct host callbacks;
int ua_mono_setup(const struct host *host) {
    if (host->size != sizeof(*host) || strcmp(host->unity_version, "2022.3.1f1")) return 9;
    callbacks = *host;
    return 0;
}
void il2cpp_runtime_invoke(void) {}
int il2cpp_init(void) {
    void *plugin = callbacks.open("ExamplePlugin", 0);
    void *steam = callbacks.open("libsteam_api.so", 0);
    if (!plugin || !steam || callbacks.open("SavedGames", 0)) return -1;
    int (*test)(int) = callbacks.lookup(plugin, "test_fn");
    bool (*online)(void) = callbacks.lookup(steam, "SteamAPI_Init");
    if (!test || !online || online() || callbacks.lookup(steam, "missing")
        || callbacks.lookup(plugin, "il2cpp_init")) return -2;
    return test(42);
}
"#,
        false,
    );
    compile(
        &guest,
        "libtest.so",
        "int test_fn(int value) { return value + 100; }",
        true,
    );
    let bridge = c_path(&native.join("libil2cpp.so")).unwrap();
    let runtime = c_path(&native.join("libmonobdwgc-2.0.so")).unwrap();
    let managed = c_path(&root).unwrap();
    let guest_dir = c_path(&guest).unwrap();
    let map_path = root.join("plugins.map");
    fs::write(&map_path, "map Missing lib libmissing.so\n").unwrap();
    assert!(load_plugins(&map_path, &guest, &native).is_err());
    fs::write(
        &map_path,
        "map steam_api stub steam\nmap libsteam_api none\n",
    )
    .unwrap();
    assert!(load_plugins(&map_path, &guest, &native).is_err());
    fs::write(&map_path, "map ExamplePlugin lib libtest.so\nmap libsteam_api stub steam\nmap steam_api stub steam\nmap SavedGames none\n").unwrap();
    let map = c_path(&map_path).unwrap();
    let args = MonoStartup {
        bridge: HostStr::new(&bridge),
        runtime: HostStr::new(&runtime),
        managed_dir: HostStr::new(&managed),
        guest_lib_dir: HostStr::new(&guest_dir),
        unity_version: HostStr::new(c"2022.3.1f1"),
        plugins_map: HostStr::new(&map),
        frame_pacing: rudroid_unity_common::mono_abi::FRAME_PACING_ANDROID,
        display_refresh_hz: 0,
    };
    extern "C" fn register(alias: &NativeAlias<'_>) -> c_int {
        let [base, _, guest, native] = alias.names;
        if base.to_cstr() != Some(ALIAS)
            || !guest
                .to_cstr()
                .unwrap()
                .to_bytes()
                .ends_with(b"/lib/arm64-v8a/libil2cpp.so")
            || !native
                .to_cstr()
                .unwrap()
                .to_bytes()
                .ends_with(b"/native/libil2cpp.so")
            || (alias.symbol)(HostStr::new(c"ua_mono_setup")) != 0
        {
            return -1;
        }
        0
    }
    extern "C" fn fake_open(name: HostStr<'_>, _: c_int) -> *mut c_void {
        let name = name.to_cstr().unwrap();
        usize::from(name.to_bytes().ends_with(b"/lib/arm64-v8a/libtest.so")) as *mut c_void
    }
    extern "C" fn fake_lookup(handle: *mut c_void, name: HostStr<'_>) -> *mut c_void {
        extern "C" fn add(value: c_int) -> c_int {
            value + 100
        }
        if handle as usize == 1 && name.to_cstr() == Some(c"test_fn") {
            add as *mut c_void
        } else {
            std::ptr::null_mut()
        }
    }
    static API: GuestLibdl = GuestLibdl {
        register_alias: register,
        open: fake_open,
        lookup: fake_lookup,
    };
    assert_eq!(bridge_symbol(HostStr::new(c"il2cpp_init")), 0);
    assert_eq!(unsafe { rudroid_unity_mono_start(&args, &API, None) }, 0);
    let symbol = bridge_symbol(HostStr::new(c"il2cpp_init"));
    assert_ne!(symbol, 0);
    let init: extern "C" fn() -> c_int = unsafe { std::mem::transmute(symbol) };
    assert_eq!(init(), 142);
    assert_eq!(init(), 142);
}
