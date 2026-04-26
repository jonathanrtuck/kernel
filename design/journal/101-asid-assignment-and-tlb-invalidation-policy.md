# D101 — ASID assignment and TLB invalidation policy

**Question:** How does the kernel assign ASIDs to Observers, and what TLB
invalidation strategy applies when a Space is unmapped?

**Rests on:** D5 (MMU-backed virtual memory), D24 (cap-mapping invariant — close
triggers unmap + TLBI), D25 (page granularity — 16 KiB pages), D26
(kernel-managed VA — ASID is kernel-internal), D46 (core lifecycle — all cores
active, all need broadcast), D56 (Observer placement — migration means stale
entries possible on any core), D88 (TTBR split — ASID in TTBR0), D89
(per-Observer page table — each Observer has unique ASID), D91 (mapping
orchestration — unmap path).

**Status:** settled.

---

## Settles

### ASID assignment and lifecycle

Each Observer receives a unique ASID at creation time from a kernel-managed
sequential counter. ARM64 supports 8-bit or 16-bit ASIDs (determined by
`ID_AA64MMFR0_EL1.ASIDBits`). The kernel reads the hardware capability at boot
and uses the maximum available width (16-bit where supported, 8-bit otherwise).
`asid_width_from_mmfr0` in `src/frame/arch/aarch64/mmu.rs` already implements
this detection.

Assignment is sequential: each new Observer gets `next_asid++`. When the counter
wraps (after 2^16 or 2^8 Observers), the kernel performs a full TLB broadcast
(`TLBI VMALLE1IS`) and resets the counter. Destroyed Observers' ASIDs are not
recycled — sequential assignment avoids the ABA problem where a reused ASID
could match stale TLB entries from a different Observer.

The wrap + full flush is the simple correct approach; ASID recycling with
generation tracking is a future optimization. With 16-bit ASIDs, 65536 Observers
must be created before a wrap occurs. The flush is a one-time cost amortized
over tens of thousands of creations.

### TLB invalidation scope

When a Space cap is closed from an Observer (D24 unmap), TLB entries for that
Space's VA range in that Observer's ASID must be invalidated. Two invalidation
strategies apply depending on the unmap size:

| Condition                 | Instruction     | What it invalidates                            |
| ------------------------- | --------------- | ---------------------------------------------- |
| `page_count <= threshold` | `TLBI VAE1IS`   | Per-page: one TLB entry per page in the range  |
| `page_count > threshold`  | `TLBI ASIDE1IS` | Per-ASID: all entries for that Observer's ASID |

The threshold is a tuning parameter (e.g., 16 pages), not a design decision.

All invalidations use the IS (inner-shareable) variant to broadcast to all
cores. This is mandatory — Observers may migrate between cores (D56), and stale
TLB entries on any core would create security violations. A `DSB ISH` follows to
ensure completion before the unmap syscall returns. The existing
`tlb_invalidate_space_pages` (per-VA) and `tlb_invalidate_asid` (per-ASID)
functions in `mmu.rs` implement these sequences.

The invalidation barrier sequence is:

```text
DSB ISHST          — page table stores visible to hardware walkers
TLBI ...IS         — invalidate stale entries (broadcast)
DSB ISH            — wait for completion across all cores
ISB                — synchronize instruction stream
```

This matches the ARM ARM D5.10.2 recommended maintenance sequence and is already
implemented in `unmap_space_from_observer` (via
`mmu::tlb_invalidate_space_pages`).

---

## Rejected alternatives

**ASID recycling pool:** Adds complexity (freelist or bitmap) for marginal gain.
2^16 = 65536 Observers before wrap. Sequential + flush-on-wrap is simple and
correct. The recycling optimization can be added later without changing the
interface — the ASID is kernel-internal (D26), so the assignment strategy is
invisible to Observers.

**Per-core ASID assignment** (different ASID on each core for same Observer):
ARM64 TTBR0 stores one ASID per Observer. Per-core ASIDs would require ASID
rewrite on every migration, complicating the context switch path. The ARM64
architecture associates the ASID with TTBR0, not with the core — a single
ASID-per-Observer is the natural model.

**Full TLBI on every unmap** (`TLBI VMALLE1IS`): Correct but wasteful.
Invalidates all user TLB entries on all cores, affecting every Observer's
mappings. Per-VA invalidation is efficient for the typical case (single Space
unmap = small number of pages). Per-ASID invalidation handles the bulk case
without the global-flush overhead.

**Non-IS (local) TLBI:** Insufficient. An Observer may have run on other cores
before the unmap; stale TLB entries there would allow memory access after cap
revocation — a security violation of D24. The IS suffix broadcasts to all cores
in the inner-shareable domain, which on ARM64 includes all cores sharing the
same coherency domain.

---

## Does NOT settle

- ASID recycling optimization (generation-tracked freelist for long-lived
  systems that create more than 65536 Observers)
- Optimal per-VA vs. per-ASID threshold value (implementation tuning, not
  design)
- Per-Space access permissions (D26 open — if different caps carry different
  rights, the TLBI scope may need refinement)
- Observer destroy TLB invalidation batching (D33 cascade may destroy multiple
  Spaces from one Observer — batching into a single `TLBI ASIDE1IS` is an
  optimization)
