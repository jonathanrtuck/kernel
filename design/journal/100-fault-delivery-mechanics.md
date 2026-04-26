# D100 — Fault delivery mechanics

**Date:** 2026-04-25

**Question:** What is the concrete register-level layout of fault messages, how
does the kernel construct the fault Observer cap, and what happens when the root
Observer faults with no userspace handler?

**Rests on:** D12 (fault delegation), D21 (fault handler at slot 0), D28
(message format — 4 data + label + badge + user cap + reply cap), D39 (Observer
9 rights), D47 (register layout — x0-x3 data, x4 label, x5 badge, x6 user cap,
x7 reply cap), D49 (CAP_ABSENT sentinel), D61 (four fault types with specific
data word assignments), D68 (pager unavailability — chain terminus → destroy),
D80 (fault delivery protocol — kernel constructs TransferredCap), A5 (kernel
absorbs complexity — fault handling protocol).

**Status:** settled.

---

## Exploration

### The question

D80 settled the fault delivery protocol: `deliver_fault` composes FaultType →
Message → enqueue to handler Field. D61 settled the four fault types and their
data word assignments. What remains are three concrete questions:

1. **Register-level layout.** When the fault message is written to the handler
   Observer's registers (after dequeue or direct delivery), exactly which
   register gets which value? D47 settled the general IPC register layout, but
   the fault-specific mapping of data words to registers needs explicit
   confirmation.

2. **Fault Observer cap construction.** D80 settled that the kernel constructs a
   TransferredCap directly (not minted from the self-cap). But exactly which
   rights, and how does the cap reach the handler's cap table?

3. **Root Observer fault with kernel as handler.** D68 settled that the chain
   terminus means destroy. But what does "destroy" mean mechanically when the
   faulting entity is the root Observer — the only Observer in the system at
   early boot?

### Decision 1: Fault message register layout

D47 settles the IPC register convention:

- x0-x3 = data words (data[0] through data[3])
- x4 = label
- x5 = badge
- x6 = user cap handle (or CAP_ABSENT)
- x7 = reply cap handle (or CAP_ABSENT)

Fault messages use the standard Message format (D28). The kernel constructs a
`Message` via `FaultType::to_message()` (D61) and the dispatch layer writes it
to the handler Observer's saved registers via `write_message_to_registers`
(D76). The register mapping is:

| Register | Field            | Fault content                                                                                     |
| -------- | ---------------- | ------------------------------------------------------------------------------------------------- |
| x0       | data[0]          | VmFault: Space slot index. ResourceRequest: resource type. CapTableFull: 0. HwException: ESR_EL1. |
| x1       | data[1]          | VmFault: byte offset. ResourceRequest: quantity. Others: 0.                                       |
| x2       | data[2]          | VmFault: access type (0=R, 1=W, 2=X). HwException: ELR_EL1. Others: 0.                            |
| x3       | data[3]          | HwException: FAR_EL1. Others: 0.                                                                  |
| x4       | label            | Kernel-reserved label per `FaultType::label()`.                                                   |
| x5       | badge            | Badge from the handler's fault cap entry at slot 0 (D21).                                         |
| x6       | user cap handle  | Faulting Observer cap handle (5-right subset, installed in handler's table).                      |
| x7       | reply cap handle | `CAP_ABSENT` (`u64::MAX`) — no reply cap in fault messages.                                       |

This is not a new decision — it falls directly out of D47 + D61 + D80. But
making the mapping explicit prevents ambiguity in the implementation.

**x7 = CAP_ABSENT (no reply cap).** Fault messages are kernel deposits, not
Call() operations. D16 reply caps are only created for Call(). The handler
resolves the fault through typed kernel operations on the Observer cap in x6
(e.g., `resume`, `install_cap`, `write_registers`), not by replying to a reply
cap.

**HwException data word assignment.** D61 specifies the table but the physical
register mapping clarifies a subtlety: ESR_EL1 in x0 (data[0]), ELR_EL1 in x1
(data[1]), FAR_EL1 in x2 (data[2]). The handler reads x0 first to classify the
exception (ESR_EL1 encodes the exception class), then x1 for the faulting PC,
then x2 for the faulting address. This ordering matches the diagnostic priority:
what happened, where in code, what address.

### Decision 2: Fault Observer cap construction

D80 settled that the kernel constructs a TransferredCap directly. The exact
rights are the 5-right `FAULT_OBSERVER` subset defined in `capability.rs`:

| Right             | Included | Why                                                 |
| ----------------- | -------- | --------------------------------------------------- |
| RESUME            | yes      | Handler restarts the faulting Observer after fix.   |
| DESTROY           | yes      | Handler can abandon the faulting Observer.          |
| INSTALL_CAP       | yes      | Handler installs Space caps for page fault fix.     |
| WRITE_REGISTERS   | yes      | Handler modifies Observer state before resume.      |
| READ_REGISTERS    | yes      | Handler reads Observer state to diagnose.           |
| SUSPEND           | no       | Observer is already stopped (Faulted state, D39).   |
| CHANGE_HANDLER    | no       | Routing is structural, not fault-recovery.          |
| MODIFY_SCHEDULING | no       | Scheduling profile is orthogonal to fault recovery. |
| CLONE             | no       | Handler already has the cap; no need to duplicate.  |

The kernel constructs a `TransferredCap` via `make_observer_fault_cap()` (D80)
with the faulting Observer's `ObjectId` and current generation. The cap is
installed in the handler's cap table via `allocate_slot` + `install_at`. The
handler sees the cap handle in x6.

**Why not mint from the self-cap at slot 2?** The kernel IS the authority source
for fault messages (D80). The self-cap is a userspace-facing abstraction —
minting from it would require resolving the self-cap, reading its rights, then
attenuating. The kernel has direct knowledge of the Observer's identity and
generation. Constructing the TransferredCap directly is both simpler and more
honest about what is happening.

**Rights justification (5 of 9, not more, not fewer):**

- Without INSTALL_CAP, the handler cannot resolve VM faults (cannot provide the
  missing Space mapping).
- Without READ_REGISTERS, the handler cannot diagnose what went wrong (cannot
  read the faulting Observer's PC, SP, or other state).
- Without WRITE_REGISTERS, the handler cannot fix register state before resume
  (e.g., set the return value for a failed syscall that caused a resource
  request).
- Without RESUME, the handler cannot restart the Observer — it would be
  permanently faulted.
- Without DESTROY, the handler has no way to abandon a fatally faulted Observer.

Each excluded right either does not apply (SUSPEND — already stopped) or
represents authority beyond what fault recovery requires (CHANGE_HANDLER,
MODIFY_SCHEDULING). CLONE is excluded because the handler already has the cap;
if it needs to delegate, that is a separate authority decision not granted by
the kernel's fault delivery.

### Decision 3: Kernel-as-root-fault-handler

D31 establishes the root Observer as the initial entity. D68 settles the chain
terminus: when the root Observer faults and its handler is the kernel (the
default when no userspace handler is installed, or when the handler chain
terminates at the kernel), the kernel terminates the faulting Observer.

Mechanically, for the root Observer:

1. The kernel detects that the handler cap at slot 0 is invalid (empty slot — no
   handler installed), or that the handler chain has terminated at the kernel.
2. `deliver_fault` returns `FaultDeliveryOutcome::HandlerUnavailable`.
3. The kernel logs the fault to serial: fault type, data words (Space slot +
   offset for VM faults, ESR_EL1/ELR_EL1/FAR_EL1 for hardware exceptions), and
   the PC at which the fault occurred.
4. The kernel calls PSCI `SYSTEM_OFF` (or `SYSTEM_RESET` on platforms that do
   not support `SYSTEM_OFF`).

**Why SYSTEM_OFF, not just destroy?** The root Observer IS the system. If it
faults with no handler, nothing can recover. Destroying it and entering idle
(WFI loop) would leave the system in an undefined state with no observer
running. PSCI SYSTEM_OFF is the clean termination path.

**Hypervisor environment:** `SYSTEM_OFF` terminates the VM. The hypervisor exit
code can distinguish fault-termination from clean test exit (the hypervisor
runner checks the exit reason). This enables automated test harnesses to detect
kernel panics.

**Real hardware:** PSCI `SYSTEM_OFF` halts the system. This is the correct
behavior — a root fault on real hardware with no handler is a fatal error.
Restarting (SYSTEM_RESET) is an option but risks boot loops if the fault is
deterministic.

**Non-root Observers with kernel terminus:** For non-root Observers, D68 Case C
(kernel-autonomous destroy) applies. The kernel destroys the faulting Observer
and reclaims its resources. This does not halt the system — only the faulting
Observer is affected. The chain is: observer faults → handler unavailable → D68
notification up the supervision chain → if chain terminus reached, destroy.

---

## Settles

### Fault message register layout (#26)

Fault messages follow the standard Message format (D28): x0-x3 = data words, x4
= label, x5 = badge, x6 = user cap (fault Observer cap), x7 = CAP_ABSENT
(`u64::MAX`). The per-fault-type data word assignments follow D61:

- VmFault: x0 = Space slot index, x1 = byte offset, x2 = access type, x3 = 0.
- ResourceRequest: x0 = resource type, x1 = quantity, x2 = 0, x3 = 0.
- CapTableFull: x0-x3 = 0.
- HardwareException: x0 = ESR_EL1, x1 = ELR_EL1, x2 = FAR_EL1, x3 = 0.

### Fault Observer cap construction (#27)

The kernel constructs a TransferredCap directly (not minted from the faulting
Observer's self-cap). Rights: exactly 5 of 9 Observer rights — RESUME, DESTROY,
INSTALL_CAP, WRITE_REGISTERS, READ_REGISTERS. These are the minimum the handler
needs to inspect and fix the fault (read registers to diagnose), modify state
(write registers, install caps), and restart (resume) or abandon (destroy).
Excluded: SUSPEND (handler does not need to pause a faulted Observer — it is
already stopped), CHANGE_HANDLER (routing is structural, not fault-recovery),
MODIFY_SCHEDULING (scheduling profile is orthogonal to fault recovery), CLONE
(handler already has the cap). The cap is installed in the handler's cap table
via `allocate_slot` + `install_at`. The handler sees the cap handle in x6.

### Kernel-as-root-fault-handler (#28)

D31 + D68: when the root Observer faults and its handler is the kernel (chain
terminus), the kernel terminates the system. Mechanically: kernel logs the fault
(type, data words, PC) to serial, calls PSCI `SYSTEM_OFF` (or `SYSTEM_RESET` on
platforms that do not support `SYSTEM_OFF`). In the hypervisor environment, PSCI
`SYSTEM_OFF` terminates the VM — the exit code distinguishes fault-termination
from clean test exit. On real hardware, this halts the system (the root Observer
is the system — if it faults, nothing can recover).

---

## Does NOT settle

- Fault label numeric values (D61 defers; current values are provisional).
- Debug fault delivery (D61 defers — single-step, breakpoint, watchpoint may
  warrant distinct labels or a sub-classification in HardwareException).
- D68 supervision notification protocol (the mechanism by which intermediate
  Observers are notified of descendant faults before the chain terminates).
- Cap table slot exhaustion during fault cap installation (if the handler's cap
  table is full when the kernel tries to install the fault Observer cap, the
  fault delivery itself fails — this is a second-order failure mode that
  requires handler-unavailable treatment).

---

## Rejected alternatives

**Fault Observer cap minted from self-cap.** Kernel IS the authority source for
fault messages. Minting from self-cap requires resolving self-cap first, then
attenuating — unnecessary indirection. The kernel has direct access to the
Observer's ObjectId and generation. D80 already settled this; this entry
confirms the rationale.

**More than 5 rights on fault cap.** Each additional right is additional
authority. SUSPEND and MODIFY_SCHEDULING are not fault-recovery concerns. CLONE
would allow the handler to propagate the cap, which is a delegation decision
beyond the scope of fault handling. CHANGE_HANDLER would allow the handler to
redirect future faults, which is a structural decision (D21 handler is set at
Observer creation or via explicit CHANGE_HANDLER operation, not as a side effect
of fault handling).

**Fewer than 5 rights.** Without INSTALL_CAP, handler cannot provide new Space
for page faults (the most common fault type). Without READ_REGISTERS, handler
cannot diagnose. Without WRITE_REGISTERS, handler cannot fix state. Without
RESUME, handler cannot restart. Without DESTROY, handler cannot abandon. All 5
are necessary for the minimum viable fault recovery protocol.

**Kernel-as-handler retries instead of terminates.** Root Observer fault with
kernel as handler has no recovery path (no userspace pager above). Retry loops
would hang on deterministic faults and waste cycles on transient ones with no
diagnostic output. Logging + halt is the correct response to an unrecoverable
fault.

**SYSTEM_RESET instead of SYSTEM_OFF on root fault.** Reset risks boot loops if
the fault is deterministic (same code path runs again, same fault occurs). Halt
(SYSTEM_OFF) is the safe default — the operator can inspect serial output and
decide whether to restart. A future configuration option could select reset
behavior for embedded systems that prefer automatic recovery.

**Separate fault message format (not standard Message).** D61 already settled
this: faults ARE IPC with the kernel as sender. A separate format would create a
parallel delivery path, duplicating Field enqueue logic, register-write logic,
and direct-delivery logic. Using the standard Message format means fault
delivery composes all existing IPC infrastructure.
