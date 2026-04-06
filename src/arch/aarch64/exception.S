// exception.S — AArch64 exception vector table and context save/restore.
//
// The vector table has 16 entries (4 exception types x 4 source contexts),
// each 128 bytes (0x80). VBAR_EL1 must be 2KB-aligned.
//
// Each entry saves x0-x1 to get scratch registers, stashes the source ID
// in the TrapFrame's pad slot, and branches to a common handler that saves
// the remaining context and calls into Rust.
//
// TrapFrame layout (288 bytes = 36 x u64, 16-byte aligned):
//
//   Offset  Field
//   0       x0
//   8       x1
//   ...
//   232     x29
//   240     x30
//   248     elr_el1
//   256     spsr_el1
//   264     esr_el1
//   272     far_el1
//   280     (source ID, reused as pad for alignment)

// ---------------------------------------------------------------------------
// Vector entry macro — inlined at each of the 16 vector positions.
//
// Allocates the TrapFrame, saves x0-x1 as scratch, writes the source ID
// into the pad slot, and branches to the common handler. Five instructions
// (20 bytes), well within the 128-byte entry limit.
// ---------------------------------------------------------------------------
.macro VECTOR_ENTRY source
.align 7
    sub     sp, sp, #288
    stp     x0, x1, [sp, #16 * 0]
    mov     x0, #\source
    str     x0, [sp, #280]
    b       __exception_common
.endm

// ---------------------------------------------------------------------------
// Vector table — 16 entries, 2KB aligned.
// ---------------------------------------------------------------------------
.section .text
.align 11
.global __vectors

__vectors:
    // Current EL, SP_EL0 (EL1t — kernel does not use this mode)
    VECTOR_ENTRY 0      // Synchronous
    VECTOR_ENTRY 1      // IRQ
    VECTOR_ENTRY 2      // FIQ
    VECTOR_ENTRY 3      // SError

    // Current EL, SP_EL1 (EL1h — kernel's active exception vectors)
    VECTOR_ENTRY 4      // Synchronous
    VECTOR_ENTRY 5      // IRQ
    VECTOR_ENTRY 6      // FIQ
    VECTOR_ENTRY 7      // SError

    // Lower EL, AArch64 (EL0 — future userspace)
    VECTOR_ENTRY 8      // Synchronous
    VECTOR_ENTRY 9      // IRQ
    VECTOR_ENTRY 10     // FIQ
    VECTOR_ENTRY 11     // SError

    // Lower EL, AArch32 (not supported)
    VECTOR_ENTRY 12     // Synchronous
    VECTOR_ENTRY 13     // IRQ
    VECTOR_ENTRY 14     // FIQ
    VECTOR_ENTRY 15     // SError

// ---------------------------------------------------------------------------
// Common exception handler — shared by all 16 vector entries.
//
// On entry: frame allocated, x0-x1 saved, source ID at [sp+280].
// Saves remaining GPRs + system registers, calls Rust, restores, returns.
// ---------------------------------------------------------------------------
__exception_common:
    // Save x2-x29 (x0-x1 already saved by vector entry).
    stp     x2, x3, [sp, #16 * 1]
    stp     x4, x5, [sp, #16 * 2]
    stp     x6, x7, [sp, #16 * 3]
    stp     x8, x9, [sp, #16 * 4]
    stp     x10, x11, [sp, #16 * 5]
    stp     x12, x13, [sp, #16 * 6]
    stp     x14, x15, [sp, #16 * 7]
    stp     x16, x17, [sp, #16 * 8]
    stp     x18, x19, [sp, #16 * 9]
    stp     x20, x21, [sp, #16 * 10]
    stp     x22, x23, [sp, #16 * 11]
    stp     x24, x25, [sp, #16 * 12]
    stp     x26, x27, [sp, #16 * 13]
    stp     x28, x29, [sp, #16 * 14]

    // Save x30 and ELR_EL1. Using x9 as scratch (already saved above).
    mrs     x9, elr_el1
    stp     x30, x9, [sp, #16 * 15]

    // Save SPSR_EL1 and ESR_EL1.
    mrs     x9, spsr_el1
    mrs     x10, esr_el1
    stp     x9, x10, [sp, #16 * 16]

    // Save FAR_EL1.
    mrs     x9, far_el1
    str     x9, [sp, #272]

    // Call Rust: exception_handler(frame: &mut TrapFrame, source: u64)
    mov     x0, sp
    ldr     x1, [sp, #280]
    bl      exception_handler

    // Restore ELR_EL1 and SPSR_EL1 (using x9 as scratch — not yet restored).
    ldr     x9, [sp, #248]
    msr     elr_el1, x9
    ldr     x9, [sp, #256]
    msr     spsr_el1, x9

    // Restore all GPRs.
    ldp     x0, x1, [sp, #16 * 0]
    ldp     x2, x3, [sp, #16 * 1]
    ldp     x4, x5, [sp, #16 * 2]
    ldp     x6, x7, [sp, #16 * 3]
    ldp     x8, x9, [sp, #16 * 4]
    ldp     x10, x11, [sp, #16 * 5]
    ldp     x12, x13, [sp, #16 * 6]
    ldp     x14, x15, [sp, #16 * 7]
    ldp     x16, x17, [sp, #16 * 8]
    ldp     x18, x19, [sp, #16 * 9]
    ldp     x20, x21, [sp, #16 * 10]
    ldp     x22, x23, [sp, #16 * 11]
    ldp     x24, x25, [sp, #16 * 12]
    ldp     x26, x27, [sp, #16 * 13]
    ldp     x28, x29, [sp, #16 * 14]
    ldr     x30, [sp, #240]

    // Deallocate frame and return from exception.
    add     sp, sp, #288
    eret
