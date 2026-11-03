//! INPUT:  private items of `src/select.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/select.rs`; declared there with `#[path]`
use super::*;
use rudroid_common::DeployEntry;
use std::path::PathBuf;

fn entry(stem: &str) -> DeployEntry {
    DeployEntry {
        path: PathBuf::from(format!("/kit/lib{stem}.so")),
        stem: stem.to_owned(),
    }
}

#[test]
fn no_plugins_section_loads_everything() {
    let config = HostConfig::parse("").unwrap();
    let selection = Selection::from_config(&config).unwrap();
    assert_eq!(selection, Selection::All);

    let entries = vec![entry("a"), entry("b")];
    let chosen = selection.apply(entries).unwrap();
    assert_eq!(chosen.len(), 2);
}

#[test]
fn allowlist_selects_subset_in_order() {
    let config = HostConfig::parse("[plugins]\nenabled = [\"log_stderr\", \"android_base\"]\n")
        .unwrap();
    let selection = Selection::from_config(&config).unwrap();
    let entries = vec![entry("android_base"), entry("unity_6"), entry("log_stderr")];
    let chosen = selection.apply(entries).unwrap();
    let stems: Vec<&str> = chosen.iter().map(|e| e.stem.as_str()).collect();
    // Allowlist order, not scan order.
    assert_eq!(stems, ["log_stderr", "android_base"]);
}

#[test]
fn allowlist_missing_id_is_fatal_and_actionable() {
    let config =
        HostConfig::parse("[plugins]\nenabled = [\"android_base\", \"fmod\"]\n").unwrap();
    let err = Selection::from_config(&config)
        .unwrap()
        .apply(vec![entry("android_base"), entry("log_stderr")])
        .unwrap_err();
    let message = err.to_string();
    assert!(message.contains("fmod"));
    assert!(message.contains("android_base, log_stderr"));
}

#[test]
fn selection_reads_device_config_with_extra_sections() {
    // Silksong device TOML has no [plugins] section: everything deploys.
    let manifest = env!("CARGO_MANIFEST_DIR");
    let config = HostConfig::from_path(format!(
        "{manifest}/../../../work/config.silksong.device.toml"
    ))
    .unwrap();
    assert_eq!(Selection::from_config(&config).unwrap(), Selection::All);
}

// Ticket 83: the reference allowlist validation rules
// (`plugin_host.cpp` invalid_plugin_allowlist /
// duplicate_plugin_allowlist_entry).

#[test]
fn allowlist_empty_entry_is_rejected() {
    let config = HostConfig::parse("[plugins]\nenabled = [\"android_base\", \"\"]\n").unwrap();
    let err = Selection::from_config(&config).unwrap_err();
    assert!(
        err.to_string()
            .contains("must be non-empty strings"),
        "{err}"
    );
}

#[test]
fn allowlist_path_shaped_entry_is_rejected() {
    let config = HostConfig::parse(
        "[plugins]\nenabled = [\"plugins/libfmod.so\"]\n",
    )
    .unwrap();
    let err = Selection::from_config(&config).unwrap_err();
    assert!(err.to_string().contains("not paths"), "{err}");
}

#[test]
fn allowlist_so_suffix_is_stripped_before_matching() {
    let config =
        HostConfig::parse("[plugins]\nenabled = [\"android_base.so\"]\n").unwrap();
    let selection = Selection::from_config(&config).unwrap();
    let chosen = selection
        .apply(vec![entry("android_base"), entry("log_stderr")])
        .unwrap();
    assert_eq!(chosen.len(), 1);
    assert_eq!(chosen[0].stem, "android_base");
}

#[test]
fn allowlist_duplicate_entry_is_rejected() {
    let config = HostConfig::parse(
        "[plugins]\nenabled = [\"android_base\", \"android_base.so\"]\n",
    )
    .unwrap();
    let err = Selection::from_config(&config).unwrap_err();
    let message = err.to_string();
    assert!(message.contains("duplicate plugin"), "{message}");
    assert!(message.contains("android_base"), "{message}");
}
