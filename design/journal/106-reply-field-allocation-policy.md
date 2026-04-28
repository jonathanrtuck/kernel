# 106 — Reply Field allocation policy: userspace-created, SetReplyField operation

**Date:** 2026-04-28. Starting from the explicitly deferred open question in
D16: "reply field allocation policy (pre-allocated at creation vs. lazy)." Also
deferred by D36 and D73.

---

## The question

When and how does the reply Field — the Field at reserved cap-table slot 1
(D43/D57) that enables RPC via Call() — get created and installed?

D16 settled the mechanism (pre-allocated reply field with send-once cap). D43
settled the location (reserved cap-table slot 1). D57 enumerated the reserved
slots. All three deferred the allocation timing.

D95's journal text stated "the kernel creates the reply Field on first Call" in
its CreateObserver protocol (step 6), but D16, D36, and D73's spec entries
continued to list the timing as unsettled. This exploration reconciles the
inconsistency.

---

## Options considered

### Option A: Kernel creates at Observer creation (root pool)

Kernel allocates a reply Field (arena + queue, capacity 1, badge_tracking=true)
during CreateObserver, from the root pool. Every Observer gets one.

- No new kernel surface. Call() always works.
- Wastes one Field per non-RPC Observer (~128 bytes queue + arena slot).
- D32 tension: kernel creates a full functional object (identity, generation,
  refcount, badge map) from root pool without userspace-supplied Space. Extends
  D32's metadata exception to functional objects. Sets precedent.

### Option B: Kernel creates lazily on first Call() (root pool)

Slot 1 starts empty. First Call() detects, allocates, installs, then proceeds.

- No waste for non-RPC Observers.
- First Call() gains allocation failure mode. Conditional on every Call().
- Same D32 tension as Option A.
- D95's journal specified this approach.

### Option C: Userspace creates, new operation installs at slot 1

Creator calls CreateField(space_cap), then a new SetReplyField operation
installs it at slot 1. The creator supplies the Space.

- Full D32 compliance: all functional objects funded by userspace Space.
- D35-aligned composable setup.
- Requires new kernel surface (SetReplyField typed operation).
- Page-granularity waste: minimum Space is one page (4KB), reply queue needs
  ~128 bytes. ~3.9KB wasted per reply Field.

---

## Why Option C

Three convergent arguments:

**1. D32 purity.** D32's type conversion model is foundational: userspace
supplies Space, kernel produces objects. The root pool exception is for "per-
object kernel metadata — bounded per object, small" (queue headers, scheduling
aggregates, tracking structs). A reply Field is not metadata — it is a full
kernel object with independent identity, generation counter, refcount, and badge
map. Extending the root pool exception from metadata to functional objects
erodes D32's boundary. The reply Field is bounded and predictable, but so could
be any future per-Observer object — the precedent matters more than the cost.

**2. Reserved slot consistency.** Slot 0 (fault handler) is userspace-supplied:
the creator chooses which Field receives faults — a policy decision passed as a
CreateObserver parameter. Slot 2 (self-cap) is kernel-supplied: structurally
determined, one correct value. The reply Field is structurally parallel to the
fault handler: both are per-Observer Field references at reserved slots serving
specific purposes. The choice of which Field serves as reply channel is a
creator concern, just as the choice of fault handler is. Having both Field-
referencing reserved slots (0 and 1) follow the same provisioning model —
userspace-supplied — is a uniform pattern.

**3. Explicit resource accounting.** The capability model's core property is
that all resources are visible and accountable. Kernel-created reply Fields from
root pool are invisible to userspace — the creator cannot see the cost, cannot
predict root pool exhaustion, cannot choose the queue capacity. Userspace-
created reply Fields make the cost explicit: the creator supplies the Space,
controls the capacity, and accounts for the resource.

---

## Rejected alternatives

| Alternative                     | Rejected because                                                                                                                                                  |
| ------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Option A (eager, root pool)     | Erodes D32 boundary (functional object from root pool); every Observer pays cost even if non-RPC                                                                  |
| Option B (lazy on Call)         | Same D32 erosion; first Call gains allocation failure mode; Call() semantically does two things                                                                   |
| Mandatory CreateObserver param  | Not all Observers Call(); forces non-RPC Observers to create a throwaway Field (A3 tension); page-granularity waste forced                                        |
| Explicit-slot install (general) | Reserved slots have per-slot semantics (slot 0: SEND, slot 1: RECEIVE, slot 2: kernel-only); general mechanism needs per-slot validation for a three-slot problem |

---

## The decision

**Userspace-created reply Field, installed via SetReplyField typed operation.**

The Observer creator creates a Field via CreateField(space_cap), then calls
SetReplyField(observer_cap, field_cap) to install it at cap-table slot 1. The
kernel:

1. Validates the observer_cap (requires INSTALL_CAP right, D39).
2. Validates the field_cap (must be a Field, valid generation).
3. If slot 1 already has a Field, closes the old entry (D11 close semantics,
   including badge-closure if tracked).
4. Writes the Field cap at slot 1 with RECEIVE rights.
5. Auto-enables `badge_tracking = true` on the Field (D73: unconditional for
   reply Fields; A5: kernel absorbs this complexity).
6. Increments badge map refcount for the installed badge (D17).

Call() gains a new check: if slot 1 is empty, returns an error (new SyscallError
variant) instead of silently proceeding with no reply cap. Without this check,
Call() with empty slot 1 zombifies the caller — blocked on nothing, no
badge-closure notification, permanently stuck.

SetReplyField is a new TypedOperation (code 20), gated by the existing
INSTALL_CAP right. No new Observer right is needed.

---

## Costs

- **Page-granularity waste.** Minimum Space is one page (4KB). Reply queue needs
  capacity 1 (~128 bytes for one Message). ~3.9KB wasted per reply Field.
  Mitigation path: sub-page Field allocation or shared-Space reply Fields are
  additive optimizations that do not require changing the allocation model.
  Named for future work, not blocking.
- **New kernel surface.** One TypedOperation (SetReplyField). Narrow, single-
  purpose. Same pattern as ObserverChangeHandler (overwrites a reserved slot).
- **Creator ceremony.** Every Observer creator that wants RPC must call
  CreateField + SetReplyField. A userspace library can wrap CreateObserver +
  SetReplyField into a single "create RPC-capable Observer" function.

---

## What this does NOT settle

- **Sub-page Field allocation.** Optimizing the page-granularity waste for small
  Fields (reply Fields, small notification Fields). Additive, deferred.
- **Call() error variant naming.** The specific SyscallError variant for "slot 1
  empty" (e.g., NoReplyField, MissingReplyField). Implementation detail.

---

## Spec inconsistency resolved

D95's journal text (step 6 of CreateObserver protocol) stated "the kernel
creates the reply Field on first Call." This was never reflected in spec.md —
D16, D36, and D73 all continued to list allocation timing as unsettled. D106
settles it differently: userspace-created, not kernel-created. D95's spec entry
is updated to read "empty initially (populated by SetReplyField, D106)." The
journal text in 095 is not rewritten (exploration history preserved), but a note
at the top points to D106.

---

## Archive convergence

The archive (journal/011) used client-created reply fields with explicit send
cap transfer. Option C is closer to the archive's approach than D95's lazy-
kernel-creation was — the client (or its creator) creates the reply Field
explicitly. The difference: D16's send-once cap mechanism (archive lacked
send-once), and D106's kernel-managed slot installation (archive used general
cap transfer).

---

## Axioms

**A1 (Rust):** Not directly load-bearing. The SetReplyField operation is a
standard typed syscall; Rust's type system does not constrain the allocation
model.

**A2 (ARM64):** Not load-bearing. The allocation policy is architecture-
independent.

**A3 (generic):** Load-bearing. A3 means no workload assumptions — not all
Observers do RPC. The optional SetReplyField operation (vs. mandatory at
creation) respects this: non-RPC Observers pay nothing.

**A4 (purely reactive):** Indirectly load-bearing. A4 means the Call() error
check is critical — without a reply Field, the caller blocks on nothing and has
no reactive signal. The error check prevents permanent zombification.

**A5 (kernel absorbs complexity):** Load-bearing in one specific place:
SetReplyField auto-enables badge_tracking (D73). The creator should not need to
know this implementation detail.

---

## Status

**Settled.**
