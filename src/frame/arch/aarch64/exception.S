// exception.S — AArch64 exception vector table and context save/restore.
//
// The vector table has 16 entries (4 exception types x 4 source contexts),
// each 128 bytes (0x80). VBAR_EL1 must be 2KB-aligned.
//
// Two exception paths (D74):
//
//   EL1t/EL1h/EL0-32 (sources 0-7, 12-15): TrapFrame path.
//     Saves full context to a stack-allocated TrapFrame (816 bytes), calls
//     exception_handler(frame, source), restores from TrapFrame, erets.
//     Used for kernel-interrupting-kernel (EL1h IRQ) and unused modes.
//
//   EL0 AArch64 (sources 8-11): RegisterState path.
//     Saves directly into the current Observer's RegisterState via
//     TPIDR_EL1 -> PerCoreData -> register_state_ptr. Calls
//     el0_exception_handler(source, esr, far) which is noreturn — it
//     calls __restore_observer or __enter_idle instead of returning.
//     Used for userspace-to-kernel transitions (syscalls, faults, IRQs).
//
// TrapFrame layout (816 bytes, 16-byte aligned):
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
//   288     q0  (128-bit FP/SIMD)
//   304     q1
//   ...
//   768     q30
//   784     q31
//   800     fpcr
//   808     fpsr
//
// RegisterState layout (816 bytes, see register_state.rs for offset constants):
//
//   Offset  Field
//   0       gprs[0] (x0)
//   8       gprs[1] (x1)
//   ...
//   240     gprs[30] (x30)
//   248     sp       (SP_EL0)
//   256     pc       (ELR_EL1)
//   264     pstate   (SPSR_EL1)
//   272     tpidr    (TPIDR_EL0)
//   288     fp_regs[0] (q0, 128-bit)
//   ...
//   768     fp_regs[30] (q30)
//   784     fp_regs[31] (q31)
//   800     fpcr
//   808     fpsr
//
// Optimization note: FP/SIMD state is saved/restored eagerly on every
// exception. This is simple and correct but costs ~100 cycles per entry/exit.
// When this becomes a bottleneck, switch to lazy save: disable FP access via
// CPACR_EL1, trap on first FP use, and save/restore only when the interrupted
// context actually used FP registers.
//
// Register state checklist (audit whenever exception entry/exit changes):
//   [x] GPRs x0-x30
//   [x] ELR_EL1 (return address)
//   [x] SPSR_EL1 (saved PSTATE)
//   [x] ESR_EL1 (exception syndrome — passed to Rust, not saved in RegisterState)
//   [x] FAR_EL1 (fault address — passed to Rust, not saved in RegisterState)
//   [x] FP/SIMD q0-q31 (128-bit NEON/VFP registers)
//   [x] FPCR (floating-point control register)
//   [x] FPSR (floating-point status register)
//   [x] SP_EL0 (saved in RegisterState for EL0 path)
//   [x] TPIDR_EL0 (saved in RegisterState for EL0 path)
//   [ ] SVE state (not applicable — SVE not enabled)

// ---------------------------------------------------------------------------
// Vector entry macro (EL1) — inlined at each EL1t/EL1h/EL0-32 vector position.
//
// Allocates the TrapFrame, saves x0-x1 as scratch, writes the source ID
// into the pad slot, and branches to the common handler. Five instructions
// (20 bytes), well within the 128-byte entry limit.
// ---------------------------------------------------------------------------
.macro VECTOR_ENTRY source
.align 7
    sub     sp, sp, #816
    stp     x0, x1, [sp, #16 * 0]
    mov     x0, #\source
    str     x0, [sp, #280]
    b       __exception_common
.endm

// ---------------------------------------------------------------------------
// Vector entry macro (EL0 AArch64) — saves x0-x1 directly to RegisterState
// via TPIDR_EL1 → PerCoreData → register_state_ptr (D74).
//
// On entry from EL0, SP is the kernel SP (SP_EL1). We park x0-x1 on the
// kernel stack temporarily, load the RegisterState pointer from PerCoreData,
// save the user's x0 and x1 there, then branch to a per-source trampoline
// that sets the source ID and continues to the shared EL0 save code.
//
// Seven instructions (28 bytes), within the 128-byte entry limit.
// ---------------------------------------------------------------------------
.macro VECTOR_ENTRY_EL0 source
.align 7
    stp     x0, x1, [sp, #-16]!    // Park x0-x1 on kernel stack (temporary)
    mrs     x0, tpidr_el1           // x0 = &PerCoreData
    ldr     x0, [x0, #0]           // x0 = register_state_ptr (PER_CORE_DATA_REGISTER_STATE_OFFSET = 0)
    str     x1, [x0, #8]           // RegisterState.gprs[1] = user x1
    ldr     x1, [sp], #16          // Recover user x0 from stack into x1
    str     x1, [x0, #0]           // RegisterState.gprs[0] = user x0
    b       __el0_trampoline_\source
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

    // Lower EL, AArch64 (EL0 — userspace, D74 path)
    VECTOR_ENTRY_EL0 8  // Synchronous
    VECTOR_ENTRY_EL0 9  // IRQ
    VECTOR_ENTRY_EL0 10 // FIQ
    VECTOR_ENTRY_EL0 11 // SError

    // Lower EL, AArch32 (not supported)
    VECTOR_ENTRY 12     // Synchronous
    VECTOR_ENTRY 13     // IRQ
    VECTOR_ENTRY 14     // FIQ
    VECTOR_ENTRY 15     // SError

// ---------------------------------------------------------------------------
// Common exception handler — shared by EL1t/EL1h/EL0-32 vector entries
// (sources 0-7, 12-15). EL0 AArch64 (sources 8-11) uses
// __el0_exception_common instead.
//
// On entry: frame allocated, x0-x1 saved, source ID at [sp+280].
// Saves remaining GPRs + system registers + FP/SIMD, calls Rust, restores
// everything, and returns via eret.
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

    // Save FP/SIMD registers q0-q31 (128-bit each).
    stp     q0, q1, [sp, #288]
    stp     q2, q3, [sp, #320]
    stp     q4, q5, [sp, #352]
    stp     q6, q7, [sp, #384]
    stp     q8, q9, [sp, #416]
    stp     q10, q11, [sp, #448]
    stp     q12, q13, [sp, #480]
    stp     q14, q15, [sp, #512]
    stp     q16, q17, [sp, #544]
    stp     q18, q19, [sp, #576]
    stp     q20, q21, [sp, #608]
    stp     q22, q23, [sp, #640]
    stp     q24, q25, [sp, #672]
    stp     q26, q27, [sp, #704]
    stp     q28, q29, [sp, #736]
    stp     q30, q31, [sp, #768]

    // Save FPCR and FPSR.
    mrs     x9, fpcr
    str     x9, [sp, #800]
    mrs     x9, fpsr
    str     x9, [sp, #808]

    // Call Rust: exception_handler(frame: &mut TrapFrame, source: u64)
    mov     x0, sp
    ldr     x1, [sp, #280]
    bl      exception_handler

    // Restore FPCR and FPSR.
    ldr     x9, [sp, #800]
    msr     fpcr, x9
    ldr     x9, [sp, #808]
    msr     fpsr, x9

    // Restore FP/SIMD registers q0-q31.
    ldp     q0, q1, [sp, #288]
    ldp     q2, q3, [sp, #320]
    ldp     q4, q5, [sp, #352]
    ldp     q6, q7, [sp, #384]
    ldp     q8, q9, [sp, #416]
    ldp     q10, q11, [sp, #448]
    ldp     q12, q13, [sp, #480]
    ldp     q14, q15, [sp, #512]
    ldp     q16, q17, [sp, #544]
    ldp     q18, q19, [sp, #576]
    ldp     q20, q21, [sp, #608]
    ldp     q22, q23, [sp, #640]
    ldp     q24, q25, [sp, #672]
    ldp     q26, q27, [sp, #704]
    ldp     q28, q29, [sp, #736]
    ldp     q30, q31, [sp, #768]

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
    add     sp, sp, #816
    eret

// ---------------------------------------------------------------------------
// EL0 per-source trampolines (D74).
//
// Each trampoline stores its source ID in x19 (callee-saved, preserved
// across the register save) and branches to the shared EL0 save code.
// After the save completes, x19 is moved to x0 (first parameter) before
// calling into the Rust handler.
// ---------------------------------------------------------------------------
__el0_trampoline_8:             // EL0/64 Synchronous
    str     x19, [x0, #152]         // Save user x19 BEFORE clobbering
    mov     x19, #8
    b       __el0_exception_common

__el0_trampoline_9:             // EL0/64 IRQ
    str     x19, [x0, #152]         // Save user x19 BEFORE clobbering
    mov     x19, #9
    b       __el0_exception_common

__el0_trampoline_10:            // EL0/64 FIQ
    str     x19, [x0, #152]         // Save user x19 BEFORE clobbering
    mov     x19, #10
    b       __el0_exception_common

__el0_trampoline_11:            // EL0/64 SError
    str     x19, [x0, #152]         // Save user x19 BEFORE clobbering
    mov     x19, #11
    b       __el0_exception_common

// ---------------------------------------------------------------------------
// EL0 common exception handler (D74).
//
// On entry: x0 = &RegisterState, gprs[0] and gprs[1] already saved,
// x19 = source ID. Saves the remaining register context directly into
// RegisterState (not TrapFrame), resets SP to the kernel stack top
// (noreturn pattern), and calls the Rust el0_exception_handler.
//
// RegisterState layout offsets (verified by compile-time assertions in
// register_state.rs):
//   0     gprs[0]    (already saved by VECTOR_ENTRY_EL0)
//   8     gprs[1]    (already saved by VECTOR_ENTRY_EL0)
//   16    gprs[2]
//   ...
//   240   gprs[30] = x30
//   248   sp         (SP_EL0)
//   256   pc         (ELR_EL1)
//   264   pstate     (SPSR_EL1)
//   272   tpidr      (TPIDR_EL0)
//   288   fp_regs[0] (q0, 128-bit)
//   ...
//   768   fp_regs[30] (q30)
//   784   fp_regs[31] (q31)
//   800   fpcr
//   808   fpsr
// ---------------------------------------------------------------------------
__el0_exception_common:
    // Save x2-x30 (x0-x1 already saved by VECTOR_ENTRY_EL0,
    // x19 already saved by the per-source trampoline at offset 152).
    stp     x2, x3, [x0, #16]
    stp     x4, x5, [x0, #32]
    stp     x6, x7, [x0, #48]
    stp     x8, x9, [x0, #64]
    stp     x10, x11, [x0, #80]
    stp     x12, x13, [x0, #96]
    stp     x14, x15, [x0, #112]
    stp     x16, x17, [x0, #128]
    str     x18, [x0, #144]         // x19 already saved by trampoline
    stp     x20, x21, [x0, #160]
    stp     x22, x23, [x0, #176]
    stp     x24, x25, [x0, #192]
    stp     x26, x27, [x0, #208]
    stp     x28, x29, [x0, #224]
    str     x30, [x0, #240]

    // Save system registers.
    mrs     x9, sp_el0
    str     x9, [x0, #248]         // RegisterState.sp
    mrs     x9, elr_el1
    str     x9, [x0, #256]         // RegisterState.pc
    mrs     x9, spsr_el1
    str     x9, [x0, #264]         // RegisterState.pstate
    mrs     x9, tpidr_el0
    str     x9, [x0, #272]         // RegisterState.tpidr

    // Save FP/SIMD registers q0-q31 (128-bit each).
    stp     q0, q1, [x0, #288]
    stp     q2, q3, [x0, #320]
    stp     q4, q5, [x0, #352]
    stp     q6, q7, [x0, #384]
    stp     q8, q9, [x0, #416]
    stp     q10, q11, [x0, #448]
    stp     q12, q13, [x0, #480]
    stp     q14, q15, [x0, #512]
    stp     q16, q17, [x0, #544]
    stp     q18, q19, [x0, #576]
    stp     q20, q21, [x0, #608]
    stp     q22, q23, [x0, #640]
    stp     q24, q25, [x0, #672]
    stp     q26, q27, [x0, #704]
    stp     q28, q29, [x0, #736]
    stp     q30, q31, [x0, #768]

    // Save FPCR and FPSR.
    mrs     x9, fpcr
    str     x9, [x0, #800]
    mrs     x9, fpsr
    str     x9, [x0, #808]

    // Read exception-specific registers (NOT saved to RegisterState —
    // these are passed as parameters to the Rust handler).
    mrs     x1, esr_el1             // x1 = esr (parameter 2)
    mrs     x2, far_el1             // x2 = far (parameter 3)

    // Reset SP to kernel stack top (noreturn handler pattern).
    // The Rust handler calls __restore_observer or __enter_idle instead
    // of returning, so we do not need to preserve the current SP.
    mrs     x9, tpidr_el1
    ldr     x9, [x9, #16]          // PerCoreData.kernel_stack_top (PER_CORE_DATA_KERNEL_STACK_TOP_OFFSET = 16)
    mov     sp, x9

    // Set up call parameters:
    //   x0 = source (from x19, set by trampoline)
    //   x1 = esr (already set above)
    //   x2 = far (already set above)
    mov     x0, x19

    // Call Rust: el0_exception_handler(source: u64, esr: u64, far: u64) -> !
    bl      el0_exception_handler
    // unreachable — el0_exception_handler is divergent

// ---------------------------------------------------------------------------
// __restore_observer — restore an Observer's context and eret to EL0 (D74).
//
// Called from the Rust el0_exception_handler (noreturn) to resume an Observer.
//
// Parameters (AArch64 calling convention):
//   x0 = register_state_ptr (*mut RegisterState)
//   x1 = page_table_root    (TTBR0_EL1 value)
//   x2 = clock_access        (0 or 1 — CNTKCTL_EL1 bit 1)
//
// Performs:
//   1. Conditional TTBR0 switch (skip if unchanged, full TLB invalidation
//      if changed)
//   2. CNTKCTL_EL1 update (branchless — BFI inserts clock_access bit)
//   3. System register restore (SP_EL0, ELR_EL1, SPSR_EL1, TPIDR_EL0)
//   4. FP/SIMD restore (FPCR, FPSR, q0-q31)
//   5. GPR restore (x0 loaded last — it is the base pointer)
//   6. eret to EL0
// ---------------------------------------------------------------------------
.global __restore_observer
__restore_observer:
    // 1. TTBR0 switch — skip if the page table root is unchanged.
    mrs     x9, ttbr0_el1
    cmp     x9, x1
    b.eq    .Lskip_ttbr
    msr     ttbr0_el1, x1
    isb
    tlbi    vmalle1is
    dsb     ish
    isb
.Lskip_ttbr:

    // 2. CNTKCTL_EL1 — branchless update of bit 1 (EL0 virtual counter access).
    mrs     x9, cntkctl_el1
    bfi     x9, x2, #1, #1         // CNTKCTL[1] = clock_access[0]
    msr     cntkctl_el1, x9

    // 3. System registers from RegisterState.
    ldr     x9, [x0, #248]         // SP_EL0
    msr     sp_el0, x9
    ldr     x9, [x0, #256]         // PC → ELR_EL1
    msr     elr_el1, x9
    ldr     x9, [x0, #264]         // PSTATE → SPSR_EL1
    msr     spsr_el1, x9
    ldr     x9, [x0, #272]         // TPIDR_EL0
    msr     tpidr_el0, x9

    // 4. FP/SIMD — FPCR/FPSR first, then q0-q31.
    ldr     x9, [x0, #800]
    msr     fpcr, x9
    ldr     x9, [x0, #808]
    msr     fpsr, x9
    ldp     q0, q1, [x0, #288]
    ldp     q2, q3, [x0, #320]
    ldp     q4, q5, [x0, #352]
    ldp     q6, q7, [x0, #384]
    ldp     q8, q9, [x0, #416]
    ldp     q10, q11, [x0, #448]
    ldp     q12, q13, [x0, #480]
    ldp     q14, q15, [x0, #512]
    ldp     q16, q17, [x0, #544]
    ldp     q18, q19, [x0, #576]
    ldp     q20, q21, [x0, #608]
    ldp     q22, q23, [x0, #640]
    ldp     q24, q25, [x0, #672]
    ldp     q26, q27, [x0, #704]
    ldp     q28, q29, [x0, #736]
    ldp     q30, q31, [x0, #768]

    // 5. GPRs — x0 loaded last (it is the base pointer).
    ldp     x2, x3, [x0, #16]
    ldp     x4, x5, [x0, #32]
    ldp     x6, x7, [x0, #48]
    ldp     x8, x9, [x0, #64]
    ldp     x10, x11, [x0, #80]
    ldp     x12, x13, [x0, #96]
    ldp     x14, x15, [x0, #112]
    ldp     x16, x17, [x0, #128]
    ldp     x18, x19, [x0, #144]
    ldp     x20, x21, [x0, #160]
    ldp     x22, x23, [x0, #176]
    ldp     x24, x25, [x0, #192]
    ldp     x26, x27, [x0, #208]
    ldp     x28, x29, [x0, #224]
    ldr     x30, [x0, #240]
    ldr     x1, [x0, #8]           // x1 second-to-last
    ldr     x0, [x0, #0]           // x0 last (base pointer consumed)

    eret

// ---------------------------------------------------------------------------
// __enter_idle — idle loop for cores with no runnable Observer.
//
// Unmasks IRQs and loops on WFI. When an IRQ fires during WFI, the CPU
// takes the exception through the EL1h vector (source 5). The existing
// TrapFrame-based irq_handler runs, processes the interrupt, and returns
// via eret back into this WFI loop.
//
// Called from the Rust el0_exception_handler when no Observer is runnable.
// Must not hold any locks (IRQs will be unmasked).
// ---------------------------------------------------------------------------
.global __enter_idle
__enter_idle:
    msr     daifclr, #2             // Unmask IRQs (DAIF.I = 0)
.Lidle_loop:
    wfi                              // Wait for interrupt
    b       .Lidle_loop             // Loop (IRQ handled via EL1h before return)
