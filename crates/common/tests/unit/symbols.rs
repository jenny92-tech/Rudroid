//! INPUT:  private items of `src/symbols.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/symbols.rs`; declared there with `#[path]`
use super::*;

#[test]
fn symbol_names_match_reference_contract() {
    assert_eq!(DESCRIPTOR, "rudroid_plugin_descriptor");
    assert_eq!(EARLY_INIT, "rudroid_plugin_early_init");
    assert_eq!(REGISTER_JNI, "rudroid_plugin_register_jni");
    assert_eq!(INIT, "rudroid_plugin_init");
    // Descriptor first (queried before any phase runs), then phases.
    assert_eq!(PHASE_SYMBOLS, [EARLY_INIT, REGISTER_JNI, INIT]);
}

#[test]
fn catalog_symbols_share_prefix() {
    for symbol in reply_catalog::ALL {
        assert!(symbol.starts_with("rudroid_reply_catalog_"));
    }
}
