// INPUT:  Android 32-bit ioctl opcode, AAPCS64 registers and native GNU ioctl
// OUTPUT:  rudroid_bionic_ioctl private ABI adapter
// POS:    widens the opcode without unpacking or copying variadic arguments
.text
.p2align 2
.global rudroid_bionic_ioctl
.hidden rudroid_bionic_ioctl
.type rudroid_bionic_ioctl, %function
rudroid_bionic_ioctl:
    .cfi_startproc
    hint #34 // BTI c on newer ARM64; compatible HINT on older CPUs.
    uxtw x1, w1 // Linux ioctl commands are unsigned 32-bit values.
    b ioctl // Tail call preserves caller LR/SP and all remaining arguments.
    .cfi_endproc
.size rudroid_bionic_ioctl, .-rudroid_bionic_ioctl
.section .note.GNU-stack,"",%progbits
