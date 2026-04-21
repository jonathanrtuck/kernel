# Kernel-Internal Memory Accounting — 2026-04-20

Thirty-third exploration. Derived the memory accounting protocol: who pays for
kernel-internal structures, how costs are tracked, and how destruction reverses
creation.

## Starting point

D8 established typed-memory backing (cap table memory from Observer's Spaces).
D9 extended it (Space = memory budget). D31 established the kernel's root Space
and the pager chain for resource acquisition. Open questions remained: page
table budget (journal 027), boot accounting, kernel bookkeeping, budget
treatment on destruction.

## Derivation

### Three accounting domains, one rule

All kernel memory allocation follows one rule: present a Space, kernel allocates
from it. Three domains emerge based on who presents:

1. **Object creation (Observer, Field):** The presented Space is consumed
   entirely — a type conversion. `create_field(space_cap)` converts a Space into
   a Field. The physical pages change purpose, not quantity.
   `create_observer(space_cap, config)` converts a Space into an Observer's
   structural backing (cap table, L0 page table root, register save area). The
   Space is gone; the object exists.

2. **Kernel per-object metadata:** The kernel's internal bookkeeping struct for
   each object (queue header, scheduling aggregate, tracking fields) is
   allocated from the kernel's root Space. This cost is invisible to userspace,
   bounded per object (fixed-size struct), and small relative to the object's
   functional backing. Total system capacity is already invisible to userspace
   (D31 — root Space is kernel-internal), so hiding per-object metadata within
   it introduces no new opacity.

3. **Kernel-internal bookkeeping:** Space manager state, IRQ→field routing
   table, per-core scheduler state. Charged to root Space. Bounded by hardware
   constants and object count. Invisible to userspace.

### Type conversion: create and destroy are inverses

Creation converts Space into an object. Destruction converts the object back
into Space:

```text
create_field(space_cap) → field_cap    (Space becomes Field)
destroy_field(field_cap) → space_cap   (Field becomes Space)
```

Conservation is structural: physical pages change purpose, not quantity. The
returned Space is a new object (new VA base per D26, new cap) with no dependency
on the original source Space — which may no longer exist.

For objects backed by multiple Spaces (e.g., an Observer whose cap table grew
via 2A fault-reply contributions), destruction returns one merged Space. D9 says
the kernel manages physical backing internally — non-contiguous physical pages
behind a contiguous VA Space is standard.

If the destroyer doesn't want the returned Space, closing the cap immediately
sends the pages to the kernel's root Space (D11 close semantics). Pages re-enter
circulation through the pager chain (D31).

### Page table subtree cost baked into Space

D26's per-Space shared page table subtrees (L1/L2/L3) have a cost that is a
deterministic function of Space size and page granularity. This cost is reserved
at Space creation (split). The parent Space shrinks by
`child_accessible_size + subtree_overhead`.

First holder: subtree populated from the Space's reserved capacity. No charge to
the acquiring Observer. Subsequent holders: reference count increment, no
allocation. The cost is a property of the Space, not the holder.

### Cap table growth: handler provides Space in fault reply (2A)

When an Observer's cap table is full (D8), the kernel faults to the handler. The
handler's reply includes a Space cap. The kernel allocates table pages from that
Space (Space consumed entirely — type conversion into cap table backing). The
handler controls which Space pays.

This is the most general protocol — it forecloses nothing. Optimizations (e.g.,
a designated growth Space tried before faulting) can be added behind the same
interface later.

### Time is asymmetric

Time comes from the kernel's per-core scheduling capacity pool (D31), not from
Space. Destroying a Time cap returns scheduling capacity to the kernel's pool.
No Space involved. Correctly asymmetric: Time and Space come from different
bounded resources, return to different pools.

### Boot structures from root Space

The kernel creates boot-time structures (root Observer, initial Spaces, initial
Time, initial Field) from its root Space. Fixed, predictable cost. No
reconciliation needed — the root Space simply has less available after boot.

### Observer destruction: held caps deferred

Destroying an Observer returns its structural backing as a Space cap to the
destroyer (same type-conversion reversal). What happens to the Observer's HELD
capabilities (caps in its table to other objects) is the destroy cascade
question — deferred to the next exploration.

## Archive convergence

Strong. Archive journal 013 describes the same type-conversion model:
`open_wormhole(space_handle) → wormhole_handle` and its reverse
`close_wormhole(wormhole_handle) → space_handle`. "Conservation holds — the
physical bytes changed purpose, they didn't multiply." Same mechanism,
independently derived.

## What remains open

- **Destroy cascade protocol.** When an Observer is destroyed, its held caps are
  closed (D11). Cascading zero-refcount destructions produce freed backing that
  needs recipients. Connects to the pager chain / supervisor model.
- **Space "create" right.** Whether Field/Observer creation from a Space cap
  requires a specific right in the rights mask (D8). Likely yes for D4.
- **Overhead reporting.** When an Observer queries a Space's size, does it see
  the accessible size minus the reserved subtree overhead, or just the
  accessible size? (The subtree overhead is baked in at split time and never
  changes, so this is a one-time decision about the size interface.)
- **Merge / join operation.** The reverse of split: two Spaces become one. Would
  allow consolidating fragments. Not required by the accounting model but a
  natural companion to split.
