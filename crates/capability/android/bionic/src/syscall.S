// INPUT:  unchanged ARM64 GNU/Android raw syscall ABI arguments
// OUTPUT:  real native syscall return plus optional bounded signal metadata
// POS:    never dereferences arguments or translates the kernel ABI
.text
.p2align 2
.global rudroid_bionic_syscall
.hidden rudroid_bionic_syscall
.hidden rudroid_bionic_signal_diagnostics
.hidden rudroid_bionic_raw_signal_metadata
.type rudroid_bionic_syscall, %function
rudroid_bionic_syscall:
    .cfi_startproc
    hint #34 // BTI c; compatible HINT on older ARM64.
    cmp w0, #134 // Native syscall selector; independently asserted in signals.c.
    b.ne .Lpassthrough
    cmp w1, #6
    b.eq .Lsignal_metadata
    cmp w1, #7
    b.ne .Lpassthrough
.Lsignal_metadata:
    adrp x8, rudroid_bionic_signal_diagnostics
    ldr w8, [x8, :lo12:rudroid_bionic_signal_diagnostics] // Own relaxed atomic flag.
    cbz w8, .Lpassthrough
    stp x29, x30, [sp, #-64]!
    .cfi_def_cfa_offset 64
    .cfi_offset x29, -64
    .cfi_offset x30, -56
    mov x29, sp
    stp x0, x1, [sp, #16]
    stp x2, x18, [sp, #32]
    .cfi_offset x18, -24
    bl syscall // All seven original syscall arguments remain in their registers.
    str x0, [sp, #48]
    mov x3, x0
    ldp x0, x1, [sp, #16]
    uxtw x0, w0
    sxtw x1, w1 // Kernel signum is int; original forwarded arguments stayed intact.
    ldr x2, [sp, #32]
    bl rudroid_bionic_raw_signal_metadata
    ldr x0, [sp, #48]
    ldr x18, [sp, #40] // Android reserves x18; the diagnostic must preserve it.
    .cfi_restore x18
    ldp x29, x30, [sp], #64
    .cfi_restore x29
    .cfi_restore x30
    .cfi_def_cfa_offset 0
    ret
.Lpassthrough:
    b syscall // Other syscalls preserve the original native tail-call path.
    .cfi_endproc
.size rudroid_bionic_syscall, .-rudroid_bionic_syscall
.section .note.GNU-stack,"",%progbits
