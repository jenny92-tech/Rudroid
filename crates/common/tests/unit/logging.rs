//! INPUT: diagnostic macros and an explicit process diagnostics setting
//! OUTPUT: error classification and disabled-formatting regression assertions
//! POS: verifies that quiet diagnostics do no formatting work
use crate::logging::is_error_tag;

#[test]
fn errors_are_explicit_and_not_inferred_from_message_words() {
    assert!(is_error_tag("ERROR"));
    assert!(is_error_tag("FATAL"));
    assert!(!is_error_tag("SDL"));
    assert!(!is_error_tag("some error counter"));
}

#[test]
fn disabled_diagnostics_do_not_evaluate_arguments() {
    let touched = std::cell::Cell::new(false);
    crate::diagnostic!("{}", { touched.set(true); "diagnostic" });
    assert_eq!(touched.get(), crate::logging::diagnostics_enabled());
}
