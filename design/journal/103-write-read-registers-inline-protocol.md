# D103 — WriteRegisters/ReadRegisters: inline register protocol

**Date:** 2026-04-26

**Question:** How does WriteRegisters/ReadRegisters transfer register state
between the caller and the target Observer? D97 deferred the memory region
designation — this resolves it.

**Rests on:** D35 (composable Observer setup — 5-step sequence needs PC, SP,
x0), D39 (Observer rights — WRITE_REGISTERS, READ_REGISTERS), D47 (register
layout — x0-x7 syscall ABI), D97 (cap table self-mutation — deferred memory
region designation for WriteRegisters/ReadRegisters), A5 (kernel absorbs
complexity — ABI should not leak RegisterState layout).

**Status:** settled.

---

## Settles

### Inline register transfer

WriteRegisters and ReadRegisters transfer a fixed set of registers inline in the
syscall argument registers, not through a memory buffer pointed to by a Space
cap.

The inline set: **PC, SP, x0, PSTATE** (4 values, carried in syscall argument
registers). PSTATE is masked to NZCV only (bits 31:28, mask `0xF000_0000`).

**WriteRegisters** arguments (caller's registers):

| Register | Content                     |
| -------- | --------------------------- |
| x0       | Target Observer cap handle  |
| x1       | New PC value                |
| x2       | New SP value                |
| x3       | New x0 value                |
| x4       | New PSTATE (masked to NZCV) |

The kernel writes these four values into the target Observer's saved
RegisterState. All other registers in the target's RegisterState are unchanged.
The target must be in a stopped state (Inert or Faulted).

**ReadRegisters** arguments and return (caller's registers):

| Register | Direction | Content                    |
| -------- | --------- | -------------------------- |
| x0 (in)  | argument  | Target Observer cap handle |
| x0 (out) | return    | Target's PC                |
| x1 (out) | return    | Target's SP                |
| x2 (out) | return    | Target's x0                |
| x3 (out) | return    | Target's PSTATE (NZCV)     |

### Arguments for inline

**A5 (kernel absorbs complexity):** A buffer-based protocol leaks the
RegisterState struct layout across the ABI boundary. Any future change to
RegisterState layout (field reordering, new fields, different padding) would
break userspace callers that construct the buffer. The inline protocol decouples
the kernel's internal RegisterState representation from the syscall interface.

**D35 (composable setup):** The 5-step Observer creation sequence needs exactly
PC, SP, and x0 (entry point, stack pointer, initial argument). Fault resolution
needs PC (redirect after fault) or x0 (set return value). These are 2-4 values
in every concrete use case — inline transfer is natural.

**Simplicity:** No buffer resolution needed. A buffer-based protocol requires
the caller to hold a Space cap with a mapped region large enough for
RegisterState (816 bytes), the kernel to resolve the Space cap, walk the page
table to find the physical address, and copy through. The inline protocol is a
direct register-to-register copy with no memory indirection.

### Arguments against inline

**D97 originally specified full batch transfer.** The inline protocol cannot
modify x1-x7 or FP/SIMD registers. A handler needing arbitrary register
modification (debugging tools, process migration, checkpoint/restore) cannot use
the inline interface. This is a real limitation.

**Mitigation:** The inline approach covers all concrete use cases today:

- **Initial setup:** Needs PC, SP, x0 (entry point, stack, argument).
- **Fault resolution:** Needs PC (redirect) or x0 (return value).
- **Typed returns:** Uses x0 (return register convention).

A future buffer-based extension for full RegisterState transfer is not
foreclosed. The inline protocol handles the common path; the buffer extension
handles the uncommon path when it is needed.

### PSTATE masking: NZCV only

PSTATE is masked to `0xF000_0000` (bits 31:28 = N, Z, C, V condition flags). All
other PSTATE bits are cleared before writing to the target Observer's saved
SPSR_EL1.

**Security invariant:** Unmasked PSTATE allows userspace to set `SPSR_EL1.M`
bits (bits 3:0), which control the exception level that `eret` returns to. A
malicious caller could set M=0b0101 (EL1h), causing the next `eret` to enter EL1
— kernel privilege escalation. The NZCV mask prevents this by zeroing all mode,
interrupt mask, and execution state bits.

PSTATE bits the kernel controls (not settable by userspace):

| Bits | Field  | Why kernel-controlled                              |
| ---- | ------ | -------------------------------------------------- |
| 9:6  | DAIF   | Interrupt masking — kernel policy, not user choice |
| 4    | M[4]   | Execution state (AArch64 vs AArch32)               |
| 3:0  | M[3:0] | Exception level — must be EL0                      |
| 21   | SS     | Software step — debug infrastructure               |
| 20   | IL     | Illegal execution state — hardware-managed         |

The kernel sets these bits when constructing the initial SPSR_EL1 for an
Observer (M=EL0, DAIF=unmasked, SS=0). WriteRegisters only allows the caller to
set the arithmetic condition flags — the one part of PSTATE that is genuinely
userspace state.

---

## Rejected alternatives

**Buffer-based full RegisterState transfer (D97 original specification):** Leaks
RegisterState layout across the ABI (A5 violation). Requires Space cap
resolution, page table walk, physical address computation, and 816-byte copy for
the common case of setting 2-4 registers. The complexity is not justified by
current use cases.

**Inline with more registers (x0-x7):** Eight inline values would cover more
registers but still not the full set (31 GPRs + SP + PC + PSTATE + 32 FP/SIMD).
The additional registers (x1-x7) are not needed for any current use case. Adding
them complicates the syscall encoding without eliminating the need for a future
buffer extension.

**No PSTATE in inline set:** Condition flags are part of userspace-visible
state. A fault handler that resumes an Observer after modifying x0 (setting a
return value) may need to clear condition flags to avoid confusing the resumed
code's conditional branches. Omitting PSTATE would force the handler to use a
future buffer extension for this common adjustment.

**Unmasked PSTATE write:** Privilege escalation. SPSR_EL1.M controls the
exception return level. Allowing userspace to set arbitrary PSTATE bits is a
kernel security vulnerability, not a feature.

---

## Does NOT settle

- Buffer-based full RegisterState transfer for arbitrary register modification
  (debugging, checkpoint/restore, process migration)
- PC alignment validation on WriteRegisters (AArch64 instructions are 4-byte
  aligned; unaligned PC would fault on resume — whether the kernel validates
  eagerly or lets the hardware fault is a policy choice)
- FP/SIMD register transfer mechanism (when needed for userspace math state
  save/restore across fault handling)
