//! INPUT:  owned AArch64 code mappings and typed C/C-unwind replacement functions
//! OUTPUT:  instruction relocation, checked startup patches and typed original publication
//! POS:    shared code-patching boundary; callers retain mapping and patch-window contracts
//! AArch64 inline-code detour support (ticket 51).
//!
//! This module turns the reference `hook_address_detour` semantics
//! (`loader/so_util_arm64.cpp`, read-only oracle) into a Rust API: an entry
//! is patched with a real branch so calls reach the replacement, while the
//! covered original instruction is relocated into a trampoline that still
//! runs the original function.
//!
//! `install_quiescent` is an explicit exception for entries that remain idle
//! through original-address publication: four instructions are replayed and
//! a 16-byte absolute redirect needs no nearby free page. It is never used
//! for concurrently executing entries; unsupported/internal branches refuse
//! before mutation and failed page sealing restores every covered word.
//!
//! Default `install` redirect chain (ONE instruction word overwritten at the
//! target — the same coverage as the oracle's arena path):
//!
//! - near destination (within the B/BL immediate range, ±128 MiB): the entry
//!   gets a single direct `B` straight to the destination;
//! - beyond that range: the entry gets a single `B` into a trampoline page
//!   allocated NEAR the target (so the `B` stays in range), which carries
//!   the absolute veneer `ldr x17, [pc+8]; br x17; .quad destination`.
//!   A function address is NEVER written into the instruction stream.
//!
//! The trampoline (callable original, oracle `orig_out` semantics) replays
//! the relocated entry instruction and then resumes the original function:
//!
//! ```text
//!   trampoline:  <relocated entry word>        (verbatim, or re-emitted)
//!                ldr x17, [pc+8]; br x17; .quad target+4
//!   hook slot:   ldr x17, [pc+8]; br x17; .quad destination   (far case)
//!   target:      b trampoline                  (near case: b destination)
//! ```
//!
//! Prologue support set (the oracle's `bd_emit_orig`): the entry word is
//! relocated verbatim when it is NOT PC-relative (`stp`, `mov`, `sub sp`,
//! `nop`, `ret`, ...); an unconditional `B` as entry (tail branch) is
//! re-emitted as an absolute jump to its decoded target, truncating the
//! replay (the words behind it are dead); `ADR`/`ADRP` are re-materialized
//! as a literal load of their absolute result into the same `Xd`
//! (`ldr xd, [pc+8]; b +12; .quad value`) before the resume jump. Everything
//! else — `BL`, `B.cond`, `CBZ/CBNZ`, `TBZ/TBNZ`, literal loads — refuses
//! the install with an explicit error and leaves the target untouched.
//!
//! Scratch-register rule (intentional, safer divergence from the oracle):
//! the resume jump behind the replayed word goes through `x17`, so an entry
//! word that WRITES `x17` (an `adr`/`adrp x17`, any instruction whose
//! `Rd`/`Rt` field is 17, or a load pair whose `Rt2` is 17) would have its
//! result clobbered before the original body reads it. The oracle's
//! `bd_emit_orig` has that bug; here the install is refused with
//! [`DetourError::ScratchRegisterConflict`]. The `Rd`/`Rt` check is
//! field-based and therefore conservative (a store of `x17` is refused
//! too).
//!
//! Patch windows keep execute permission (oracle parity): the target page
//! and a reused trampoline page are opened `RWX` for the write and re-sealed
//! `RX` afterwards. Installs happen while guest threads already run (the
//! gamepatches install at `Il2cppReady`, after Unity's worker/GC threads
//! exist; FMOD at module load), and `mprotect` acts on whole pages — a
//! window without `X` would fault every other thread executing a function
//! or a live trampoline that shares the page. Only a freshly mapped,
//! never-executed trampoline page is written `RW`.
//!
//! Instruction cache: after the last byte is written, the patched ranges
//! run `dc cvau` + `dsb ish` + `ic ivau` + `dsb ish` + `isb` over every
//! cache line. `dc cvau` pushes the new instruction bytes to the point of
//! unification, `ic ivau` drops the stale I-cache lines, `dsb ish` makes
//! the maintenance visible process-wide and `isb` syncs the local pipeline
//! — the AArch64 self-modifying-code sequence glibc's `__clear_cache`
//! emits. The redirect itself is ONE aligned 32-bit store of a `B` (or
//! `NOP`) word, so a concurrent executor observes either the old or the new
//! instruction; trampoline words are written before the redirect that makes
//! them reachable. CTR_EL0 supplies the separate D/I line sizes; visiting
//! each line once avoids the old repeated maintenance of every 4-byte word.
//!
//! Trampoline pages live in a process-wide pool (hook installs are rare:
//! `MAX_PATCHES` bounds them). Pages are never unmapped, matching the
//! oracle's process-lifetime plugin handles. Each page is allocated with a
//! hint inside the target's branch range. If the fixed hints miss, a bounded
//! Linux mapping snapshot supplies candidate gaps; exact non-replacing mmap never replaces
//! existing mappings. When no free page can be found
//! there the install fails explicitly instead of writing an out-of-range
//! branch.

#![deny(unsafe_op_in_unsafe_fn)]

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
use std::sync::Mutex;

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
use libc::{
    c_int, c_void, mprotect, MAP_ANONYMOUS, MAP_FAILED, MAP_PRIVATE, PROT_EXEC, PROT_READ,
    PROT_WRITE,
};

/// B/BL immediate range: ±2^25 words = ±128 MiB.
pub const BRANCH_RANGE_BYTES: i64 = (1 << 25) * 4;

/// `ldr x17, [pc+8]` — literal load of the veneer's own `.quad` (verified
/// against GNU as: `58000051  ldr x17, 8`).
pub const LDR_X17_P8: u32 = 0x5800_0051;
/// `br x17` — X17 is the ABI's intra-procedure scratch register, so clobbering
/// it around the redirect is contractually safe (oracle uses the same).
pub const BR_X17: u32 = 0xD61F_0220;

/// Redirect shape chosen by [`plan_redirect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookShape {
    /// Destination within ±128 MiB: the entry `B` goes straight to it.
    Branch,
    /// Destination out of range: the entry `B` reaches a near trampoline
    /// page whose absolute veneer jumps the rest of the way.
    AbsoluteJump,
}

/// Why an install refused to touch a target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DetourError {
    /// Inline detours are supported only on Linux AArch64.
    UnsupportedTarget,
    /// Function entries are 4-byte aligned on AArch64; anything else is not
    /// an instruction stream.
    Unaligned(usize),
    /// A null destination can never be a callable replacement.
    NullDestination,
    /// The covered entry word is a PC-relative instruction this support set
    /// cannot relocate. The target is left untouched.
    UnsupportedPrologue { instruction: u32, pc_offset: usize },
    /// The covered entry word writes `x17`, the trampoline's scratch
    /// register for the resume jump. The target is left untouched.
    ScratchRegisterConflict { instruction: u32, pc_offset: usize },
    /// mmap/mprotect failure while opening a write window, or no free page
    /// within the branch range for a far trampoline.
    Protection(String),
    NoNearTrampoline { target:usize },
}

impl std::fmt::Display for DetourError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DetourError::UnsupportedTarget => write!(f, "inline detours require an aarch64 linux target"),
            DetourError::Unaligned(address) => {
                write!(f, "detour target {address:#x} is not 4-byte aligned")
            }
            DetourError::NullDestination => {
                write!(f, "detour destination is null")
            }
            DetourError::UnsupportedPrologue {
                instruction,
                pc_offset,
            } => write!(
                f,
                "unsupported prologue instruction {instruction:#010x} at {pc_offset:#x}: \
                 PC-relative entry words (BL/B.cond/CBZ/TBZ/literal loads) \
                 cannot be relocated by this support set"
            ),
            DetourError::ScratchRegisterConflict {
                instruction,
                pc_offset,
            } => write!(
                f,
                "entry instruction {instruction:#010x} at {pc_offset:#x} writes x17, the \
                 trampoline's resume scratch register; relocating it would clobber its \
                 result"
            ),
            DetourError::Protection(reason) => {
                write!(f, "page protection change failed: {reason}")
            }
            DetourError::NoNearTrampoline{target}=>write!(f,"no free trampoline page within ±128 MiB of target {target:#x}"),
        }
    }
}

impl std::error::Error for DetourError {}

/// An installed detour. `restore` needs the saved word; the trampoline stays
/// valid for the process lifetime (calling it still runs the original).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledDetour {
    pub target: usize,
    pub destination: usize,
    /// Trampoline entry: replays the relocated entry word and resumes the
    /// original function — the callable "original" (oracle `orig_out`).
    pub trampoline: usize,
    pub shape: HookShape,
    /// Always 4: exactly one instruction word is overwritten at the target.
    pub covered_bytes: usize,
    /// The saved entry word, written back by restore.
    pub saved_word: u32,
}

/// The callable original of a detoured function (its trampoline), typed
/// once as `F` — the hooked function's `unsafe extern "C" fn` type — so hook
/// bodies call `ORIGINAL.get()` instead of transmuting addresses. Zero means
/// "not installed". Calling the original stays `unsafe`: it runs guest code
/// with whatever arguments the hook forwards.
pub struct Original<F: HookFn> {
    address: std::sync::atomic::AtomicUsize,
    _type: std::marker::PhantomData<fn() -> F>,
}

mod hook_fn {
    pub trait Sealed {}
}

/// Fixed-arity C/C-unwind function pointers accepted by [`Original`].
/// Sealed so arbitrary pointer-sized data cannot become a callable original.
///
/// ```compile_fail
/// use rudroid_common::arm64_detour::Original;
/// let original = Original::<usize>::new();
/// ```
pub trait HookFn: hook_fn::Sealed + Copy {
    #[doc(hidden)]
    fn address(self) -> usize;
}

macro_rules! hook_fns {
    ($abi:literal; $(($($arg:ident),*)),* $(,)?) => { $(
        impl<R, $($arg,)*> hook_fn::Sealed for unsafe extern $abi fn($($arg),*) -> R {}
        impl<R, $($arg,)*> HookFn for unsafe extern $abi fn($($arg),*) -> R {
            fn address(self) -> usize { self as usize }
        }
    )* };
}
hook_fns!("C"; (), (A), (A, B), (A, B, C), (A, B, C, D), (A, B, C, D, E), (A, B, C, D, E, F));
hook_fns!("C-unwind"; (), (A), (A, B), (A, B, C), (A, B, C, D), (A, B, C, D, E), (A, B, C, D, E, F));

impl<F: HookFn> Original<F> {
    pub const fn new() -> Self {
        Self { address: std::sync::atomic::AtomicUsize::new(0), _type: std::marker::PhantomData }
    }

    /// # Safety
    /// `trampoline` is 0 or the callable original entry of a function of
    /// type `F` that stays mapped for the process.
    pub unsafe fn set(&self, trampoline: usize) {
        const { assert!(std::mem::size_of::<F>() == std::mem::size_of::<usize>()) };
        self.address.store(trampoline, std::sync::atomic::Ordering::Release);
    }

    /// Install a replacement of the slot's exact function type, then publish
    /// its original. On failure the slot is unchanged.
    ///
    /// # Safety
    /// As [`install`]: `target` is a live, process-owned entry of type `F`.
    /// The replacement and target stay mapped for the process. No caller may
    /// reach the replacement before this function publishes the original.
    /// Rust checks replacement/slot types, not the guest entry's actual ABI.
    ///
    /// ```compile_fail
    /// use rudroid_common::arm64_detour::Original;
    /// let original = Original::<unsafe extern "C" fn(i32) -> i32>::new();
    /// unsafe extern "C" fn wrong(_: f32) -> i32 { 0 }
    /// unsafe { original.install(0x1000, wrong); }
    /// ```
    pub unsafe fn install(&self, target: usize, replacement: F) -> Result<InstalledDetour, DetourError> {
        // SAFETY: caller supplies the mapped entry, matching guest ABI
        // and publication window; replacement's type matches this slot.
        unsafe {
            let installed = install(target, replacement.address())?;
            self.set(installed.trampoline);
            Ok(installed)
        }
    }

    /// Install the four-word quiescent redirect and publish its typed original.
    /// On failure the slot is unchanged.
    ///
    /// # Safety
    /// As [`Self::install`], plus [`install_quiescent`]: the target stays idle
    /// throughout the four-word write and original-address publication.
    pub unsafe fn install_quiescent(&self, target: usize, replacement: F) -> Result<usize, DetourError> {
        // SAFETY: caller's matching ABI and quiescence contract covers
        // both the code write and publication into this typed slot.
        unsafe {
            let trampoline = install_quiescent(target, replacement.address())?;
            self.set(trampoline);
            Ok(trampoline)
        }
    }

    /// Mark the hook not installed (`get` returns `None`).
    pub fn clear(&self) {
        self.address.store(0, std::sync::atomic::Ordering::Release);
    }

    pub fn get(&self) -> Option<F> {
        const { assert!(std::mem::size_of::<F>() == std::mem::size_of::<usize>()) };
        let address = self.address.load(std::sync::atomic::Ordering::Acquire);
        // SAFETY: `set` stored a callable entry of type `F`; sizes match.
        (address != 0).then(|| unsafe { std::mem::transmute_copy::<usize, F>(&address) })
    }

    pub fn address(&self) -> usize {
        self.address.load(std::sync::atomic::Ordering::Acquire)
    }
}

impl<F: HookFn> Default for Original<F> {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Instruction encoding/decoding (pure functions, unit-testable off-target).

fn is_b(insn: u32) -> bool {
    insn & 0xFC00_0000 == 0x1400_0000
}

fn is_bl(insn: u32) -> bool {
    insn & 0xFC00_0000 == 0x9400_0000
}

fn is_adr(insn: u32) -> bool {
    insn & 0x9F00_0000 == 0x1000_0000
}

fn is_adrp(insn: u32) -> bool {
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
fn ldr_literal_p8(rt: u32) -> u32 {
    0x5800_0040 | (rt & 0x1F)
}

/// `b +12`: skips a two-word literal that sits behind the branch.
const B_PLUS_12: u32 = 0x1400_0003;

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
fn relocate_quiescent_entry(words: &[u32; 4], target: usize) -> Result<Vec<u32>, DetourError> {
    let mut body = Vec::new();
    for (index, word) in words.iter().enumerate() {
        if let Some((inverted,destination))=conditional_redirect(*word,target+index*4){
            if (target..target+16).contains(&destination){
                return Err(DetourError::UnsupportedPrologue{instruction:*word,pc_offset:target+index*4});
            }
            // Invert the original predicate and skip a four-word absolute jump.
            // LDR/BR preserve flags; integer/test bit/register widths are unchanged.
            body.push(inverted);body.extend_from_slice(&absolute_jump(destination));continue;
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
fn conditional_redirect(word:u32,pc:usize)->Option<(u32,usize)>{
    let (mask,bits,invert)=if word&0xff00_0010==0x5400_0000 && word&0xf<14 {
        (0x00ff_ffe0u32,19u32,1u32)
    }else if word&0x7e00_0000==0x3400_0000 {
        (0x00ff_ffe0,19,1<<24)
    }else if word&0x7e00_0000==0x3600_0000 {
        (0x0007_ffe0,14,1<<24)
    }else{return None;};
    let immediate=(word&mask)>>5;
    let signed=((immediate as i32)<<(32-bits))>>(32-bits);
    let destination=(pc as i64+signed as i64*4)as usize;
    let inverted=((word^invert)&!mask)|(5<<5);
    Some((inverted,destination))
}

/// Choose the redirect shape for a (target, destination) pair.
pub fn plan_redirect(target: usize, destination: usize) -> HookShape {
    if in_branch_range(target, destination) {
        HookShape::Branch
    } else {
        HookShape::AbsoluteJump
    }
}

// ---------------------------------------------------------------------------
// Page plumbing (the install surface only exists on the production target:
// aarch64 linux — the dc cvau/ic ivau asm and mprotect patch window are both
// target-specific; encoding functions above stay unit-testable everywhere).

/// # Safety
/// sysconf is always safe.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn page_size() -> usize {
    // SAFETY: sysconf(_SC_PAGESIZE) is always safe.
    unsafe { libc::sysconf(libc::_SC_PAGESIZE) as usize }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn align_down(value: usize, page: usize) -> usize {
    value & !(page - 1)
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn align_up(value: usize, page: usize) -> usize {
    (value + page - 1) & !(page - 1)
}

/// mprotect the page range covering `[address, address + len)`.
///
/// # Safety
/// The caller must own the mapped range.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
unsafe fn set_protection(address: usize, len: usize, prot: c_int) -> Result<(), DetourError> {
    let page = page_size();
    let start = align_down(address, page);
    let end = align_up(address + len, page);
    // SAFETY: [start, end) covers only the caller-owned range, page-rounded.
    let rc = unsafe { mprotect(start as *mut c_void, end - start, prot) };
    if rc != 0 {
        Err(DetourError::Protection(
            std::io::Error::last_os_error().to_string(),
        ))
    } else {
        Ok(())
    }
}

/// AArch64 self-modifying-code barrier over `[start, start + len)`: clean the
/// data cache to the point of unification, invalidate the instruction cache,
/// order the maintenance (`dsb ish`) and sync the pipeline (`isb`). Runs while
/// the pages are still writable and before execute permission returns — see
/// the module docs for why this placement and sequence are sufficient.
///
/// # Safety
/// `[start, start + len)` must be mapped and owned.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub unsafe fn flush_code(start: *mut u8, len: usize) {
    if len == 0 {
        return;
    }
    let end = start as usize + len;
    // SAFETY: mapped range; asm! is volatile, so these barriers keep their
    // program order relative to the volatile word stores around them.
    unsafe {
        let ctr: u64;
        std::arch::asm!("mrs {0}, ctr_el0", out(reg) ctr, options(nostack, preserves_flags));
        let dline = 4usize << ((ctr >> 16) & 0xf);
        let iline = 4usize << (ctr & 0xf);
        let mut address = start as usize & !(dline - 1);
        while address < end {
            std::arch::asm!("dc cvau, {0}", in(reg) address);
            address += dline;
        }
        std::arch::asm!("dsb ish");
        address = start as usize & !(iline - 1);
        while address < end {
            std::arch::asm!("ic ivau, {0}", in(reg) address);
            address += iline;
        }
        std::arch::asm!("dsb ish", "isb");
    }
}

/// One trampoline page: anonymous mapping, written while RW, sealed RX after
/// every write cycle. A sealed page may already hold live trampolines, so it
/// is re-opened RWX (never without execute) for the next install.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
struct TrampolinePage {
    map: *mut u8,
    len: usize,
    used: usize,
    writable: bool,
}

// SAFETY: the pool owns every mapping exclusively (process-lifetime pages,
// no interior mutability); raw pointers only track that ownership, like
// elf_load's ExecutableModule.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
unsafe impl Send for TrampolinePage {}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
impl TrampolinePage {
    /// Anonymous RW page: zero lets the kernel choose; a nonzero address is reserved without replacing mappings.
    /// Fails with `DetourError::Protection` when mmap refuses.
    fn new(hint: usize, len: usize) -> Result<Self, DetourError> {
        // SAFETY: anonymous private mapping. A nonzero address uses NOREPLACE,
        // never MAP_FIXED: a competing mapping is rejected rather than overwritten.
        let map = unsafe {
            libc::mmap(
                hint as *mut c_void,
                len,
                PROT_READ | PROT_WRITE,
                MAP_PRIVATE | MAP_ANONYMOUS | if hint==0 {0}else{libc::MAP_FIXED_NOREPLACE},
                -1,
                0,
            )
        };
        if map == MAP_FAILED {
            return Err(DetourError::Protection(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        // Older kernels may ignore NOREPLACE and return another address. That is
        // still a fresh owned mapping, but not our requested trampoline location.
        if hint!=0 && map as usize!=hint {
            // SAFETY: mmap just returned this fresh owned mapping; it is not any pre-existing range.
            unsafe {libc::munmap(map,len);}
            return Err(DetourError::Protection("kernel did not reserve the requested non-replacing address".into()));
        }
        Ok(TrampolinePage {
            map: map as *mut u8,
            len,
            used: 0,
            writable: true,
        })
    }

    /// Reserve `bytes` (8-byte aligned) at the bump pointer and leave the
    /// page writable. A sealed page keeps execute permission while open:
    /// other threads may be running the trampolines already written there.
    /// The mapping is owned by this private page object.
    fn begin_write(&mut self, bytes: usize) -> Result<*mut u8, DetourError> {
        let need = align_up(bytes, 8);
        if self.used + need > self.len {
            return Err(DetourError::Protection(
                "trampoline page exhausted".to_owned(),
            ));
        }
        // SAFETY: our own page-sized mapping.
        if !self.writable {
            let rc = unsafe {
                mprotect(
                    self.map as *mut c_void,
                    self.len,
                    PROT_READ | PROT_WRITE | PROT_EXEC,
                )
            };
            if rc != 0 {
                return Err(DetourError::Protection(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            self.writable = true;
        }
        // SAFETY: the checked reservation lies in this page's owned mapping.
        let pointer = unsafe { self.map.add(self.used) };
        self.used += need;
        Ok(pointer)
    }

    /// Flush the instruction caches over everything written and seal RX.
    /// The mapped prefix is owned by this private page object.
    fn end_write(&mut self) -> Result<(), DetourError> {
        if !self.writable {
            return Ok(());
        }
        // SAFETY: our own mapping and its written prefix.
        unsafe { flush_code(self.map, self.used) };
        // SAFETY: our own mapping.
        let rc = unsafe { mprotect(self.map as *mut c_void, self.len, PROT_READ | PROT_EXEC) };
        if rc != 0 {
            return Err(DetourError::Protection(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        self.writable = false;
        Ok(())
    }

    /// Append and publish one body in this page's exclusively reserved slot.
    fn append(&mut self, body: &[u32]) -> Result<usize, DetourError> {
        let slot = self.begin_write(std::mem::size_of_val(body))?;
        for (index, word) in body.iter().enumerate() {
            // SAFETY: begin_write reserved this complete, aligned slot in
            // our writable mapping; no other body uses these bytes.
            unsafe { slot.cast::<u32>().add(index).write_volatile(*word) };
        }
        self.end_write()?;
        Ok(slot as usize)
    }

}

/// Process-wide trampoline pool. Hook installs are rare (bounded by
/// `MAX_PATCHES`), pages live forever — the same lifetime model as the
/// oracle's plugin handles.
#[derive(Default)]
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
struct TrampolinePool {
    pages: Vec<TrampolinePage>,
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
static TRAMPOLINES: Mutex<TrampolinePool> = Mutex::new(TrampolinePool { pages: Vec::new() });

/// Candidate offsets from the target for a near trampoline page (2 MiB
/// granularity keeps huge-page alignments intact).
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
const ARENA_HINT_DELTAS: [i64; 12] = [
    1 << 26, // +64 MiB
    -(1 << 26),
    1 << 25, // ±32 MiB
    -(1 << 25),
    (3 << 25) + (1 << 24), // ±112 MiB
    -((3 << 25) + (1 << 24)),
    1 << 24, // ±16 MiB
    -(1 << 24),
    1 << 23, // ±8 MiB
    -(1 << 23),
    1 << 21, // ±2 MiB
    -(1 << 21),
];

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
const HUGE_PAGE: usize = 1 << 21;

/// Bounded edge/interior candidates per real gap; nearest candidates first. The snapshot is
/// only a hint: the subsequent non-replacing mmap remains the ownership authority
/// if another thread changes mappings after this read.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn near_gap_hints(target: usize, page: usize, snapshot: &str) -> Option<Vec<usize>> {
    if !page.is_power_of_two() { return None; }
    let low = target.saturating_sub(BRANCH_RANGE_BYTES as usize).max(page);
    let high = target.saturating_add(BRANCH_RANGE_BYTES as usize);
    let mut hints = Vec::new();
    let mut cursor = low;
    let mut previous_end = 0;
    let mut seen = false;
    let mut gap = |start: usize, end: usize| {
        let Some(first) = start.checked_add(page - 1).map(|value| align_down(value, page)) else { return; };
        let Some(last) = end.checked_sub(page).map(|value| align_down(value, page)) else { return; };
        if first <= last {
            // Include an interior candidate: a VMA-adjacent hint is not a guarantee
            // that the kernel's placement policy will choose that boundary page.
            hints.extend([first,last,align_down(first+(last-first)/2,page)]);
        }
    };
    for line in snapshot.lines().filter(|line| !line.trim().is_empty()) {
        let (start, end) = line.split_whitespace().next()?.split_once('-')?;
        let start = usize::from_str_radix(start, 16).ok()?;
        let end = usize::from_str_radix(end, 16).ok()?;
        if start >= end || start < previous_end { return None; }
        previous_end = end;
        seen = true;
        if end <= low { continue; }
        if cursor < start.min(high) { gap(cursor, start.min(high)); }
        cursor = cursor.max(end).min(high);
    }
    if !seen { return None; }
    if cursor < high { gap(cursor, high); }
    hints.sort_unstable();
    hints.dedup();
    hints.sort_unstable_by_key(|hint| hint.abs_diff(target));
    Some(hints)
}

/// Write `body` into a pool page within `B` range of `target` (far trampolines
/// are reached by a direct branch from the entry) and return its address.
/// Pool-internal; the returned address is valid for the process lifetime.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn pool_write_near(target: usize, body: &[u32]) -> Result<usize, DetourError> {
    let bytes = body.len() * 4;
    let mut pool = TRAMPOLINES.lock().expect("trampoline pool lock");
    let page_size = page_size();

    // Reuse a page that still reaches the target and has room.
    let reusable = pool.pages.iter_mut().find(|page| {
        page.used + align_up(bytes, 8) <= page.len
            && in_branch_range(target, page.map as usize + page.used)
            && in_branch_range(target, page.map as usize + page.used + bytes.saturating_sub(4))
    });
    if let Some(page) = reusable {
        return page.append(body);
    }

    // Atomically reserve a fresh page near the target without overwriting any mapping.
    // Older kernels' different returned addresses are rejected by TrampolinePage::new.
    let base = align_down(target, HUGE_PAGE);
    let fixed = ARENA_HINT_DELTAS.into_iter().filter_map(|delta| base.checked_add_signed(delta as isize));
    let mapped = std::iter::once_with(|| {
        use std::io::Read;
        const LIMIT: usize = 4 * 1024 * 1024;
        let Ok(file) = std::fs::File::open("/proc/self/maps") else { return Vec::new(); };
        let mut snapshot = String::new();
        if file.take((LIMIT + 1) as u64).read_to_string(&mut snapshot).is_err() || snapshot.len() > LIMIT {
            return Vec::new();
        }
        near_gap_hints(target, page_size, &snapshot).unwrap_or_default()
    }).flatten();
    for hint in fixed.chain(mapped) {
        if hint == 0 {
            continue;
        }
        let mut page = match TrampolinePage::new(hint, page_size) {
            Ok(page) => page,
            Err(_) => continue,
        };
        if !in_branch_range(target, page.map as usize)
            || !(page.map as usize).checked_add(page.len - 4).is_some_and(|last| in_branch_range(target, last)) {
            // SAFETY: a mapping this function just created.
            unsafe { libc::munmap(page.map as *mut c_void, page.len) };
            continue;
        }
        let trampoline = page.append(body)?;
        pool.pages.push(page);
        return Ok(trampoline);
    }
    Err(DetourError::NoNearTrampoline{target})
}

/// Overwrite ONE aligned instruction word in mapped code: the page is opened
/// `RWX` (other functions on it stay executable for concurrent threads, the
/// oracle's window), the word is stored with a single 32-bit store, the
/// caches are flushed and the page is re-sealed `RX`.
///
/// # Safety
/// `address` must be a 4-byte-aligned word in mapped code pages this process
/// owns. Either `word` and the old word are both safe to observe
/// concurrently (`B`/`NOP`, per the AArch64 concurrent-modification rules),
/// or no thread executes `address` during the call.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub unsafe fn patch_word(address: usize, word: u32) -> Result<(), DetourError> {
    // SAFETY: same owned-code and quiescence contract as this function.
    unsafe {
        patch_word_with(address, word, |protection| {
            set_protection(address, 4, protection)
        })
    }
}

/// Immutable evidence required before a quiescent instruction patch. Addresses
/// may name code or read-only image metadata; this type grants no ownership.
#[derive(Clone, Copy)]
pub struct CodeGuard<'a> {
    pub address: usize,
    pub words: &'a [u32],
}

/// Replace up to four instructions only if the original words and all image
/// guards match. Checks readable mappings before touching any guard; the target
/// must already be executable. Unknown images leave code and permissions alone.
/// Uses the same cache-flush / failed-seal rollback transaction as detours.
///
/// # Safety
/// Every mapped guard/target range must belong to a caller-pinned image and
/// remain mapped with stable permissions and contents throughout this call.
/// No thread may execute the target instructions until the whole patch returns.
/// The replacement must be valid code for this exact image and its live ABI.
/// Mapping checks reject absent ranges, but do not establish ownership or stop
/// concurrent unmapping, code writes or execution on the caller's behalf.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub unsafe fn patch_words_if_matches(
    address: usize,
    expected: &[u32],
    replacement: &[u32],
    guards: &[CodeGuard<'_>],
) -> Result<bool, DetourError> {
    use std::io::Read;
    if expected.len() != replacement.len() || expected.is_empty() || expected.len() > 4
        || guards.len() > 8
    {
        return Err(DetourError::Protection("invalid checked patch length".into()));
    }
    let target = CodeGuard { address, words: expected };
    let ranges = std::iter::once(&target).chain(guards);
    for guard in ranges.clone() {
        if guard.address % 4 != 0 {
            return Err(DetourError::Unaligned(guard.address));
        }
        if guard.words.is_empty() || guard.words.len() > 128
            || guard.address.checked_add(guard.words.len() * 4).is_none()
        {
            return Err(DetourError::Protection("invalid image guard range".into()));
        }
    }
    const LIMIT: usize = 4 * 1024 * 1024;
    let file = std::fs::File::open("/proc/self/maps")
        .map_err(|error| DetourError::Protection(error.to_string()))?;
    let mut snapshot = String::new();
    file.take((LIMIT + 1) as u64).read_to_string(&mut snapshot)
        .map_err(|error| DetourError::Protection(error.to_string()))?;
    if snapshot.len() > LIMIT {
        return Err(DetourError::Protection("mapping snapshot exceeds limit".into()));
    }
    for (index, guard) in ranges.clone().enumerate() {
        if !readable_image_range(&snapshot, guard.address, guard.words.len() * 4, index == 0) {
            return Err(DetourError::Protection("image guard is not mapped with required permissions".into()));
        }
    }
    // SAFETY: caller pins all image ranges and keeps target execution idle;
    // lengths, overflow, alignment and mapping coverage were checked above.
    // No caller-controlled callback runs between comparisons and the write.
    unsafe {
        for guard in ranges {
            for (index, expected) in guard.words.iter().enumerate() {
                if (guard.address as *const u32).add(index).read_volatile() != *expected {
                    return Ok(false);
                }
            }
        }
        patch_words_with(address, replacement, |protection| {
            set_protection(address, replacement.len() * 4, protection)
        })?;
    }
    Ok(true)
}

/// A single readable VMA must contain the complete range. Conservatively
/// refusing ranges split across VMAs avoids guessing changing permissions.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
fn readable_image_range(snapshot: &str, address: usize, bytes: usize, executable: bool) -> bool {
    let Some(limit) = address.checked_add(bytes) else { return false; };
    if bytes == 0 { return false; }
    snapshot.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let Some((start, end)) = fields.next().and_then(|range| range.split_once('-')) else { return false; };
        let Some(permissions) = fields.next() else { return false; };
        let (Ok(start), Ok(end)) = (usize::from_str_radix(start, 16), usize::from_str_radix(end, 16)) else { return false; };
        start <= address && limit <= end && permissions.as_bytes().first() == Some(&b'r')
            && (!executable || permissions.as_bytes().get(2) == Some(&b'x'))
    })
}

/// A failed seal must leave the old instruction callable: callers publish
/// their original trampoline only after this operation succeeds.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
unsafe fn patch_word_with(
    address: usize,
    word: u32,
    protect: impl FnMut(c_int) -> Result<(), DetourError>,
) -> Result<(), DetourError> {
    unsafe { patch_words_with(address, &[word], protect) }
}

#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
unsafe fn patch_words_with(
    address: usize,
    words: &[u32],
    mut protect: impl FnMut(c_int) -> Result<(), DetourError>,
) -> Result<(), DetourError> {
    if address % 4 != 0 {
        return Err(DetourError::Unaligned(address));
    }
    let end = address.checked_add(words.len().saturating_mul(4)).and_then(|end| end.checked_sub(1));
    if words.is_empty()
        || words.len() > 4
        || !end.is_some_and(|end| align_down(address, page_size()) == align_down(end, page_size()))
    {
        return Err(DetourError::Protection(
            "patch must fit in one code page".into(),
        ));
    }
    // SAFETY: caller contract — owned code page; execute stays on.
    unsafe {
        protect(PROT_READ | PROT_WRITE | PROT_EXEC)?;
        let mut saved = [0u32; 4];
        for (index, word) in words.iter().enumerate() {
            saved[index] = (address as *const u32).add(index).read_volatile();
            (address as *mut u32).add(index).write_volatile(*word);
        }
        flush_code(address as *mut u8, words.len() * 4);
        if let Err(error) = protect(PROT_READ | PROT_EXEC) {
            // Failed mprotect preserves the preceding RWX mapping. Roll back
            // before reporting failure, even if sealing fails a second time.
            for (index, word) in saved[..words.len()].iter().enumerate() {
                (address as *mut u32).add(index).write_volatile(*word);
            }
            flush_code(address as *mut u8, words.len() * 4);
            let _ = protect(PROT_READ | PROT_EXEC);
            return Err(error);
        }
    }
    Ok(())
}

/// Install a process-lifetime absolute redirect covering four instructions.
/// Large adjacent guest images may leave no unmapped page within B range;
/// this explicitly quiescent API allocates its original trampoline anywhere.
/// The concurrent one-word installer remains unchanged.
///
/// # Safety
/// All 16 entry bytes must be valid owned code in one page, with no thread
/// executing any covered instruction until installation AND publication of
/// the returned original address complete. Destination and mappings must
/// remain callable for process life. Interior entry points are unsupported.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub unsafe fn install_quiescent(target: usize, destination: usize) -> Result<usize, DetourError> {
    if target % 4 != 0 {
        return Err(DetourError::Unaligned(target));
    }
    if destination == 0 {
        return Err(DetourError::NullDestination);
    }
    let mut saved = [0u32; 4];
    for (index, word) in saved.iter_mut().enumerate() {
        // SAFETY: caller owns the live entry and all four readable words.
        *word = unsafe { (target as *const u32).add(index).read_volatile() };
    }
    let body = relocate_quiescent_entry(&saved, target)?;
    let mut page = TrampolinePage::new(0, page_size())?;
    let original = match page.append(&body) {
        Ok(original) => original,
        Err(error) => {
            // SAFETY: the fresh page has not been published and belongs to us.
            unsafe { libc::munmap(page.map.cast(), page.len) };
            return Err(error);
        }
    };
    // SAFETY: caller keeps the owned entry idle throughout the four-word write.
    let patched = unsafe {
        patch_words_with(target, &absolute_jump(destination), |protection| {
            set_protection(target, 16, protection)
        })
    };
    if let Err(error) = patched {
        // SAFETY: this fresh page has not been published or executed.
        unsafe { libc::munmap(page.map.cast(), page.len) };
        return Err(error);
    }
    TRAMPOLINES
        .lock()
        .expect("trampoline pool lock")
        .pages
        .push(page);
    Ok(original)
}

/// Install an inline detour: patch `target` so calls reach `destination`,
/// relocate the covered entry word into a near trampoline and return the
/// record needed to restore.
///
/// Contract (the oracle's): `target` is a 4-byte-aligned entry in mapped
/// code pages this process owns, and no thread is executing the entry word
/// itself while it is replaced. Other threads may run elsewhere — on the
/// same page included — because every write window keeps execute
/// permission. The destination must be a callable function address — never
/// write it as data.
///
/// # Safety
/// See the contract above; violations crash the process like any patcher.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub unsafe fn install(target: usize, destination: usize) -> Result<InstalledDetour, DetourError> {
    if target % 4 != 0 {
        return Err(DetourError::Unaligned(target));
    }
    if destination == 0 {
        return Err(DetourError::NullDestination);
    }

    let shape = plan_redirect(target, destination);

    // Read and classify the ONE word we are about to cover. Refusal happens
    // BEFORE touching anything: a bad prologue leaves the target completely
    // intact (the oracle logs and leaves the original intact, too).
    // SAFETY: target is a mapped code entry (contract).
    let saved_word = unsafe { (target as *const u32).read_volatile() };
    let tramp_body = relocate_entry(saved_word, target)?;

    // Near trampoline within the entry's branch range: the trampoline (and,
    // for far destinations, the absolute veneer right behind it) must stay
    // reachable by the single-entry-word `B`.
    let mut far_veneer = None;
    let mut tramp_body = tramp_body;
    if shape == HookShape::AbsoluteJump {
        far_veneer = Some(absolute_jump(destination));
        tramp_body.extend_from_slice(far_veneer.as_ref().unwrap());
    }
    let trampoline = pool_write_near(target, &tramp_body)?;

    // The entry word that sends calls to the replacement:
    // - near: one direct B to the destination;
    // - far: one direct B to the veneer sitting behind the trampoline replay.
    let redirect_target = match shape {
        HookShape::Branch => destination,
        HookShape::AbsoluteJump => trampoline + (tramp_body.len() - far_veneer.unwrap().len()) * 4,
    };
    let redirect = encode_b(target, redirect_target).ok_or_else(|| {
        DetourError::Protection(format!(
            "redirect {target:#x} -> {redirect_target:#x} left the branch range"
        ))
    })?;

    // The trampoline is complete and flushed before this store makes it
    // reachable.
    // SAFETY: caller contract — owned code pages.
    unsafe { patch_word(target, redirect)? };

    Ok(InstalledDetour {
        target,
        destination,
        trampoline,
        shape,
        covered_bytes: 4,
        saved_word,
    })
}

/// Undo an install: write the saved entry word back through the same
/// execute-preserving patch window. The trampoline page stays mapped
/// (process lifetime), so calling it still runs the original function.
///
/// The saved word may be any instruction (an `stp`, an `adrp`, ...), which
/// the AArch64 concurrent-modification rules do not cover the way they cover
/// `B`/`NOP`; the production Il2cppReady lifecycle therefore never restores
/// at runtime (the entry stays redirected; the trampoline keeps the original
/// callable).
///
/// # Safety
/// Same contract as [`install`], and no thread may be executing the target
/// entry while the saved word goes back.
#[cfg(all(target_arch = "aarch64", target_os = "linux"))]
pub unsafe fn restore(detour: &InstalledDetour) -> Result<(), DetourError> {
    // SAFETY: caller contract — owned code pages, entry not executing.
    unsafe { patch_word(detour.target, detour.saved_word) }
}

/// Unsupported hosts refuse installation before inspecting the target.
///
/// # Safety
/// Same contract as the Linux AArch64 implementation.
#[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
pub unsafe fn install(_target: usize, _destination: usize) -> Result<InstalledDetour, DetourError> {
    Err(DetourError::UnsupportedTarget)
}

/// Unsupported hosts refuse installation before inspecting the target.
///
/// # Safety
/// Same contract as the Linux AArch64 implementation.
#[cfg(not(all(target_arch = "aarch64", target_os = "linux")))]
pub unsafe fn install_quiescent(_target: usize, _destination: usize) -> Result<usize, DetourError> {
    Err(DetourError::UnsupportedTarget)
}

#[cfg(test)]
#[path = "../tests/unit/arm64_detour.rs"]
mod tests;
