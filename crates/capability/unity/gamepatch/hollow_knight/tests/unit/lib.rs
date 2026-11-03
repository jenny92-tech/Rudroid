//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;

#[test]
fn ortho_cache_keeps_repeated_values_stable_and_refuses_invalid_or_excess_entries() {
    *lock(&ORTHO_SLOTS) = [(0, 0.0, 0.0); 16];
    let scale = 4.0 / 3.0;
    for id in 1..=16 {
        let patched = remember_ortho(id, 2.0, scale).unwrap();
        assert_eq!(patched, 2.0 * scale);
        for _ in 0..32 { assert_eq!(remember_ortho(id, patched, scale), Some(patched)); }
    }
    assert_eq!(remember_ortho(17, 2.0, scale), None, "full cache must not compound untracked values");
    assert_eq!(remember_ortho(1, 3.0, scale), Some(4.0), "legitimate native changes replace the old base");
    for invalid in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert_eq!(remember_ortho(1, invalid, scale), None);
    }
    assert_eq!(remember_ortho(0, 2.0, scale), None);
    assert_eq!(remember_ortho(1, f32::MAX, scale), None);
}

#[test]
fn capped_log_does_not_format_suppressed_messages() {
    let calls = std::cell::Cell::new(0);
    for _ in 0..4 {
        bounded_log("test_lazy_log", 1, || {
            calls.set(calls.get() + 1);
            String::new()
        });
    }
    assert_eq!(calls.get(), 1);
    bounded_log("test_disabled_log", 0, || panic!("disabled log formatted"));
}

fn config(body: &str) -> HostConfig {
    HostConfig::parse(&format!("[package]\npackageName = \"com.teamcherry.hollowknight\"\n{body}"))
        .unwrap()
}

/// Reference parity: the HK patch is UNGATED — it applies to every
/// package (the gate-conflict decision keeps the reference behavior and
/// isolates Titles by deployment, not by code).
#[test]
fn ungated_patch_applies_to_any_package() {
    let hk = config("");
    assert!(applies_to(&hk));
    let silksong =
        HostConfig::parse("[package]\npackageName = \"com.game.silksong\"\n").unwrap();
    assert!(applies_to(&silksong), "reference parity: HK patch is ungated");
    let no_package = HostConfig::parse("").unwrap();
    assert!(applies_to(&no_package));
}

#[test]
fn reference_switch_defaults() {
    let features = ViewportFeatures::from_config(&config(""));
    assert_eq!(
        features,
        ViewportFeatures {
            debug: false,
            force_full_viewport: true,
            tk2d_width_safe: true,
            ui_orthographic_width_safe: true,
        }
    );
}

/// The reference fallback chain: legacy `hk_viewport.<name>` first, the
/// `game_patches.hollow_knight_viewport.<name>` key wins when present.
#[test]
fn legacy_prefix_then_new_key_wins() {
    // Legacy dialect only.
    let legacy = config("[hk_viewport]\nforce_full_viewport = false\n");
    let features = ViewportFeatures::from_config(&legacy);
    assert!(!features.force_full_viewport);
    assert!(features.tk2d_width_safe);
    // New key wins over legacy.
    let both = config(
        "[hk_viewport]\nforce_full_viewport = false\n[game_patches.hollow_knight_viewport]\nforce_full_viewport = true\n",
    );
    assert!(ViewportFeatures::from_config(&both).force_full_viewport);
    // New-key-only dialect.
    let new_only = config("[game_patches.hollow_knight_viewport]\ntk2d_width_safe = false\ndebug = true\n");
    let features = ViewportFeatures::from_config(&new_only);
    assert!(!features.tk2d_width_safe);
    assert!(features.debug);
    assert!(features.force_full_viewport);
}

/// The override chain's boundary semantics, pinned (ticket 87 Phase C):
/// an explicit `false` at the new key WINS (a present value always
/// beats the fallback chain), and a present-but-non-boolean value falls
/// through exactly like a missing key — at the new key it degrades to
/// the legacy value, at the legacy key to the default.
#[test]
fn override_chain_boundaries_explicit_false_and_type_errors() {
    // Explicit false at the new key wins over a legacy true.
    let cfg = config(
        "[hk_viewport]\ntk2d_width_safe = true\n[game_patches.hollow_knight_viewport]\ntk2d_width_safe = false\n",
    );
    assert!(!ViewportFeatures::from_config(&cfg).tk2d_width_safe);
    // Non-boolean at the new key: falls through to the legacy value.
    let cfg = config(
        "[hk_viewport]\ntk2d_width_safe = false\n[game_patches.hollow_knight_viewport]\ntk2d_width_safe = \"nope\"\n",
    );
    assert!(!ViewportFeatures::from_config(&cfg).tk2d_width_safe);
    // Non-boolean at BOTH keys: the default wins.
    let cfg = config(
        "[hk_viewport]\nforce_full_viewport = \"nope\"\n[game_patches.hollow_knight_viewport]\nforce_full_viewport = 3\n",
    );
    assert!(
        ViewportFeatures::from_config(&cfg).force_full_viewport,
        "type errors at both keys must reach the default (true)"
    );
    // Legacy integer 0 is a convertible false (get_bool's rule).
    let cfg = config("[hk_viewport]\nui_orthographic_width_safe = 0\n");
    assert!(!ViewportFeatures::from_config(&cfg).ui_orthographic_width_safe);
}

/// The registry rows carry the reference triggers/targets/defaults, and
/// the switch views follow the config (tk2d_width_safe also arms with
/// debug, the always-on group has no key).
#[test]
fn plan_registers_the_reference_set() {
    let features = ViewportFeatures::from_config(&config(""));
    let entries = plan(&features);
    let behaviors: Vec<&str> = entries.iter().map(|p| p.entry.behavior).collect();
    for expected in [
        "force_full_viewport (camera rect)",
        "force_full_viewport (AutoScaleViewport override)",
        "ui_orthographic_width_safe",
        "tk2d_width_safe",
        "overscan pass-through logging (always-on group)",
        "GameSettings overscan logging",
    ] {
        assert!(behaviors.contains(&expected), "plan misses {expected}");
    }
    // Defaults: everything armed except debug-only extras.
    assert!(entries.iter().all(|p| p.enabled));
    assert_eq!(entries[0].entry.reference_default, true);
    assert_eq!(entries[0].entry.trigger, Trigger::Il2cppPostInit);
    assert!(entries[0].entry.target.contains("set_rect_Injected"));
    // Switched-off views.
    let off = ViewportFeatures::from_config(
        &config("[game_patches.hollow_knight_viewport]\nforce_full_viewport = false\ntk2d_width_safe = false\nui_orthographic_width_safe = false\n"),
    );
    let entries_off = plan(&off);
    let enabled: Vec<bool> = entries_off.iter().map(|p| p.enabled).collect();
    assert_eq!(
        enabled,
        [false, false, false, false, true, true],
        "switched-off hooks leave the always-on group + registered-only rows"
    );
    // debug arms the tk2d hook even with tk2d_width_safe off.
    let debug_only = ViewportFeatures::from_config(
        &config("[game_patches.hollow_knight_viewport]\ntk2d_width_safe = false\ndebug = true\n"),
    );
    assert!(
        plan(&debug_only)
            .iter()
            .find(|p| p.entry.behavior == "tk2d_width_safe")
            .unwrap()
            .enabled
    );
}

/// Switched-off features produce Skipped events (and never detours).
#[test]
fn plan_skips_disabled_hooks() {
    let off = ViewportFeatures {
        debug: false,
        force_full_viewport: false,
        tk2d_width_safe: false,
        ui_orthographic_width_safe: false,
    };
    let entries = plan(&off);
    let hook_entries: Vec<bool> = entries
        .iter()
        .map(|p| matches!(p.entry.action, Action::Hook(_)))
        .collect();
    assert_eq!(hook_entries, [true, true, true, true, true, false]);
}

/// The hooks wait for `Il2cppReady`; without il2cpp_patch in the kit the
/// patch would arm silently and never install, so the loader must refuse it.
#[test]
fn descriptor_requires_il2cpp_patch() {
    // SAFETY: the descriptor is a process-lifetime static with C strings.
    let descriptor = unsafe { &*crate::rudroid_plugin_descriptor() };
    let required = unsafe { std::ffi::CStr::from_ptr(descriptor.required_plugins) };
    assert_eq!(required.to_str().unwrap(), "il2cpp_patch");
}
