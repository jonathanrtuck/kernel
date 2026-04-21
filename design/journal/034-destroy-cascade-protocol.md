# Destroy Cascade Protocol — 2026-04-20

Thirty-fourth exploration. Derived the protocol for object destruction: cascade
ordering, freed backing destination, preemptibility, and destroy authority.

## Starting point

D11 settled the base revocation primitive (close-only + authoritative destroy)
but deferred the cleanup protocol (inline vs. preemptible), who authorizes
destroy, and cross-core prompt-effect policy. D17 noted tension T5 (badge-
closure checks during Observer destroy). D32 settled type-conversion destroy
(object → Space cap to destroyer) but deferred held caps on Observer destroy.

## Derivation

### Concrete scenario: Observer destroy

When `destroy_observer(handle)` is invoked:

1. Kernel validates the cap (D8 rights mask — "destroy" right required).
2. Object marked destroyed. Outstanding caps become dead handles (D11). This
   happens FIRST — the object is dead before cleanup begins.
3. Observer's structural backing returned as Space cap to caller (D32).
4. Observer's cap table iterated. For each held cap: a. Close (D11 — decrement
   refcount). b. Badge-closure check on tracked endpoints (D17). If queue full
   (D18): drop. c. If refcount reaches zero: the referenced object is destroyed
   too (cascade). Backing → kernel root Space (no caller for cascading destroy).
   If the cascaded object is an Observer, recurse into its cap table.
5. Per-Space cleanup: last holder triggers subtree detach (D26). Last
   system-wide holder frees subtree pages (back into the Space, per D32).
6. Per-Time cleanup: scheduling capacity returns to kernel pool (D32).
7. Pending fault list: remove destroyed Observer from any pending lists (D18).
   Wake Observers pending on destroyed endpoints with error.

### Key structural properties

**Only Observers cascade.** Spaces, Endpoints, and Times don't hold caps.
Destroying an Endpoint frees its backing but doesn't recurse. Cascade depth =
length of exclusively-held Observer chains. Total cascade work = O(total objects
reachable through exclusive references).

**Single Observer destroy is O(N + M).** N = cap table entries (close each). M =
badge-closure checks (tracked endpoints). Both bounded by Observer's Space
budget (D32 — table size bounded by backing Space).

**Object is dead before cleanup begins.** D11 says dead handles are created at
destroy time. The cascade is cleanup of an already-dead object. No partially-
alive state is externally visible. Other Observers see dead handles immediately.

### Preemptible cascade (settled)

The cascade is processed in bounded steps. Between steps, the timer interrupt
can preempt and the scheduler can run higher-priority Observers. The kernel
saves cascade continuation state: current position in cap table iteration, stack
of cascading objects (each entry: Observer being cleaned + cap table index).

Arguments for preemptible over inline:

- A3 (generic): workloads include real-time Observers that need bounded
  preemption latency. Inline destroy blocks the core for the entire cascade
  duration — not RT-compatible.
- D1 (cold-path): destroy is cold-path, meaning it doesn't affect hot-path
  latency. But "cold-path" means "infrequent," not "arbitrarily long is
  acceptable." A core running inline destroy is unavailable for hot-path work.
- seL4 MCS precedent: preemption within revocation traversal, resumable on
  kernel re-entry. Demonstrates feasibility.
- D-1 (object already dead): no consistency concern from pausing mid-cascade.
  The object is dead; the cascade is just cleanup. Preemption doesn't create
  observable intermediate states.

Inline is a special case of preemptible (infinite step size). Preemptible
forecloses nothing; inline forecloses bounded destroy time.

Continuation state: a small stack (depth = cascade depth, bounded by Observer
chains). Each entry is ~16 bytes (pointer + index). Kernel-internal, per-core.
A5 tension accepted: the kernel absorbs this complexity for RT-compatibility.

### Structural backing only returned to destroyer (settled)

The top-level destroy returns one Space cap: the destroyed object's structural
backing. Cascade-freed backing (refcount-zero objects destroyed during cascade)
goes to the kernel's root Space. Pages re-enter circulation through the pager
chain (D31).

Three arguments against returning all cascade-freed backing to the destroyer:

1. **Shared resources break the return model.** If the destroyed Observer shared
   Spaces with other Observers, those Spaces aren't destroyed (refcount > 0).
   Only when the LAST holder's cap is closed does the Space die — and that last
   holder's supervisor might be unrelated to the original allocator. The return
   goes to an arbitrary supervisor, not the delegator.

2. **Internal reorganization makes returns unpredictable.** The destroyed
   Observer may have split, combined, and received Spaces from multiple sources.
   The cascade-freed backing is a mix of pages from different origins. The
   destroyer can't predict the returned size or content.

3. **Predictable return value.** Structural backing is fixed-size and
   predictable. The destroyer knows what it gets: the pages it consumed to
   create the Observer. Cascade-freed is variable and depends on internal state.

Supervisors wanting to recover resources before destroy can inspect the child's
cap table and pull specific caps (Observer rights model, D14 downstream — an
"extract" operation). This makes selective recovery proactive rather than
depending on cascade behavior.

### Destroy right in the rights mask (settled)

D4 requires capability-mediated authority for destroy. A "destroy" right bit in
D8's per-cap rights mask. Same pattern as send/receive/mint (D17). The creator
controls who can trigger cascades by attenuating caps to omit destroy.

### Badge-closure is best-effort during cascade (derived)

D18 applies unchanged. Badge-closure notifications on tracked endpoints are
enqueued if space permits, dropped if queue full. No special cascade logic.

### Pending fault list cleanup (derived)

D18's intrusive pending list: destroyed Observer unlinked O(1). Destroyed
endpoint's pending Observers woken with error, O(pending count).

## Archive convergence

**Partial convergence.** Archive claims.toml: "When an Object is destroyed, its
entire subtree is destroyed and its resources are reclaimed." — Converges on
cascade through owned resources.

**Divergence on return destination.** Archive claims.toml: "When an Object is
destroyed, its bound resources return to its supervisor. Resources do not
disappear — they flow up the ownership tree." The archive returned resources to
the supervisor (2B-like). This derivation returns structural backing to
destroyer, cascade-freed to root Space (2A). Divergence explained by structural
difference: the archive built on a supervision/ownership tree (resources flow up
the tree). This design uses flat caps + refcounting + pager chains (D6 no kernel
grouping, D27 flat cardinality). Shared resources under flat caps have no
"lowest common ancestor" to return to — root Space is the neutral destination.

**Archive does not discuss preemptibility.** No convergence data point for
inline vs. preemptible.

## What remains open

- **Cap table close ordering.** Whether ordering within the table matters (e.g.,
  close backed objects before their backing Space). The kernel can handle
  encounters with already-freed backing during cascade — dead handles (D11).
- **Cross-core prompt-effect policy.** Whether destroy is strongly observed
  (IPI-bounded) or weakly observed (eventual). D11 deferred this. May vary by
  object type.
- **TLB shootdown batching during cascade.** Optimization — accumulate and batch
  TLBI operations rather than one-at-a-time. Deferred.
- **Observer rights model: "extract" operation.** Ability for a supervisor to
  pull caps from a child's table before destroy. Part of the Observer rights
  model (D14 downstream).
- **Continuation state cleanup on core migration.** If the core running a
  preemptible cascade needs to migrate the work (e.g., core shutdown), the
  continuation must transfer. Likely a non-issue (the cascade is tied to the
  destroy syscall's calling context).
