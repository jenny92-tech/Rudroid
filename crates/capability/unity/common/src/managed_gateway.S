// INPUT:  checked AAPCS64 scalar/reference method plans and Rust dispatch
// OUTPUT:  sixteen private gateways and rudroid_managed_call_original
// POS:    preserves integer/SIMD arguments and unwind frames without casting arbitrary C prototypes
// SPDX-License-Identifier: LicenseRef-Rudroid-NC-SourceShare-1.0
.text
.hidden rudroid_managed_dispatch
.macro SLOT index
.p2align 2
.global rudroid_managed_slot_\index
.hidden rudroid_managed_slot_\index
.type rudroid_managed_slot_\index, %function
rudroid_managed_slot_\index:
    mov w16, #\index
    b .Lmanaged_gateway
.size rudroid_managed_slot_\index, .-rudroid_managed_slot_\index
.endm
.irp index,0,1,2,3,4,5,6,7,8,9,10,11,12,13,14,15
SLOT \index
.endr

// Frame: x0..x8 + padding (80), q0..q7 (128), result x0/x1 (16),
// result q0/q1 (32) = 256 bytes; saved x29/x30 follows at 256.
.p2align 2
.type .Lmanaged_gateway, %function
.Lmanaged_gateway:
    .cfi_startproc
    sub sp, sp, #272
    .cfi_def_cfa_offset 272
    stp x29, x30, [sp, #256]
    .cfi_offset 29, -16
    .cfi_offset 30, -8
    stp x0, x1, [sp, #0]
    stp x2, x3, [sp, #16]
    stp x4, x5, [sp, #32]
    stp x6, x7, [sp, #48]
    stp x8, xzr, [sp, #64]
    stp q0, q1, [sp, #80]
    stp q2, q3, [sp, #112]
    stp q4, q5, [sp, #144]
    stp q6, q7, [sp, #176]
    stp xzr, xzr, [sp, #208]
    stp xzr, xzr, [sp, #224]
    stp xzr, xzr, [sp, #240]
    mov w0, w16
    mov x1, sp
    bl rudroid_managed_dispatch
    ldp x0, x1, [sp, #208]
    ldp q0, q1, [sp, #224]
    ldp x29, x30, [sp, #256]
    add sp, sp, #272
    .cfi_def_cfa_offset 0
    ret
    .cfi_endproc
.size .Lmanaged_gateway, .-.Lmanaged_gateway

.p2align 2
.global rudroid_managed_call_original
.hidden rudroid_managed_call_original
.type rudroid_managed_call_original, %function
rudroid_managed_call_original:
    .cfi_startproc
    stp x19, x30, [sp, #-16]!
    .cfi_def_cfa_offset 16
    .cfi_offset 19, -16
    .cfi_offset 30, -8
    mov x16, x0
    mov x19, x1
    ldp q0, q1, [x19, #80]
    ldp q2, q3, [x19, #112]
    ldp q4, q5, [x19, #144]
    ldp q6, q7, [x19, #176]
    ldp x0, x1, [x19, #0]
    ldp x2, x3, [x19, #16]
    ldp x4, x5, [x19, #32]
    ldp x6, x7, [x19, #48]
    ldr x8, [x19, #64]
    blr x16
    stp x0, x1, [x19, #208]
    stp q0, q1, [x19, #224]
    ldp x19, x30, [sp], #16
    .cfi_def_cfa_offset 0
    ret
    .cfi_endproc
.size rudroid_managed_call_original, .-rudroid_managed_call_original
.section .note.GNU-stack,"",%progbits
