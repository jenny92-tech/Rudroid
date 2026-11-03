// INPUT:  Android 256-byte jump buffer, AAPCS64 registers and typed native mask/cookie helpers
// OUTPUT:  rudroid_bionic_setjmp/_setjmp/sigsetjmp/longjmp private ABI entry points
// POS:    restores guest C frames without GNU jmp_buf layout or a Rust ownership frame
/*
 * Copyright (C) 2013 The Android Open Source Project
 * All rights reserved.
 *
 * Redistribution and use in source and binary forms, with or without
 * modification, are permitted provided that the following conditions
 * are met:
 *  * Redistributions of source code must retain the above copyright
 *    notice, this list of conditions and the following disclaimer.
 *  * Redistributions in binary form must reproduce the above copyright
 *    notice, this list of conditions and the following disclaimer in
 *    the documentation and/or other materials provided with the
 *    distribution.
 *
 * THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS
 * "AS IS" AND ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT
 * LIMITED TO, THE IMPLIED WARRANTIES OF MERCHANTABILITY AND FITNESS
 * FOR A PARTICULAR PURPOSE ARE DISCLAIMED. IN NO EVENT SHALL THE
 * COPYRIGHT OWNER OR CONTRIBUTORS BE LIABLE FOR ANY DIRECT, INDIRECT,
 * INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES (INCLUDING,
 * BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES; LOSS
 * OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED
 * AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY,
 * OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT
 * OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF
 * SUCH DAMAGE.
 */


// Adapted from Android 15 arch-arm64/bionic/setjmp.S. Same register groups
// and 25-word public-service storage bound; remaining caller bytes untouched.
// Linux service preserves full x18 but does not implement Android SCS/MTE.
.text
.p2align 2
.macro ENTRY name
.global \name
.hidden \name
.type \name,%function
\name:
    .cfi_startproc
    hint #34 // BTI c, legacy-compatible HINT.
.endm
.macro END name
    .cfi_endproc
.size \name,.-\name
.endm
.macro MANGLE cookie
    .irp register,x18,x19,x20,x21,x22,x23,x24,x25,x26,x27,x28,x29,x30,x10
        eor \register,\register,\cookie
    .endr
.endm
.macro CHECKSUM output,base,scratch
    mov \output,#0
    .irp index,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20,21,22,23
        ldr \scratch,[\base,#(\index*8)]
        eor \output,\output,\scratch
    .endr
.endm
ENTRY rudroid_bionic_setjmp
    mov w1,#1 // Android setjmp saves the signal mask.
    b rudroid_bionic_sigsetjmp
END rudroid_bionic_setjmp
ENTRY rudroid_bionic__setjmp
    mov w1,#0
    b rudroid_bionic_sigsetjmp
END rudroid_bionic__setjmp
ENTRY rudroid_bionic_sigsetjmp
    hint #25 // PACIASP when supported; signs LR at the caller's SP.
    .cfi_negate_ra_state
    stp x0,x30,[sp,#-32]!
    .cfi_def_cfa_offset 32
    .cfi_offset x30,-24
    stp x18,x1,[sp,#16] // GNU helpers need not preserve Android's x18.
    mov w0,w1
    bl rudroid_bionic_jump_cookie
    mov x1,x0
    ldr x0,[sp]
    stp x1,xzr,[x0] // cookie/flag + initially empty mask.
    bl rudroid_bionic_jump_mask_save
    ldp x18,x1,[sp,#16]
    ldp x0,x30,[sp],#32
    .cfi_def_cfa_offset 0
    .cfi_restore x30
    ldr x1,[x0]
    bic x1,x1,#1
    mov x10,sp
    MANGLE x1
    stp x30,x10,[x0,#16]
    stp x28,x29,[x0,#32]
    stp x26,x27,[x0,#48]
    stp x24,x25,[x0,#64]
    stp x22,x23,[x0,#80]
    stp x20,x21,[x0,#96]
    stp x18,x19,[x0,#112]
    MANGLE x1
    stp d14,d15,[x0,#128]
    stp d12,d13,[x0,#144]
    stp d10,d11,[x0,#160]
    stp d8,d9,[x0,#176]
    CHECKSUM x12,x0,x2
    str x12,[x0,#192]
    mov w0,#0
    hint #29 // AUTIASP at the same caller SP, then return normally.
    .cfi_negate_ra_state
    ret
END rudroid_bionic_sigsetjmp
ENTRY rudroid_bionic_longjmp
    CHECKSUM x12,x0,x2
    ldr x2,[x0,#192]
    cmp x2,x12
    b.ne rudroid_bionic_jump_corrupt // Reject damaged state before using SP/LR.
    stp x0,x1,[sp,#-32]!
    .cfi_def_cfa_offset 32
    stp x18,x30,[sp,#16]
    .cfi_offset x30,-8
    ldr x0,[x0]
    bl rudroid_bionic_jump_validate
    ldr x0,[sp]
    bl rudroid_bionic_jump_mask_restore
    ldp x0,x1,[sp],#32
    .cfi_def_cfa_offset 0
    ldr x2,[x0]
    bic x2,x2,#1
    ldp x30,x10,[x0,#16]
    .cfi_restore x30
    .cfi_negate_ra_state
    ldp x28,x29,[x0,#32]
    ldp x26,x27,[x0,#48]
    ldp x24,x25,[x0,#64]
    ldp x22,x23,[x0,#80]
    ldp x20,x21,[x0,#96]
    ldp x18,x19,[x0,#112]
    MANGLE x2
    ldp d14,d15,[x0,#128]
    ldp d12,d13,[x0,#144]
    ldp d10,d11,[x0,#160]
    ldp d8,d9,[x0,#176]
    mov sp,x10
    cmp w1,wzr
    csinc w0,w1,wzr,ne // longjmp(0) makes setjmp return 1.
    hint #29 // Authenticate the stored caller LR, using restored caller SP.
    .cfi_negate_ra_state
    ret
END rudroid_bionic_longjmp
.section .note.GNU-stack,"",%progbits
