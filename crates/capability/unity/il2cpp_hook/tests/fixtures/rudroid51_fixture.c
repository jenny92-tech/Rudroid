// INPUT:  (none)
// OUTPUT:  rudroid51_hook_target_plain/nop/stp/tailb/adrp, rudroid51_replacement_b
// POS:    Ticket 51 hand-written ARM64 detour fixture: various relocatable entry prologue shapes (including ADRP rematerialization)
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
//
// Compiled by the Docker verification with `gcc -shared -fPIC` and passed to
// the detour integration tests via RUDROID51_SO. Every function returns 65
// ('A'); rudroid51_replacement_b returns 66 ('B'). Naked functions pin their
// entry instruction shapes so the detour support set is exercised on
// purpose-built prologues, not on whatever the optimizer emits.

// Plain compiler prologue (the -O0 stp frame pair): the common relocatable
// case — the first four words must survive relocation verbatim.
int rudroid51_hook_target_plain(void) { return 65; }

// Replacement inside the same DSO: a near-range detour destination.
int rudroid51_replacement_b(void) { return 66; }

// Explicit NOP entry (padding-shaped prologue).
__attribute__((naked)) int rudroid51_hook_target_nop(void) {
    __asm__("nop; nop; mov w0, #65; ret");
}

// Classic STP/LDP frame pair: relocatable words with side effects on sp and
// the frame registers, replayed verbatim from the trampoline.
__attribute__((naked)) int rudroid51_hook_target_stp(void) {
    __asm__("stp x29, x30, [sp, #-16]!; ldp x29, x30, [sp], #16; mov w0, #65; ret");
}

// Static tail target: only reachable through the entry's unconditional B,
// so it must not be exported (the branch binds locally at link time).
static __attribute__((naked)) int rudroid51_tail_target(void) {
    __asm__("mov w0, #65; ret");
}

// Entry IS an unconditional B: the redirect must keep reaching the tail
// target through the trampoline's absolute re-emit; the words behind the
// branch are dead code.
__attribute__((naked)) int rudroid51_hook_target_tailb(void) {
    __asm__("b rudroid51_tail_target; nop; nop; nop");
}

// ADRP entry: the real-world shape of a prologue that touches a global
// (PC-relative page address). The relocator re-materializes the page address
// into the same register (x1), so the trampoline still loads the global and
// returns 65 only when that address is right.
__attribute__((used)) static int rudroid51_adrp_data = 65;

__attribute__((naked)) int rudroid51_hook_target_adrp(void) {
    __asm__("adrp x1, rudroid51_adrp_data; add x1, x1, :lo12:rudroid51_adrp_data; ldr w0, [x1]; ret");
}
