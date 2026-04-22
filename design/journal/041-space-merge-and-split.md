# 041 — Space merge and split

**Date:** 2026-04-21

## Starting point

D9 deferred "specific operations on Spaces (split, COW/clone, resize)." D27
re-deferred as "D9 downstream." D40 identified Space resize as "the
highest-leverage open question for enabling traditional demand paging" — under
D26's capability-addressed memory model, a new Space cannot resolve an
out-of-bounds fault because the faulting instruction retries at the same VA, and
a new Space gets a different kernel-assigned VA base.

The question: can a Space be resized after creation, and if so, what are the
semantics?

## Exploration

### Framing shift: merge and split, not resize

The initial framing was "resize" — growing and shrinking a Space. This framing
implies Space material appears or vanishes. It doesn't. Physical memory is
conserved (D32). The correct framing is two topology-changing operations:

- **Merge:** two Spaces become one. The source Space is absorbed into the
  target. The target's VA range extends. The source ceases to exist as an
  independent Space. Pages change membership, not quantity.
- **Split:** one Space becomes two. A portion of the target is extracted into a
  new independent Space with its own kernel-assigned VA base. The target's VA
  range contracts.

These are the only operations that change Space boundaries. Everything else —
object creation (D32 type conversion), object destruction — changes what
_occupies_ a Space, not the Space itself. A Field created from a Space "takes
up" the Space; the Space is still there as physical memory, just not usable for
other purposes until the Field is destroyed.

Split is already established. D31, D32, and D33 all use "the parent Space
shrinks by the allocation cost" — that IS split. Every `create_observer` and
`create_field` call implicitly splits the creation Space (the consumed portion
becomes the object's backing; if partial consumption were supported, the
remainder would be a smaller Space). Merge is the genuinely new primitive.

### Why merge is needed: the D26 demand-paging gap

D40 established that without merge, out-of-bounds page faults are error
notifications — the handler destroys the Observer or performs cooperative
recovery (PC surgery via write-registers, redirect to trampoline, explicit
resource request for a new Space, adapt to new VA base). This cooperative
recovery protocol is:

1. Handler calls `observer_write_registers(obs, modified_state)` — sets PC to a
   pre-arranged trampoline in the Observer's code
2. Handler calls `observer_resume(obs)` — Observer wakes at the trampoline
3. Observer requests a new Space via D31 resource request path
4. Observer receives new Space cap with a different VA base
5. Observer updates internal bookkeeping to use the new base
6. Observer retries the operation

This is a multi-syscall, two-Observer protocol that every Observer needing
memory growth must support. The kernel could absorb this complexity: a single
merge operation grows the faulting Space in-place, and the retried instruction
succeeds at the original VA.

Applying A5 and O4: the cooperative recovery protocol is essential complexity
being pushed to userspace. The kernel has all the information needed to extend
the Space's VA range (it controls VA assignment per D26, it manages page tables
per D24). The merge operation absorbs this complexity into a simpler kernel
interface — consistent with A5's placement principle.

### Consequence analysis

**D26 (VA base stability):** The base stays fixed on both merge and split. Merge
extends the range upward from the fixed base. Split contracts the range; the
extracted portion gets its own new base. This is implementable because the
kernel controls VA layout (D26) and ARM64's 48-bit VA space (256 TB per half)
provides ample room for growth headroom between Space VA bases.

**D24 (cap-mapping invariant):** All holders of a Space see the merge/split
immediately — the page table is a materialized view of cap state. For merge: all
holders gain access to additional pages. For split: holders may lose access to
pages in the extracted portion (those pages now belong to the new Space, and
holders don't automatically receive a cap to it). The split visibility parallels
destroy (D11) — holders learn of access loss via fault.

**D32 (type conversion / conservation):** Merge follows the same pattern. The
source Space is consumed entirely — its pages become part of the target. The
source's page table subtree memory is absorbed into the target's subtree
extension. Conservation holds: total physical pages across all Spaces is
unchanged.

**D33 (page table subtree cost):** D33 says "page table subtree cost is baked
into the Space at split time." Merge requires extending the subtree — additional
L2/L3 page table entries. The consumed source Space provides the physical memory
for these entries. The kernel computes the overhead deterministically from the
size delta and page granularity (D25). Subtree management becomes incremental
rather than one-shot.

**D1 (hot/cold):** Both merge and split are cold-path operations. Split may
require TLB invalidation for shared Spaces across cores (O2 — IPIs). Merge
likely doesn't require TLB invalidation on ARM64 (the architecture does not
cache translation faults for unmapped ranges).

**D4, D8 (authority):** Both operations require dedicated rights in the Space
rights mask. Merge requires a right on the target Space (authority to extend it)
and consumes the source Space entirely. Split requires a right on the target
Space (authority to divide it). Whether merge and split use separate right bits
or share a single "topology" right is one level down.

**VA adjacency on merge:** Growing extends the VA range upward. If no adjacent
VA space is available (another Space's base is too close), merge fails — the
kernel returns an error. The kernel's VA layout policy (how much headroom to
leave between Spaces) determines how often this occurs. This is kernel-internal
policy, not exposed to userspace.

**Space-Time asymmetry:** Time is fungible — multiple Time caps are additive
(D30), and the kernel maintains a cached aggregate per Observer. Space is not
fungible — each Space has its own VA base (D26) and object identity
(vocabulary). Time "merge" is implicit (handing a second Time cap to an Observer
increases the aggregate). Space merge is structural: one Space absorbs another,
the source ceases to exist, the target's VA range extends. This asymmetry is a
consequence of D26, not a design choice.

### What was rejected

**Grow/shrink as separate primitives from merge/split.** The initial framing
proposed `space_grow(target, source)` and `space_shrink(target, amount) → cap`.
These are the same operations as merge and split under different names. The
merge/split framing is preferred because it makes conservation explicit (two →
one, one → two) rather than implying material creation or destruction.

**Grow-only (no split).** Split is already established in the design — D31/D32
use partial Space consumption on object creation. Providing merge without split
as an explicit operation would leave the design asymmetric for no axiomatic
reason.

**No merge/split (cooperative recovery only).** Rejected on A5 grounds. The
cooperative recovery protocol pushes essential complexity into userspace. The
kernel has all information needed to perform the operation (VA layout, page
table management). O4(a) applies: moving this complexity from kernel to
userspace is an A5 violation.

**Virtual resize (extend VA range without physical backing).** Would violate D9
— Spaces are always physically backed. The source Space for merge must be fully
backed.

**Relocating VA base.** Foreclosed by D26 — the base is a stable property of the
Space. Relocation would invalidate all cached base addresses in all holding
Observers.

## Archive convergence

The archive (claims.toml, "space-non-fungibility") noted: "Space splitting
produces distinguishable children" (line 1107). Same conclusion as this
derivation for the split direction.

The archive did not derive merge. The archive's Space model was VA-addressed
("space-named-by-virtual-address" — Objects refer to Space by virtual address,
the page table IS the naming mechanism). Under that model, demand paging works
through traditional page fault resolution (the pager controls VA placement), so
merge was not needed. The divergence is explained by D26: capability-addressed
memory with kernel-assigned VA bases creates the demand-paging constraint that
motivates merge.

## The decision

Spaces support two topology-changing operations:

- **Merge** (two → one): a source Space is absorbed into a target Space. The
  source ceases to exist as an independent Space. The target's VA range extends
  upward from its fixed base. All holders see the extended range. The source's
  physical pages and page table subtree memory are absorbed. Follows D32
  conservation: pages change membership, not quantity.

- **Split** (one → two): a portion of a target Space is extracted into a new
  independent Space. The new Space receives its own kernel-assigned VA base
  (D26). The target's VA range contracts. Holders of the target may lose access
  to the extracted portion (no automatic cap to the new Space). Follows D32
  conservation: total pages unchanged.

Both are typed kernel syscalls (D7), cold-path (D1), require dedicated rights in
the Space rights mask (D4/D8), and operate at page granularity (D25).

This resolves D40's demand-paging gap: a pager handling an out-of-bounds fault
can merge a source Space into the faulting Space to cover the offset, then
resume the Observer. The existing pager protocol (install_cap + resume) is
unchanged — merge is an additional step before resume.

D32's unsettled "merge/join operation (reverse of split)" is resolved: merge is
the reverse of split.

## What remains open (one level down)

- Syscall signatures: `space_merge(target_cap, source_cap)` consumes source
  entirely (D32 pattern)? Or `space_merge(target_cap, source_cap, amount)` for
  partial merge (combines merge + split in one call)?
- `space_split(target_cap, amount) → new_space_cap` — always extracts from the
  high end (preserving base)?
- Separate merge/split rights, or a single topology right?
- VA headroom policy (kernel-internal — how much room to leave for growth)
- Space vocabulary: "not cumulative" wording may need refinement (the claim's
  quantity changes on merge/split, but each state is a snapshot)
- Space rights mask (merge/split rights feed into the broader Space rights
  question deferred by D9)
- COW/clone (D9 deferred — orthogonal to merge/split)

## Status

**Settled.** Spaces support merge and split as topology-changing operations.
Merge is new; split was already established as a pattern (D31/D32/D33). Both
follow D32 conservation. Resolves D40's demand-paging gap and D32's unsettled
merge/join.

Revisit if D26 is revised (different VA assignment model may change the
demand-paging constraint that motivates merge), if D32 is revised (changes the
conservation model that merge/split rely on), or if a downstream derivation
reveals that the VA adjacency constraint on merge creates essential complexity
(e.g., merge failure rates are unacceptable under realistic workloads).
