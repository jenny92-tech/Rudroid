// INPUT:  unchanged pthread wait arguments and the guest caller's ARM64 LR
// OUTPUT:  native wait result with an extra received return-address scalar
// POS:    opt-in diagnostic entry; no stack, memory reads, or observer-supplied wake
.text
.p2align 2
.global rudroid_bionic_guest_cond_wait
.hidden rudroid_bionic_guest_cond_wait
.hidden rudroid_bionic_cond_wait_at
.type rudroid_bionic_guest_cond_wait, %function
rudroid_bionic_guest_cond_wait:
    .cfi_startproc
    hint #34 // BTI c; compatible HINT on older ARM64.
    mov x2, x30
    b rudroid_bionic_cond_wait_at // Original LR/SP and x0/x1 stay intact.
    .cfi_endproc
.size rudroid_bionic_guest_cond_wait, .-rudroid_bionic_guest_cond_wait

.p2align 2
.global rudroid_bionic_guest_cond_timedwait
.hidden rudroid_bionic_guest_cond_timedwait
.hidden rudroid_bionic_cond_timedwait_at
.type rudroid_bionic_guest_cond_timedwait, %function
rudroid_bionic_guest_cond_timedwait:
    .cfi_startproc
    hint #34
    mov x3, x30 // Preserve the original deadline in x2.
    b rudroid_bionic_cond_timedwait_at
    .cfi_endproc
.size rudroid_bionic_guest_cond_timedwait, .-rudroid_bionic_guest_cond_timedwait
.section .note.GNU-stack,"",%progbits
