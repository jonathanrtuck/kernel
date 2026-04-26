# D92 — Page table memory accounting

**Question:** Whose Space backs the page table pages — L1 root, L2 tables, and
L3 tables?

**Rests on:** D31 (root pool — kernel metadata from root Space), D32 (type
conversion — subtree cost baked into Space), D35 (Observer creation — consumed
Space becomes structural backing), D43 (Observer schema — page table root in
structural backing), D70 (arena slabs from root pool), D89 (3-level structure:
per-Observer L1/L2, per-Space L3).

**Status:** settled.

---

## Settles

### Three structures, three sources

| Structure | Scope        | Source                                 | When allocated                          |
| --------- | ------------ | -------------------------------------- | --------------------------------------- |
| L1 root   | per-Observer | Observer's consumed Space (D35)        | Observer creation                       |
| L2 tables | per-Observer | kernel root pool (D31)                 | first cap install in a 64 GiB L1 region |
| L3 tables | per-Space    | Space's type conversion overhead (D32) | Space creation                          |

### L3 tables — charged to the Space

D32 already settles this: "Page table subtree cost is baked into the Space at
split time. The parent Space shrinks by `child_size + subtree_overhead`."

With D89's shared L3 tables, the L3 table is the Space's shareable subtree. It
is allocated at Space creation and freed at Space destruction. All Observers
sharing the Space point to the same L3 table — no per-holder allocation.

Cost per Space: one L3 table (16 KiB) per 32 MiB of Space content.
`type_conversion_overhead` already computes this.

### L1 root — charged to the Observer

D43 lists the page table root as part of Observer structural backing. D35 says
consumed Space becomes structural backing (register save area, cap table pages,
page table root). The L1 root is one 16 KiB page, allocated once at Observer
creation from the consumed Space.

### L2 tables — charged to the kernel root pool

L2 tables are per-Observer connectivity that links the L1 root to shared L3
tables. They are allocated on demand (D91: when an Observer's first Space cap in
a 64 GiB L1 region is installed) and freed when the last Space in that region is
unmapped.

This parallels D70's arena slab pages — internal kernel data structures drawn
from the root pool. The cost is:

- Bounded: one 16 KiB L2 table per active 64 GiB region per Observer
- Typical: 1 L2 table (16 KiB) per Observer — all Spaces fit in one 64 GiB
  region
- Maximum: 2048 L2 tables (32 MiB) per Observer — only if Spaces span all L1
  entries

D32's "per-object kernel metadata is allocated from the kernel's root Space
(D31), bounded per object, small relative to functional backing" applies here.

### Total memory budget

Per Observer: L1 root (16 KiB, from consumed Space) + ~1 L2 table (16 KiB, from
root pool) = ~32 KiB.

Per Space: ~1 L3 table (16 KiB, from type conversion overhead) + physical pages.

System-wide for N Observers sharing M Spaces: N × 32 KiB + M × 16 KiB =
O(Observers + Spaces). Matches D26.

---

## Does NOT settle

- Large Space handling (Spaces > 32 MiB need multiple L3 tables — the overhead
  scales linearly with Space size, already handled by
  `type_conversion_overhead`)
- L2 table pre-reservation vs. on-demand allocation tradeoff (settled as
  on-demand; pre-reservation wastes memory for the typical single-region case)
- Root pool exhaustion policy for L2 allocation failure (parallels arena slab
  allocation failure — propagate error to caller)
