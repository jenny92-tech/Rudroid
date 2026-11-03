//! INPUT:  private teardown gate, synthetic idle image and typed HostApi export lookup
//! OUTPUT:  exact-image acceptance, mismatch/overflow/unreadable rejection and both-call preservation tests
//! POS:    exercises startup installation without executing or distributing vendor code

use super::*;
use rudroid_common::arm64_detour::patch_word;
use rudroid_common::registry::HostStr;
use std::ffi::c_void;

extern "C" fn symbols(module: *mut c_void, name: HostStr<'_>) -> usize {
    EXPORTS.iter().find(|(key, _)| Some(*key) == name.to_cstr())
        .and_then(|(_, offset)| (module as usize).checked_add(*offset)).unwrap_or(0)
}

extern "C" fn absent(_: *mut c_void, _: HostStr<'_>) -> usize { 0 }

fn api() -> HostApiV0 {
    HostApiV0 { so_symbol: symbols, ..HostApiV0::stubbed() }
}

#[test]
fn unknown_exports_overflow_and_unmapped_guards_never_patch() {
    let unknown = HostApiV0 { so_symbol: absent, ..api() };
    // SAFETY: unknown or absent ranges may be refused; there are no live
    // image mappings or concurrent owners at these sentinel addresses.
    unsafe {
        assert_eq!(install(&unknown, 0x1000), Ok(false));
        assert_eq!(install(&api(), 0), Ok(false));
        assert_eq!(install(&api(), usize::MAX & !0xfff), Ok(false));
        assert!(matches!(install(&api(), 0x1000), Err(DetourError::Protection(_))));
    }
}

#[test]
fn exact_image_checks_every_guard_and_preserves_both_cleanup_calls() {
    // One private, leaked image owns complete 64KiB pages. Only the few guard
    // pages are touched; this fixture is never entered as executable code.
    let layout = std::alloc::Layout::from_size_align(0x700000, 0x10000).unwrap();
    // SAFETY: nonzero layout; kept mapped until the test process exits.
    let base = unsafe { std::alloc::alloc_zeroed(layout) } as usize;
    assert_ne!(base, 0);
    let pieces: [(usize, &[u32]); 5] = [
        (0x200, &BUILD_ID), (0x67e478, &PARENT),
        (0x67664c, &CACHE_CLEANUP), (0x530c40, &MEMORY_CLEANUP),
        (0x69473c, &PLAYER_TEARDOWN),
    ];
    // SAFETY: all offsets and word slices lie inside this private RW image.
    unsafe {
        for (offset, words) in pieces {
            std::ptr::copy_nonoverlapping(words.as_ptr(), (base + offset) as *mut u32, words.len());
        }
        // A matching image with a non-executable target still refuses writes.
        assert!(matches!(install(&api(), base), Err(DetourError::Protection(_))));
        patch_word(base + SITE, PARENT[4]).unwrap();
        // Corrupt metadata, parent context and each callee separately. Matching
        // branch words alone never authorize a different/modified image.
        for (offset, words) in pieces {
            patch_word(base + offset, words[0] ^ 1).unwrap();
            assert_eq!(install(&api(), base), Ok(false), "guard at {offset:#x}");
            assert_eq!((base.wrapping_add(SITE) as *const u32).read(), PARENT[4]);
            assert_eq!(((base + SITE + 4) as *const u32).read(), PARENT[5]);
            patch_word(base + offset, words[0]).unwrap();
        }
        let unknown = HostApiV0 { so_symbol: absent, ..api() };
        assert_eq!(install(&unknown, base), Ok(false));
        assert_eq!(install(&api(), base), Ok(true));
        let patched = std::slice::from_raw_parts((base + SITE) as *const u32, 2);
        assert_eq!(patched, REORDERED);
        // The new BLs still reach exactly the two original cleanup bodies.
        assert_eq!(rudroid_common::arm64_detour::decode_b(patched[0], SITE), 0x67664c);
        assert_eq!(rudroid_common::arm64_detour::decode_b(patched[1], SITE + 4), 0x530c40);
        assert_eq!(install(&api(), base), Ok(false), "already modified images are not patched twice");
        assert_eq!(std::slice::from_raw_parts((base + 0x530c40) as *const u32, MEMORY_CLEANUP.len()), MEMORY_CLEANUP);
        assert_eq!(std::slice::from_raw_parts((base + 0x67664c) as *const u32, CACHE_CLEANUP.len()), CACHE_CLEANUP);
    }
}
