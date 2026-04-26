# D91 — Cap-to-mapping protocol

**Question:** What page table operations happen when a Space capability is
installed into or removed from an Observer's cap table?

**Rests on:** D8 (flat cap table), D11 (close/destroy refcount), D24
(cap-mapping invariant — no cap → no mapping), D26 (capability-addressed
memory), D33 (destroy cascade), D41 (split/merge), D89 (shared L3 tables,
per-Observer L1/L2), D90 (eager population — L3 tables are already populated).

**Status:** settled.

---

## Settles

### Core principle

With D89's shared L3 tables and D90's eager population, **all per-Observer page
table work is at the L2 level**: write one table descriptor to map a Space,
clear it to unmap. The L3 tables are populated once at Space creation and shared
immutably across Observers until the Space itself changes (split/merge/destroy).

### Install Space cap

When a Space cap is installed into Observer A's cap table (via `install_cap`
syscall, IPC cap transfer, or initial setup):

1. Look up the Space's VA base and L3 table PA
2. Compute L1 index and L2 index from VA base
3. Check A's L2 entry for this region
4. **If L2 entry already points to this Space's L3 table** → already mapped
   (Observer has a duplicate cap). No page table work.
5. **If L2 entry is invalid** → write table descriptor:
   `A.L2[l2_idx] = table_descriptor(space.l3_pa)`. Allocate L2 table if L1 entry
   is also invalid.
6. No TLB invalidation — adding a valid entry where there was none is not a
   break-before-make violation (ARM ARM D8.14.1).

Cost: 1–2 table descriptor writes (L1 + L2 if both new). Cold path.

### Close Space cap

When a Space cap is closed in Observer A's cap table (via `close` syscall, cap
move on IPC, or destroy cascade):

1. Close the cap slot (existing `Table::close` — returns `CloseResult`)
2. **Check whether Observer A still holds any other cap to this Space** — scan
   the cap table for remaining entries with `SlotTag::Space` pointing to the
   same arena index. O(cap_table_capacity), cold path.
3. **If other caps remain** → no page table work. Space stays mapped.
4. **If no caps remain** (last cap to this Space in this Observer):
   - Clear `A.L2[l2_idx] = 0`
   - If L2 table is now empty: clear `A.L1[l1_idx] = 0`, free L2 table
   - **TLB invalidation**: `TLBI VAE1IS` for each page in the Space's VA range,
     using A's ASID. Follow with `DSB ISH; ISB`.

Cost: O(cap_table_capacity) scan + 1 L2 clear + TLB invalidation. Cold path. The
scan is ~0.25 μs for 256 slots, ~1 μs for 1024.

### Space split (D41)

Space S shrinks; new Space T created from the split portion.

1. S's existing L3 table: kernel clears entries for the pages that moved to T
2. T gets a new L3 table populated with those pages' descriptors (D90 eager)
3. **All Observers holding S** see the shrinkage immediately via the shared L3
   table. No per-Observer L2 work — the L2 entries still point to S's L3 table.
4. **TLB invalidation** for the cleared entries: `TLBI VAE1IS` for each cleared
   page, broadcast across all ASIDs that map S. (The kernel must iterate S's
   holder set or use `TLBI VMALLE1IS` as a conservative fallback.)
5. T's L3 table is available for cap install (step 1 above) to any Observer.

### Space merge (D41)

Source T absorbed into target S. T ceases to exist.

1. **S's VA range extends.** If T was in the same L3 table (adjacent within 32
   MiB): kernel writes additional entries into S's L3 table. If T spans a
   separate L3 table: S absorbs T's L3 table — S now has multiple L3 tables
   covering its extended VA range.
2. **Observers holding S** that need the extension: kernel writes a new L2 entry
   for T's former VA region → S's (newly-absorbed) L3 table. This is like a new
   Space install for the extended portion.
3. **Observers holding T**: T is destroyed after merge. The destroy cascade
   closes T's caps, triggering the close protocol (above) on each holder — L2
   entries for T are cleared, TLB invalidated.
4. No break-before-make issue: the new L2 entries (step 2) are for a VA region
   that was previously mapped to T's L3 table (now S's). Observers holding both
   S and T already had an L2 entry for T's region — this entry now points to the
   same physical L3 table under S's ownership. The descriptor value is
   unchanged; only the logical ownership moved.

### Space destroy (D11/D33)

1. All caps revoked first (D11 cascade) → close protocol (above) clears every
   Observer's L2 entry for this Space + TLB invalidation
2. After all caps closed: free L3 table(s), return physical pages to
   SpaceManager

### Duplicate cap handling

An Observer may hold multiple caps to the same Space (e.g., an original +
attenuated clone, D52). With permissive descriptors (D89 — all RW), all caps
produce the same page table mapping. The L2 entry is written on first cap
install and cleared on last cap close. The mapping is binary (present or
absent), not per-cap.

When per-Space access permissions are settled (D26 open), this may need revision
— different caps with different rights could require different descriptors. The
protocol's cap-table-scan step accommodates this: it can be extended to
recompute the effective descriptor from remaining caps.

---

## Rejected alternatives

**Per-Observer per-Space refcount** (explicit count in Observer struct or L2
entry software bits): avoids the cap table scan on close but adds state that
must be maintained across install, close, cascade, and transfer. The scan is
fast enough (~1 μs worst case) on a cold path that the extra state isn't
justified.

**TLB flush per ASID** (TLBI ASIDE1IS on every unmap): simpler than per-VA
invalidation but over-invalidates — flushes ALL of the Observer's TLB entries,
not just the Space being unmapped. Acceptable as a conservative fallback but
per-VA invalidation is preferred when the page count is small.

---

## Does NOT settle

- Per-Space access permissions (D26 open — affects whether duplicate caps with
  different rights need different descriptors)
- Cross-core TLB invalidation optimization (IPI to target core vs. broadcast)
- Cap table scan optimization (fast-path for common case of single cap per
  Space)
- Merge mechanics for Spaces spanning multiple L3 tables (large Spaces > 32 MiB)
