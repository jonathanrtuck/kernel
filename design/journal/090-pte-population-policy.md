# D90 — PTE population policy: eager

**Question:** When does the kernel populate L3 page table entries for a Space's
physical pages — at Space creation (eager) or on first access (demand fault)?

**Rests on:** D1 (hot path — faults are expensive), D12 (fault delegation — all
faults go to pager), D26 (kernel manages page tables), D32 (type conversion —
physical pages committed at creation), D42 (real-time precision — deterministic
timing), D61 (VM_FAULT semantics — faults mean "no access"), D89 (shared L3
tables per-Space).

**Status:** settled — eager.

---

## Settles

### Eager population at Space creation

When a Space is created (D32 type conversion), the kernel allocates the L3
table(s) and immediately populates all entries with descriptors for the Space's
physical pages. When any Observer subsequently acquires a cap to the Space, the
L2 entry points to a fully-populated L3 table. Every page is accessible from the
first touch with no fault.

### Cost analysis

A 32 MiB Space (maximum per L3 table): 2048 entries × 8 bytes = 16 KiB of
stores. At L1 cache speed: ~0.5 μs. This is cold-path work (Space creation).

A single demand fault (exception entry → kernel check → descriptor write →
resume) costs ~1–5 μs. Eagerly writing all 2048 entries is cheaper than handling
one demand fault.

Physical pages are committed at Space creation (D32) regardless of population
policy. Eager vs. demand does not affect physical memory usage — only whether
the L3 descriptors point to committed pages now or later.

### Consequences

- **Fault path stays clean (D12, D61).** Every translation fault is a true fault
  — the Observer accessed memory it has no capability for, or an out-of-bounds
  offset. No kernel-internal "was this a demand fault?" check on the exception
  path. The kernel delegates all faults to the pager.

- **Deterministic first-access latency (D42).** No surprise page faults on first
  touch. Every page in a held Space has the same access cost.

- **Shared L3 tables (D89) are populated once.** The L3 table is written at
  Space creation. All Observers that subsequently acquire the Space cap benefit
  immediately — no "first Observer pays the fault cost" asymmetry.

---

## Rejected alternatives

**Demand faulting (lazy population):** leave L3 entries invalid, populate on
first access via translation fault. Benefits large sparse Spaces (only accessed
pages get entries). Rejected because: (1) per-fault overhead (~1–5 μs) exceeds
per-entry write cost (~0.25 ns) by ~4000×, making demand faulting strictly more
expensive unless zero pages are ever accessed; (2) adds a demand-fault check to
every exception; (3) creates non-deterministic first-access latency; (4)
complicates the fault path (kernel must distinguish demand faults from true
faults before delegating to the pager).

**Hybrid (eager for small, demand for large):** adds a threshold heuristic
without meaningful benefit — even the largest Space per L3 table (32 MiB, 2048
entries) takes ~0.5 μs to populate eagerly.

---

## Does NOT settle

- Physical page zeroing policy (zero on allocation vs. zero on first access)
- Lazy L2 table allocation for Observers (separate concern — per-Observer, not
  per-Space)
