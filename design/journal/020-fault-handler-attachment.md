# 020 — Per-Observer fault handler attachment

**Date:** 2026-04-18 **Starting point:** D12 settled fault delegation to
userspace pagers but deferred where the handler attaches: "Does the fault
handler attach to the Observer or the address space (D10)?" The question was
subsequently noted as open in journal 014 (Observer as capability-held) and
journal 017 (badge semantics). D17 identified a structural connection between
fault handler representation and badge-closure lifecycle visibility. All parent
decisions are settled: D12, D14, D17, D10, D4, D6.

---

## The question

Where does the fault handler attach — to the Observer (per-Observer), to the
address space (per-address-space, D10), or both (address space default with
Observer override)?

This is a two-facet question:

1. **Attachment point** — which kernel object stores the fault handler endpoint
   reference?
2. **Representation** — is the reference a cap-table entry or a kernel-internal
   field?

The attachment point is the primary structural decision. The representation is a
connected sub-question that the attachment decision shapes.

---

## Derived constraints (not choices)

Three things follow mechanically before the attachment-specific analysis:

**1. Badge must be per-Observer regardless of endpoint attachment.** D17 derived
constraint #4: the kernel synthesizes fault messages without a sender cap, so a
badge must be stored alongside the handler reference. If the endpoint is
per-address-space, the badge must still be per-Observer — otherwise the pager
can't distinguish which of N Observers in a shared address space faulted. A
shared badge makes the badge mechanism void for fault traffic.

**2. D14 already delivers the Observer handle in fault messages.** The pager
always receives a capability handle to the faulting Observer via cap transfer
(D14). This provides identification. But cap transfer is structurally heavier
than badge reading — it involves D8 slot allocation in the receiver's table. The
badge provides lightweight identification; the handle provides
authority-carrying identification. Both serve the pager.

**3. D12 requires every Observer to have a fault handler.** This is a hard
invariant. The question is whether it maps to a local property (non-null field
on the Observer struct) or an indirect property (Observer must bind to an
address space that has a handler).

---

## Per-address-space attachment: five tensions with settled decisions

Systematic pass through spec.md revealed five tensions between per-address-space
attachment and settled decisions:

**T1: D6 (no kernel grouping).** D6 explicitly says "process" is a userspace
convention; the kernel has no grouping mechanism. Per-address-space fault
handler attachment creates an implicit kernel-level grouping: the address space
becomes a de facto process with a shared fault policy. D6 rejected policy
grouping because D4 capabilities handle lifecycle without target cooperation and
A3 makes grouping non-universal.

Counter-argument: D10 already groups Observers by mapping — the address space IS
a shared configuration point. But D10's grouping is structural (memory access),
while fault handler grouping is policy (fault routing). D6 rejected policy
grouping specifically.

**T2: D4 (independent delegation).** Per-Observer allows independent delegation
of fault handler configuration authority via Observer cap rights. Per-address-
space ties fault handler authority to address space authority — you can't give
someone per-Observer fault handler control without giving them address space
access. D4's "designation = authority" principle supports fine-grained,
independently delegatable authorities.

**T3: D17 badge-closure lifecycle visibility.** D17's opt-in per-badge tracking
enables badge-closure notifications: when the last send cap with badge B to
endpoint E is closed, the kernel enqueues a closure notification. If the fault
handler reference is a per-Observer cap-table entry, Observer destruction closes
the cap, triggering badge-closure. The pager receives "child with badge B is
gone" for free.

Per-address-space: individual Observer destruction does NOT close the address
space's handler cap. Badge-closure doesn't provide per-Observer lifecycle
visibility. The pager must learn about child destruction through a separate
mechanism.

**T4: D11 destroy cascade.** If the handler endpoint is per-address-space and
the endpoint is destroyed (D11 authoritative destroy), all Observers in the
address space simultaneously lose their handler. D12's invariant ("every
Observer has a handler") is violated for all of them at once. The kernel must
handle this cascade — kill the Observers? Leave them in an invalid state?

Per-Observer: each handler is independent. Destroying one Observer's handler
doesn't affect others.

**T5: D1 hot-path cost.** Per-Observer reads (endpoint, badge) from the Observer
struct — one cache line, already in cache from register save. Per-address-space
reads the endpoint from the address space struct (one extra pointer chase) and
the badge from the Observer struct (split across two objects). Small cost, but
D1 says hot-path simplicity matters.

---

## Per-both (address space default, Observer override): partially mitigates

The Mach (thread overrides task) and Zircon (thread > process > job)
hierarchical models partially mitigate the per-address-space tensions: the
Observer can always override. But:

- Both Mach and Zircon have kernel-level process/task/job concepts. This kernel
  explicitly rejected kernel grouping (D6). The hierarchical model is designed
  to compose with grouping that doesn't exist here.
- Interface surface: two configuration paths (set_observer_handler,
  set_address_space_handler), override/fallback semantics.
- Mixed D11 cascades: address space handler destroy affects only Observers
  without overrides. Some Observers affected, some not — harder to reason about
  than per-Observer's uniform behavior.
- D17 badge-closure works only for the override path, not the default path.
  Inconsistent lifecycle visibility.
- D12 invariant maintenance: at least one of the two must be non-null. More
  complex than a single non-null check.

---

## Foreclosed alternatives

**Pure per-address-space with shared badge** is functionally broken. If both the
endpoint AND badge are per-address-space, the pager can't distinguish which
Observer faulted via badge. The Observer handle (D14) in the fault message
provides identification, but the badge mechanism becomes semantically void for
fault traffic. Contradicts D17's structural requirement.

**Per-region (Coyotos GPT model)** is foreclosed by D9 + D5. D9's variable- size
kernel-managed memory objects hide address space structure. D5's memory
interface is objects-and-permissions, not page-table-specific concepts. There is
no userspace-visible region tree to attach handlers to.

---

## Per-Observer: where the derivation chain points

Every settled decision either favors per-Observer or is neutral:

- **D6:** consistent — no grouping.
- **D4:** independent delegation — fault handler control separable from address
  space authority.
- **D17:** badge-closure lifecycle visibility works (if cap-table entry).
- **D12:** local invariant — non-null field, checked at creation.
- **D14:** natural configuration noun — set_fault_handler(observer_handle, ...).
- **D1:** simplest hot path — single cache-line access.
- **D11:** per-Observer cascade only — no cross-Observer effects.

The only cost is redundant configuration when N Observers in the same address
space want the same handler. This was examined as the best argument against
per-Observer during evaluation. The cost is userspace ergonomics: a library
function that creates Observers can supply the same handler/badge to each one.
No kernel complexity cost. No structural foreclosure.

Handler migration (changing the handler for N Observers) costs N syscalls
instead of 1. This is an administrative operation, not hot-path, and a userspace
process manager can iterate.

---

## The decision

**The fault handler attaches to the Observer.** Each Observer stores a fault
handler endpoint reference and a badge. On fault, the kernel reads both from the
faulting Observer's struct and delivers a fault notification to the handler
endpoint with the stored badge, plus the faulting Observer's capability handle
via cap transfer (D14).

Every Observer creation must supply a fault handler endpoint and badge. The
kernel rejects creation of an Observer without a handler (D12 invariant enforced
at creation time).

---

## Representation: cap-table entry vs. kernel-internal (closely connected)

The attachment decision shapes but does not fully settle the representation
question.

**Cap-table entry** (the fault handler is a regular capability in the Observer's
capability table, at a kernel-known slot index):

- Badge-closure fires on Observer destroy → pager gets lifecycle visibility
  (D17). This is the unique structural advantage.
- Uniform cleanup via generic cap-close path — no special-case kernel logic.
- Full participation in the capability system: rights mask, ABA protection
  (D11), revocation.
- Cost: one cap-table slot per Observer.

**Kernel-internal field** (a direct (kernel object pointer, badge) tuple in the
Observer struct, outside the cap table):

- No badge-closure on Observer destroy. Pager needs a separate lifecycle
  visibility mechanism.
- Requires explicit kernel cleanup logic on destroy (manual bookkeeping that the
  capability system handles automatically for cap-table entries).
- The handler exists outside the capability system — no rights mask, no ABA tag.
  If the handler endpoint is destroyed (D11), the kernel must manually
  invalidate this reference.
- Slightly faster fault dispatch (direct pointer, no table lookup). Doesn't
  consume a cap-table slot.

The derivation strongly indicates cap-table entry: D17 badge-closure is a
structural advantage with no equivalent substitute, and the costs of
kernel-internal (manual cleanup, exiting the capability system) are real
burdens. But this sub-question was not the focus of the evaluation discussion
and is recorded as a closely connected open question rather than a settled
decision.

---

## Archive convergence

The archive (restart-1) chose per-Context (≡ per-Observer) in journal/012 (badge
assignment): "Fault path stores `(endpoint_ref, badge)`" per Context. The
current chain arrives at the same conclusion from independently derived
foundations (D6, D4, D17, D14 — none of which existed in the archive's chain at
the time of its fault handler decision).

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. The Observer struct layout and cap-table
entry implementation are shaped by Rust, but the attachment question does not
pass through A1.

**A2 (ARM64)** is not load-bearing. ARM64 provides the fault information
(ESR_EL1, FAR_EL1) but does not push toward or away from any attachment model.

**A3 (generic)** is not directly load-bearing. A3's work is done through D6 (no
kernel grouping) and D12 (generic paging policy via delegation). A3 does provide
secondary support: generic workloads include both same-handler and
different-handler patterns for co-located Observers, so assuming all Observers
in an address space want the same handler is unjustified.

**A5 (kernel absorbs complexity)** is not load-bearing in the primary chain. The
"redundant configuration" cost of per-Observer is userspace ergonomics, not
essential complexity pushed to userspace. A userspace library absorbs it.

---

## What remains open

- **Fault handler representation.** Cap-table entry vs. kernel-internal field.
  The derivation strongly indicates cap-table entry (D17 badge-closure is the
  structural advantage), but is not formally settled here. The D17 connection
  makes this a high-value downstream question.

- **Fault handler mutability.** Can the fault handler be changed after Observer
  creation? Per D14's open "Observer rights model" question, this is a right in
  the rights mask: set_fault_handler(observer_handle, endpoint_handle, badge)
  requiring appropriate rights. Whether this right exists and who controls it is
  part of the Observer rights derivation.

- **Fault handler in Observer creation API.** Whether the handler is a creation
  parameter (all-params-upfront) or a post-creation configure operation
  (create-then-configure). Part of the open Observer creation API question.

- **Pager unavailability protocol.** D12 downstream: what happens when the pager
  is destroyed/unresponsive. Unaffected by the attachment decision (still open
  regardless).

- **Root/bootstrap fault handling.** D12 downstream: the initial Observer has no
  userspace pager. Unaffected by attachment (still open).
