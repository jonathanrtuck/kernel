# 011 — Base revocation primitive: close-only + authoritative destroy

**Question:** How does capability revocation work? What mechanism allows
authority conveyed by a capability to be taken back, and by whom?

**Answer (partial):** The base revocation primitive is close-only (refcount on
holder-drop) plus authoritative destroy (an entity with appropriate rights can
destroy the underlying object; outstanding capabilities become dead handles,
observable as errors on next use). Handles carry a generational slot tag —
bumped on slot reuse — to prevent stale-handle aliasing of reused table slots
(ABA defense, not revocation).

Add-on mechanisms for mass invalidation (generation-as-revocation) and selective
revocation (CDT, badges) are deferred. Their value-vs-cost calculus depends on
the IPC model, which has not been settled. Specifically, the case for omitting
generation-as-revocation rests on endpoint rotation (destroy + recreate a
session endpoint, clients reconnect) as the alternative — but endpoint rotation
presupposes endpoint-like kernel objects, a property of the IPC model. Badges
likewise ride on IPC. Committing to add-ons now would skip a level.

---

## Prior work

D4 and D8 both listed revocation as a one-level-down open sub-question:

- journal/004 (D4): named refcount, destroy, CDT, and generation numbers as
  candidates with different cost profiles under D1 and O2.
- journal/008 (D8): established that the flat capability table is compatible
  with refcount, destroy, and generation numbers; CDT would require a separate
  tracking structure. D8 also deferred "handle numbering / ABA prevention,"
  naming generation counters as a candidate.

No current-chain journal has derived revocation. Two tangential mentions:

- journal/009 (D9): seL4 pushes CDT tracking into userspace as part of its
  userspace-managed memory model — rejected on A5 grounds.
- journal/001 (D1): Barrelfish makes cross-core revocation a userspace
  distributed-consensus problem — rejected on A5 grounds.

The archive (restart-1) tentatively landed on close-only + destroy, with CDT
explicitly rejected and generation left open. The archive's chain also deferred
"revocation scope."

Landscape §1.4: seL4 uses CDT; Zircon uses close + destroy with an 8-bit handle
seed for ABA (not revocation); EROS/KeyKOS use prepared/unprepared chains plus
factory/yield for architectural revocation; Coyotos uses non-delegable opaque
capabilities; Mach destroys receive rights; Genode destroys children top-down;
Barrelfish uses two-phase cross-core messaging (~1ms per 2-core revoke, Nevill
2012). Research documents: `authority-models.md` §5.2 tabulates costs per
system, §6.3 documents seL4's unbounded-revocation WCET concern addressed by MCS
preemption points; `capability-revocation.md` organizes the mechanism space
(close-only, authoritative destroy, CDT traversal, link chain, generation
numbers) with cost-at-revoke vs. cost-at-use comparison and stale-capability
discovery modes (eager nulling vs. lazy on-use detection), citing Coyotos's
lazy-rewrite allocation-count pattern, SemperOS's cross-kernel-domain cost
multipliers, and L4.Sec's versioned thread-ID experiment as additional data
points.

---

## Derivation

### Two-level decomposition

The question is not a single choice among four mechanisms. It is a two-level
decision:

1. **Base primitive** — what is always present?
2. **Add-ons** — what additional mechanisms, if any, extend the base?

The mechanisms are not mutually exclusive. Close-only, destroy, and generation
coexist in real systems; CDT is the only one that demands a separate kernel data
structure. Treating the four as parallel options obscures the actual shape.

### Level 1: Base-A vs. Base-B

**Base-A: close-only (pure refcount).** A capability's lifetime is bounded only
by its holders. When the last holder closes it, the object is freed. No entity
can authoritatively invalidate a capability held by another Observer. This is
the archive's tentative default.

**Base-B: close-only + authoritative destroy.** The default revocation is
close-only. In addition, an entity holding sufficient authority can destroy the
underlying object; all outstanding capabilities to it become dead handles
observable as errors on next use.

**Workload survey.** Four structural patterns where cooperative close cannot
serve the workload, under A3's generic coverage:

- _Adversarial targets._ Multi-tenant hosting, security-incident response,
  plugin sandboxing, capability leak remediation. The target has an interest in
  not cooperating; close-only cannot express force-termination of caps held by
  an unwilling party.
- _Failure-mode targets._ Debuggers, watchdog recovery, stuck processes, orphan
  cleanup on parent crash. The target cannot cooperate even if willing —
  deadlocked, infinite loop, or crashed.
- _Pressure response._ OOM killer, real-time deadline-miss recovery, hot code
  replacement. Cooperative shutdown is too slow or structurally impossible when
  the target is blocked on the resource causing pressure.
- _Structural cascade._ Parent destroys child subtree (Genode pattern); session
  teardown on server disconnect; container orchestration with force-stop.

Without destroy at the kernel level, workloads in these patterns must construct
force-termination in userspace. For kernel-owned resources (Observers, address
spaces, memory objects), the userspace construction cannot interpose on
MMU-level access — it would have to route through another kernel mechanism,
which would itself be a form of authoritative destroy under a different name.

This is the O4 (a) pattern: essential complexity forced into userspace. A5
forecloses it. Destroy must be part of the base primitive.

### Level 2: add-ons deferred

Three candidate add-ons:

- **Generation-as-revocation.** Mass invalidation at O(1). Cost: ~1-2 cycles per
  capability check on every Observer (universal payment for non-universal need —
  similar in shape to the hot-path asymmetry D1 was designed to avoid). ~4-8
  bytes per table entry, accounted via D3.
- **CDT.** Selective revocation of a subtree. Cost: separate kernel data
  structure (D8 tension), potentially unbounded synchronous walk without
  preemption points (A4 + D7 tension; seL4 MCS precedent), cross-core
  coordination cost on distributed derivation.
- **Badges on mint.** IPC-carried discrimination tag; service-enforced selective
  revocation for IPC-mediated capabilities. Cost: ~4-8 bytes per table entry;
  delivered on IPC receive.

Each add-on's value proposition rests on an alternative that, in its absence,
userspace would otherwise use:

- _Mass invalidation without generation:_ endpoint rotation — destroy a session
  endpoint, recreate it, clients reconnect. One IPC per client at rotation time,
  none per operation. Requires endpoint-like kernel objects to exist.
- _Selective revocation without CDT:_ badges (service-enforced) or proxy
  indirection (userspace proxy mediates). Both require IPC.
- _Selective revocation of kernel-owned resources without CDT:_ only
  destroy-the-holding-Observer (Base-B destroy applied to the Observer). Works
  for "stop A from running" cases; does not support "retract one cap while
  keeping A alive."

**The dependency.** Each alternative requires the IPC model to have specific
properties — endpoint-like objects for endpoint rotation, per-message tag
delivery for badges, mediation endpoints for proxies. The IPC model is open.
Settling add-ons now means either committing to add-on cost without knowing
whether the alternative makes them unnecessary, or implicitly committing to
IPC-model properties — skipping a level.

Applying the philosophy principle "work one level at a time" and "a decision
cannot be more settled than its least-settled ancestor": the add-ons must wait
on the IPC model. They re-enter the queue when IPC is settled.

### The ABA question

D8 explicitly deferred handle numbering / ABA prevention. The concern: a handle
whose table slot has been freed and reused can accidentally designate the new
occupant.

A generational slot tag — 8-16 bits per handle, incremented on slot reuse —
closes this. It is not revocation: incrementing the tag does not invalidate live
capabilities; it only ensures stale handles do not alias new entries.

The distinction:

- **ABA slot tag (included in D11):** per-slot, bumped on free-and-reuse. Live
  capabilities unaffected.
- **Generation-as-revocation (deferred):** per-object, bumped on explicit
  revoke. All live capabilities to the object become invalid.

Zircon's 8-bit "seed" is the ABA form. Only 256 distinct values suffice for ABA
because the window between slot free and next stale-handle presentation is
short; it would wrap too quickly for revocation use. A 32-bit tag is comfortable
for slot reuse over realistic kernel lifetimes. The exact size is a downstream
implementation choice; the commitment here is that the tag exists.

Including the ABA tag in the base primitive closes D8's deferred ABA question
without committing to generation-as-revocation semantics.

### Derived implications

These are consequences of Base-B + ABA under the existing axioms, not choices:

- **Synchronous revocation.** A4 + D7: no background sweeper; each revocation
  completes before the invoking syscall returns.
- **Forward-effective only.** D4: revocation stops future uses; prior uses
  cannot be undone.
- **Table-slot release ≠ object release.** D8 + D9: closing a slot frees the
  slot; object release follows refcount-zero or authoritative destroy.
- **Cross-core prompt effect costs IPIs.** O2: another core observing a
  revocation before its next natural memory read requires an IPI. Weak
  (eventually-observed) revocation is IPI-free; strong (prompt) is IPI-bounded.
  The policy — which is offered when — is deferred.

### Convergence

Three paths land at Base-B + ABA:

| Path                   | Argument                                                                                                                                                     |
| ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| A5 + workload analysis | Four structural patterns under A3 require terminate-by-force for kernel-owned resources; close-only alone forces userspace reconstruction — O4 (a) violation |
| Archive                | restart-1 chain reached close-only + destroy independently; CDT rejected, generation left open                                                               |
| Landscape              | Every surveyed system with authoritative revocation has destroy as a base primitive; add-on selection varies                                                 |

---

## What this does NOT settle

- **Mass invalidation.** Whether to add generation-as-revocation. Deferred with
  IPC model — the alternative (endpoint rotation via destroy) depends on IPC
  providing endpoint-like objects.
- **Selective revocation.** Whether to add CDT or rely on badge-based
  service-mediated discrimination. Deferred with IPC model — badges are
  IPC-carried.
- **Who authorizes destroy.** Candidates: any holder; a distinct
  destroy-capability; creator-only; parent-hierarchy (Genode). Can be decided
  independently once IPC model lands.
- **Cross-core prompt-effect policy.** Whether destroy is strongly observed
  (IPI-bounded) or weakly observed (eventual). May vary by object type.
- **Destroy cleanup protocol.** Whether cleanup runs inline to completion or has
  preemption points. WCET concern for real-time; interacts with A4 + D7.
- **ABA tag size and encoding.** 8, 16, 32 bits; embedded in handle or stored
  alongside.
- **Budget treatment of freed slots.** When a slot is closed, is its backing
  table memory returned to the Observer's Space or held in the committed pool
  for reuse?
- **Table-full fault ↔ revocation interaction.** D8 deferred the table-full
  fault protocol; revocation's slot-reclamation interacts with it.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Rust's ownership expresses refcount via
`Drop`; atomic slot-tag updates are expressible in safe or narrowly `unsafe`
code. The derivation's core argument — A5 pressure on Base-A via workload
patterns — holds regardless of implementation language.

**A2 (ARM64)** is not load-bearing. Cross-core memory ordering on ARM64 informs
O2, which is cited. No ARM-specific pattern shapes the base primitive.

**A3 (generic)** is load-bearing via the workload survey but is not cited under
"Rests on" because A5 (with O4 (a)) carries the argument structurally; A3 enters
through the inventory of generic workloads that need terminate-by-force. Named
here for traceability.

---

## Status

**Accepted as `spec.md#D11` — settled.**

Revisit if:

- The IPC model decision reveals that Base-B plus IPC-level mechanisms (endpoint
  rotation, badges) do not cover A3-workload needs that generation-as-revocation
  or CDT would otherwise serve.
- A5 is revised (would re-open Base-A).
- A downstream lifecycle derivation (Observer, address space) reveals the base
  primitive is structurally insufficient for a specific pattern.
