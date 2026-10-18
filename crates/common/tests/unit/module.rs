//! INPUT:  private items of `src/module.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/module.rs`; declared there with `#[path]`
use super::*;

#[test]
fn discriminants_match_reference() {
    assert_eq!(ModuleKind::Unknown as u32, 0);
    assert_eq!(ModuleKind::Main as u32, 1);
    assert_eq!(ModuleKind::Il2cpp as u32, 2);
    assert_eq!(ModuleKind::Unity as u32, 3);
    assert_eq!(ModuleKind::Burst as u32, 4);
    assert_eq!(ModuleKind::Fmod as u32, 5);
    assert_eq!(ModuleKind::FmodStudio as u32, 6);
    assert_eq!(ModuleKind::UnityHelpers as u32, 7);
    assert_eq!(ModuleKind::Xlua as u32, 8);
    // Reference range ends at 8; the Rust lifecycle pseudo-event sits
    // above it and must never collide with a guest module kind.
    assert_eq!(ModuleKind::Il2cppReady as u32, 100);
    for kind in ModuleKind::ALL {
        assert_eq!(ModuleKind::from_u32(kind as u32), Some(kind));
    }
    assert_eq!(ModuleKind::from_u32(9), None);
    assert_eq!(ModuleKind::UnityFrame as u32, 103);
    assert_eq!(ModuleKind::from_u32(104), None);
}
