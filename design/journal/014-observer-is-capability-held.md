# 014 — Observer is a capability-held kernel object type

**Date:** 2026-04-18 **Starting point:** D6 settles Observer = single execution
unit but explicitly defers: "Observer lifecycle. Create, destroy, suspend,
resume. Whether Observer is a capability-held object type." D12 creates
structural demand for resume (a suspended Observer can't participate in IPC). D7
provides the mechanism family (typed kernel syscalls). This is the
highest-leverage open question before entering the IPC cluster — the archive
discovered it at the wrong time (archive/013, after building IPC mechanisms) and
had to reverse two prior entries.

---

## The question

Is Observer a kernel object type designated by capabilities, with lifecycle
operations as typed kernel syscalls? Or is Observer an emergent composition
managed indirectly through its constituent resources and IPC?

---

## The derivation chain

Five settled decisions form a mechanical chain:

1. **D12 (fault delegation)** requires the pager to resume a faulted Observer.
   The Observer is involuntarily suspended — it never called receive(). It can't
   participate in IPC. The kernel must change its state directly.

2. **D7 (split interaction model)** says Observer→Kernel operations are typed
   kernel syscalls, not IPC. Resume is Observer→Kernel (the pager asks the
   kernel to change the target's state). Therefore resume is a typed kernel
   syscall.

3. **D4 (capability-based authority)** requires the syscall to name its target
   through a capability handle. No ambient privilege, no "resume by ID." The
   noun is a capability handle designating the target Observer.

4. **D8 (flat capability table)** accommodates Observer handles with no special
   infrastructure. An Observer capability is an entry: (kernel object pointer,
   rights mask). The rights mask governs which operations are permitted. The
   generational slot tag (D11) prevents stale-handle aliasing.

5. **D11 (base revocation)** provides the termination primitive.
   Destroy(observer_handle) eliminates the Observer. Outstanding capabilities
   become dead handles, observable as errors on next use. No new termination
   mechanism needed.

Each step follows from a settled decision. The chain is D12 → D7 → D4 → D8 →
D11, with D6 providing the definition of what the handle designates.

---

## The alternative: Observer is NOT a capability-held type

The archive explored this path across three journal entries:

**Archive/006:** "Context is not an object type." Lifecycle through resource
control and IPC indirection. The argument: every post-creation operation on a
Context can be mediated through resource control, IPC, or field indirection.

**Archive/011:** Introduced control Fields — per-Context, kernel-intercepted,
processed inline (no queue), state-checked before acting. Used for fault resume
(handler sends "resume" to control Field) and lifecycle management.

**Archive/013:** Reversed both. The critical discovery: resume requires a direct
kernel handle. The suspended Observer never called receive() — IPC can't reach
it. The control Field was "a syscall interface wearing an IPC costume" —
kernel-intercepted, no queue, processed inline. The semantics diverged
completely from peer IPC while sharing the send() entry point. Making it an
actual syscall was more honest.

The current chain's settled decisions reinforce the archive's conclusion:

- D7 (settled after the archive's chain) independently identifies the
  Observer→Kernel asymmetry and creates the typed-kernel-syscall mechanism
  family. Control Fields would violate D7.
- D12 (settled after the archive) creates explicit structural demand for resume
  that the archive discovered only through IPC exploration.
- D4 (settled in both chains) requires a capability handle as the noun.

The alternative was explored, found insufficient, and reversed by the archive.
The current chain's independently derived foundations (D7, D12) reach the same
conclusion without depending on the archive's specific path.

---

## Landscape convergence

Every surveyed capability system makes the execution unit a capability-held
kernel object type:

| System       | Object             | Capability-held?   | Operations                      |
| ------------ | ------------------ | ------------------ | ------------------------------- |
| seL4         | TCB                | Yes                | Configure, Resume, Suspend, ... |
| seL4 MCS     | TCB + SchedContext | Yes (separate)     | + SchedContext bind             |
| Zircon       | Thread             | Yes (handle)       | create, start, kill, suspend    |
| Mach         | Thread             | Yes (port)         | create, terminate, suspend      |
| EROS/Coyotos | Process            | Yes                | Unified model                   |
| Barrelfish   | Dispatcher         | Yes (cap)          | Create, invoke, delete          |
| Composite    | Thread             | Yes + SchedContext | Migrate, sched bind             |

No surveyed capability system manages execution-unit lifecycle through IPC
indirection alone. Applying "when independent paths converge, trust the
convergence" (philosophy): the derivation chain, the archive's independent
exploration, and the landscape survey all point the same way.

---

## The decision

**Observer is a capability-held kernel object type.** Observer joins Space,
Time, Coordinate System (D10), and field (D13) as the fifth kernel object type.
Lifecycle operations — at minimum resume and destroy — are typed kernel syscalls
(D7) taking Observer capability handles. The capability's rights mask governs
which operations are permitted.

**Fault resume flow (D12 + D14):** Observer faults → kernel delivers fault
notification as field message (D13) containing an Observer handle via capability
transfer → pager processes fault → pager calls resume(observer_handle) as typed
kernel syscall (D7) → kernel changes Observer state from suspended to runnable.

**Termination (D11 + D14):** destroy(observer_handle) eliminates the Observer.
D11's dead-handle semantics apply. close(observer_handle) drops the holder's
reference without destroying the object.

---

## Composition with D13 (tentative)

D14 does not depend on D13's IPC model specifics. The derivation chain
(D12→D7→D4) is load-bearing; D13 provides a convenient delivery mechanism
(capability transfer in field messages carries the Observer handle to the pager)
but is not structurally required. If D13 moved — different IPC model, different
message format — the Observer handle would still need to reach the pager through
whatever delivery mechanism replaced it, and resume() would still be a typed
kernel syscall.

D14 is independent of D13's tentative status. The IPC model determines HOW the
handle is delivered; D14 determines THAT a handle exists.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Rust's type system will shape the Observer
struct and capability entry representation, but the derivation does not pass
through A1. A1 becomes relevant one level down (Observer struct layout, trait
implementations).

**A2 (ARM64)** is not load-bearing. ARM64 defines the register file (what the
Observer's saved state contains concretely) but does not push toward or away
from Observer-as-object-type. The derivation rests on D12, D7, D4, D8, D11.

**A3 (generic)** is not load-bearing here. A3 was load-bearing for D12 (no
single paging policy) and D6 (no kernel grouping), which are upstream. By the
time this derivation runs, A3's work is done through its descendants.

**A5 (kernel absorbs complexity)** is not load-bearing in the primary chain
(D12→D7→D4 does the work). A5 provides a secondary confirmation: if Observer
lifecycle were pushed to userspace via IPC indirection, userspace would rebuild
syscall semantics from IPC primitives — an O4(a) violation. But the primary
chain forecloses this path before A5 needs to weigh in.

---

## What this does NOT settle

- **Observer creation API shape.** Create-then-configure (seL4 — maximum
  flexibility, Observer created in inert state, configured via capability
  operations, started separately) vs. all-params-upfront (archive — one syscall,
  all resources provided at creation). Judgment about interface ergonomics,
  atomic guarantees, and error handling.

- **Observer rights model beyond resume and destroy.** Candidates: suspend
  (pause non-faulted Observer), inspect register state (debugging), modify
  scheduling properties (D2), change fault handler, change address space binding
  (D10 "binding mutability"). Each right = a potential typed kernel syscall.

- **Observer handle clonability.** Clonable: multiple independent lifecycle
  managers, flexible delegation. Non-clonable (like Time): exactly one manager.
  Affects whether handle=handler unification is possible.

- **Suspend as a distinct operation.** Is there external suspension (not caused
  by fault)? If yes, Observer state gains a fourth value (runnable, blocked,
  faulted, externally-suspended) and the rights model must address resume
  disambiguation.

- **Fault handler attachment.** D12 defers per-Observer vs. per-address-space.
  D14 provides the Observer handle as a natural per-Observer configuration noun.
  Decision still open.

- **Time reclamation on destroy.** Observer holds one Time (D6). On destroy:
  return to destroyer? To creator? Destroy the Time too? Interacts with Time's
  non-clonable property.

---

## Status

**Settled.** Observer is a capability-held kernel object type with lifecycle
operations as typed kernel syscalls. The derivation chain (D12→D7→D4→D8→D11) is
mechanical — each step follows from a settled decision. Archive convergence
(archive/013) and landscape convergence (100% of surveyed capability systems)
provide independent confirmation.

Revisit if D7 is revised (unified model would change the mechanism family) or if
D12 is revised (removing fault delegation would remove the structural demand for
resume — though D4 and D6 would still support Observer-as-object-type on
independent grounds).
