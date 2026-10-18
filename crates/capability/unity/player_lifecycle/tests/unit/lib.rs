//! INPUT:  private items of `src/lib.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/lib.rs`; declared there with `#[path]`
use super::*;
use rudroid_common::registry::RUDROID_UNITY_PROFILE_NONE;

const CONTEXT_SPEC: Spec = Spec {
    id: "unity_test",
    log_tag: c"UNITYTEST",
    registered_message: "test profile registered",
    player_class: "com/unity3d/player/UnityPlayer",
    player_constructor_signature: "()V",
    init_jni_signature: INIT_JNI_CONTEXT,
    flags: RUDROID_UNITY_PROFILE_NONE,
    init_jni_int_arg: 0,
    render_sleep_us: 0,
    extra_player_callbacks: &[],
    publishes_current_activity: false,
    register_java_services: None,
};

#[test]
fn a_consistent_spec_validates() {
    assert_eq!(CONTEXT_SPEC.validate(), Ok(()));
    let int_shape = Spec {
        init_jni_signature: INIT_JNI_CONTEXT_INT,
        flags: RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG,
        extra_player_callbacks: &[PlayerCallback { name: c"initializeGoogleAr", body: CallbackBody::False }],
        ..CONTEXT_SPEC
    };
    assert_eq!(int_shape.validate(), Ok(()));
}

#[test]
fn inconsistent_specs_are_refused_before_registration() {
    let unknown_shape = Spec { init_jni_signature: "(II)V", ..CONTEXT_SPEC };
    assert!(unknown_shape.validate().unwrap_err().contains("no dispatch shape"));
    let flag_mismatch = Spec { init_jni_signature: INIT_JNI_CONTEXT_INT, ..CONTEXT_SPEC };
    assert!(flag_mismatch.validate().unwrap_err().contains("disagree"));
    let stray_flag = Spec { flags: RUDROID_UNITY_PROFILE_INIT_JNI_INT_ARG, ..CONTEXT_SPEC };
    assert!(stray_flag.validate().unwrap_err().contains("disagree"));
    let duplicate = Spec {
        extra_player_callbacks: &[PlayerCallback { name: c"loadLibrary", body: CallbackBody::False }],
        ..CONTEXT_SPEC
    };
    assert!(duplicate.validate().unwrap_err().contains("registered twice"));
    let sleep = Spec { render_sleep_us: 1_000_001, ..CONTEXT_SPEC };
    assert!(sleep.validate().unwrap_err().contains("render_sleep_us"));
    let no_tag = Spec { log_tag: c"", ..CONTEXT_SPEC };
    assert!(no_tag.validate().unwrap_err().contains("log_tag"));
}

#[test]
fn common_callbacks_are_unique_and_shaped_by_their_body() {
    for (index, callback) in COMMON_PLAYER_CALLBACKS.iter().enumerate() {
        assert!(
            COMMON_PLAYER_CALLBACKS[index + 1..].iter().all(|other| other.name != callback.name),
            "{:?} listed twice",
            callback.name
        );
    }
    assert_eq!(CallbackBody::DrainMainThreadJobs.signature(), c"()V");
    assert_eq!(CallbackBody::False.signature(), c"()Z");
    assert_eq!(CallbackBody::LibraryUnavailable.signature(), c"(Ljava/lang/String;)Z");
    assert_eq!(CallbackBody::NoActivityIndicator.signature(), c"()V");
    assert_eq!(CallbackBody::IgnoredActivityIndicator.signature(), c"(I)V");
}

#[test]
fn every_callback_body_matches_its_native_prototype() {
    let extra = PlayerCallback { name: c"initializeGoogleAr", body: CallbackBody::False };
    for callback in COMMON_PLAYER_CALLBACKS.iter().chain([&extra]) {
        assert_eq!(callback.body.native(callback.name).name(), callback.name);
    }
}

#[test]
fn render_sleep_clamp_matches_the_reference_policy() {
    assert_eq!(clamp_render_sleep(16_000, 0), 16_000);
    assert_eq!(clamp_render_sleep(0, 0), 0);
    assert_eq!(clamp_render_sleep(1_000_000, 0), 1_000_000);
    assert_eq!(clamp_render_sleep(1_000_001, 7), 7, "above the clamp: spec fallback");
    assert_eq!(clamp_render_sleep(-1, 7), 7, "negative: spec fallback");
}

#[test]
fn registration_contract_names_its_requirements() {
    assert_eq!(HOST_API_VERSION, 4);
}
