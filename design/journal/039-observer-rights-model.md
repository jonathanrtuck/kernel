# 039 — Observer rights model

2026-04-21. Starting from the explicit open question in spec.md: "Observer
rights model. D14 settles resume and destroy as minimum. D23 settles
clonability, enabling rights separation across multiple caps. D35 adds
install-cap and write-registers as confirmed rights. D38 settles that clone is a
per-type right and that rights sets are defined per object type."

All parent decisions settled: D14 (Observer as capability-held), D23
(clonability), D35 (creation API — install-cap, write-registers, resume), D38
(per-type rights, clone in Observer's set), D34 (destroy in rights mask), D7
(typed kernel syscalls), D8 (flat table with rights mask), D20/D21 (per-Observer
fault handler as cap-table entry), D2 (abstract scheduling properties), D36
(Time/Observer split — Time carries compute units, Observer carries scheduling
hints), D37 (Time donation — priority inheritance deferred to D2).

---

## Confirmed rights entering this derivation

Five rights were settled by prior derivations:

1. **resume** — D14 (minimum), D35 (creation). Transitions Observer from faulted
   or inert to runnable.
2. **destroy** — D14 (minimum), D34 (rights mask). Triggers D33 cascade.
3. **install-cap** — D35 (creation, fault resolution, dynamic delegation).
   Writes a cap into the target Observer's table.
4. **write-registers** — D35 (creation, register configuration). Sets register
   state (PC, SP, general-purpose registers).
5. **clone** — D38 (per-type right). Duplicate the capability reference with
   optional attenuation.

---

## Derived (mechanical, not choices)

### Read-registers

D28 (line 1343) already assumes `inspect(observer_handle)` exists as a D7 typed
kernel operation: "Full Observer state (registers, PC, PSTATE) is accessible via
inspect(observer_handle)." D28 is settled. Excluding read-registers would
require revising D28.

Landscape: universal. seL4 TCB_ReadRegisters, Zircon thread_read_state, Mach
thread_get_state, L4 ExchangeRegisters. 100% convergence.

Read-registers is the structural dual of write-registers (D35). D35 established
the write direction; D28 assumes the read direction. Adding read-registers as a
right in the Observer mask makes the assumed operation explicit and gatable
through D23's rights-separation mechanism.

### Duplicate-control removed from this derivation

D23 deferred duplicate-control (Zircon ZX_RIGHT_DUPLICATE) as "can be added to
all kernel object types uniformly whenever the rights model is derived." D38
settled that clone is a per-type right. Duplicate-control — whether a specific
cap can be further cloned — is a cap-table-level mechanism (D8) applicable to
all types uniformly: Space, Field, Time, Observer. It is not Observer-specific.
Deferred to a D8 rights-mask derivation.

---

## Four candidate rights evaluated

### Suspend — included

**Structural demand.** A3 (generic kernel) requires support for diverse
workloads. Three workload patterns require non-destructive external pause of an
Observer: debugging (pause execution to inspect state), checkpointing (quiesce
before snapshot), and resource pressure (park idle Observers without destroying
them).

Without suspend, the only external control over a running Observer is: destroy
(destructive — loses the computation), or IPC request (cooperative — requires
the Observer to be responsive and willing). Neither serves the non-cooperative,
non-destructive case.

**Observer state machine.** Suspend adds a fifth state: externally-suspended,
alongside inert (D35), runnable, blocked (D13 — waiting on Field), and faulted
(D12). Resume covers all stopped states uniformly — the same resume right
transitions inert→runnable, faulted→runnable, and suspended→runnable. This is
the landscape consensus: seL4 TCB_Resume, Zircon task_resume, and Mach
thread_resume all use a single resume operation across stopped states.

Resume disambiguation (what happens if both fault and suspend apply) is a
kernel-internal concern: the kernel tracks which stopped states are active and
resume clears them. An Observer that faults while suspended stays suspended
until both conditions are resolved. The rights model does not need separate
resume variants — a single resume right covers all transitions.

**Interaction with blocked state.** An Observer blocked on a Field (waiting for
a message) can be suspended. The Observer transitions to
suspended-while-blocked. Resume returns it to blocked (not runnable). The block
is a voluntary wait; suspension is an external overlay. This follows Mach and
Zircon semantics.

**Landscape:** Every surveyed capability system includes suspend on the
execution unit. seL4 TCB_Suspend, Zircon task_suspend/task_suspend_token, Mach
thread_suspend, Barrelfish SYSCALL_SUSPEND. Exclusion would be a novel position
with no precedent among capability systems.

**Cost:** One additional state in the Observer state machine. The kernel must
track the suspended flag independently of the other stopped states. This is a
small per-Observer cost (one bit).

### Extract-cap — excluded

D34 identified an "extract" operation (pulling caps from a child's table before
destroy) as a downstream question. The primary use case: a supervisor recovers
resources from a child Observer before destroying it, preventing resources from
cascading to the kernel's root Space (D33).

However, the use case rests on a reactive supervision model — the supervisor
reaches into a failed child to rescue resources it didn't previously hold. Under
D6 (no kernel grouping), "supervisor" and "child" are userspace conventions, not
kernel concepts. The kernel does not know about supervision trees.

Userspace can implement any supervision policy without extract-cap through
proactive cap sharing: when an Observer creates a sub-object (via
create_observer, create_field), the userspace supervision library clones the
resulting cap (D23 — clonable) and sends it to the parent via IPC (D28's user
cap slot) before continuing. The parent holds caps to everything the child
created. If the child fails, the parent already has what it needs.

Extract-cap's value is limited to the emergency case where the child failed
without having shared its caps — a supervision protocol bug, not a kernel
mechanism gap. The kernel should not provide mechanism to compensate for
userspace policy failures.

Additionally, extract-cap is an information-disclosure channel: it reveals what
capabilities the target Observer holds (D4: capability holdings ARE authority).
While gatable through the rights mask, the mechanism's only use case (reactive
recovery) is already served by correct proactive policy.

Deferred. Can be reconsidered if a downstream derivation reveals a structural
use case that proactive cap sharing cannot serve.

### Change-handler — included as separate right

D20 settled per-Observer fault handler. D21 settled the handler as a cap-table
entry at a kernel-reserved slot index. Changing the handler is structurally a
cap-table write at the reserved slot — the same shape as install-cap (D35).

The question: should install-cap govern the handler slot, or is handler change
independently gated?

**Separate right.** The handler slot is already structurally special — it is the
only kernel-reserved slot in D8's flat table (D21). The kernel already
distinguishes it from regular slots for fault delivery (reads from a known
index). D12 establishes the fault handler as structurally critical: every
Observer MUST have a fault handler (hard invariant enforced at creation). The
handler determines which entity handles the Observer's faults — it is the root
of the Observer's supervision relationship (in userspace terms).

Install-cap is designed for routine cap provisioning: adding Space caps for
memory access, delegating Fields for communication, providing Time caps. These
are normal operational concerns. Changing the fault handler changes WHO
supervises the Observer — a fundamentally different kind of authority.

Under D23 (clonable, attenuatable), rights separation is the primary access
control mechanism. A supervisor that delegates install-cap authority to a helper
(e.g., a resource provisioner that adds Spaces to the child on demand) should
not implicitly delegate the power to redirect faults to a different entity.
Separate rights make this decomposition possible; bundled rights foreclose it.

D4 (designation = authority): if handler change is a meaningfully different
authority from routine cap provisioning — and D12's structural criticality
argues it is — it should have its own right.

**Landscape:** seL4 has a dedicated TCB_SetSpace that configures the
handler/CSpace/VSpace — separate from CNode operations that install caps. Zircon
separates exception channel setup from handle table operations. The landscape
leans toward separation.

**Cost:** One additional right in the mask. The kernel must check a different
right for writes to the reserved handler slot vs. regular slots. This is a mild
D8 uniformity cost — the handler slot already behaves differently
(kernel-reserved index, read during fault delivery).

### Modify-scheduling — included

D2 settled that Observers carry abstract scheduling properties (priority, CPU/IO
classification, optional deadline). D36 settled the Time/Observer split: Time
carries compute quantity, Observer carries scheduling hints. D37 notes that
priority-level inheritance during IPC requires scheduling hints to be
"dynamically modifiable" (spec.md open questions, line 2065).

If scheduling hints are externally modifiable by a supervisor (not just by the
kernel during IPC), that modification is an Observer-cap operation requiring a
right under D4 and D7.

**Structural demand.** Three use cases require external scheduling modification:

1. Supervisor-directed priority adjustment. A parent raises a child's priority
   before a latency-sensitive operation and lowers it afterward. Without this
   right, priority is fixed at creation time (or only modifiable by the Observer
   itself, requiring cooperation).

2. Explicit priority inheritance. The server's supervisor reads the caller's
   priority from the Call message metadata and writes it to the server's
   scheduling hints. This complements D37's Time donation (which transfers
   compute capacity but not priority).

3. Load-balancing policy. A supervisor adjusts CPU/IO classification hints based
   on observed behavior (an Observer that was IO-bound has become
   compute-bound).

All three are supervision-level concerns that require external authority over
the target Observer.

**What this right gates.** The concrete scheduling properties are unsettled (D2
open question: minimum abstract scheduling properties). The modify-scheduling
right gates whatever properties D2 eventually settles. The right's existence is
independent of the property set — the question "can an external entity modify
scheduling hints?" is answered here; "which hints exist?" is answered by D2.

**Self-modification.** Whether an Observer can modify its own scheduling hints
is a D7/D4 question about self-reference capabilities. D7 requires a capability
handle as the syscall noun. If the Observer holds a cap to itself (a
self-reference in its cap table), it can modify its own hints using the same
right. If it does not, self-modification requires a separate mechanism or is not
supported. This is deferred — it does not affect the external-modification
right.

**Landscape:** Universal. seL4 TCB_SetPriority/SetMCPriority/SetSchedParams,
Zircon profile application to threads, Mach thread_policy, L4 Schedule,
Barrelfish SYSCALL_DISPATCHER_PROPERTIES. Every surveyed system allows external
scheduling modification. Exclusion would be novel with no precedent.

**Cost:** Concurrent modification complexity. If both the kernel (during IPC
priority inheritance) and an external entity (via this right) can modify
scheduling hints, the kernel must define ordering semantics. The simplest model:
external modification sets the base hints; kernel-internal priority inheritance
is a temporary overlay that reverts on reply. This is the seL4 MCS model (base
priority + effective priority).

---

## The complete Observer rights set

Nine rights:

| Right             | Syscall                                        | Source           |
| ----------------- | ---------------------------------------------- | ---------------- |
| resume            | observer_resume(cap)                           | D14, D35         |
| destroy           | destroy(cap)                                   | D14, D34         |
| install-cap       | observer_install_cap(cap, source_cap) → slot   | D35              |
| write-registers   | observer_write_registers(cap, state)           | D35              |
| clone             | clone(cap, reduced_rights) → new_cap           | D38              |
| read-registers    | observer_read_registers(cap) → state           | D28 (derived)    |
| suspend           | observer_suspend(cap)                          | D39 (this entry) |
| change-handler    | observer_change_handler(cap, field_cap, badge) | D39 (this entry) |
| modify-scheduling | observer_set_scheduling(cap, hints)            | D39 (this entry) |

### Properties of the set

**D7 consistency:** each right corresponds to exactly one typed kernel syscall.

**D8 mask size:** nine rights fit in a 16-bit or 32-bit mask with room for
future additions (duplicate-control, deferred).

**D23 decomposition:** the set supports meaningful authority decomposition. Key
separations enabled:

- resume-only (fault handler needs to resume but not configure)
- install-cap without change-handler (provisioner without supervision authority)
- read-registers without write-registers (debugger can inspect but not modify)
- destroy without suspend (can kill but not pause)
- modify-scheduling without write-registers (priority control without register
  access)

**Relationship to other types' rights:**

- Space: read, write, execute, clone, destroy, create (D31). Six rights.
- Field: send, receive, mint, clone, destroy. Five rights (send-once is a
  specialized send). Plus split/combine (D22, TBD).
- Time: split, destroy. Two rights (no clone — D38). Transfer is a cap-table
  operation, not a Time-specific right.
- Observer: nine rights (this derivation).

Observer has the largest rights set, consistent with the landscape: the
execution unit is the most-operated-upon kernel object in every surveyed system.

---

## Observer state machine (updated)

Five states:

```text
inert → runnable → blocked → faulted
                      ↕         ↕
                 externally-suspended
```

- **inert:** created (D35) but never started. No register state configured, or
  registers written but resume not yet called.
- **runnable:** eligible for scheduling by the per-core Time manager (D2).
- **blocked:** voluntarily waiting on a Field (D13 receive).
- **faulted:** involuntarily stopped by a fault (D12). Fault message delivered
  to handler.
- **externally-suspended:** paused by an external entity via suspend. Can
  co-occur with blocked (suspended-while-blocked) or faulted
  (suspended-while-faulted). Resume clears the suspension; if an underlying
  blocked or faulted condition remains, the Observer stays in that state.

Transitions:

- inert → runnable: resume (first start)
- runnable → blocked: Observer calls receive() on a Field with no messages
- blocked → runnable: message arrives on the waited Field
- runnable → faulted: hardware fault (page fault, invalid cap, etc.)
- faulted → runnable: resume (after handler resolves fault)
- runnable → externally-suspended: suspend
- blocked → suspended-while-blocked: suspend
- faulted → suspended-while-faulted: suspend
- externally-suspended → runnable: resume (if no underlying block/fault)
- suspended-while-blocked → blocked: resume
- suspended-while-faulted → faulted: resume

Resume is a single right covering all →runnable/→unblock transitions.

---

## What this does NOT settle

- **Observer minimum schema.** The concrete struct fields are a separate
  derivation. D39 constrains it: the struct must track all five states (inert,
  runnable, blocked, faulted, externally-suspended) including co-occurrence of
  suspended with blocked/faulted.
- **D2 minimum scheduling properties.** modify-scheduling gates whatever D2
  settles. The property set is open.
- **Self-reference capabilities.** Whether an Observer holds a cap to itself
  (enabling self-modification of scheduling hints, handler, etc.) is deferred.
- **Duplicate-control right.** Deferred to a D8 derivation applicable to all
  types uniformly.
- **Extract-cap.** Deferred. Reconsider if a structural use case emerges that
  proactive cap sharing cannot serve.
- **Specific syscall encoding.** Register conventions, error codes, how hints
  are represented in modify-scheduling — implementation details.
- **Concurrent scheduling modification semantics.** External modification vs.
  kernel-internal priority inheritance ordering. The base/effective priority
  model (seL4 MCS precedent) is a natural candidate but not settled here.

---

## Archive convergence

Restore the archive for convergence checking:

The archive (restart-1) had a 10-syscall design including: create_context,
destroy_context, context_resume, context_suspend, read_context_state,
write_context_state, context_install_cap, context_grant_time, set_fault_handler,
set_scheduling_params.

**Strong convergence on operations.** The archive's set maps closely to D39's:
resume, destroy, suspend, read-registers (read_context_state), write-registers
(write_context_state), install-cap, change-handler (set_fault_handler),
modify-scheduling (set_scheduling_params). The archive included create (bundled
with creation, not a right on an existing Observer) and grant_time (subsumed by
install-cap under D30 multi-Time as regular cap-table entries).

**Divergence on creation model.** The archive bundled creation into the Observer
rights set (create_context as an Observer operation). D35 settled creation as a
Space-consuming operation — the "create" right lives on the Space cap (D31), not
the Observer cap.

**Divergence on Time handling.** The archive had a dedicated context_grant_time.
Under D30 (Time caps in regular cap-table slots), Time installation is a special
case of install-cap — no separate operation needed.

**Convergence on handler as separate.** The archive's set_fault_handler was a
dedicated syscall, separate from context_install_cap. D39 reaches the same
separation for different reasons (D12 criticality + D21 reserved slot
specialness).

---

## Status

**Settled.** The Observer capability rights set is: resume, destroy,
install-cap, write-registers, clone, read-registers, suspend, change-handler,
modify-scheduling. Nine rights, each corresponding to a typed kernel syscall
(D7), each a bit in D8's per-cap rights mask.

Revisit if:

- D35 is revised (changes the creation API model that established install-cap
  and write-registers as separate operations)
- D20/D21 are revised (changes the fault handler representation that motivates
  separate change-handler)
- D2 is revised (changes the scheduling property model; may affect
  modify-scheduling's scope but not its existence)
- A downstream derivation reveals that extract-cap serves a structural need that
  proactive cap sharing cannot (would add a tenth right)
- The Observer minimum schema derivation reveals that the five-state machine
  creates essential complexity that a simpler model would avoid (would reopen
  suspend)
