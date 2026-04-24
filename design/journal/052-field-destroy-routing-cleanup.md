# 052 — Field destroy routing-cleanup protocol

**Date:** 2026-04-24

## Starting point

D54 settled the routing table mechanism: back-pointer intrusive list on
destination Fields, walked on destroy, O(1) per source. D45 settled the
semantic: routing rule removed, traffic falls back to source queue. D33 settled
the destroy cascade protocol for Observers (preemptible, 7-step sequence) and
noted "Only Observers cascade. Destroying a Field frees its backing but doesn't
recurse."

Three questions remained open:

1. **Preemptibility.** The back-pointer walk is O(K) where K = sources routing
   to this destination. Is it inline or preemptible?
2. **Stale-rule handling.** D11 requires the object to be dead before cleanup
   begins. Between dead-marking and routing cleanup, source Fields have routing
   rules pointing to a dead destination. What happens to sends that hit these
   stale rules?
3. **Cross-core synchronization.** A source Field's routing table is hot-path
   data (D50: evaluated on every send). When the destroying core needs to remove
   a routing entry from a source on another core, how is the modification
   sequenced with respect to concurrent sends?

## Exploration

### Choice 1: Preemptibility

D33 committed to preemptible Observer cascade with an argument that is
structural, not Observer-specific: "inline forecloses bounded destroy time;
preemptible forecloses nothing." The A3 (RT bounded latency) and A4 (reactive,
no background cleanup) arguments apply identically to Field routing cleanup —
O(K) removals during a single syscall is the same structural problem as O(N)
cap-table closes during Observer destroy.

Using inline for Field routing while using preemptible for Observer cascade
would be an inconsistency: the same problem with different treatment. D33's
continuation framework (position + stack) already exists; extending it to track
back-pointer walk position is incremental.

**Inline (1A) rejected:** forecloses bounded destroy time for large K.
Inconsistent with D33's structural argument.

**Hybrid (1C) rejected:** two code paths; threshold is a global commitment with
no empirical basis. Inline is a special case of preemptible (D33's own
observation) — the "hybrid" is just preemptible with an optimization.

### Choice 2: Stale-rule handling during the D11 window

The D11 dead-before-cleanup guarantee creates a window where source Fields have
routing rules pointing to a dead destination. This is the D11 dead-handle
pattern applied to kernel-internal references rather than userspace
capabilities.

D11 already uses generational slot tags for ABA prevention on capability slots.
The same pattern extends to routing entries: each routing entry stores the
destination's generation alongside its ObjectId. On routing evaluation, the
kernel compares the entry's stored generation against the destination object's
current generation. A mismatch means the destination has been destroyed (or
reused) — the entry is treated as absent, and the send falls back to the source
queue.

This is "find the abstraction that absorbs the edge cases" — the D11 dead-handle
protocol extended uniformly. Dead cap → error on use. Dead routing destination →
treated as absent on evaluation. Same mechanism.

Performance: the generation comparison happens only for the matching routing
entry (after binary search on badge range). The generation field is in the same
cache line as the badge range condition. The branch (destination alive) is
always taken in normal operation — the predictor learns it instantly.
Effectively free on the hot path.

**Liveness check via pointer dereference (2A) rejected:** requires chasing a
pointer to check liveness on every routed send — a cache miss in the worst case.
The generation check is a local comparison in the same cache line.

**Fail the send (2B) rejected:** no precedent for transient userspace-visible
errors during kernel-internal cleanup. D18's error-on-full is about queue
capacity, not destination liveness. A new error class for a transient internal
state is philosophically wrong — the kernel's internal cleanup race should be
invisible to userspace.

**Atomic same-core (2C) rejected:** partial — solves same-core but not
cross-core. A partial solution that requires a second mechanism for the
remaining case is not "find the abstraction that absorbs the edge cases."

### Choice 3: Cross-core source modification

D1 says no shared mutable state on the hot path. O2 says cross-core coordination
requires IPIs. The source Field's routing table is evaluated on every send (D50)
— it is hot-path data. The destroying core must not directly modify another
core's hot-path data.

IPI-requested removal: the destroying core sends an IPI to the source Field's
core. The IPI handler removes the routing entry from the source's routing table.
The removal happens in the source core's own execution context — no shared
mutable state, no lock on the send hot path, no concurrent modification.

The philosophy's "react to reality, don't poll for it" reinforces this: IPI is
reactive (the destroy event triggers cleanup on the target core).
Deferred-on-send (3C) is effectively polling (every send checks whether
something changed).

During the IPI delay (the target core may be mid-syscall; the IPI is handled at
the next exception return), sends to the source Field may hit the stale routing
entry. Choice 2D (generation check) handles this: the stale entry is detected
and treated as absent. The IPI eventually removes it, restoring the array to its
clean state.

**Direct modification with lock (3A) rejected:** adds a lock to the hot path of
every routed send. Even uncontended, atomic operations cost ~10–20 cycles. D1
violation.

**Deferred on send (3C) noted as a verification stepping stone:** per-core,
synchronous, no cross-core reasoning — easier to verify in Verus. A pragmatic
first implementation could use 3C, upgrading to 3B once the IPI framework is
verified. The generation check (2D) is the detection mechanism in both cases;
the difference is who removes the stale entry (send path in 3C, IPI handler in
3B).

**Per-core routing tables (3D) rejected:** structural change beyond D54's scope.
Would require routing table duplication and a new consistency model.

## The decision

**D55 — Field destroy routing-cleanup protocol: preemptible walk, generation
check, IPI-requested removal.**

When a destination Field is destroyed:

1. The Field is marked dead (D11). Outstanding caps become dead handles. The
   routing entries on source Fields are not yet modified — they now point to a
   dead destination.

2. The kernel begins walking the destination's back-pointer list (D54). For each
   back-pointer entry:

   a. If the source Field is on the same core: remove the routing entry from the
   source's sorted array inline. The sorted array is not concurrently accessed
   (same core, within syscall context, no preemption between read and write).

   b. If the source Field is on a different core: send an IPI to the source's
   core requesting removal of the specific routing entry. The IPI handler on the
   target core performs the removal.

3. The walk is preemptible (D33 pattern). Between bounded steps, the timer can
   preempt. Continuation state: position in the back-pointer list, plus any
   pending cross-core IPIs. On resume, the walk continues from the saved
   position.

4. During the window between dead-marking (step 1) and routing entry removal
   (steps 2a/2b), sends to source Fields evaluate routing normally. If a send
   matches a stale routing entry, the generation check detects the mismatch —
   the entry is treated as absent and the send falls back to the source's queue
   (D45 fallback). The stale entry is not removed by the send; it is removed by
   the ongoing cleanup walk or IPI handler.

5. After all back-pointer entries are processed and all cross-core IPIs are
   acknowledged, the destination Field's backing is returned to the caller
   (D32).

The generation field in each routing entry is the destination's ObjectId
generation at the time the routing rule was installed. This extends D11's ABA
tag pattern from userspace capability slots to kernel-internal routing
references.

## What remains open (one level down)

- **Generation field placement in routing entry layout.** D54 leaves the exact
  entry layout unsettled. The generation field must be co-located with the badge
  range condition for cache-line locality on the evaluation path.
- **IPI batching.** When a destination has multiple cross-core sources on the
  same remote core, a single IPI with a batch of removals is more efficient than
  one IPI per entry. Protocol details deferred.
- **IPI acknowledgment protocol.** Step 5 requires knowing when all IPIs have
  been handled. Mechanism deferred (could be per-IPI ack, could be a barrier).
- **Flattened routing table invalidation.** D45/D54 leave flattened tables
  unsettled. If the kernel flattens routing chains (D24-parallel optimization),
  destroying a mid-level destination requires invalidating ancestor flattened
  entries. The back-pointer list connects only direct sources. Flattened table
  invalidation is a separate concern, gated on flattened tables being adopted.
- **Continuation state layout.** Extends D33's per-core continuation state.
  Details depend on the final D33 continuation structure.

## Archive convergence

The archive does not contain a concept of badge-range routing or routing
cleanup. No convergence data point.

## Status

**Settled.** Preemptible back-pointer walk with generation-checked stale-rule
handling and IPI-requested cross-core removal.

Revisit if D54 is revised (changes the back-pointer mechanism), if D33 is
revised (changes the preemptibility framework), if D11 is revised (changes the
dead-before-cleanup guarantee or ABA tag pattern), if O2 is revised (changes the
cross-core coordination mechanism), or if Verus verification reveals that the
IPI protocol is infeasible to specify (would motivate 3C deferred-on-send as the
permanent solution).
