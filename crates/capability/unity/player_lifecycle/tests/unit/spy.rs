//! INPUT:  private items of `src/spy.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/spy.rs`; declared there with `#[path]`
use super::*;

#[test]
fn spy_event_kinds_have_stable_names() {
    assert_eq!(kind_name(KIND_NATIVE_LOADER_LOAD), "NATIVE_LOADER_LOAD");
    assert_eq!(kind_name(KIND_RENDER), "RENDER");
    assert_eq!(kind_name(999), "UNKNOWN");
    let mut event = Event { kind: 1, a: 2, b: 3, text: [0; 48] };
    event.text[..4].copy_from_slice(b"dir/");
    assert_eq!(event.text_lossy(), "dir/");
}
