# D86 — SVC decode and EL0 exception dispatch

**Question:** How does the Rust EL0 handler decode ESR_EL1 to dispatch syscalls
and deliver faults?

**Rests on:** D47 (syscall ABI), D48 (syscall enumeration), D49 (error
encoding), D12 (fault delegation), D61 (fault types), ARM64 ESR encoding
(D17.2.38).

**Status:** settled.

---

## Settles

### ESR_EL1 field extraction

ESR_EL1 layout (ARM ARM D17.2.38):

| Bits    | Field | Meaning                                       |
| ------- | ----- | --------------------------------------------- |
| [31:26] | EC    | Exception Class                               |
| [25]    | IL    | Instruction Length (always 1 for AArch64 SVC) |
| [24:0]  | ISS   | Instruction-Specific Syndrome                 |

Extraction:

```rust
let ec = (esr >> 26) & 0x3F;
let svc_imm = (esr & 0xFFFF) as u16;  // ISS[15:0] for EC 0x15
```

For SVC (EC 0x15), ISS bits [24:16] are RES0 (architecturally zero). Masking
with `0xFFFF` is sufficient and safe.

### EL0 synchronous dispatch (source 8)

```text
EC 0x15 (SVC AArch64):
    SVC #0  → dispatch_typed(TypedOperation::from_code(x4))
    SVC #1  → dispatch_ipc(IpcOperation::Send)
    SVC #2  → dispatch_ipc(IpcOperation::Receive)
    SVC #3  → dispatch_ipc(IpcOperation::Call)
    SVC #4  → dispatch_ipc(IpcOperation::ReplyRecv)
    SVC #5  → dispatch_ipc(IpcOperation::Yield)
    SVC #6+ → fault delivery (invalid syscall)

EC 0x20 (Instruction abort from EL0):
    → VM fault delivery (FAR_EL1 = faulting VA, IFSC in ISS[5:0])

EC 0x24 (Data abort from EL0):
    → VM fault delivery (FAR_EL1 = faulting VA, DFSC in ISS[5:0], WnR in ISS[6])

EC 0x00 (Unknown):
    → fault delivery (undefined instruction)

All other EC values from EL0:
    → fault delivery (HardwareException with full ESR)
```

Edge case: SVC #0 with x4 >= 20 (invalid typed op code).
`TypedOperation::from_code` returns None. This must deliver a fault (invalid
syscall), not silently ignore.

### DFSC/IFSC encoding (for VM fault classification)

| Code      | Meaning                                                  |
| --------- | -------------------------------------------------------- |
| 0x04-0x07 | Translation fault, levels 0-3 (page not mapped)          |
| 0x0C-0x0F | Permission fault, levels 0-3 (mapped, wrong permissions) |
| 0x21      | Alignment fault                                          |

For this kernel's FaultType::VmFault (D61), the relevant information is:

- FAR_EL1: faulting virtual address
- DFSC/IFSC bits [5:0]: distinguishes translation from permission
- WnR (data abort ISS bit 6): read vs write

D26 makes VA kernel-internal, so the handler receives (Space slot index, byte
offset, access type) rather than raw FAR. The kernel maps FAR → (slot, offset)
using the VA-to-Space mapping.

### EL0 IRQ dispatch (source 9)

```text
GIC acknowledge → INTID
    VTIMER   → core.handle_timer()
    Device   → core.handle_irq(intid)
    Spurious → return (no action)
GIC end_of_interrupt(intid)
DispatchResult → restore path
```

Identical to the current EL1h IRQ handler logic, but the restore path returns to
a (possibly different) Observer via RegisterState instead of returning to the
interrupted kernel code via TrapFrame.

### EL0 FIQ and SError (sources 10, 11)

FIQ (source 10): currently unused. Deliver as fault or ignore. The GIC routes
all interrupts as IRQ, not FIQ.

SError (source 11): fatal. Asynchronous hardware error. Print diagnostics and
halt (same as the current EL1h SError path).

### ESR_EL1 read timing safety

Hardware masks PSTATE.I on exception entry to EL1. No IRQ can fire between
exception entry and reading ESR_EL1. ESR_EL1 is stable until the next
synchronous exception to EL1.

The EL0 handler receives ESR and FAR as function parameters (D84), so the
assembly reads them before branching to Rust. Even if Rust later unmasks
interrupts, the values are captured.

### Other EC values that can arrive from EL0

| EC   | Name                     | Action                                        |
| ---- | ------------------------ | --------------------------------------------- |
| 0x01 | WFI/WFE trap             | Fault delivery                                |
| 0x07 | FP/SIMD trap             | Fault delivery (future: lazy FP save trigger) |
| 0x0E | Illegal execution state  | Fault delivery                                |
| 0x18 | System instruction trap  | Fault delivery                                |
| 0x22 | PC alignment fault       | Fault delivery                                |
| 0x26 | SP alignment fault       | Fault delivery                                |
| 0x2C | FP exception             | Fault delivery                                |
| 0x30 | Breakpoint (lower EL)    | Debug fault delivery                          |
| 0x32 | Software step (lower EL) | Debug fault delivery                          |
| 0x34 | Watchpoint (lower EL)    | Debug fault delivery                          |
| 0x3C | BRK (AArch64)            | Debug fault delivery                          |

EC 0x07 is notable: if lazy FP save/restore is implemented (future optimization
noted in exception.S), CPACR_EL1 would trap EL0 FP access, and the kernel would
handle it by enabling FP and saving/restoring the previous context's FP state.
Not currently implemented (eager FP save is used).

### Speculation considerations

SVC is NOT a serializing instruction. The Branch History Buffer is not cleared
on exception entry. On hardware without CSV3, a BHB clearing sequence must
execute in the assembly vector preamble before any indirect branch in kernel
code. On Apple Silicon (CSV3), this is not needed.

Match-based dispatch (`from_svc`, `from_code`) is safe from Spectre v1 — the
match arms produce constants, not memory dereferences. The SB barrier is needed
later at cap-table indexing (already handled by the existing `speculation.rs`
infrastructure).

## Reference check

Matches implementation plan Task 2.3 exactly. The edge case (SVC #0 + invalid
x4) was not explicitly called out in the plan but follows from the existing
`from_code` → None → fault delivery path.
