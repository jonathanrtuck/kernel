# 027 — Capability-addressed memory

**Date:** 2026-04-19. **Starting point:** The asymmetry between explicit map and
implicit unmap (D24) prompted the question: can the mapping step be eliminated
entirely? If holding a capability to a Space is sufficient authority to access
that memory, why require a separate map() operation?

---

## The question

Can an Observer access memory by presenting a Space capability and an offset,
rather than by explicitly mapping memory objects into an address space at chosen
virtual addresses?

---

## Exploration path

### The asymmetry that started this

D24 settled auto-unmap: when an Observer loses its last capability to a mapped
Space, the kernel removes the mapping. But map() is explicit — the Observer must
call a syscall to create the mapping, choosing a virtual address. Acquire a cap:
must act. Lose a cap: automatic. This asymmetry suggests the programming model
has a seam that doesn't need to exist.

The desired property: **cap acquisition = access, cap loss = no access.** Both
directions driven by capability state, no separate map/unmap API.

### Why not auto-map?

Journal 026 rejected auto-map on three grounds:

1. **D10 cascade:** Auto-map in a shared Coordinate System creates mappings
   visible to co-located Observers without caps — contradicting D24.
2. **Cap-without-mapping is useful:** Resource managers hold caps for authority
   without needing memory access.
3. **Lazy map dissolves into explicit map:** Even deferred PTE installation
   requires the Observer to choose a virtual address.

These grounds are revisited below after the model is developed.

### The (cap, offset) model

Instead of mapping Spaces into a flat virtual address space, the Observer
addresses memory as (Space_cap, offset):

```text
Space01[0x300] = some_value;   // write byte at offset 0x300 of Space01
let x = Space02[0x000];        // read first byte of Space02
```

Each Space capability is its own namespace starting at offset 0. There is no
global virtual address space from the Observer's perspective. The Observer never
chooses a VA, never calls map(), never thinks about address layout between
Spaces. Holding the cap IS the authority; the offset identifies the byte.

This is the **capability-addressed memory** model — the same conceptual family
as IBM System/38 (1979), Multics segments (1965), EROS/KeyKOS page capabilities,
and CHERI capability hardware.

### Hardware bridge: ARM64 uses flat VAs

ARM64 instructions interpret register values as flat virtual addresses.
`ldr x0, [x1]` takes x1 as a VA and walks the page table. There is no
`ldr x0, [cap, #offset]` instruction (outside CHERI extensions, which are not
mainline ARM64).

The (cap, offset) model must bridge to flat VAs somewhere. Three approaches
evaluated:

**A. Cap index encoded in VA bits.** Upper VA bits = cap index, lower bits =
offset. The page table maps each cap's range to physical pages. Zero per-access
overhead. But a fixed bit partition (e.g., 9 bits for cap index, 39 for offset)
imposes hard limits: max 512 caps, max 512GB per Space. These limits are
workload assumptions — an **A3 violation** (no assumptions about the OS or
workload).

**B. Runtime base lookup.** The kernel assigns each Space cap a base VA when the
cap is granted. The Observer stores base VAs in a per-cap table. Access is
`base_of(cap) + offset` — one table load, one add. No cap count limit, no
per-Space size limit. A3-clean.

**C. Trap-and-emulate.** Page table empty, every access faults, kernel
translates. Correct but catastrophically slow.

**Approach B is the right choice.** Performance analysis:

- The base table is one u64 per Space cap. 20 caps = 160 bytes = two cache
  lines, hot in L1 for any active Space.
- Per-access cost: one load (L1 hit: ~1–4 cycles) + one add (~1 cycle).
- The pointer is computed once and reused — subsequent accesses through the same
  derived pointer are normal loads with no lookup.
- Worst-case workload (scatter across hundreds of Spaces, no reuse, tight loop):
  ~1–2 µs of overhead over thousands of iterations. Unmeasurable against memory
  latency (~100–200 cycles per DRAM access) and TLB costs.
- The eliminated map() syscall (~100–1000 cycles) makes the (cap, offset) model
  a likely **net performance win** over explicit mapping.

### Per-Observer segments with per-Space VA bases

Each Space cap held by an Observer gets a contiguous VA region. The Observer's
"address space" is the union of its Space cap VA regions — a **segment table**
derived from its cap holdings.

**Per-Space bases, not per-Observer.** The kernel assigns each Space a VA base
at creation time. The base is a property of the Space — all holders see the same
Space at the same VA. This was chosen over per-Observer bases for two reasons:

1. **Page table sharing.** If two Observers hold the same Space at the same VA
   base, their page table subtrees for that Space are identical. The kernel can
   share L1/L2/L3 subtrees (reference-counted), with only L0 tables
   per-Observer. For 100 Observers sharing 20 Spaces: ~640KB (shared subtrees)
   vs. ~24MB (per-Observer duplication). The difference is O(Observers + Spaces)
   vs. O(Observers × Spaces) in page table memory — significant for the
   many-threads pattern (D6) and embedded targets (A3).

2. **Pointer sharing.** With per-Space bases, a pointer into Space S is
   `base_of(S) + offset`, and `base_of(S)` is the same for all holders. Absolute
   VAs are consistent across Observers sharing the same Spaces. Cross-Observer
   pointer sharing within shared Spaces works without coordination.

This dissolves the Coordinate System (D10) as a separate kernel object type.
There is no shared address space to bind to. "Shared memory" is two Observers
holding caps to the same Space — the physical memory is shared, the VA base is
the same, and page table subtrees are shared as a kernel-internal optimization.

The page table becomes a kernel-internal mechanism: it materializes the
Observer's segment table into something the MMU understands. The Observer never
interacts with the page table directly.

**Layout control.** Each Space is a contiguous VA block. Layout decisions within
a Space (where code, data, structures go) are offset-based and entirely under
the Observer's control. Layout between Spaces (which Space is at which base VA)
is the kernel's decision and irrelevant to the Observer — it works with (cap,
offset), not absolute VAs.

**VA allocator.** The kernel allocates contiguous VA blocks per Space, not per
page and not per Observer. This is segment-level allocation — coarser and
simpler than page-level VA management. With 48-bit user VA (256TB) and
segment-granularity allocation, fragmentation is minimal. All Spaces in the
system share one global VA range (similar to a single address space OS), but
each Observer's page table contains entries only for Spaces it holds caps to.

### Revisiting journal 026's auto-map rejection

The three grounds for rejecting auto-map in journal 026:

1. **D10 cascade:** Auto-map in a shared Coordinate System makes mappings
   visible to capless co-located Observers. — **Dissolved.** There is no shared
   Coordinate System. Each Observer has its own segment table. An Observer's
   page table contains entries only for Spaces it holds caps to.

2. **Cap-without-mapping is useful:** Resource managers hold caps for authority
   without mapping. — **Survives in modified form.** In the (cap, offset) model,
   the kernel assigns a base VA and creates a base-table entry when a cap is
   granted, but does NOT populate page table entries until first access (demand
   faulting). A resource manager that never accesses the Space incurs only one
   u64 of base-table overhead — no page table memory, no TLB entries.
   Additionally, the cap's rights mask could exclude memory-access rights,
   preventing base-table entry creation entirely.

3. **Lazy map dissolves into explicit map** because the Observer must choose a
   VA. — **Dissolved.** The Observer does not choose VAs. The kernel assigns
   bases. The Observer accesses (cap, offset) and the kernel handles VA
   assignment internally.

Two of three grounds are dissolved by the model. The surviving ground (cap
without mapping) is handled through demand faulting and rights masks — no
separate map() operation needed.

### Cross-Observer pointer sharing

With per-Space VA bases, pointer sharing works naturally. Two Observers holding
the same Space cap see it at the same VA base. A pointer `base_of(S) + offset`
computed by Observer A is valid for Observer B if B also holds a cap to S.

If B does not hold a cap to S, the pointer dereferences into an unmapped region
— B faults, which is correct (no cap → no access). Pointer validity is exactly
cap validity. This is a stronger property than the D10 model, where a pointer
could be valid (mapped in a shared Coordinate System) even for an Observer
without a cap to the underlying Space.

---

## What this affects

### D10 — Address space is a first-class kernel object

D10 derived first-class address spaces from three independent paths:

- **A5 (mapping consistency):** Co-located Observers need consistent shared
  mappings without userspace coordination. — In the (cap, offset) model, each
  Observer's mappings are derived from its own caps. Consistency is per-Space:
  all holders of a Space cap access the same physical memory. No userspace
  coordination needed. A5 is satisfied differently.
- **D1 (TLB pressure):** Per-Observer ASIDs cause TLB duplication for
  same-address-space Observers. — Each Observer has its own page table
  (different cap sets = different entries). Shared Space caps produce identical
  VA→PA entries per-Observer. Future optimization: share page table subtrees for
  shared Spaces. TLB pressure is not worse than the current model.
- **D4 (independent delegation):** Address space access delegable independently
  of Observer access. — The Space cap IS the delegation unit. No separate
  address space capability needed.

All three paths are satisfied by the (cap, offset) model without a separate
Coordinate System object.

Per-Space VA bases with shared page table subtrees provide the D10 model's
memory efficiency as a kernel-internal optimization, without exposing the
Coordinate System as a user-visible kernel object type. The kernel shares
L1/L2/L3 subtrees for Spaces held by multiple Observers — the sharing is
automatic and invisible. D10 dissolves as a user-facing concept; its
optimization benefit is preserved internally.

### D24 — Cap-mapping invariant

D24's invariant (no cap → no mapping) is **strengthened**. Page table entries
exist only for Spaces the Observer holds caps to AND has accessed. There is no
separate map() operation that could create a mapping inconsistent with cap
holdings. The invariant is enforced structurally, not by API discipline.

### D25 — Page size exposure

Page size exposure survives but through a different interface. With no explicit
map() call, page size no longer appears in map() alignment arguments. Instead,
page size matters for:

- Space allocation: minimum Space size = page size (creating a 1-byte Space
  still consumes one page of physical memory)
- Space budget accounting: the Space budget charges in page-sized increments
- Fault granularity: demand faulting populates one page at a time

The decisive scenario from journal 026 (two 4KB Spaces on 16KB-page hardware)
still applies: the Observer needs to know page size to predict memory
consumption. The exposure mechanism changes (query for allocation, not alignment
for mapping).

### D6 — Observer is a single schedulable execution unit

D6's "one address space binding" changes. Instead of binding to one Coordinate
System, the Observer's address layout is derived from its Space cap holdings.
There is no explicit binding — the segment table is a materialized view of the
cap table's Space entries.

### D12 — Fault delegation to userspace pagers

Fault delegation still holds. When an Observer accesses a Space at an offset
whose page table entry is not yet populated, the kernel delivers a fault to the
designated pager Observer. The pager provides the physical backing. The fault
message carries the Space identity and offset rather than a bare VA — giving the
pager more semantic information than the current model.

---

## What this does NOT settle

- **Base table management.** Who owns the base table? Kernel-maintained
  read-only page mapped into Observer (like Linux VDSO)? Or Observer-managed
  with kernel-provided bases on cap grant? Security properties differ.
- **Cap rights for memory access.** Should a Space cap's rights mask include a
  "memory access" right separate from other rights (transfer, destroy,
  attenuate)? This determines whether a resource manager holding a Space cap
  gets a base table entry.
- **Demand fault vs. eager population.** Should the kernel populate page table
  entries eagerly on cap acquisition (for small Spaces) or always demand-fault?
  Tradeoff between first-access latency and unused-mapping waste.
- **Interaction with D9/D25 page-size interface.** The Space allocation
  interface needs to communicate page granularity — the Observer should know
  that a 100-byte Space allocation actually consumes one page. Interface shape
  deferred.
- **Impact on Observer minimum schema.** The Observer struct may need a segment
  table pointer (or the segment table may be derived from the cap table at fault
  time, requiring no additional per-Observer state).
- **Page table memory budget.** With per-Space VA bases, page table subtrees are
  shared. Whose budget pays for the shared subtrees? The Space creator? Split
  across holders? This is the D10 budget question in new form.
- **VA base reclamation.** A Space's VA base is assigned at creation and
  persists for the Space's lifetime. After the Space is destroyed, the VA range
  is reclaimable. Long-lived systems with high Space churn could fragment the
  global VA range. 48-bit VA (256TB) provides substantial headroom, but the
  reclamation policy needs design.
- **Vocabulary revision.** "Coordinate System" may become an implementation
  concept (page table configuration) or be retired. The substance/framework
  split in the vocabulary simplifies if all user-visible kernel objects are
  substance-shaped (Space, Time, Observer, Endpoint).

---

## Status

**Exploratory — approaching decision readiness.** The (cap, offset) model with
runtime base lookup and per-Space VA bases is a coherent alternative to explicit
mapping. It dissolves the map/unmap asymmetry, dissolves D10 as a user-visible
concept (preserving its optimization benefit as kernel-internal page table
sharing), strengthens D24 to a structural property, and respects A3.

The model affects D5, D6, D9, D10, D12, D24, and D25. Of these, D10 is the
largest change (dissolution of a kernel object type). The others require
language updates and interface adjustments, not structural rederivation.

Formal derivation is needed to settle this as a decision. The exploration has
identified no blocking issues.
