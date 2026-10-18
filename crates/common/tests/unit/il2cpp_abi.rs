//! INPUT:  private items of `src/il2cpp_abi.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/il2cpp_abi.rs`; declared there with `#[path]`
use super::*;

#[cfg(target_arch = "aarch64")]
#[test]
fn exact_evidenced_build_selects_pointer_width() {
    assert_eq!(require_gc_handle_abi("6000.0.58f1"), Ok(GcHandleAbi::PointerWidth));
    assert_eq!(
        require_gc_handle_abi("6000.0.61f1"),
        Ok(GcHandleAbi::PointerWidth)
    );
}

#[test]
fn adjacent_builds_legacy_and_custom_suffixes_are_not_inferred() {
    for version in [
        "",
        "6000",
        "6000.0.57f1",
        "6000.0.59f1",
        "6000.0.58f1c1",
        "6000.0.62f1",
        "6000.1.61f1",
        "6000.0.61a1",
        "6000.0.61f1c1",
        "6000.0.61f1\0",
        "2021.3.16f1",
        "2020.3.1f1",
    ] {
        assert!(require_gc_handle_abi(version).is_err(), "{version}");
    }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
#[test]
fn stripped_metadata_requires_every_native_anchor_and_guard_word() {
    let limit = STRIPPED_GC_GUARDS.iter()
        .map(|(offset, words)| offset + words.len() * 4).max().unwrap();
    // u32 owns correctly aligned, stable storage for actual /proc/self/mem
    // reads. It is never executed or made into an Il2cpp function binding.
    let mut image = vec![0u32; (limit + 3) / 4];
    for &(offset, words) in STRIPPED_GC_GUARDS {
        image[offset / 4..offset / 4 + words.len()].copy_from_slice(words);
    }
    let base = image.as_ptr() as usize;
    let exports = STRIPPED_GC_EXPORTS.map(|offset| Some(base + offset));
    assert_eq!(stripped_gc_handle_abi(base, exports), Some(GcHandleAbi::U32));
    for index in 0..3 {
        let mut missing = exports;
        missing[index] = None;
        assert_eq!(stripped_gc_handle_abi(base, missing), None);
        let mut shifted = exports;
        shifted[index] = shifted[index].map(|v| v + 4);
        assert_eq!(stripped_gc_handle_abi(base, shifted), None);
    }
    for &(offset, words) in STRIPPED_GC_GUARDS {
        for index in 0..words.len() {
            image[offset / 4 + index] ^= 1;
            assert_eq!(stripped_gc_handle_abi(base, exports), None,
                "changed native word at {:#x}", offset + index * 4);
            image[offset / 4 + index] ^= 1;
        }
    }
    assert_eq!(stripped_gc_handle_abi(base, exports), Some(GcHandleAbi::U32));
}

#[test]
fn unavailable_or_overflowing_native_memory_has_no_abi() {
    for base in [0, 1, 4, usize::MAX - 3] {
        let exports = [0x8443c4usize, 0x844404, 0x84444c]
            .map(|offset| base.checked_add(offset));
        assert_eq!(stripped_gc_handle_abi(base, exports), None);
    }
}
