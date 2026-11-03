//! INPUT:  same-kit HostApiV0 configuration getters and plugin descriptor
//! OUTPUT:  il2cpp_probe descriptor and config-observing init callback
//! POS:    proves loader CLI config reaches separately compiled hook providers
//! il2cpp_hook CLI test fixture (ticket 50): a runtime-kind capability whose
//! init reads the legacy `[[il2cpp_patch]]` table through the Host API v0
//! config getters exactly the way the C++ reference plugin consumes it
//! (`config_array_size("il2cpp_patch")`, then
//! `il2cpp_patch.<index>.<field>` dotted reads), logging every observation
//! so the CLI integration test can assert field-level parity with the source
//! TOML. Deployed alongside the real il2cpp_patch capability it proves the
//! legacy table reaches hook providers through the actual CLI run mode.
//! Test fixture only: registers nothing, hooks nothing, touches no guest
//! memory.

use std::ffi::{c_int, CStr};

use rudroid_common::host_api::{HostApiV0, HOST_API_VERSION};
use rudroid_common::registry::HostStr;

rudroid_common::declare_descriptor!("il2cpp_probe", rudroid_common::Kind::Runtime);

const TAG: &CStr = c"PATCHPROBE";

fn log(api: &HostApiV0, message: &str) {
    assert!(!message.contains('\0'), "probe log lines are NUL-free");
    api.log_line(TAG, message);
}

/// `config_get_string` + the return flag, C++ `config_string()` style: a
/// 512-byte scratch buffer the getter fills with the fallback on a miss.
fn read_string(api: &HostApiV0, key: &CStr, fallback: &CStr) -> (c_int, String) {
    let (found, value) = api.config_string_with_fallback(key, fallback, 512)
        .expect("512-byte capacity fits the Host API");
    (c_int::from(found), value)
}

fn read_i64(api: &HostApiV0, key: &CStr, fallback: i64) -> i64 {
    api.config_i64(key, fallback)
}

fn read_f64(api: &HostApiV0, key: &CStr, fallback: f64) -> f64 {
    api.config_f64(key, fallback)
}

/// Raw getter: the probe's `-1` fallback makes a miss visible.
fn read_bool(api: &HostApiV0, key: &CStr, fallback: c_int) -> c_int {
    (api.config_get_bool)(HostStr::new(key), fallback)
}

#[no_mangle]
extern "C" fn rudroid_plugin_init(api: Option<&'static HostApiV0>) -> c_int {
    let Some(api) = api else {
        return -1;
    };
    if api.abi_version != HOST_API_VERSION {
        return -2;
    }

    let size = api.config_len(c"il2cpp_patch");
    log(api, &format!("size={size}"));

    let reads: [(&CStr, &CStr); 4] = [
        (c"il2cpp_patch.0.class", c"?"),
        (c"il2cpp_patch.0.namespace", c"?"),
        (c"il2cpp_patch.0.method", c"?"),
        (c"il2cpp_patch.0.mode", c"?"),
    ];
    let names = ["p0.class", "p0.namespace", "p0.method", "p0.mode"];
    for ((key, fallback), name) in reads.iter().zip(names) {
        let (ret, value) = read_string(api, key, fallback);
        log(api, &format!("{name}={value} ret={ret}"));
    }
    log(
        api,
        &format!("p0.argc={}", read_i64(api, c"il2cpp_patch.0.argc", -99)),
    );
    log(
        api,
        &format!("p0.arg={}", read_i64(api, c"il2cpp_patch.0.arg", -99)),
    );
    log(
        api,
        &format!("p0.field={}", read_i64(api, c"il2cpp_patch.0.field", -99)),
    );
    log(
        api,
        &format!("p0.mult={}", read_f64(api, c"il2cpp_patch.0.mult", -99.0)),
    );
    log(
        api,
        &format!(
            "p0.enabled={}",
            read_bool(api, c"il2cpp_patch.0.enabled", -1)
        ),
    );

    let (ret, class) = read_string(api, c"il2cpp_patch.1.class", c"?");
    log(api, &format!("p1.class={class} ret={ret}"));
    let (ret, mode) = read_string(api, c"il2cpp_patch.1.mode", c"?");
    log(api, &format!("p1.mode={mode} ret={ret}"));
    // Entry 1 carries a deliberate type error (`argc` is a string): the
    // getter must keep the probe's fallback.
    log(
        api,
        &format!("p1.argc={}", read_i64(api, c"il2cpp_patch.1.argc", -99)),
    );

    // Absent index: explicit miss (ret=0) with the fallback written through.
    let (ret, missing) = read_string(api, c"il2cpp_patch.9.class", c"MISSING");
    log(api, &format!("p9.class={missing} ret={ret}"));
    // Out-of-bounds numeric segment.
    log(
        api,
        &format!("oob.field={}", read_i64(api, c"il2cpp_patch.2.field", -99)),
    );
    // Segments the reference lexer rejects: negative and non-numeric.
    let (ret, neg) = read_string(api, c"il2cpp_patch.-1.class", c"NEG");
    log(api, &format!("neg.class={neg} ret={ret}"));
    let (ret, garbage) = read_string(api, c"il2cpp_patch.x.class", c"GARBAGE");
    log(api, &format!("garbage.class={garbage} ret={ret}"));
    // Leading zeros still parse like from_chars.
    let (ret, p00) = read_string(api, c"il2cpp_patch.00.class", c"?");
    log(api, &format!("p00.class={p00} ret={ret}"));

    // The new-dialect table stays readable through plain dotted keys.
    let (ret, hook) = read_string(api, c"il2cpp_hooks.extra_probe", c"?");
    log(api, &format!("hook.extra={hook} ret={ret}"));

    // config_set_string is an explicit v0 failure — never a silent write.
    let set_ret = (api.config_set_string)(HostStr::new(c"il2cpp_patch.0.class"), HostStr::new(c"HACKED"));
    log(api, &format!("set.ret={set_ret}"));
    let (ret, after) = read_string(api, c"il2cpp_patch.0.class", c"?");
    log(api, &format!("set.after={after} ret={ret}"));

    0
}
