//! INPUT: ARM64 instruction words and code addresses
//! OUTPUT: checked instruction relocation and branch encoding
//! POS: pure detour planning shared by the native installers

#![forbid(unsafe_code)]
use super::*;

fn is_b(insn: u32) -> bool {
    insn & 0xFC00_0000 == 0x1400_0000
}

fn is_bl(insn: u32) -> bool {
    insn & 0xFC00_0000 == 0x9400_0000
}

fn is_adr(insn: u32) -> bool {
    insn & 0x9F00_0000 == 0x1000_0000
}

pub(super) fn is_adrp(insn: u32) -> bool {
    insn & 0x9F00_0000 == 0x9000_0000
}

/// PC-relative instruction families that reference their own address (oracle
/// `bd_is_pcrel`).
fn is_pcrel(insn: u32) -> bool {
    is_b(insn)
        || is_bl(insn)
        || is_adr(insn)
        || is_adrp(insn)
        || insn & 0xFF00_0010 == 0x5400_0000 // B.cond
        || insn & 0x7E00_0000 == 0x3400_0000 // CBZ/CBNZ
        || insn & 0x7E00_0000 == 0x3600_0000 // TBZ/TBNZ
        || insn & 0x3B00_0000 == 0x1800_0000 // LDR/LDRSW/PRFM (literal)
}

/// Absolute result of an `ADR`/`ADRP` executed at `pc` (oracle
/// `bd_adr_target`): the 21-bit signed immediate `immhi:immlo` is a byte
/// offset for `ADR` and a 4 KiB page offset from `pc`'s page for `ADRP`.
pub fn decode_adr(insn: u32, pc: usize) -> usize {
    let immlo = (insn >> 29) & 0x3;
    let immhi = (insn >> 5) & 0x7_FFFF;
    let imm = ((((immhi << 2) | immlo) << 11) as i32 >> 11) as i64; // sign-extend 21 bits
    if is_adrp(insn) {
        ((pc as i64 & !0xFFF) + (imm << 12)) as usize
    } else {
        (pc as i64 + imm) as usize
    }
}

/// The resume scratch register (`ldr x17, [pc+8]; br x17`).
const SCRATCH: u32 = 17;

/// Conservative "this word may write x17": the `Rd`/`Rt` field (bits 4:0)
/// names x17, or a load/store pair's `Rt2` (bits 14:10) does.
fn writes_scratch(insn: u32) -> bool {
    let load_store_pair = insn & 0x3A00_0000 == 0x2800_0000;
    insn & 0x1F == SCRATCH || (load_store_pair && (insn >> 10) & 0x1F == SCRATCH)
}

/// `ldr x<rt>, [pc+8]` (64-bit literal load, imm19 = 2 words).
pub(super) fn ldr_literal_p8(rt: u32) -> u32 {
    0x5800_0040 | (rt & 0x1F)
}

/// `b +12`: skips a two-word literal that sits behind the branch.
pub(super) const B_PLUS_12: u32 = 0x1400_0003;

/// `delta` fits the B/BL immediate field (±2^25 words = ±128 MiB).
pub fn in_branch_range(pc: usize, target: usize) -> bool {
    let delta = target as i64 - pc as i64;
    delta >= -BRANCH_RANGE_BYTES && delta <= BRANCH_RANGE_BYTES - 4
}

/// Encode a direct `B`; `None` when the target is out of the immediate range
/// or misaligned.
pub fn encode_b(pc: usize, target: usize) -> Option<u32> {
    let delta = target as i64 - pc as i64;
    if delta % 4 != 0 || !in_branch_range(pc, target) {
        return None;
    }
    Some(0x1400_0000 | ((delta as u32) >> 2 & 0x03FF_FFFF))
}

/// Encode a direct `BL` (same immediate range as `B`). The redirect itself
/// always uses `B` (the replacement owns the return register), so this stays
/// unit-test-only until a relocator needs it.
#[allow(dead_code)]
pub fn encode_bl(pc: usize, target: usize) -> Option<u32> {
    let delta = target as i64 - pc as i64;
    if delta % 4 != 0 || !in_branch_range(pc, target) {
        return None;
    }
    Some(0x9400_0000 | ((delta as u32) >> 2 & 0x03FF_FFFF))
}

/// Decode a `B` back to its absolute target.
pub fn decode_b(insn: u32, pc: usize) -> usize {
    let imm26 = (insn & 0x03FF_FFFF) as i32;
    let imm = (imm26 << 6) >> 6; // sign-extend 26 bits
    (pc as i64 + imm as i64 * 4) as usize
}

/// The absolute veneer: `ldr x17, [pc+8]; br x17; .quad destination`.
/// Four words, self-contained (the literal follows the two instructions),
/// no range limit on `destination`.
pub fn absolute_jump(destination: usize) -> [u32; 4] {
    [
        LDR_X17_P8,
        BR_X17,
        destination as u32,
        (destination >> 32) as u32,
    ]
}

/// How the covered entry word moves into the trampoline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relocated {
    /// Not PC-relative: executes identically from the trampoline, and the
    /// original function resumes at `target + 4`.
    Verbatim(u32),
    /// Unconditional `B`: control leaves the entry here — the trampoline
    /// jumps to the decoded absolute target; the words behind it are dead
    /// and there is no resume.
    TailBranch(usize),
    /// `ADR`/`ADRP`: the trampoline loads the absolute address the original
    /// would have computed into the same `Xd`, then resumes at `target + 4`.
    Materialize { rd: u32, value: usize },
}

/// Classify the covered entry word. PC-relative words other than `B`,
/// `ADR` and `ADRP` refuse the install (documented support set).
pub fn classify(insn: u32, pc: usize) -> Result<Relocated, DetourError> {
    if is_b(insn) {
        return Ok(Relocated::TailBranch(decode_b(insn, pc)));
    }
    // Every remaining shape is followed by the x17 resume jump.
    if !is_pcrel(insn) || is_adr(insn) || is_adrp(insn) {
        if writes_scratch(insn) {
            return Err(DetourError::ScratchRegisterConflict {
                instruction: insn,
                pc_offset: pc,
            });
        }
    }
    if is_adr(insn) || is_adrp(insn) {
        return Ok(Relocated::Materialize {
            rd: insn & 0x1F,
            value: decode_adr(insn, pc),
        });
    }
    if is_pcrel(insn) {
        return Err(DetourError::UnsupportedPrologue {
            instruction: insn,
            pc_offset: pc,
        });
    }
    Ok(Relocated::Verbatim(insn))
}

/// Build the trampoline replay for one covered entry word: a verbatim copy
/// (or, for `ADR`/`ADRP`, a position-independent literal load of the
/// computed address) followed by the resume jump to `target + 4`, or — for a
/// tail branch — the absolute jump to its decoded target with NO resume
/// (dead tail).
pub fn relocate_entry(insn: u32, target: usize) -> Result<Vec<u32>, DetourError> {
    let mut body = match classify(insn, target)? {
        Relocated::Verbatim(word) => vec![word],
        Relocated::Materialize { rd, value } => vec![
            ldr_literal_p8(rd),
            B_PLUS_12,
            value as u32,
            (value as u64 >> 32) as u32,
        ],
        Relocated::TailBranch(tail_target) => {
            return Ok(absolute_jump(tail_target).to_vec());
        }
    };
    body.extend_from_slice(&absolute_jump(target + 4));
    Ok(body)
}

/// Replay a quiescent four-word entry without returning into overwritten
/// instructions. Every PC-relative word is classified before installation.
pub(super) fn relocate_quiescent_entry(
    words: &[u32; 4],
    target: usize,
) -> Result<Vec<u32>, DetourError> {
    let mut body = Vec::new();
    for (index, word) in words.iter().enumerate() {
        if let Some((inverted, destination)) = conditional_redirect(*word, target + index * 4) {
            if (target..target + 16).contains(&destination) {
                return Err(DetourError::UnsupportedPrologue {
                    instruction: *word,
                    pc_offset: target + index * 4,
                });
            }
            // Invert the original predicate and skip a four-word absolute jump.
            // LDR/BR preserve flags; integer/test bit/register widths are unchanged.
            body.push(inverted);
            body.extend_from_slice(&absolute_jump(destination));
            continue;
        }
        match classify(*word, target + index * 4)? {
            Relocated::Verbatim(word) => body.push(word),
            Relocated::Materialize { rd, value } => body.extend_from_slice(&[
                ldr_literal_p8(rd),
                B_PLUS_12,
                value as u32,
                (value as u64 >> 32) as u32,
            ]),
            Relocated::TailBranch(tail) => {
                if (target..target + 16).contains(&tail) {
                    return Err(DetourError::UnsupportedPrologue {
                        instruction: *word,
                        pc_offset: target + index * 4,
                    });
                }
                body.extend_from_slice(&absolute_jump(tail));
                return Ok(body);
            }
        }
    }
    body.extend_from_slice(&absolute_jump(target + 16));
    Ok(body)
}

/// Relocate only conditional branches to external code; internal targets are rejected above.
pub(super) fn conditional_redirect(word: u32, pc: usize) -> Option<(u32, usize)> {
    let (mask, bits, invert) = if word & 0xff00_0010 == 0x5400_0000 && word & 0xf < 14 {
        (0x00ff_ffe0u32, 19u32, 1u32)
    } else if word & 0x7e00_0000 == 0x3400_0000 {
        (0x00ff_ffe0, 19, 1 << 24)
    } else if word & 0x7e00_0000 == 0x3600_0000 {
        (0x0007_ffe0, 14, 1 << 24)
    } else {
        return None;
    };
    let immediate = (word & mask) >> 5;
    let signed = ((immediate as i32) << (32 - bits)) >> (32 - bits);
    let destination = (pc as i64 + signed as i64 * 4) as usize;
    let inverted = ((word ^ invert) & !mask) | (5 << 5);
    Some((inverted, destination))
}

/// Choose the redirect shape for a (target, destination) pair.
pub fn plan_redirect(target: usize, destination: usize) -> HookShape {
    if in_branch_range(target, destination) {
        HookShape::Branch
    } else {
        HookShape::AbsoluteJump
    }
}
