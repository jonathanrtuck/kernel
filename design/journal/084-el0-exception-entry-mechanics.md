# D84 — EL0 exception entry mechanics

**Question:** How does the EL0 exception entry assembly save registers into
RegisterState via TPIDR_EL1, given that all 31 GPRs hold user values on entry?

**Rests on:** D74 (direct-to-RegisterState save), D83 (PerCoreData at TPIDR_EL1,
register_state_ptr at offset 0), ARM64 exception model (D1.10).

**Status:** settled.

---

## Settles

### Bootstrap sequence: 7 instructions, 28 bytes

The core problem: reading TPIDR_EL1 requires a scratch register, but all GPRs
hold user values that must be preserved. The solution uses SP_EL1 (the kernel
stack) as temporary parking for a single register.

On exception from EL0, hardware automatically switches to SP_EL1 (sets
PSTATE.SP=1). SP_EL1 is valid from boot and untouched by EL0 code.

```asm
.macro VECTOR_ENTRY_EL0 source
.align 7
    stp     x0, x1, [sp, #-16]!   // Park x0-x1 on kernel stack
    mrs     x0, tpidr_el1          // x0 = &PerCoreData (user x0 gone, on stack)
    ldr     x0, [x0, #0]          // x0 = &RegisterState (register_state_ptr)
    str     x1, [x0, #8]          // Save user x1 directly (still in x1!)
    ldr     x1, [sp], #16         // Recover user x0 from stack into x1
    str     x1, [x0, #0]          // Save user x0 to RegisterState.gprs[0]
    b       __el0_exception_common // x0 = &RegisterState
.endm
```

Key insight: after `stp x0, x1` and `mrs x0, tpidr_el1`, only x0 has been
clobbered. x1 still holds the original user value and can be saved directly to
RegisterState without a stack round-trip. Only x0 needs the stack recovery.

7 instructions = 28 bytes, well within the 128-byte (32 instruction) vector
entry limit. Leaves room for future BHB clearing on non-CSV3 hardware.

seL4 on AArch64 uses an equivalent pattern: park x0 in a scratch slot, read
TPIDR_EL1 into x0, use x0 as the save-area base pointer.

### TPIDR_EL1 access timing

TPIDR*EL1 is readable immediately after exception entry via `mrs`. It is not
banked or modified by the exception. No ISB is needed before the read — ISB is
only required after \_writing* system registers that affect instruction fetch
(VBAR_EL1, SCTLR_EL1, etc.). TPIDR_EL1 is a simple data register.

### SP_EL0 and TPIDR_EL0 save

Both are readable at any point during the handler via `mrs xN, sp_el0` and
`mrs xN, tpidr_el0`. No ordering constraints relative to other system register
reads. Saved alongside other system registers in the common handler, after GPRs.

SP_EL0 → RegisterState.sp (offset 248). TPIDR_EL0 → RegisterState.tpidr (offset
272).

### ESR_EL1 and FAR_EL1: parameters, not RegisterState

ESR_EL1 and FAR_EL1 are exception-specific transient values, not part of the
Observer's persistent identity (D74). They are NOT saved to RegisterState.

Approach: read in the assembly common handler into scratch registers after GPR
save, pass to the Rust handler as function parameters:

```rs
el0_exception_handler(source: u64, esr: u64, far: u64)
```

Safe under A4 non-reentrancy: ESR/FAR are stable from exception entry until the
next synchronous exception to EL1, and PSTATE.I is hardware-masked on entry (no
IRQ can fire between entry and the read).

Alternative considered: read ESR/FAR directly in Rust via `sysreg::esr_el1()`
and `sysreg::far_el1()`. Also correct under A4, but passing as parameters makes
the dependency explicit in the function signature and avoids Rust-side unsafe
for system register access.

If A4 non-reentrancy is ever relaxed (nested interrupts during EL0 handling),
ESR/FAR must be captured before enabling interrupts — the parameter approach
handles this automatically.

### Stack usage in the EL0 path

The EL0 path does NOT allocate a TrapFrame on the kernel stack
(`sub sp, sp, #816` is EL1h only). Registers go directly to RegisterState. The
kernel stack is used for:

1. Temporary x0/x1 parking (16 bytes, immediately deallocated).
2. Rust function call stack frames (managed by the compiler).

SP_EL1 must be valid and have sufficient space for Rust dispatch. This is
guaranteed by boot — each core's kernel stack is allocated during
initialization. SP_EL1 is inaccessible from EL0.

### Register save ordering

No hard ordering constraints between GPR, system register, and FP/SIMD saves.
The convention (matching RegisterState field order):

1. GPRs x0-x30 (x0-x1 in the vector entry, x2-x30 in the common handler)
2. System registers: SP_EL0, ELR_EL1, SPSR_EL1
3. FP/SIMD: q0-q31, FPCR, FPSR

Soft constraints documented for future:

- SPSR_EL1 must be saved before enabling interrupts (if ever done).
- ELR_EL1, ESR_EL1, FAR_EL1 must be consumed before any nested exception
  (irrelevant under A4, but the invariant must be documented).

## Rejected alternatives

**Read ESR/FAR in Rust instead of assembly.** Correct under A4, but makes the
timing dependency implicit. A future change enabling interrupts earlier could
silently break it. The parameter approach is explicit and future-proof.

**Two-register park (stp x0, x1 then recover both).** Works but wastes an
instruction — x1 doesn't need the stack round-trip since only x0 is clobbered by
`mrs x0, tpidr_el1`.

## Prerequisite

RegisterState must have compile-time `offset_of!` assertions for every field
used by assembly (gprs, sp, pc, pstate, tpidr, fp_regs, fpcr, fpsr). TrapFrame
already has these; RegisterState only asserts total size.

## Reference check

Matches implementation plan Task 2.2 (EL0 exception entry assembly). The plan
specifies a `VECTOR_ENTRY_EL0` macro and 11-step save sequence. The
7-instruction bootstrap settles the first 6 steps; the common handler covers
7-11.
