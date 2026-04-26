# D89 — Per-Observer page table structure

**Question:** What is the structure of each Observer's user-space page table,
and how are Space capabilities materialized as page table entries?

**Rests on:** D5 (MMU-backed isolation), D24 (cap-mapping invariant), D26
(capability-addressed memory — shared L3 subtrees), D43 (Observer
page_table_root = TTBR0), D88 (TTBR0/TTBR1 split — 3-level user walk L1→L2→L3).

**Status:** settled.

---

## Settles

### 3-level structure with shared L3 tables

The 3-level user walk (D88: T0SZ=17, L1→L2→L3) maps cleanly onto D26's sharing
model:

- **L1 root**: per-Observer (16 KiB, 2048 entries, each covers 64 GiB)
- **L2 tables**: per-Observer (16 KiB each, one per active 64 GiB L1 region —
  typically just 1)
- **L3 tables**: **per-Space**, shared across all Observers holding that Space
  cap (16 KiB each, 2048 entries, each maps one 16 KiB page)

Memory: O(Observers × 32 KiB) + O(Spaces × 16 KiB) = O(Observers + Spaces). This
is exactly D26's claim.

### 32 MiB VA alignment per Space

Each Space is assigned a VA base aligned to an L2 entry boundary (32 MiB). This
ensures each L3 table belongs to exactly one Space — no two Spaces share entries
in the same L3 table.

With 128 TiB user VA (D88), 32 MiB alignment gives ~4 million Space slots. The
alignment cost is invisible against that much address space.

### Sharing mechanics

When Observer A acquires a cap to Space S:

1. Kernel looks up S's L3 table PA (stored on the Space object)
2. Computes the L1 and L2 indices from S's VA base
3. Ensures A's L2 table exists for that L1 region (allocate if needed)
4. Writes a table descriptor into A's L2 entry → S's L3 table

When Observer A loses its cap to Space S:

1. Kernel clears A's L2 entry for S's VA region
2. If A's L2 table is now empty, clear the L1 entry and free the L2 table

No reference counting on L3 tables — they are owned by the Space. When the Space
is destroyed (all caps already revoked per D11), the L3 tables are freed.

### L3 table lifecycle

- **Allocated** when a Space is created (type conversion, D32). The L3 table is
  part of the Space's structural overhead.
- **Populated** with page descriptors for the Space's physical pages.
- **Shared** by writing table descriptors in each holding Observer's L2 table.
- **Freed** when the Space is destroyed.

### User page descriptor format

All Space pages use a single descriptor template:

```text
[54]    UXN     0       EL0 can execute (permissive default)
[53]    PXN     1       EL1 cannot execute from user pages
[11]    nG      1       Non-global (ASID-tagged, D88)
[10]    AF      1       Access Flag (no fault on first access)
[9:8]   SH      0b11    Inner Shareable (SMP coherent)
[7:6]   AP      0b01    EL0 read/write (AP_RW_EL0)
[4:2]   AttrIndx 0b001  Normal memory (MAIR index 1)
[1]     Page    1       L3 page descriptor
[0]     Valid   1
```

**Access permissions are permissive**: all Space pages are EL0 RW + executable.
D26 leaves per-Space access rights as open. The permissive default is safe: EL0
can only access its own TTBR0 mappings, kernel is protected in TTBR1.

**nG=1**: user mappings are ASID-tagged. The TLB uses the ASID from TTBR0 to
disambiguate entries from different Observers.

### Merge compatibility

D41 merge requires VA adjacency. With 32 MiB alignment, the SpaceManager assigns
merge targets at VAs adjacent to the merge source — within the same 32 MiB slot.
The merge is a kernel operation that combines physical pages in the shared L3
table; no per-Observer page table changes are needed.

### TLB invalidation

- **After map (L2 entry write)**: no invalidation needed (new valid entry where
  previously invalid)
- **After unmap (L2 entry clear)**: `TLBI VAE1IS` per page in the unmapped
  range, or `TLBI ASIDE1IS` for bulk removal
- **After destroy_page_table**: `TLBI ASIDE1IS` (all entries for that ASID)
- All TLBI with IS (inner-shareable broadcast) for SMP correctness

---

## Rejected alternatives

**Per-Observer L3 tables** (no sharing): simpler, but memory scales as
O(Observers × Spaces). For 100 Observers sharing 3 Spaces: 4.8 MiB vs 96 KiB.
Was the initial D89 approach under 2-level tables — the move to 3 levels made
sharing practical, eliminating the need for this compromise.

**L3 sharing with 2-level tables** (32 MiB alignment in 64 GiB VA): limits total
Spaces to 2048, wastes VA for small Spaces. The 3-level design dissolves this
tradeoff.

---

## Does NOT settle

- Per-Space or per-cap access permissions (D26 open: separate "access" right?)
- SpaceManager VA assignment policy details (bump allocator with 32 MiB stride)
- ASID allocation and recycling
- Break-before-make protocol for updating existing valid entries
- Large Space handling (Spaces > 32 MiB need multiple L3 tables)
- Page table memory accounting — whose Space backs L3 pages (D92)
