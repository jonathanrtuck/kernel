# Capability Table Structure — 2026-04-16

Eighth exploration. How should the kernel organize and resolve capabilities
per-Frame? The table that maps opaque integer handles to (kernel object, rights
mask) — who owns its structure, and what shape does it take?

## Starting point

D4 settles capability-based authority with per-Frame capability tables. D7
settles a split interaction model where the syscall number encodes the operation
before the capability table is consulted. The table's role is narrower than in a
unified model: it is a designation/rights lookup, not a dispatch structure.

The design space: kernel-managed opaque handle table (Zircon, Mach, EROS) vs.
userspace-managed CNode tree (seL4) vs. variants.

## D7 narrows the table's role

This is the most structurally significant observation. Under the unified model
(seL4), the capability table participates in operation dispatch — the capability
type determines what operation is performed. CNode trees serve this dual role
well because hierarchical addressing naturally encodes authority relationships
that affect dispatch.

D7 eliminates dispatch from the table's responsibilities. The kernel already
knows the operation from the syscall number. The table is consulted for one
thing: "does this Frame hold a capability to resource X with right Y?" Each
entry is a value — (object pointer, rights mask) — not a dispatch target. The
minimum sufficient structure is: integer handle → (object, rights). This is a
lookup table.

The extra structural expressiveness of CNode trees (hierarchical addressing,
authority-graph encoding, subtree delegation) does not serve the table's actual
function under D7. The tree structure was designed for a role this kernel has
already assigned elsewhere.

## A5 and the CNode model

A5 says the kernel presents a simple interface and absorbs complexity behind it,
rather than exposing primitives that force complexity into userspace.

In the CNode model, userspace must: allocate CNode objects from untyped memory,
choose tree depth and slot counts, manage slot assignment and free-slot
tracking, link CNodes into a tree, and handle CSpace exhaustion. This is
authority-space structure management pushed to userspace.

**Considered:** D2 pushed scheduling algorithm choice to userspace (the kernel
provides mechanism; per-core algorithms are policy). Could authority-space
structuring be analogous? The kernel provides the table mechanism, userspace
chooses the structuring policy.

**Rejected:** D2's argument rests on A3 (generic, can't mandate one scheduling
algorithm). But scheduling algorithms are behind the scheduler interface — the
Frame carries abstract properties, and algorithm-specific state lives in the
leaf (the per-core scheduler). Authority-space structure is NOT behind an
interface — it IS the interface. CNode tree structure determines how handles
work, how delegation works, how sharing works. It's connective tissue, not a
leaf. Applying "push complexity to the leaves": connective tissue must be
simple; complexity belongs in leaves.

CNode management is interface complexity, not leaf complexity. A5 says the
kernel should absorb it.

## Flat table with typed memory backing

A flat kernel-managed table has mild tensions with A4 (synchronous growth during
cold-path syscalls) and A3 (kernel must choose sizing policy). Both are
well-mitigated: growth is cold-path (D1), amortized doubling handles diverse
workloads, and Zircon/Mach ship successfully with kernel-managed sizing.

The remaining concern: memory accounting. Under a pure kernel-managed model, the
kernel allocates table memory from its own pool. The Frame doesn't see the cost.
A Frame acquiring thousands of capabilities consumes kernel memory with no
explicit accounting — a resource exhaustion vector.

The CNode model solves this by making CNodes typed objects allocated from the
Frame's physical memory budget. But this bundles accounting with tree structure.
These are separable concerns.

**Resolution:** The kernel manages the table structure (flat array, growth
strategy, slot reuse) but the physical memory backing the table comes from the
Frame's memory budget. The Frame (or its creator) commits physical memory for
capability storage. When more slots are needed, more memory must be committed.
The kernel manages the layout; the Frame controls the budget.

This gives:

- A5 satisfied — kernel absorbs structural complexity
- Explicit accounting — table size bounded by Frame's physical memory budget
- D7 aligned — flat lookup map for a flat lookup role
- D1 aligned — one memory access per capability lookup

When a capability transfer arrives and the table is full, the kernel faults the
Frame ("table full"). The fault handler (supervisor) commits more memory, then
retries. This mirrors how page faults work under D5 — the Frame doesn't pre-map
all memory, faults trigger allocation.

## Capability table sharing

**Considered:** Can Frames share a capability table?

In a capability system, the natural model for multi-Frame parallelism is
separate Frames with separate address spaces sharing specific memory objects via
capabilities — not Frames sharing an entire address space (the POSIX threads
model). Each Frame has its own trust domain. They SHOULD have separate
authority. A shared memory buffer between Frames does not imply shared authority
over everything else.

The "POSIX threads" model (same address space, same file descriptor table) is a
specific pattern where table sharing makes ergonomic sense. But whether this
kernel supports same-address-space Frame groups is the open Frame-Space binding
model question — it is not a capability table question.

**Decision:** Each Frame always has its own capability table. Table sharing is
deferred to the Frame-Space binding model. If that derivation later settles on
supporting same-address-space Frame groups, table sharing can be reconsidered as
a downstream consequence.

## Foreclosed alternatives

**CNode tree (seL4 model).** Rejected. D7 eliminates the dispatch role that
CNode trees structurally serve. A5 creates genuine tension with CNode management
pushed to userspace. The CNode model's advantages (CDT revocation, partial
subtree sharing) either have alternative solutions (revocation via other
mechanisms) or are downstream of decisions not yet made (Frame-Space binding).
Its costs (expanded syscall surface for CNode operations, two+ memory accesses
per lookup, userspace authority-space management burden) are real.

A5 is not load-bearing here in the sense of "A5 alone forces the decision."
Rather: D7 removes the CNode model's structural justification, and A5 confirms
that the resulting complexity has nowhere productive to go — it's interface
complexity without interface benefit. Both are needed; neither alone is
sufficient.

**Per-core replicated tables (Barrelfish model).** Rejected. D1's shared cold
path + ARM64 cache coherence (A2) makes replication unnecessary. Shared tables
with read-mostly access pattern are correct for cache-coherent ARM64.

**Unified cap/page tables (Composite model).** Rejected. D5 + A2 require ARM64
hardware page table format. Capability entries have different structure.
Applying "use what the hardware provides": the hardware page table walker
expects a specific format; the kernel should program it, not reimplement it.

## Landscape check

**Zircon** uses a kernel-managed flat handle table per process. 32-bit integer
handles, kernel-side, opaque. This is the closest match to the decision — though
Zircon's table memory comes from the kernel's pool rather than the process's
budget.

**seL4** uses CNode trees. The reasoning for CNodes in seL4 includes formal
verification properties (the CNode structure is part of the access-control
proof) and unified-model dispatch. This kernel's D7 (split model) removes the
dispatch motivation. The formal verification motivation is real but does not
require CNode trees specifically — flat tables with clear invariants can also be
verified.

**Mach/XNU** uses a flat port-name table per task. O(1) lookup. Closest to the
flat model.

**EROS/KeyKOS** uses flat c-lists (16-slot nodes). Shallow. Closer to flat than
to CNode trees.

No surveyed system uses the specific combination of flat kernel-managed table
with typed-memory backing (Frame pays from its own budget). This is a novel
position. The novelty is in the accounting model, not the table structure — the
flat table itself is well-validated.

## Archive convergence

The archive chain (restart-1, journal 006) implicitly chose kernel-managed
opaque handle tables without explicitly arguing against CNodes. The current
chain reaches the same structural conclusion but with an explicit argument: D7's
narrowing of the table's role removes the CNode model's structural
justification. The archive did not have D7 (it hadn't derived the split
interaction model at that point in its chain).

The typed-memory accounting model is novel to the current chain. The archive
used implicit kernel-pool accounting.

## What this derivation does NOT settle

- **Handle numbering and ABA prevention.** Generation counters, slot reuse
  strategy. Internal to the table — implementation, not interface.
- **Entry layout.** Fields beyond (object pointer, rights mask): type tag,
  badge, generation counter. Depends on IPC model (badges) and revocation model
  (generation counters).
- **Revocation model.** Flat table is compatible with refcount, authoritative
  destroy, and generation numbers. CDT would require a separate tracking
  structure — whether that's needed is the revocation question.
- **Table-full fault protocol.** Exact mechanism for "table full" faults and the
  commit-more-memory interaction. Depends on fault delegation model.
- **Frame-Space binding and table sharing.** Deferred.
- **Maximum table size.** Bounded by the Frame's physical memory budget, but any
  kernel-imposed hard cap is a separate policy question.

## Status

**Accepted as `spec.md#D8` — settled.**

Revisit if:

- D7 is revised (a move to unified model would re-motivate CNode trees as
  dispatch structures)
- The Frame-Space binding model reveals that same-address-space Frame groups
  need shared capability tables and the per-Frame-table model forces essential
  complexity into userspace that cannot be covered by capability transfer alone
- The revocation model requires CDT and the absence of tree structure makes CDT
  impractical (would pressure toward CNode structure or a separate derivation
  tree)
