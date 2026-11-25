//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  Spec and empty-Host-API registration regressions
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use rudroid_unity_common::RUDROID_UNITY_PROFILE_NONE;

#[test]
fn spec_matches_observed_unity_2022_literal() {
    assert_eq!(ENGINE_SPEC.id, "unity_2022_3");
    assert_eq!(ENGINE_SPEC.log_tag, c"UNITY2022");
    assert_eq!(
        ENGINE_SPEC.registered_message,
        "Unity 2022.3 JNI and early engine profile registered"
    );
    assert_eq!(ENGINE_SPEC.player_class, "com/unity3d/player/UnityPlayer");
    assert_eq!(
        ENGINE_SPEC.player_constructor_signature,
        "(Landroid/content/Context;)V"
    );
    assert_eq!(ENGINE_SPEC.init_jni_signature, INIT_JNI_CONTEXT);
    assert_eq!(ENGINE_SPEC.flags, RUDROID_UNITY_PROFILE_NONE);
    assert_eq!(ENGINE_SPEC.init_jni_int_arg, 0);
    assert_eq!(ENGINE_SPEC.render_sleep_us, 0);
    assert!(ENGINE_SPEC.extra_player_callbacks.is_empty());
    assert!(ENGINE_SPEC.publishes_current_activity);
    assert!(ENGINE_SPEC.register_java_services.is_some());
    assert_eq!(ENGINE_SPEC.validate(), Ok(()));
}

#[test]
fn register_jni_rejects_empty_api() {
    assert_eq!(rudroid_plugin_register_jni(None), -1);
}
