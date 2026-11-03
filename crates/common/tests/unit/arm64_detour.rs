//! INPUT:  private items of `src/arm64_detour.rs` via `super`
//! OUTPUT:  `tests` unit tests, compiled into the crate only under cfg(test)
//! POS:    keeps test bodies out of `src/arm64_detour.rs`; declared there with `#[path]`
use super::*;

// Verified against GNU as: `mov w0, #65` / `#66`, `stp`/`ldp` frame pair.
const MOVZ_W0_65: u32 = 0x5280_0820;
const MOVZ_W0_66: u32 = 0x5280_0840;
const RET: u32 = 0xD65F_03C0;
const NOP: u32 = 0xD503_201F;
const STP_X29_X30_PRE: u32 = 0xA9BF_7BFD; // stp x29, x30, [sp, #-16]!
const LDP_X29_X30_POST: u32 = 0xA8C1_7BFD; // ldp x29, x30, [sp], #16

#[test]
fn branch_encoding_round_trips_within_range() {
    for (pc, target) in [(0x1000usize, 0x9000usize), (0x8000_0000, 0x7F00_0000)] {
        let insn = encode_b(pc, target).expect("in range");
        assert_eq!(insn & 0xFC00_0000, 0x1400_0000);
        assert_eq!(decode_b(insn, pc), target);
    }
    // Backward branch.
    let insn = encode_b(0x9000, 0x1000).expect("backward in range");
    assert_eq!(decode_b(insn, 0x9000), 0x1000);
}

#[test]
fn branch_range_boundary_matches_imm26() {
    // Largest forward delta: (2^25 - 1) words * 4.
    let limit = BRANCH_RANGE_BYTES - 4;
    assert!(encode_b(0x1000, 0x1000 + limit as usize).is_some());
    assert!(!in_branch_range(0x1000, 0x1000 + limit as usize + 4));
    // Largest backward delta: -2^25 words * 4 (from a high-enough pc).
    let pc = 0x4000_0000usize;
    assert!(encode_b(pc, pc - BRANCH_RANGE_BYTES as usize).is_some());
    assert!(!in_branch_range(pc, pc - BRANCH_RANGE_BYTES as usize - 4));
    // Misaligned targets never encode.
    assert!(encode_b(0x1000, 0x1002).is_none());
}

#[test]
fn bl_encoding_matches_gnu_as() {
    // GNU as: `bl f` at offset 0xc to offset 0 → 94000000; offset math:
    // pc=0x10, target=0x0 → delta -16 → imm26 = -4.
    assert_eq!(encode_bl(0x10, 0x0), Some(0x9400_0000 | 0x03FF_FFFC));
}

#[test]
fn absolute_jump_layout_carries_the_literal() {
    let words = absolute_jump(0x0000_AAAA_0000_BEEF);
    assert_eq!(&words[..2], &[LDR_X17_P8, BR_X17]);
    assert_eq!(words[2], 0x0000_BEEF); // low word first (little-endian .quad)
    assert_eq!(words[3], 0x0000_AAAA);
}

#[test]
fn shape_selection_follows_distance() {
    assert_eq!(plan_redirect(0x1000, 0x1000 + 4096), HookShape::Branch);
    assert_eq!(
        plan_redirect(0x1000, 0x1000 + BRANCH_RANGE_BYTES as usize * 2),
        HookShape::AbsoluteJump
    );
}

#[test]
fn classify_supports_documented_set() {
    let pc = 0x1000;
    for insn in [
        MOVZ_W0_65,
        MOVZ_W0_66,
        RET,
        NOP,
        STP_X29_X30_PRE,
        LDP_X29_X30_POST,
    ] {
        assert_eq!(
            classify(insn, pc),
            Ok(Relocated::Verbatim(insn)),
            "{insn:#010x}"
        );
    }
    // Entry B decodes to its absolute tail target.
    let b_insn = encode_b(pc, 0x42_0000).unwrap();
    assert_eq!(classify(b_insn, pc), Ok(Relocated::TailBranch(0x42_0000)));
    // ADR / ADRP are re-materialized (oracle parity).
    assert_eq!(
        classify(0x1000_0805, 0x4), // adr x5, #+0x100
        Ok(Relocated::Materialize {
            rd: 5,
            value: 0x104
        })
    );
    assert_eq!(
        classify(0xB000_0023, 0x2000), // adrp x3, #+5 pages
        Ok(Relocated::Materialize {
            rd: 3,
            value: 0x7000
        })
    );
    // BL / B.cond / CBZ / TBZ / LDR-literal refuse.
    let refused = [
        0x9400_0000u32, // BL
        0x5400_0005u32, // B.ne
        0x3400_0040u32, // CBZ w0, ...
        0x3608_0060u32, // TBZ w0, #16, ...
        0x5800_0000u32, // LDR literal
    ];
    for insn in refused {
        assert!(
            matches!(
                classify(insn, pc),
                Err(DetourError::UnsupportedPrologue { .. })
            ),
            "{insn:#010x} must refuse"
        );
    }
}

#[test]
fn relocate_entry_replays_and_resumes_behind_the_redirect() {
    let target = 0x2000;
    let body = relocate_entry(STP_X29_X30_PRE, target).expect("relocatable");
    assert_eq!(body[0], STP_X29_X30_PRE);
    assert_eq!(&body[1..], &absolute_jump(target + 4));
}

#[test]
fn relocate_entry_truncates_at_a_tail_branch() {
    let target = 0x2000;
    let b_target = 0x42_0000;
    let insn = encode_b(target, b_target).unwrap();
    let body = relocate_entry(insn, target).expect("B is supported");
    // Absolute jump to the tail target, NO resume: the words behind an
    // unconditional branch cannot execute.
    assert_eq!(body, absolute_jump(b_target));
}

/// Entry words that write x17 are refused before any patch: the resume
/// jump would clobber them (the oracle silently miscompiles these).
#[test]
fn entry_words_writing_x17_are_refused() {
    let pc = 0x2000;
    for insn in [
        0xB000_0031u32, // adrp x17, #+1 page
        0x1000_0811,    // adr x17, #+0x100
        0xD280_0011,    // mov x17, #0
        0xF940_0011,    // ldr x17, [x0]
        0xA940_47F0,    // ldp x16, x17, [sp]
    ] {
        assert_eq!(
            relocate_entry(insn, pc),
            Err(DetourError::ScratchRegisterConflict {
                instruction: insn,
                pc_offset: pc
            }),
            "{insn:#010x}"
        );
    }
    // x16 and the frame pair stay relocatable.
    assert!(relocate_entry(0xB000_0030, pc).is_ok(), "adrp x16");
    assert!(relocate_entry(STP_X29_X30_PRE, pc).is_ok());
    assert!(DetourError::ScratchRegisterConflict {
        instruction: 0xB000_0031,
        pc_offset: pc
    }
    .to_string()
    .contains("writes x17"));
}

/// Encodings verified against the LLVM AArch64 disassembler.
#[test]
fn adr_and_adrp_decode_to_the_oracle_targets() {
    // adr x5, . + 0x100 at pc 4; adr x7, . - 4 at pc 8.
    assert_eq!(decode_adr(0x1000_0805, 0x4), 0x104);
    assert_eq!(decode_adr(0x10FF_FFE7, 0x8), 0x4);
    // adrp x3, +5 pages at 0x2000; adrp x1, -1 page at 0x2004 (the
    // page base, not the pc, is the origin).
    assert_eq!(decode_adrp_checked(0xB000_0023, 0x2000), 0x7000);
    assert_eq!(decode_adrp_checked(0xF0FF_FFE1, 0x2004), 0x1000);
    // High addresses: the result stays a full 64-bit address.
    assert_eq!(
        decode_adr(0xB000_0023, 0x0000_7F12_3456_7ABC),
        0x0000_7F12_3456_C000
    );
}

fn decode_adrp_checked(insn: u32, pc: usize) -> usize {
    assert!(is_adrp(insn), "{insn:#010x} is ADRP");
    decode_adr(insn, pc)
}

#[test]
fn relocate_entry_materializes_adrp_into_the_same_register() {
    let target = 0x0000_7F12_3456_7ABC;
    let value = 0x0000_7F12_3456_C000usize;
    let body = relocate_entry(0xB000_0023, target).expect("ADRP is supported");
    // ldr x3, [pc+8]; b +12; .quad value — then resume at target + 4.
    assert_eq!(body[0], 0x5800_0043, "ldr x3, #8");
    assert_eq!(body[1], 0x1400_0003, "b #12 skips the literal");
    assert_eq!(body[2], value as u32);
    assert_eq!(body[3], (value >> 32) as u32);
    assert_eq!(&body[4..], &absolute_jump(target + 4));
    // The veneer's literal load matches the shared X17 encoding.
    assert_eq!(ldr_literal_p8(17), LDR_X17_P8);
}

#[test]
fn relocate_entry_materializes_adr() {
    let body = relocate_entry(0x10FF_FFE7, 0x8).expect("ADR is supported");
    assert_eq!(body[0], 0x5800_0047, "ldr x7, #8");
    assert_eq!(body[1], B_PLUS_12);
    assert_eq!((body[2], body[3]), (0x4, 0));
    assert_eq!(&body[4..], &absolute_jump(0xC));
}

#[test]
fn relocate_entry_refuses_unsupported_before_any_patch() {
    let target = 0x2000;
    assert!(matches!(
        relocate_entry(0x9400_0000, target), // BL entry
        Err(DetourError::UnsupportedPrologue {
            instruction: 0x9400_0000,
            ..
        })
    ));
}

#[test]
fn quiescent_relocation_checks_each_pc_and_refuses_internal_branches() {
    let target = 0x2000;
    let words = [NOP, 0x1000_0805, NOP, NOP]; // adr x5, +0x100 at target+4
    let body = relocate_quiescent_entry(&words, target).unwrap();
    assert_eq!(body[3], 0x2104);
    assert_eq!(&body[7..], &absolute_jump(target + 16));
    let branch = encode_b(target + 4, target + 12).unwrap();
    assert!(relocate_quiescent_entry(&[NOP, branch, NOP, NOP], target).is_err());
    assert!(relocate_quiescent_entry(&[NOP, 0x9400_0000, NOP, NOP], target).is_err());
    assert!(relocate_quiescent_entry(&[0xb5000040,NOP,NOP,NOP],target).is_err(),"conditional jump into overwritten prefix");
    assert!(conditional_redirect(0x5400000e,target).is_none(),"AL/NV are not invertible predicates");
    let (inverse,destination)=conditional_redirect(0xb5ffffe0,target).unwrap();
    assert_eq!(destination,target-4);assert_eq!(inverse,0xb40000a0);
    let (inverse,destination)=conditional_redirect(0xb62fffE0,target).unwrap();
    assert_eq!(destination,target-4);assert_eq!(inverse&!(0x0007ffe0|1<<24),0xb6280000);
}

/// Live installs on hand-assembled code pages (production target only).
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
mod live {
    use super::super::*;
    use super::{LDP_X29_X30_POST, NOP, STP_X29_X30_PRE};
    use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
    use std::sync::Arc;

    const RET: u32 = 0xD65F_03C0;
    const MOVZ_X0_5: u32 = 0xD280_00A0; // mov x0, #5
    const MOVZ_X0_9: u32 = 0xD280_0120; // mov x0, #9
    const ADRP_X0_P1: u32 = 0xB000_0000; // adrp x0, #+1 page

    /// One fresh RX page holding `words` at their byte offsets.
    fn code_page(words: &[(usize, u32)]) -> usize {
        let len = page_size();
        // SAFETY: anonymous private mapping owned by the test.
        let map = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                len,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS,
                -1,
                0,
            )
        };
        assert_ne!(map, MAP_FAILED);
        let base = map as usize;
        for (offset, word) in words {
            // SAFETY: inside the fresh RW mapping.
            unsafe { ((base + offset) as *mut u32).write_volatile(*word) };
        }
        // SAFETY: the mapping above.
        unsafe {
            flush_code(base as *mut u8, len);
            set_protection(base, len, PROT_READ | PROT_EXEC).unwrap();
        }
        base
    }

    extern "C" fn replacement() -> u64 {
        7
    }

    #[test]
    fn gap_hints_validate_ranges_alignment_and_snapshot_order() {
        let target = 0x2000_0000usize;
        let low = target - BRANCH_RANGE_BYTES as usize;
        let high = target + BRANCH_RANGE_BYTES as usize;
        let gap = target + 40 * 1024 * 1024;
        let maps = format!("{low:x}-{gap:x} ---p\n{:x}-{high:x} ---p\n", gap + page_size());
        assert_eq!(near_gap_hints(target, page_size(), &maps), Some(vec![gap]));
        assert_eq!(near_gap_hints(target, page_size(), &maps.replace(&format!("{:x}-{high:x}", gap + page_size()), &format!("{:x}-{high:x}", gap + page_size() - 1))), Some(vec![]));
        for invalid in ["", "garbage", "2000-1000 r-xp", "1000-3000 r-xp\n2000-4000 r-xp"] {
            assert_eq!(near_gap_hints(target, page_size(), invalid), None);
        }
        assert_eq!(near_gap_hints(target, 0, &maps), None);
        assert_eq!(near_gap_hints(target, 3, &maps), None);
        assert!(near_gap_hints(usize::MAX, page_size(), &maps).is_some());
    }

    #[test]
    fn exact_trampoline_reservation_cannot_overwrite_an_existing_mapping() {
        let length=page_size()*2;
        // SAFETY: this test owns the anonymous RW range and keeps it mapped during both collision attempts.
        unsafe {
            let map=libc::mmap(std::ptr::null_mut(),length,PROT_READ|PROT_WRITE,MAP_PRIVATE|MAP_ANONYMOUS,-1,0);
            assert_ne!(map,MAP_FAILED);let bytes=map.cast::<u8>();
            bytes.write_volatile(0xa5);bytes.add(page_size()).write_volatile(0x5a);
            assert!(TrampolinePage::new(map as usize,page_size()).is_err());
            assert!(TrampolinePage::new(map as usize+page_size(),page_size()).is_err());
            assert_eq!(bytes.read_volatile(),0xa5);assert_eq!(bytes.add(page_size()).read_volatile(),0x5a);
            assert_eq!(libc::munmap(map,length),0);
        }
    }

    #[test]
    fn large_gaps_include_an_interior_non_overlapping_candidate() {
        let target=0x2000_0000usize;let page=page_size();let start=target+40*1024*1024;let end=start+32*page;
        let maps=format!("{:x}-{start:x} ---p\n{end:x}-{:x} ---p\n",target-BRANCH_RANGE_BYTES as usize,target+BRANCH_RANGE_BYTES as usize);
        let hints=near_gap_hints(target,page,&maps).unwrap();
        assert_eq!(hints.len(),3);assert!(hints.contains(&start));assert!(hints.contains(&(end-page)));
        assert!(hints.iter().any(|hint|*hint>start && *hint<end-page));
    }

    #[test]
    fn sparse_near_trampoline_finds_a_single_page_between_dense_mappings() {
        let length = 2 * BRANCH_RANGE_BYTES as usize + 4 * HUGE_PAGE;
        // SAFETY: private inaccessible reservation; the test owns every page.
        let mapping = unsafe { libc::mmap(std::ptr::null_mut(), length, libc::PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0) };
        assert_ne!(mapping, MAP_FAILED);
        let base = mapping as usize;
        let target = base + BRANCH_RANGE_BYTES as usize + 2 * HUGE_PAGE;
        let hole = target + 40 * 1024 * 1024;
        struct Reservation { base: usize, end: usize, hole: usize }
        impl Drop for Reservation {
            fn drop(&mut self) {
                // SAFETY: owned reservation; exclude the one hole, which may
                // now belong to the process-pinned shared trampoline pool.
                unsafe {
                    libc::munmap(self.base as *mut c_void, self.hole - self.base);
                    let after_hole = self.hole + page_size();
                    libc::munmap(after_hole as *mut c_void, self.end - after_hole);
                }
            }
        }
        let _reservation = Reservation { base, end: base + length, hole };
        // SAFETY: release only the test-owned hole and initialize a private
        // code page. No other thread receives the target address.
        unsafe {
            assert_eq!(libc::munmap(hole as *mut c_void, page_size()), 0);
            set_protection(target, page_size(), PROT_READ | PROT_WRITE).unwrap();
            (target as *mut u32).write(MOVZ_X0_5);
            ((target + 4) as *mut u32).write(RET);
            flush_code(target as *mut u8, 8);
            set_protection(target, page_size(), PROT_READ | PROT_EXEC).unwrap();
            let detour = install(target, replacement as usize).expect("a free page exists at +40 MiB, outside every fixed hint");
            assert_eq!(call(target), 7);
            assert_eq!(call(detour.trampoline), 5);
            restore(&detour).unwrap();
            assert_eq!(call(target), 5);
        }
    }

    #[test]
    fn quiescent_absolute_redirect_replays_all_four_entry_words() {
        unsafe extern "C-unwind" fn replacement_unwind() -> u64 { 7 }
        let target = code_page(&[
            (0, MOVZ_X0_5),
            (4, 0x9100_0400),
            (8, 0x9100_0400),
            (12, 0x9100_0400),
            (16, RET),
        ]);
        assert_eq!(call(target), 8);
        let original = Original::<unsafe extern "C-unwind" fn() -> u64>::new();
        // SAFETY: owned, idle entry of the replacement's exact signature;
        // target and trampoline stay mapped for the process.
        let trampoline = unsafe { original.install_quiescent(target, replacement_unwind) }.unwrap();
        assert_eq!(call(target), 7);
        assert_eq!(original.address(), trampoline);
        // SAFETY: the typed trampoline takes no arguments and its page is live.
        assert_eq!(unsafe { original.get().unwrap()() }, 8);
        // Keep target mapped: original is process-pinned and resumes it.
    }

    #[test]
    fn quiescent_conditional_originals_preserve_both_paths_flags_widths_and_test_bits(){
        use super::NOP;
        unsafe extern "C-unwind" fn replacement_arg(_:u64)->u64{7}
        for (opcode,prefix,inputs,expected) in [
            (0xb4000000,NOP,[0,1],[9,5]),(0xb5000000,NOP,[0,1],[5,9]),
            (0x34000000,NOP,[1u64<<32,1],[9,5]),
            (0xb6280000,NOP,[0,1u64<<37],[9,5]),(0xb7280000,NOP,[0,1u64<<37],[5,9]),
            (0x54000000,0xf100001f,[0,1],[9,5]),(0x54000001,0xf100001f,[0,1],[5,9]),
        ]{
            let target=code_page(&[(0,prefix),(4,NOP),(8,NOP),(12,opcode|(3<<5)),(16,MOVZ_X0_5),(20,RET),(24,0xd2800120),(28,RET)]);
            let slot=Original::<unsafe extern "C-unwind" fn(u64)->u64>::new();
            // SAFETY: private complete 32-byte code body and exact scalar ABI, idle through publication.
            unsafe{slot.install_quiescent(target,replacement_arg)}.unwrap();
            for (input,expected) in inputs.into_iter().zip(expected){assert_eq!(unsafe{slot.get().unwrap()(input)},expected,"opcode={opcode:#x} input={input:#x}");}
        }
    }

    #[test]
    fn failed_four_word_seal_restores_every_covered_instruction() {
        let words = [MOVZ_X0_5, 0x9100_0400, 0x9100_0400, 0x9100_0400];
        let address = code_page(&[
            (0, words[0]),
            (4, words[1]),
            (8, words[2]),
            (12, words[3]),
            (16, RET),
        ]);
        let mut calls = 0;
        let result = unsafe {
            patch_words_with(
                address,
                &absolute_jump(replacement as *const () as usize),
                |protection| {
                    calls += 1;
                    if calls >= 2 {
                        return Err(DetourError::Protection("injected seal failure".into()));
                    }
                    set_protection(address, 16, protection)
                },
            )
        };
        assert!(result.is_err());
        assert_eq!(call(address), 8);
        unsafe { libc::munmap(address as *mut c_void, page_size()) };
    }

    #[test]
    fn checked_two_call_patch_changes_order_only_after_all_guards_match() {
        let first = encode_bl(8, 0x40).unwrap();
        let second = encode_bl(12, 0x50).unwrap();
        let expected = [first, second];
        let reordered = [encode_bl(8, 0x50).unwrap(), encode_bl(12, 0x40).unwrap()];
        let parent = [STP_X29_X30_PRE, MOVZ_X0_5, first, second, LDP_X29_X30_POST, RET];
        let add_one = [0x9100_0400, RET];
        let set_nine = [MOVZ_X0_9, RET];
        let page = code_page(&[
            (0, parent[0]), (4, parent[1]), (8, first), (12, second),
            (16, parent[4]), (20, RET), (0x40, add_one[0]), (0x44, RET),
            (0x50, set_nine[0]), (0x54, RET),
        ]);
        assert_eq!(call(page), 9, "add then set");
        let guards = [
            CodeGuard { address: page, words: &parent },
            CodeGuard { address: page + 0x40, words: &add_one },
            CodeGuard { address: page + 0x50, words: &set_nine },
        ];
        // SAFETY: private pinned pages, no concurrent execution or mutation;
        // absent/invalid guard ranges are deliberately rejected before reads.
        unsafe {
            let mismatch = [CodeGuard { address: page + 0x40, words: &[NOP, RET] }];
            assert_eq!(patch_words_if_matches(page + 8, &expected, &reordered, &mismatch), Ok(false));
            assert_eq!(patch_words_if_matches(page + 8, &reordered, &expected, &guards), Ok(false));
            assert!(patch_words_if_matches(page + 8, &expected, &reordered, &[CodeGuard { address: 0, words: &[NOP] }]).is_err());
            assert!(patch_words_if_matches(usize::MAX & !3, &expected, &reordered, &[]).is_err());
            assert!(patch_words_if_matches(page + 8, &expected, &reordered[..1], &[]).is_err());
            assert_eq!(call(page), 9, "every rejected attempt leaves the original callable");
            assert_eq!(patch_words_if_matches(page + 8, &expected, &reordered, &guards), Ok(true));
            assert_eq!(call(page), 10, "set then add: both functions still execute");
            assert_eq!(patch_words_if_matches(page + 8, &expected, &reordered, &guards), Ok(false));
            assert_eq!(libc::munmap(page as *mut c_void, page_size()), 0);
        }
    }

    #[test]
    fn failed_two_word_seal_rolls_back_the_entire_patch() {
        let words = [MOVZ_X0_5, 0x9100_0400];
        let page = code_page(&[(0, words[0]), (4, words[1]), (8, RET)]);
        let mut calls = 0;
        // SAFETY: exclusively owned idle code page; the injected seal failure
        // preserves RWX so rollback can restore and flush both original words.
        let result = unsafe {
            patch_words_with(page, &[MOVZ_X0_9, 0x9100_0800], |protection| {
                calls += 1;
                if calls >= 2 { return Err(DetourError::Protection("injected seal failure".into())); }
                set_protection(page, 8, protection)
            })
        };
        assert!(result.is_err());
        assert_eq!(calls, 3);
        assert_eq!(call(page), 6);
        // SAFETY: the test owns the full mapping and no code pointer escapes.
        unsafe {
            assert_eq!(std::slice::from_raw_parts(page as *const u32, 2), words);
            assert_eq!(libc::munmap(page as *mut c_void, page_size()), 0);
        }
    }

    fn call(address: usize) -> u64 {
        // SAFETY: test-assembled `() -> u64` leaf functions.
        let f: extern "C" fn() -> u64 = unsafe { std::mem::transmute(address) };
        f()
    }

    #[test]
    fn failed_seal_restores_the_original_instruction() {
        for persistent in [false, true] {
            let address = code_page(&[(0, MOVZ_X0_5), (4, RET)]);
            let mut calls = 0;
            let result = unsafe {
                patch_word_with(address, MOVZ_X0_9, |protection| {
                    calls += 1;
                    if calls == 2 || (persistent && calls == 3) {
                        return Err(DetourError::Protection("injected seal failure".into()));
                    }
                    set_protection(address, 4, protection)
                })
            };
            assert!(result.is_err());
            assert_eq!(calls, 3);
            assert_eq!(call(address), 5, "failure keeps the original callable");
            unsafe { libc::munmap(address as *mut c_void, page_size()) };
        }
    }

    #[test]
    fn unaligned_flush_covers_code_spanning_cache_lines() {
        // The MOV/RET pair crosses typical 64/128/256-byte line ends.
        let page = code_page(&[(252, MOVZ_X0_5), (256, RET)]);
        for value in [MOVZ_X0_9, MOVZ_X0_5].into_iter().cycle().take(100) {
            // SAFETY: exclusively owned mapped page. Keep execute
            // permission while opening the same write window as a hook.
            unsafe {
                set_protection(page, page_size(), PROT_READ | PROT_WRITE | PROT_EXEC).unwrap();
                ((page + 252) as *mut u32).write_volatile(value);
                flush_code((page + 253) as *mut u8, 7);
                set_protection(page, page_size(), PROT_READ | PROT_EXEC).unwrap();
            }
            assert_eq!(call(page + 252), if value == MOVZ_X0_9 { 9 } else { 5 });
        }
        // SAFETY: no code pointer escapes this test.
        assert_eq!(
            unsafe { libc::munmap(page as *mut libc::c_void, page_size()) },
            0
        );
    }

    #[test]
    fn adrp_entry_is_detoured_and_the_trampoline_keeps_its_value() {
        let page = code_page(&[(0, ADRP_X0_P1), (4, RET)]);
        let expected = (page & !0xFFF) + 0x1000;
        assert_eq!(call(page) as usize, expected);
        let original = Original::<unsafe extern "C" fn() -> u64>::new();
        // SAFETY: owned test page; nothing else executes it.
        let detour = unsafe { original.install(page, replacement) }.expect("ADRP entry installs");
        assert_eq!(original.address(), detour.trampoline);
        assert_eq!(call(page), 7, "calls reach the replacement");
        assert_eq!(
            call(detour.trampoline) as usize,
            expected,
            "the trampoline re-materializes the page address the original computed"
        );
        // SAFETY: as above.
        unsafe { restore(&detour) }.unwrap();
        assert_eq!(call(page) as usize, expected);
    }

    #[test]
    fn failed_typed_install_preserves_the_previous_original_and_entry_word() {
        let unsupported = 0x9400_0000; // BL cannot be relocated by the one-word installer.
        let page = code_page(&[(0, unsupported), (4, RET)]);
        let original = Original::<unsafe extern "C" fn() -> u64>::new();
        // SAFETY: replacement has the slot's signature and lives for the process.
        unsafe { original.set(replacement as usize) };
        // SAFETY: owned, idle code entry with the slot's signature; it is
        // deliberately refused before any code write or original publication.
        assert!(matches!(unsafe { original.install(page, replacement) }, Err(DetourError::UnsupportedPrologue { .. })));
        assert_eq!(original.address(), replacement as usize);
        // SAFETY: the test owns this readable code word.
        assert_eq!(unsafe { (page as *const u32).read_volatile() }, unsupported);
        // SAFETY: the previous typed original takes no arguments and is still live.
        assert_eq!(unsafe { original.get().unwrap()() }, 7);
    }

    /// Installs and restores while another thread keeps executing a
    /// neighbour function on the target page and the live trampolines of
    /// earlier installs (the reused pool page). A patch window without
    /// execute permission faults that thread.
    #[test]
    fn patch_windows_keep_neighbours_and_live_trampolines_executable() {
        let page = code_page(&[(0, MOVZ_X0_5), (4, RET), (0x100, MOVZ_X0_9), (0x104, RET)]);
        let neighbour = page + 0x100;
        let trampoline = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let calls = Arc::new(AtomicU64::new(0));
        let spinner = {
            let trampoline = Arc::clone(&trampoline);
            let stop = Arc::clone(&stop);
            let calls = Arc::clone(&calls);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Acquire) {
                    assert_eq!(call(neighbour), 9);
                    let live = trampoline.load(Ordering::Acquire);
                    if live != 0 {
                        assert_eq!(call(live), 5);
                    }
                    calls.fetch_add(1, Ordering::Release);
                }
            })
        };
        // Overlap is the point: start patching only once the spinner
        // really executes on the page.
        let started = std::time::Instant::now();
        while calls.load(Ordering::Acquire) < 64 {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "spinner never started executing"
            );
            std::thread::yield_now();
        }
        let before = calls.load(Ordering::Acquire);
        for _ in 0..100 {
            // SAFETY: owned test page; the spinner never runs `page`.
            let detour = unsafe { install(page, replacement as usize) }.expect("install");
            assert_eq!(call(page), 7);
            trampoline.store(detour.trampoline, Ordering::Release);
            // SAFETY: as above.
            unsafe { restore(&detour) }.expect("restore");
            assert_eq!(call(page), 5);
        }
        let during = calls.load(Ordering::Acquire) - before;
        stop.store(true, Ordering::Release);
        spinner.join().expect("the spinner never faulted");
        assert!(during > 0, "the spinner ran while the pages were patched");
    }
}

#[test]
fn original_is_typed_once_and_unset_until_installed() {
    unsafe extern "C" fn doubled(value: i32) -> i32 {
        value * 2
    }
    static ORIGINAL: Original<unsafe extern "C" fn(i32) -> i32> = Original::new();
    assert!(ORIGINAL.get().is_none());
    // SAFETY: `doubled` has exactly the stored type and lives for the process.
    unsafe { ORIGINAL.set(doubled as usize) };
    assert_eq!(ORIGINAL.address(), doubled as usize);
    // SAFETY: plain integer argument.
    assert_eq!(unsafe { ORIGINAL.get().unwrap()(21) }, 42);
    ORIGINAL.clear();
    assert!(ORIGINAL.get().is_none());
}
