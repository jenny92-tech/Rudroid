//! INPUT:  pure stick/trigger conversion, hysteresis and reference direction dialect
//! OUTPUT:  exact endpoints, neutral jitter, release and direction alias regressions
//! POS:    verifies input normalization without SDL devices or JNI pointers
use super::*;
#[test]
fn axis_endpoints_and_hysteresis_preserve_physical_motion() {
    assert_eq!(normalized(i16::MIN, false), -1.0);
    assert_eq!(normalized(i16::MAX, false), 1.0);
    assert_eq!(normalized(i16::MIN, true), 0.0);
    assert_eq!(normalized(i16::MAX, true), 1.0);
    assert!(!direction_down(0.5, false, true, false));
    assert!(direction_down(0.7, false, true, false));
    assert!(direction_down(0.5, false, true, true));
    assert!(!direction_down(0.2, false, true, true));
    assert!(direction_down(-1.0, false, false, false));
    assert!(!direction_down(-1.0, false, true, false));
    assert!(direction_down(0.6, true, true, false));
    assert!(!direction_down(0.3, true, true, true));
}
#[test]
fn direction_aliases_and_defaults_preserve_reference_trigger_buttons() {
    assert_eq!(direction("DPAD_LEFT"), Some(2));
    assert_eq!(direction("dpup"), Some(0));
    assert_eq!(direction("bad"), None);
    let p = InputPolicy::default();
    assert_eq!(p.axes[8], Some(104));
    assert_eq!(p.axes[9], Some(105));
    assert!(p.startup.is_empty());
}

#[test]
fn normalized_identity_is_opt_in_and_does_not_remap_controls() {
    let mut policy = InputPolicy::default();
    assert_eq!(policy.gamepad_identity("Physical controller"), ("Physical controller", 0, 0));
    let buttons = policy.buttons;
    let axes = policy.axes;
    policy.android_xbox360_identity = true;
    assert_eq!(policy.gamepad_identity("Physical controller"), ("Xbox 360 Controller", 0x045e, 0x028e));
    assert_eq!(policy.buttons, buttons);
    assert_eq!(policy.axes, axes);
}
