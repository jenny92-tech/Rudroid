//! INPUT:  private items of `src/gamepatch_support.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/gamepatch_support.rs`; declared there with `#[path]`
use super::*;

/// Phase C: new key wins; legacy is consulted only when the new key is
/// absent; default when both are absent (ticket 50 compat rule).
#[test]
fn override_prefers_new_then_legacy_then_default() {
    let table = std::sync::Mutex::new(Vec::<(String, String)>::new());
    let mut reader = |key: &str| -> Option<String> {
        table
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    let set = |k: &str, v: &str| {
        table
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|(key, _)| key != k);
        table
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((k.to_owned(), v.to_owned()));
    };

    assert_eq!(override_setting(&mut reader, "new", "old", "dflt".to_owned()), "dflt");
    set("old", "legacy");
    assert_eq!(
        override_setting(&mut reader, "new", "old", "dflt".to_owned()),
        "legacy",
        "legacy fallback when the new key is absent"
    );
    set("new", "modern");
    assert_eq!(
        override_setting(&mut reader, "new", "old", "dflt".to_owned()),
        "modern",
        "new key shadows the legacy fallback"
    );
}
