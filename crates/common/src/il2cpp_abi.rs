//! INPUT:  an exact Unity build string or a pinned IL2CPP image's native build evidence
//! OUTPUT:  GcHandleAbi and evidence-gated build selectors, including stripped metadata
//! POS:    shares exact-build ABI decisions without capability-local compatibility tables
#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcHandleAbi {
    /// Legacy native builds verified independently of the managed version getter.
    U32,
    PointerWidth,
}

/// Add builds only after native ABI inspection and regression evidence.
/// This is immutable compatibility data, not a cross-cdylib state cache.
pub fn supported_gc_handle_abi(version: &str) -> Option<GcHandleAbi> {
    match version {
        // Original ARM64 Silksong 1.1.5; SHA-256 1aaf09b5931f2322958b0921b23cccf7
        // ddce8f57e9279bd7bdf9dc4b30325076. new returns a 64-bit slot address;
        // get_target/free mask x0 as a page/slot pointer (native disassembly recorded).
        "6000.0.58f1" if cfg!(target_arch = "aarch64") => Some(GcHandleAbi::PointerWidth),
        // Original ARM64 Vampire Survivors 1.14, native exports inspected;
        // resource cleanup and shader fallback validated in real gameplay.
        "6000.0.61f1" if cfg!(target_arch = "aarch64") => Some(GcHandleAbi::PointerWidth),
        _ => None,
    }
}

pub fn require_gc_handle_abi(version: &str) -> Result<GcHandleAbi, String> {
    supported_gc_handle_abi(version).ok_or_else(|| {
        format!("unverified Unity build/target GC-handle ABI: {version}; related calls skipped")
    })
}

// Native evidence: tests/fixtures/legacy-gc-handle-abi.md. These are IL2CPP
// identities, not package names or a nearby Unity version assumption. The
// public export anchors, GNU build ID, handle return and decode instructions
// all have to match. A stripped getter does not make other images eligible.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
const STRIPPED_GC_EXPORTS: [usize; 3] = [0x8443c4, 0x844404, 0x84444c];
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
const STRIPPED_GC_GUARDS: &[(usize, &[u32])] = &[
    (0x200, &[
        4, 20, 3, 0x00554e47, 0xeed7a633, 0x042bcda7, 0xf2f9f9cc,
        0xe5b4cb29, 0x293007aa,
    ]),
    (0x8443c4, &[0x12000021, 0x17fea642]),
    (0x844404, &[0x17fea782]),
    (0x84444c, &[0x17fea7dd]),
    // The complete allocator return forms the handle in w0, not a slot pointer.
    (0x7ee108, &[
        0x39405268, 0xa9457bfd, 0xa9444ff4, 0xa9425ff8, 0x11000508,
        0x2a160d00, 0xa94357f6, 0xa94167fa, 0xa8c66ffc, 0xd65f03c0,
    ]),
    // Both consumers decode w0's low-three-bit kind and 32-bit slot index.
    (0x7ee20c, &[
        0xf81b0ffa, 0xa90163f9, 0xa9025bf7, 0xa90353f5, 0xa9047bf3,
        0x12000808, 0x51000518, 0x71000f1f, 0x54000069, 0xaa1f03f4,
        0x14000055, 0x2a0003f4, 0x53037c17,
    ]),
    (0x7ee3c0, &[
        0xf81b0ffa, 0xa90163f9, 0xa9025bf7, 0xa90353f5, 0xa9047bf3,
        0x12000808, 0x51000517, 0x71000eff, 0x54000be8, 0x2a0003f4,
        0x53037c18, 0x94027f8f,
    ]),
];

/// Cold discovery only. Kernel-mediated reads return errors for unavailable
/// memory; there is no raw pointer dereference, retained owner or code write.
/// The binding still has to pin its trusted, immutable code before using the
/// returned ABI. In particular, this is not a loader for arbitrary SO files.
pub(crate) fn stripped_gc_handle_abi(
    base: usize,
    exports: [Option<usize>; 3],
) -> Option<GcHandleAbi> {
    #[cfg(all(target_arch = "aarch64", target_os = "linux"))]
    {
        use std::os::unix::fs::FileExt;
        if base == 0 || base % 4 != 0 { return None; }
        for (address, offset) in exports.into_iter().zip(STRIPPED_GC_EXPORTS) {
            if address != Some(base.checked_add(offset)?) { return None; }
        }
        let memory = std::fs::File::open("/proc/self/mem").ok()?;
        let mut bytes = [0u8; 64];
        for &(offset, words) in STRIPPED_GC_GUARDS {
            let address = base.checked_add(offset)?;
            let count = words.len().checked_mul(4)?;
            let output = bytes.get_mut(..count)?;
            address.checked_add(count)?;
            memory.read_exact_at(output, u64::try_from(address).ok()?).ok()?;
            if output.chunks_exact(4).zip(words).any(|(chunk, word)| {
                chunk != word.to_le_bytes()
            }) { return None; }
        }
        Some(GcHandleAbi::U32)
    }
    #[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
    { let _ = (base, exports); None }
}

#[cfg(test)]
#[path = "../tests/unit/il2cpp_abi.rs"]
mod tests;
