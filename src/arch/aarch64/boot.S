// boot.S — Minimal boot trampoline for QEMU virt (aarch64).
//
// Responsibilities (and nothing more):
//   1. Park secondary cores
//   2. Set up a stack
//   3. Zero BSS
//   4. Drop from EL2 to EL1 (QEMU enters at EL2 by default)
//   5. Enable FP/ASIMD
//   6. Jump to Rust

.section .text.boot, "ax"
.global _start
_start:
    // Park secondary cores — only core 0 boots.
    mrs     x0, mpidr_el1
    and     x0, x0, #0xFF
    cbnz    x0, .Lpark

    // Set up a physical stack for early boot.
    ldr     x0, =__stack_top
    mov     sp, x0

    // Zero BSS.
    ldr     x1, =__bss_start
    ldr     x2, =__bss_end
.Lzero_bss:
    cmp     x1, x2
    b.ge    .Lbss_done
    str     xzr, [x1], #8
    b       .Lzero_bss
.Lbss_done:

    // Check if we entered at EL2 (QEMU default). If so, drop to EL1.
    mrs     x1, CurrentEL
    lsr     x1, x1, #2
    cmp     x1, #2
    b.ne    .Lel1

    // --- EL2 → EL1 transition ---

    // HCR_EL2.RW = 1: EL1 executes in AArch64.
    mov     x0, #(1 << 31)
    msr     hcr_el2, x0

    // Let EL1 access the physical timer.
    mrs     x0, cnthctl_el2
    orr     x0, x0, #3
    msr     cnthctl_el2, x0
    msr     cntvoff_el2, xzr

    // Don't trap FP/ASIMD at EL2.
    mrs     x0, cptr_el2
    bic     x0, x0, #(1 << 10)
    msr     cptr_el2, x0

    // Return to .Lel1 at EL1h.
    mov     x0, #0b0101
    msr     spsr_el2, x0
    adr     x0, .Lel1
    msr     elr_el2, x0
    isb
    eret

.Lel1:
    // Use SP_EL1.
    msr     spsel, #1
    isb
    ldr     x0, =__stack_top
    mov     sp, x0

    // Enable FP/ASIMD (otherwise any float instruction faults).
    mrs     x0, cpacr_el1
    orr     x0, x0, #(3 << 20)
    msr     cpacr_el1, x0
    isb

    // Jump to Rust.
    bl      kernel_main
    b       .Lpark

.Lpark:
    wfe
    b       .Lpark
