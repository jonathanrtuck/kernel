# 051 — Routing table structure: nullable pointer to external sorted array

**Date:** 2026-04-24

## Starting point

D45 settled Field split as badge-range routing with fallback-on-destroy but
explicitly left "routing table structure (kernel-internal)" as open. The current
Field struct (field.rs) has no routing table field. The question: where does the
routing table — the set of badge-range → destination-Field mappings — physically
live on a Field?

## Exploration

### D32's categorization gap

D32 distinguishes two memory sources for kernel objects: functional backing
(Space consumed at creation → becomes the object) and per-object kernel metadata
(from root Space, "bounded per object, fixed-size, and small"). The routing
table fits neither category. It's variable-size (grows with each split), doesn't
exist at creation time (the Field is created unsplit), and emerges from an
external trigger (the split operation). No existing kernel object has a
component that grows after creation.

This is a new accounting category: kernel-internal variable-size infrastructure.
Each split adds one entry (~40–48 bytes: badge range + destination ObjectId +
back-pointer linkage). Bounded per operation, small, invisible to userspace. The
IRQ→Field routing table from D22 is arguably the same category — a
kernel-internal variable-size mapping maintained for routing purposes.

Resolution: root Space (D31) pays. D32's spirit holds — the cost is invisible to
userspace, small per operation, and predictable. "Bounded per object" extends to
"bounded per operation" without architectural disruption.

### Bidirectional linkage is forced

D45 says routing rules hold kernel-internal references to destinations,
contributing to the destination's refcount. When a destination Field is
destroyed, all source Fields' routing rules pointing to it must be cleaned up
(the rule removed, traffic falls back). The kernel must find those source
Fields.

Global scan is O(total Fields) and foreclosed by D34's O(1)-per-source cleanup
promise. Therefore the destination must hold back-pointers to its sources. The
established pattern is an intrusive list (paralleling the waiters list). Each
routing entry on a source Field is simultaneously a node in the destination's
back-pointer list.

This means the Field struct gains two optional structures: a forward routing
table (on source Fields) and a back-pointer list head (on destination Fields). A
Field can be both simultaneously (a mid-level Field in a routing hierarchy).

### Three options evaluated

**Option A — Nullable pointer to external sorted array.** The Field struct gains
one nullable pointer in the hot partition. Null when unsplit (zero cost: one
load + branch, in the cache line already touched for waiters/queue_len). On
first split, the kernel allocates a sorted array from root Space. Growth via
geometric doubling on the split path (cold). One code path for lookup. One
invariant (sorted, non-overlapping array).

**Option B — Small inline array + overflow pointer.** The Field struct holds 2–4
routing entries inline plus an overflow pointer. Saves ~10 cycles (one pointer
dereference) for Fields with few splits. Costs: 64–192 bytes added to every
Field in the arena (most never split), two code paths for lookup, inline count
is a global arena-slot-size commitment. The inline→overflow transition is an
additional correctness concern with a rarely-exercised code path.

**Option C — Routing entries in queue pages (repurposed functional backing).**
No new allocation — routing entries occupy message slots, shrinking queue
capacity. Costs: routing capacity coupled to queue capacity (a heavily-split
Field has fewer message slots), queue pages' typed access pattern
(`page.as_ref::<MessageSlots>()`) broken by dual-typed pages, and a liveness
problem — a full queue blocks routing reconfiguration because adding a routing
entry would shrink capacity below current queue_len.

### Why Option A

Option A is the clear choice on all three evaluation dimensions:

**Performance.** The ~10-cycle dereference cost on split Fields is well within
D45's budget (10–20 cycles for binary search on 5 splits, out of ~400 total).
Any Field receiving enough messages for 10 cycles to matter has a warm cache —
the dereference hits L1. Unsplit Fields pay nothing (null check in the existing
hot cache line). B saves those ~10 cycles for the inline case but pays 64–192
bytes on every Field. C avoids the dereference entirely but adds conditional
offset logic to every enqueue/dequeue — a cost paid by all Fields, not just
split ones.

**Correctness.** One data structure (sorted array behind Option), one code path,
one invariant. B has three states (empty, inline-only, overflowed) with a
transition that must preserve the sorted invariant across a copy. C breaks type
safety on queue pages and introduces a variable-capacity circular buffer. A is
the simplest to verify, and simplicity matters for a Verus-targeted codebase.

**Scalability.** A scales to any number of splits, bounded only by root Space. B
scales identically beyond overflow but wastes inline space. C has a hard ceiling
(routing entries compete with message slots in fixed queue pages) and cannot
independently scale routing and queue capacity.

### What was rejected

**Option B** rejected for paying arena-bloat cost on all Fields to save ~10
cycles on a minority. The inline count is a global commitment — wrong by
construction since the right value depends on workload and can't be known ahead
of time.

**Option C** rejected for the liveness coupling (full queue blocks split), type
safety violation (dual-typed pages), and inability to independently scale
routing vs. queue capacity.

## The decision

**D54 — Routing table structure: nullable pointer to external sorted array.**

The Field struct gains a nullable pointer to an externally-allocated sorted
array of routing entries. Null when unsplit — zero hot-path cost. On first
split, the kernel allocates the array from root Space (D31). Each routing entry
holds: badge range condition, destination Field ObjectId, and intrusive-list
linkage for the destination's back-pointer cleanup list.

Destination Fields gain a back-pointer list head (intrusive list paralleling
waiters). When a destination is destroyed, the kernel walks its back-pointer
list and removes each corresponding routing entry from the source Field's table.

Growth via geometric doubling (amortized O(1) per split). The array is
contiguous for binary search cache-friendliness. Resize happens on the split
path (cold).

The routing table pointer lives in the Field struct's hot partition — the null
check shares the cache line already loaded for `waiters` and `queue_len`.

## What remains open (one level down)

- Exact routing entry layout (field widths, alignment, intrusive-list node
  placement)
- Initial array capacity on first split (1? 4? workload-dependent?)
- Slab allocator vs. general sub-page allocator for routing arrays
- Flattened routing table (D24-parallel optimization) structure and update
  protocol — likely a separate external allocation per root Field
- Whether D32's vocabulary should be updated to name the "kernel-internal
  variable-size infrastructure" category explicitly (the IRQ→Field table and the
  routing table are both instances)

## Status

**Settled.** The routing table is a nullable pointer to an external sorted
array, allocated from root Space.

Revisit if D45 is revised (changes the routing mechanism), if D32 is revised
(changes the memory accounting model), if D1 is revised (changes the hot-path
constraint), or if profiling reveals the pointer dereference cost is
unacceptable for a structurally required workload pattern (would motivate Option
B's inline approach).
