# One Logical Space Manager — 2026-04-15

Records the reasoning behind `spec.md#D3`.

## Starting point

The archived restart-1 chain committed to "shared global memory pool with a
single Space manager." When that decision was re-examined after the reset, its
justification turned out to rest on a load-bearing but unstated assumption:
"small cache-coherent SoC, 4–8 cores." Under fresh derivation against A1–A5
(which say nothing about core count or SoC scale), the question had to be
reopened.

## Exploration

### Two commitments conflated into one

The archive's phrasing — "shared global memory pool with a single Space manager"
— bundled two distinct commitments:

1. **Interface commitment.** The kernel exposes a single, logical interface for
   physical memory management. Other components of the kernel call one
   allocator, one accountant, one escalation target.
2. **Implementation commitment.** That interface is satisfied by a single global
   allocator operating on a single shared pool.

These are not the same thing. Commitment 1 is about structure; commitment 2 is
about strategy. Conflating them had a specific consequence under fresh
derivation: commitment 2 baked a topology assumption into the kernel's skeleton.

### Why the implementation commitment is wrong

A2 (ARM64) covers hardware ranging from small cache-coherent SoCs (Apple
silicon, 4–8 cores, uniform memory) to multi-socket NUMA servers (AmpereOne,
Graviton, 64+ cores, 1.5–3× memory latency deltas across nodes). On NUMA
hardware, a single global pool without topology awareness gives a Frame memory
from a region that may be remote to the core it runs on; the Frame pays that
latency cost on every memory access, not just at allocation time.

A5 (kernel is a leaf node) says topology-specific complexity belongs behind an
interface in a leaf, not in the kernel's skeleton. A skeleton commitment to
"single global pool" forecloses NUMA support — adding NUMA-awareness later would
require restructuring the kernel's memory management, not swapping a leaf. That
is precisely the A5 violation the principle is designed to prevent.

A3 (generic kernel) reinforces this: we cannot assume workload shape, and
therefore we cannot assume a regime in which single-pool contention and
NUMA-blindness don't bite. A workload shape we cannot exclude (many short-lived
Frames on a server-class chip) is one where single-pool commitment would be
pathological.

### Why the interface commitment is right

The interface commitment buys structural properties that the rest of the
derivation will rely on:

- **One source of truth for Space accounting.** Total memory in the system
  equals the sum of what the Space manager has allocated plus what it has free.
  Conservation is trivially statable.
- **Single escalation target for resource faults.** When a Frame needs more
  Space and faults upward to its handler, the terminal escalation in the fault
  chain is the Space manager. There is one place the kernel's resource-policy
  decisions live (noted in archive journal 013 as a convergence point).
- **One component to audit for memory-safety invariants.** The Space manager is
  where physical-page provenance and permissions are tracked. A single audit
  surface is smaller than N distributed audit surfaces.
- **Simple interface for other components.** Call sites do not have to reason
  about which allocator, which pool, which region — they call the interface.

These benefits are structural, not aesthetic. Several future derivations are
likely to cite D3 under "Rests on" precisely because of them.

### Applying "interfaces are the design, implementations are leaf nodes"

The Company OS claim "Settle the approach before choosing the technology —
interfaces are the design, implementations are leaf nodes that can be swapped"
does load-bearing work. The interface is the design. Whether the implementation
is a single global free-list, per-NUMA-node pools with cross-node fallback, or
per-core caches backed by a global reserve is downstream of the interface — a
leaf-node decision recorded where the implementation is chosen, not a
skeleton-level commitment.

This framing lets us decide the structural question now (one logical Space
manager) without deciding the scaling question yet (which internal strategy).
The latter will be decided when implementation proceeds and when a benchmark
shape is known.

### On the archive's "small SoC" framing

The archive's "small cache-coherent SoC" justification was implementation-level
reasoning masquerading as architectural reasoning. It was correct _about the
implementation it supported_ — a single global pool is genuinely fine on a small
cache-coherent SoC — but it was incorrect _as justification for the
architectural decision_, because the architectural decision should not depend on
hardware regime under A2's unqualified scope.

Replacing that framing with A3 + A5 + D1 as the load-bearing predecessors makes
D3 robust across A2's full range.

### D1's role

D1 (per-core hot path, shared cold path) is cited because allocation is
cold-path work. Space allocation happens at Frame creation and in response to
resource-escalation faults — not on the context-switch hot path. That placement
is what makes interface-level simplicity cheap: consumers of the Space manager
interface are not context-switch- frequency callers. The interface can be
crossed at cold-path latencies without affecting hot-path performance.

If a future workload profile shifts allocation to the hot path (for example,
zero-copy IPC patterns that frequently re-map pages as part of message
delivery), the D1-cold assumption under D3 would be violated and D3 must be
revisited. This is named in the spec entry's Status line.

## Status

**Accepted as `spec.md#D3` — settled, with revisit triggers.**

Settled because the reasoning is complete: the interface commitment is justified
independently of implementation choice, and the implementation choice is
explicitly deferred to leaf-node decisions. There is no hidden assumption.

Revisit triggers (named in the spec entry):

- A workload makes allocation hot-path, violating the D1-cold assumption on
  which D3 rests.
- The single-interface commitment itself starts costing — for example, if a
  kernel component finds it needs to bypass the Space manager and talk to memory
  hardware directly, the interface commitment leaks and must be re-examined.

**What D3 explicitly does NOT commit to:**

- Shared global allocator.
- Per-CPU caches.
- NUMA-partitioning.
- Any specific internal strategy.

Those are leaf-node decisions made at the point of implementation, recorded in
their own journal entries at that time.
