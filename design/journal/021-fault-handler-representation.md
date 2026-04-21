# 021 — Fault handler is a cap-table entry

**Date:** 2026-04-18 **Starting point:** D20 settled per-Observer fault handler
attachment but deferred the representation question: is the per-Observer handler
reference a regular entry in the Observer's D8 flat capability table, or a
kernel-internal field (direct pointer outside the capability system)? D20's
journal noted the derivation "strongly indicates cap-table entry" via D17
badge-closure but did not formally settle it. D17's journal identified the
structural connection between representation and badge-closure lifecycle
visibility.

---

## The question

Each Observer stores a fault handler field reference and a badge (D20). Two
representation options:

- **Cap-table entry:** The handler is a regular capability in the Observer's D8
  flat table at a kernel-known slot index. Entry format matches D8: (object_ptr,
  rights_mask, badge, slot_tag).
- **Kernel-internal field:** The handler is a (field_ptr, badge) tuple stored
  directly in the Observer struct, outside the capability table.

## Systematic pass — interactions found

A1–A5, O1–O4: no direct interaction. The representation is kernel-internal
structure; axioms don't push either way.

### D11 — Destroy invalidation (strongest structural argument)

When the fault handler field is destroyed via D11 authoritative destroy:

**Cap-table entry:** D11's destroy mechanism walks capability tables to find and
invalidate all capabilities pointing to the destroyed object. The fault handler
entry is a regular entry — found and invalidated automatically. The slot tag is
bumped. Next fault: kernel reads the entry, finds it dead, handles the "pager
unavailable" case. Zero special-case logic.

**Kernel-internal:** The handler reference is invisible to D11's capability
table walk. The kernel must maintain a separate tracking structure —
back-pointers from each field to all Observers using it as a handler — so that
field destroy can find and nullify those references. This is a parallel
revocation system for a single use case. Alternatively, deferred invalidation
with generation-number machinery — but D11's slot tag already provides this for
cap-table entries.

The structural argument: don't rebuild what the existing system handles.
Applying "push complexity to the leaves" fractally within the kernel — the
capability system is the leaf that already absorbs reference lifecycle; creating
a second leaf for the same concern adds accidental complexity (O4 (c) check: is
this essential? No — the cap-table entry eliminates it).

### D17 — Badge-closure lifecycle visibility

**Cap-table entry:** Observer destroy closes all caps in the Observer's table.
The fault handler entry is closed. If the handler field has opt-in per-badge
tracking, this triggers a badge-closure notification. The mechanism is generic —
cap-close is the universal trigger. No Observer-destroy-specific logic.

**Kernel-internal:** Observer destroy must explicitly check whether the handler
field has per-badge tracking and invoke the badge-closure mechanism manually.
This couples Observer-destroy code to the badge-closure subsystem. D17 journal
identified this: "no equivalent substitute" for the cap-table entry's automatic
lifecycle visibility.

### D8 — ABA protection

**Cap-table entry:** D11's generational slot tag prevents stale-handle aliasing.
If the handler field is destroyed and a new field is created at the same memory
address, the old cap-table entry is already invalidated.

**Kernel-internal:** A direct pointer to the field. If the field is destroyed
and its memory reused, the pointer becomes dangling. Prevention requires the
same tracking structure as D11 above.

### D4 — Capability-based authority

With cap-table entry, the handler participates fully in the capability system:
designation = authority extends to the handler reference. With kernel-internal,
it's a special case — a kernel object reference outside the system that D4 + D8

- D11 provide.

Weakened by the fact that the kernel doesn't check rights on the fault path
(kernel-as-sender reads the field pointer directly). Rights matter at
configuration time (set_fault_handler), not at use time. But the handler still
participates in D11's revocation and D17's lifecycle mechanisms, even though
rights aren't exercised on the fault path.

### D1 — Hot-path cost (tension, weak)

Cap-table entry adds one dependent memory access on the fault path: Observer
struct → cap table base pointer → cap_table[index]. If the cap table is in a
different cache line (likely — separate allocation), one additional L1/L2 miss.

However, the fault path continues with IPC message delivery through the field
queue (~400 cycles on ARM64), which is several more memory accesses. The handler
lookup is a fraction of total fault-path cost. D1's structural concern (no
cross-core shared state on the hot path) is satisfied by both options — both are
per-Observer data.

Tension is real but marginal.

---

## The decision

**The fault handler reference is a cap-table entry.** A regular capability in
the Observer's D8 flat table at a kernel-known slot index. The entry carries
send rights to the handler field, the per-Observer badge (D20), and a slot tag
(D11).

The cap-table entry model requires zero new infrastructure:

- D11 destroy-invalidation: free (standard cap-table walk)
- D17 badge-closure on Observer destroy: free (generic cap-close)
- D8 ABA protection: free (generational slot tag)
- Observer destroy cleanup: generic (close all caps)

The kernel-internal model requires:

- A parallel tracking structure for field-destroy invalidation
- Explicit coupling between Observer-destroy and badge-closure
- Dangling-pointer prevention machinery

The sole cost of cap-table entry — one extra memory access on the fault path —
is marginal relative to the IPC delivery that follows.

### Reserved slot

The handler cap occupies a kernel-reserved slot index (e.g., slot 0). The kernel
always knows where to find it on the fault path without a secondary lookup. This
is a convention in the "kernel manages slots" model (D8), not a structural
commitment — the slot index is an implementation constant.

This means the Observer struct does NOT need a separate fault-handler-slot-index
field. The kernel knows the index at compile time.

---

## Downstream implications

- **Observer minimum schema:** The fault handler is NOT a separate field in the
  Observer struct. It's an entry in the Observer's capability table. The
  Observer struct stores the cap table pointer (already required by D8); the
  handler lives at a known index in that table. The Observer minimum schema need
  not include a fault handler field — only the cap table pointer.

- **Observer creation API:** Creating an Observer must install a send cap to the
  handler field (with the designated badge) at the reserved slot index. This is
  a cap-table write, not a separate struct-field initialization.

- **set_fault_handler() syscall:** If the Observer rights model includes fault
  handler mutability, the operation replaces the cap-table entry at the reserved
  slot. Same mechanism as any capability slot operation, with an Observer-cap
  right check.

- **Structural precedent:** This settles the representation for the fault
  handler. The address space binding (D10) is a parallel question with different
  access-frequency characteristics (TTBR on every context switch vs. handler on
  faults). The choice here does not force the same answer for D10's binding, but
  the arguments rhyme — D11 invalidation and ABA protection apply equally.

---

## Archive convergence

**Divergence.** The archive (restart-1) chose kernel-internal: "Fault handler —
(wormhole_ref, badge) pair (kernel-internal, not a handle)" (archive spec.md
line 149). The archive's IPC primitive ("wormhole") had badges but not D17's
opt-in per-badge lifecycle tracking with badge-closure notifications. Without
badge-closure, the strongest structural argument for cap-table entry (D17
lifecycle visibility) did not exist in the archive's derivation context.

The current chain's D17 (badge semantics with opt-in tracking) creates a
structural advantage for cap-table entry that the archive could not have
derived. The divergence is explained by the difference in settled decisions, not
by a reasoning error in either chain.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Rust's ownership and Drop semantics could map
to either cap-table close or explicit cleanup. The choice is made on structural
grounds (D11, D17), not language grounds.

**A2 (ARM64)** is not load-bearing. No ARM64-specific concern pushes either way.

**A3 (generic)** is not load-bearing. The representation is kernel-internal
structure, not workload-facing.

**A4 (purely reactive)** is not load-bearing. Both representations are
compatible with synchronous fault handling.

**A5 (leaf node)** is not directly load-bearing. Both options are
kernel-internal complexity; neither pushes complexity to userspace. The "push
complexity to the leaves" philosophy principle applies fractally (the capability
system is the leaf that absorbs reference lifecycle), but that is the philosophy
operating as a strategy, not A5 operating as a design input.

---

## What remains open

- **Reserved slot index value.** Implementation detail — slot 0 is natural but
  arbitrary. Resolve during implementation.
- **Rights on the handler cap.** Likely: send right only. The kernel uses the
  field pointer directly (kernel-as-sender), so rights are checked at
  configuration time, not fault time. Whether the handler cap should carry
  additional rights (receive? mint?) depends on the Observer rights model.
- **Address space binding representation.** Parallel question opened by this
  decision. Same structural arguments (D11, D8 ABA) apply, but different access
  frequency (every context switch) may justify a different answer or a
  cache-in-struct optimization.
- **Pager unavailability.** What happens when the kernel reads the handler cap
  and finds it dead (field destroyed)? This is the "pager unavailable" protocol
  — still open (D12 downstream). D21 makes the detection mechanism clear: the
  cap-table entry is dead (D11 invalidated).
