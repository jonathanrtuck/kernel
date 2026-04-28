# D107 — Auto-destroy on zero refcount

**Question:** Should the kernel auto-destroy objects when their last capability
reference is removed (refcount hits zero on Close), or keep the current model
where Close decrements the refcount but only an explicit Destroy syscall frees
the arena slot?

**Answer:** Auto-destroy. When any Close operation brings an object's refcount
to zero, the kernel destroys the object inline. Structural backing returns to
the kernel's root Space (D31), not to the closer. Explicit Destroy remains for
force-killing objects with live references (refcount > 0), where the destroyer
receives the backing Space cap (D98).

---

## Prior work

**Journal 024 (Observer Handle Clonability):** Explicitly names "auto-destroy on
last-cap-close (collapsing the close/destroy distinction)" as one solution to
the orphan risk under non-clonable handles. The decision settled on clonable
handles (dissolving the non-clonable orphan risk) but did not settle whether
auto-destroy applies generally.

**D33 (Destroy Cascade Protocol):** "Objects reaching refcount zero are
destroyed too" — within cascade context. The cascade auto-destroys at zero
refcount as part of cleanup. This entry extends that behavior to all close
paths.

**D11 (Base Revocation):** Journal line 187: "Table-slot release ≠ object
release. Closing a slot frees the slot; object release follows refcount-zero or
authoritative destroy." Names refcount-zero as a path to object release without
settling when it applies. Defers "memory reclamation of freed slots" — this
entry settles it.

**D98 (Destroy Cascade and Return):** Settles cascade mechanics. Cascade-freed
objects return backing to root Space. Auto-destroy on close follows the same
pattern.

`design/research/destroy-cascade-protocol.md`: Zircon auto-destroys on last
handle close. seL4 auto-destroys during CNode deletion. EROS/KeyKOS use explicit
sell-back. The landscape splits on this question; Zircon's approach is the
closest model for this kernel's design.

---

## Derivation

### The leak problem

Under explicit-only Destroy, a zero-refcount object is permanently unreachable.
No entity holds a cap, so no entity can call Destroy. Under A4 (purely
reactive), no background sweeper exists to detect or reclaim the object. The
arena slot, backing pages, and (for Time) compute capacity are permanently lost.

This is not an edge case. D38 settles Time as non-clonable: refcount is always 0
or 1. Every close of a Time cap sets refcount to zero. Under explicit-only,
every Time close creates a permanent orphan with leaked compute capacity. This
is the normal outcome of the simplest usage pattern — create a Time, use it,
close it.

### Three precedents

The kernel already acts on zero-refcount events:

1. **D24 (cap-mapping invariant):** "When an Observer loses its last capability
   to a Space, the kernel removes the corresponding page table entries." Same
   structural pattern — kernel action triggered by last-reference removal —
   applied to mappings rather than object lifetime.

2. **D33 (cascade):** "Objects reaching refcount zero are destroyed too." Auto-
   destroy at zero refcount already exists within cascades. Whether the close
   that reaches zero happens inside or outside a cascade is an implementation
   detail of call ordering, not a semantic distinction.

3. **D17 (badge-closure):** "When the last send cap with badge B to field E is
   closed, the kernel enqueues a closure notification." The kernel already
   tracks and acts on per-badge last-reference events.

### D4 authority semantics

The DESTROY right in D8's rights mask controls authoritative destruction —
force-killing an object with live references, depriving other cap-holders of
access. At zero refcount, no cap-holder exists to be deprived. The destruction
is reclamation of an unreachable object, not revocation of authority. The
DESTROY right remains meaningful for its designed purpose (refcount > 0).

D68 states "the kernel does not autonomously destroy (D4)" in the context of
protecting cap-holders from kernel-initiated loss. At zero refcount, the
protection has no subject. D68 itself carves an exception at the chain terminus
(Case C: kernel destroys faulting Observer when no higher authority exists).
Auto-destroy at zero refcount is analogous — the chain of capability holders has
terminated.

### Backing return destination

Explicit Destroy returns structural backing as a Space cap to the destroyer
(D32/D98). On auto-destroy-via-close, the closer called Close, not Destroy —
they don't expect a returned Space cap and may lack a free slot. Returning to
the closer would change Close's semantics unpredictably.

Auto-destroy returns backing to the kernel's root Space (D31). This is
consistent with D33/D98's cascade-freed behavior: objects destroyed as side
effects (not as top-level targets) return backing to root Space. Pages re-enter
the system pool via the pager chain.

Users wanting direct resource reclamation call Destroy (hold the last cap, call
Destroy, receive the backing). Auto-destroy handles cleanup of objects nobody
claimed. The two operations serve different purposes.

### Close-path latency

Auto-destroying a non-Observer (Space, Time, Field, Pulsar) on last-close is
O(1) — no cascade. Auto-destroying an Observer on last-close triggers a
preemptible cascade (D33). The closer called Close (expected O(1)) but gets
cascade work.

This asymmetry is accepted for three reasons:

1. D33's preemptibility bounds per-step latency (~1-2µs per step). The closer
   yields between steps — other Observers run. The total work is O(N+M) but
   never monopolizes the core.

2. Under A4, someone must do the cleanup inline. There is no deferred-work
   mechanism and no better candidate than the current closer. The alternative
   (explicit-only) doesn't avoid the work — it avoids the work by leaking.

3. The scenario is rare and controllable. RT Observers don't manage other
   Observers' lifecycles. The closer being the sole holder of a large Observer
   requires an unusual authority pattern. When it matters, the user calls
   Destroy instead of Close.

### CloseResult does not report was_last_reference

The Close return value does not indicate whether auto-destroy fired. The closer
gave up their last cap — they can't interact with the object regardless. The
information is purely informational with no actionable consequence for the
caller. Keeping Close's return simple follows A5.

---

## Rejected alternatives

**Explicit Destroy only (Variant B).** Rejected because zero-refcount objects
are permanently leaked with no recovery path. D38 Time non-clonability makes
this a guaranteed outcome, not an edge case. Internal inconsistency with D33
(cascade auto-destroys but normal close does not). Pushes refcount-tracking and
Destroy-calling complexity to every userspace supervisor.

**Kernel orphan detection.** A background scan for unreachable objects. Rejected
by A4 — no background work. No natural trigger point for lazy detection exists.
The only A4-compliant orphan cleanup is inline auto-destroy on the close path.

**Type-specific auto-destroy (non-Observers only).** Auto-destroy non-Observer
types (O(1) destruction) but require explicit Destroy for Observers (cascade
concern). Rejected: breaks D8 uniformity (type-specific close semantics), adds
complexity (A5 tension), and the latency concern is addressed by D33
preemptibility.

**Return backing to closer.** Changes Close's return type unpredictably
(sometimes a Space cap, sometimes not). Closer may not have free slots. Rejected
in favor of root Space return, consistent with cascade-freed behavior.

---

## What this settles

- D11's deferred "memory reclamation of freed slots": settled as auto-destroy on
  zero refcount. Arena slots freed inline when the last reference is closed.
- The scope of D33's zero-refcount auto-destroy: extended from cascade-only to
  all close paths.
- Backing destination for auto-destroyed objects: root Space (D31), consistent
  with cascade-freed behavior.

## Does NOT settle

- **Cascade accounting for auto-destroy.** If close triggers a cascade, the
  closer's Time is charged. Whether this is acceptable or whether cascade work
  should be attributed differently is a scheduling-layer concern.
- **Close ordering within auto-destroy cascade.** Same as D33's deferred cap
  table close ordering.

---

## Status

**Settled.** Auto-destroy at zero refcount on all close paths.

Five convergent arguments:

1. Zero-refcount objects are permanently unreachable and unrecoverable (A4 — no
   sweeper)
2. D38 Time non-clonability makes the leak unavoidable under explicit-only
3. D24 and D33 establish precedent (kernel acts on last-reference events)
4. D4 authority semantics are preserved (DESTROY right meaningful for refcount >
   0; at zero refcount, no authority is violated)
5. Internal consistency: D33 cascade already auto-destroys at zero refcount;
   extending to all close paths eliminates an arbitrary scope boundary

Revisit if D11 is revised (changes the close/destroy base primitive), if D33 is
revised (changes cascade behavior), if a downstream derivation reveals a
legitimate use case for zero-refcount objects, or if close-path cascade latency
proves unacceptable for a workload class that cannot be addressed by using
Destroy instead.
