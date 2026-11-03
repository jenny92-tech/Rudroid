//! INPUT:  pinned, idle Unity image, exact export anchors and checked common code patcher
//! OUTPUT:  evidence-gated cleanup repair and terminal-lifecycle identity for one Unity 2020.2.2f1 image
//! POS:    version-owned native lifetime fix; no title lookup, teardown bypass or frame work

use rudroid_common::arm64_detour::{patch_words_if_matches, CodeGuard, DetourError};
use rudroid_common::host_api::HostApiV0;

pub(crate) static INSTALL_ATTEMPTED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

// Native evidence and device A/B: tests/fixtures/teardown-order.md. No nearby
// Unity build may inherit these offsets. All three exports, the GNU build ID,
// the complete parent and both cleanup bodies must match before any write.
const EXPORTS: [(&std::ffi::CStr, usize); 3] = [
    (c"JNI_OnLoad", 0x695274),
    (c"JNI_OnUnload", 0x695448),
    (c"UnitySendMessage", 0x41901c),
];
const BUILD_ID: [u32; 9] = [
    4, 20, 3, 0x00554e47, // NT_GNU_BUILD_ID note header and "GNU\0".
    0xeea95aca, 0x80eec7e1, 0x7c99619c, 0x8f310fe2, 0xff1d3672,
];
const PARENT: [u32; 12] = [
    0xf81f0ffe, 0x97ffa161, 0x97ffa408, 0x97fff648, 0x97fac9ee, 0x97ffe070,
    0xb0008848, 0xb0008849, 0x3912c11f, 0x3913613f, 0xf84107fe, 0xd65f03c0,
];
const CACHE_CLEANUP: [u32; 22] = [
    0xa9bf7bf3, 0x90008888, 0x911d4108, 0xf9400513, 0xa9007d1f, 0xb40001b3,
    0x91002268, 0xc85ffd09, 0xd100052a, 0xc80bfd0a, 0x35ffffab, 0xb50000e9,
    0xf9400268, 0xaa1303e0, 0xf9400908, 0xd63f0100, 0xaa1303e0, 0x942f1903,
    0x90008880, 0x911c4000, 0xa8c17bf3, 0x17f4f45d,
];
const MEMORY_CLEANUP: [u32; 44] = [
    0xd10183ff, 0xf90023f4, 0xa9057bf3, 0xd53bd053, 0xf9401668, 0xf9001fe8,
    0xf0009248, 0x3967e109, 0x34000309, 0x3927e11f, 0x9401ceb1, 0x97fbccaa,
    0x97fbdc8d, 0x97fbcca8, 0xd0007108, 0x91340508, 0xa9007fe8, 0x910043e8,
    0x910003e1, 0x97fbdab7, 0x910043e0, 0x97f0e5bd, 0x97fff04f, 0x97fcfe59,
    0x90008e34, 0xf9455a94, 0xf9400280, 0xb40001a0, 0x320003e1, 0x97fc7210,
    0x941cebd8, 0x97fc6ea3, 0xf9401668, 0xf9401fe9, 0xeb09011f, 0x54000101,
    0xa9457bf3, 0xf94023f4, 0x910183ff, 0xd65f03c0, 0x97fc6c42, 0xf9400280,
    0x17fffff2, 0x97f03545,
];
// Complete nativeDone and nativePause bodies: the verified nativeDone owns
// destruction; nativePause would suspend its mixer before streamed releases.
const PLAYER_TEARDOWN: [u32; 54] = [
    0xa9be53f5, 0xa9017bf3, 0x97f6269c, 0x39455008, 0x7100011f, 0x1a9f17f4,
    0x97f62698, 0x39455408, 0x7100011f, 0x1a9f07f5, 0x97ff48a7, 0xaa0003f3,
    0x97ff49c5, 0xaa1303e0, 0x97ff4af0, 0x35000080, 0x91002260, 0x97eaaafc,
    0x34000100, 0xaa1303e0, 0x97ff49c5, 0x2a1f03f4, 0xa9417bf3, 0x2a1403e0,
    0xa8c253f5, 0xd65f03c0, 0x4a150294, 0x97ffa714, 0xaa1303e0, 0x97ff49bc,
    0x97ffa731, 0x17fffff7, 0xf81e0ff4, 0xa9017bf3, 0x97ff488f, 0xaa0003f3,
    0x97ff49ad, 0xaa1303e0, 0x97ff4ad8, 0x35000080, 0x91002260, 0x97eaaae4,
    0x34000100, 0x2a1f03f4, 0xaa1303e0, 0x97ff49ac, 0xa9417bf3, 0x12000280,
    0xf84207f4, 0xd65f03c0, 0x320003e0, 0x97ffa676, 0x2a0003f4, 0x17fffff7,
];
const SITE: usize = 0x67e488;
const REORDERED: [u32; 2] = [0x97ffe071, 0x97fac9ed];

/// Release the native module/unwind metadata cache while its allocator is still
/// alive, then destroy the allocator. Both existing no-argument cleanup calls
/// remain present; their return values are unused. No nativeDone step is skipped.
///
/// # Safety
/// `base` names the host's pinned Unity image, and the module-loaded callback
/// runs before guest execution can reach these teardown instructions. The host
/// must keep image contents and mappings stable throughout this installation.
pub(crate) unsafe fn install(api: &HostApiV0, base: usize) -> Result<bool, DetourError> {
    if base == 0 || base.checked_add(0x695448).is_none() {
        return Ok(false);
    }
    for (name, offset) in EXPORTS {
        if api.guest_symbol(base, name) != Some(base + offset) {
            return Ok(false);
        }
    }
    let guards = [
        CodeGuard { address: base + 0x200, words: &BUILD_ID },
        CodeGuard { address: base + 0x67e478, words: &PARENT },
        CodeGuard { address: base + 0x67664c, words: &CACHE_CLEANUP },
        CodeGuard { address: base + 0x530c40, words: &MEMORY_CLEANUP },
        CodeGuard { address: base + 0x69473c, words: &PLAYER_TEARDOWN },
    ];
    // SAFETY: the module event pins this exact image and precedes native player
    // startup. Common verifies all readable ranges and every expected word, then
    // performs one two-word transaction with I-cache flush and failure rollback.
    unsafe { patch_words_if_matches(base + SITE, &PARENT[4..6], &REORDERED, &guards) }
}

#[cfg(test)]
#[path = "../tests/unit/teardown_order.rs"]
mod tests;
