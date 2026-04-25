# Journal 069 — Sub-page packing: slab allocator with page return

**Date:** 2026-04-24 **Settles:** D70 — Arena internal structure: per-type slab
with page return

---

## Question

How should the kernel allocate and reclaim physical memory for sub-page-sized
kernel objects (Observer metadata, Field structs, Pulsar structs, capability
entries) given D32's fixed-size-per-type mandate and D53's per-type arena model?

This question applies to kernel-internal allocations from root Space (D32
category 2: per-object metadata). It does NOT apply to userspace Space objects
(D24/D25 settle those at page granularity) or to variable-size auxiliaries (D54
routing arrays, D43 multi-field WaitEntries — separate sub-problems).

---

## Clarification: D24 does not constrain this

The sub-page packing concern raised in journals 025 and 026 was about _userspace
Space objects_ sharing hardware pages — a scenario where the cap-mapping
invariant (D24) creates cleanup complications. Kernel-internal structs are never
mapped into any Observer's address space. D24's auto-unmap does not apply to
them. D24 itself confirms this: "Does NOT settle: sub-page packing strategy
(kernel-internal implementation concern)."

The question here is purely about the internal structure of D53's `Arena<T>`.

---

## Foreclosed options

### Copy-on-compact

A strategy that moves live objects to consolidate partially-empty pages is ruled
out by four independent constraints:

1. **A4 (purely reactive):** No background compaction. Synchronous compaction
   during a syscall requires updating all capability pointers to moved objects —
   O(N) in capability holders, unbounded.
2. **D33 (preemptible cascade):** During preemption between cascade steps,
   another core may read from a live object being relocated. Stopping all
   readers requires a stop-the-world pause.
3. **D4 (pointer = capability):** Every outstanding capability is a pointer into
   the arena. Moving an object without updating all capabilities creates
   dangling pointers. Global cap-table scan is O(total capabilities
   system-wide).
4. **SMP concurrency:** Compaction under concurrent access requires
   synchronization that contradicts the kernel's responsiveness model.

Object addresses are stable for their entire lifetime.

### Buddy allocator for fixed-size types

D32 mandates fixed-size per object type. A buddy allocator for fixed-size
requests degenerates to a power-of-two freelist with roundup waste (96-byte
Observer → 128-byte buddy block, wasting 32 bytes per object). The splitting and
coalescing machinery provides no benefit over a simple freelist. Buddy remains
viable for variable-size auxiliaries (D54 routing arrays).

---

## Viable options after foreclosures

### Option A: One object per page

Each kernel struct gets its own hardware page. A 96-byte Observer occupies a 4
KB page, wasting 3,998 bytes (97.6%). On 16 KB pages: 99.4% waste.

**Strengths:** Zero coordination on free (destroy → return page). Minimal unsafe
surface. Simplest to verify formally (seL4 chose this for provability).

**Weaknesses:** Memory overhead is catastrophic for any meaningful object
population. 1,000 live Observers = 4 MB of kernel metadata overhead on 4 KB
pages (vs. ~96 KB of actual data). Objects scattered one-per-page, guaranteeing
a TLB miss per object on sequential access. ARM64 L1 dTLB is typically 64
entries — a 1,000-object scan causes 1,000 TLB misses. Root Space consumption is
dominated by waste, limiting total live objects under memory pressure.

**Prior art:** seL4 (deliberately, for formal proof; not for production
efficiency). No non-verification kernel uses this intentionally.

**Rejected** because A3 (generic kernel, including long-lived servers) cannot
absorb the memory cost, and D1's hot-path cache behavior is actively harmed by
per-page scattering.

### Option B: Slab allocator with page return (CHOSEN)

Each per-type arena is a slab: pages of N fixed-size slots. Free slots form an
intrusive freelist. When all N slots on a page are free, the page returns to the
root Space pool.

**Strengths:**

- Dense packing: 96-byte Observer on 4 KB page → 42 objects per page, ~97.5
  bytes overhead per object (alignment only). 42× more memory-efficient than
  one-per-page.
- Page return: memory usage proportional to steady-state live object count, not
  peak. Pages cycle back as object populations shrink. Critical for A3's
  long-lived server workloads.
- Cache locality: same-type objects adjacent in memory. Sequential access
  (scheduler scans under D56, destroy cascades under D33) benefits maximally.
  TLB working set proportional to live_count / objects_per_page.
- Per-type lock (D53): each arena's slab has one SpinLock. No cross-arena
  coordination on alloc/free.
- Cold-path allocation (D1): slab setup cost (page mapping, freelist
  initialization) is amortized across many objects and only fires on infrequent
  operations.

**Weaknesses:**

- Partial-page retention during destroy cascade: a slab page cannot return until
  all its slots are free. During a cascade that destroys 42 Observers across
  preemption points, the page is held until the last slot is freed. In practice
  this is benign — freed slots are immediately available for reuse, and cascade
  frees are sequential (no new allocations during teardown).
- Unsafe implementation: intrusive freelist requires `MaybeUninit<T>` and raw
  pointer arithmetic. This unsafe lives in the framekernel core (journal 023).
  The pattern is well-understood and battle-tested across Linux SLUB, Zircon,
  and QNX.
- Page-size sensitivity: on 16 KB pages, 170 Observer slots per page → page
  return requires 170 successive frees. The slab reads page size at boot (D25
  query) and configures accordingly.

**Prior art:** Zircon (per-type dispatcher slabs), Linux SLUB (per-type
`kmem_cache`), QNX Neutrino (per-type fixed-size pools). The dominant strategy
in production microkernels with kernel-managed object allocation.

### Option C: Per-type arena, grows-never-shrinks

Same dense packing as the slab, but pages are never returned to root Space.
Memory footprint bounded by peak allocation, not steady-state.

**Strengths:** No page-return coordination. Simpler than slab (no per-page
occupancy counter). Fewer invariants to verify.

**Weaknesses:** Memory stranding. After a peak of 10,000 Observers, the arena
retains ceil(10,000 / 42) = 239 pages (~956 KB) permanently, even if only 10
Observers remain. Under A3's generic kernel goal, this is incorrect resource
behavior for server-class workloads. Violates the spirit of D32's "bounded per
live object" intent.

**Prior art:** Genode kernel (deliberate simplicity for bounded component
deployments). Not used in general-purpose microkernels.

**Rejected** because A3 requires correct behavior across workload types,
including long-lived servers where peak ≠ steady-state.

---

## Decision: Option B — slab allocator with page return

The slab wins on both performance (cache locality, TLB pressure, memory
efficiency) and behavioral correctness under A3 (steady-state-proportional
memory). The only axis where it loses to Option A (verification simplicity) or
Option C (implementation simplicity) is secondary given that:

1. D1's cold-path allocation means the slab's extra invariants only fire on
   infrequent operations.
2. The slab pattern is thoroughly battle-tested (Linux since 2.6.23, Zircon,
   QNX).
3. The unsafe lives entirely in the framekernel core (journal 023), contained
   behind the `Arena<T>` interface.

---

## What this does NOT settle

- **Variable-size auxiliary allocation** (D54 routing arrays, D43 multi-field
  WaitEntries). These are not fixed-size and do not fit the per-type slab model.
  Whether they use power-of-two size classes, direct page allocation, or
  co-location with their owning object is a separate design question.
- **Per-core arena sharding** (D53's flagged future SMP optimization). The slab
  model is compatible with per-core magazines but does not require them. Under
  D1 (cold-path allocation), global per-type locks are acceptable.
- **Object zeroing policy.** When a slab slot is reused, whether old bytes are
  zeroed before construction. A kernel security property that must be addressed
  during implementation.
- **Root Space pool recycling behavior.** The slab returns pages to root Space,
  but whether root Space itself recycles those pages depends on D3's internal
  implementation (explicitly a leaf-node concern by D3).
