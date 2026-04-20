# 026 — Page size is exposed to userspace

**Date:** 2026-04-19. **Starting point:** Open question in spec.md: "expose page
granularity to userspace (proven, universal) or hide it behind byte-addressed
objects (archive's novel position, no precedent in surveyed systems)?" This is
D9's highest-level unsettled downstream question. D24 (cap-mapping invariant)
added new pressure via the sub-page packing concern.

---

## The question

Should the memory object interface expose page granularity to userspace
(page-addressed) or hide it behind byte-addressed objects?

---

## Exploration path

### Detour: auto-map vs. explicit map

The exploration initially paused to resolve an upstream question: should the
kernel automatically map memory when an Observer acquires a cap (auto-map), or
is map() a separate explicit operation? If auto-map, page size would never
appear in any syscall, dissolving the page-size question entirely.

Auto-map was rejected on structural grounds:

1. **D10 cascade:** Auto-map in a shared Coordinate System creates mappings
   visible to co-located Observers without caps — contradicting D24's own "no
   cap → no mapping" invariant.
2. **Cap-without-mapping is genuinely useful.** Resource managers hold caps for
   authority (destroy, transfer) without mapping into their Coordinate System.
   Cap routers pass through caps without accessing memory. Cold storage avoids
   unnecessary page-table entries.
3. **Fault-driven map dissolved into explicit map.** A lazy approach still
   requires the Observer to associate a memory object with a virtual address —
   which IS map(), just with deferred PTE installation.

D24's "map is explicit" was reaffirmed. Map() exists as a userspace-facing
operation. The page-size question remains.

### Three positions evaluated

**A. Full exposure (seL4 model).** All operations require page-aligned inputs.
PAGE_SIZE is a constant. Universal landscape precedent.

**B. Implicit exposure (Zircon model).** Operations accept byte values, kernel
rounds to page granularity internally. PAGE_SIZE is queryable but not required
for basic operations. Production precedent (Zircon, approximately Genode).

**C. Full hiding (archive's novel position).** No PAGE_SIZE concept in the
interface. Byte-addressed throughout. No precedent in any surveyed system.

### The axiom pressure

Four axioms/settled decisions push toward hiding (C):

- A2: page size is architecture-specific; A2 says those live behind interfaces
- A3: exposing creates hardware-dependent ABI (4K vs 16K vs 64K)
- A5: page management complexity pushed to userspace under exposure
- D5 CHERI note: page-table-specific interface tension

One settled decision pushes toward exposure (A/B):

- D24: auto-unmap at page granularity; sub-page packing is load-bearing

### The decisive scenario

The axiom pressure appeared to favor hiding — until a concrete scenario
demonstrated that page-size hiding creates structural failures:

**Setup:** Two separate 4KB memory objects (M1, M2). Page size is 16KB. Observer
requests map(M1, X) and map(M2, X+4KB).

The kernel cannot satisfy this request. One PTE covers 16KB; two separate memory
objects with separate physical backing cannot occupy the same 16KB virtual page.
The kernel's options:

1. **Reject:** Observer gets an error it can't understand — the addresses don't
   overlap from its byte-addressed perspective, but they share a page.
2. **Move M2:** Map M2 at the next page boundary (X+16KB). A 12KB gap appears.
   The same code on 4KB hardware has no gap. Can fail if address space is nearly
   full — hardware-dependent failure.
3. **Sub-page packing:** Put M1 and M2 in one physical page. If M1 is shared
   with another Observer via cap transfer, that Observer can access M2's bytes —
   **D4 violation** (access without authority) and **D24 violation** (access
   without cap). Security failure.
4. **One page per object:** Each 4KB object gets a 16KB page. 24KB wasted for
   8KB of content. The Space budget charges 16KB for a 4KB allocation —
   dishonest accounting or unpredictable budget across hardware. Can fail under
   physical memory pressure — hardware-dependent failure.

Every option either leaks page-level information through failures (1, 2, 4),
creates security violations (3), or produces hardware-dependent behavior (2, 4).
The scenario is not exotic — two small objects mapped adjacently is a completely
normal pattern.

### The O4 answer

Page-size knowledge is essential complexity, not accidental. Hiding it does not
eliminate it — it converts predictable constraints ("your map address must be
16KB-aligned") into unpredictable failures ("your map worked on this hardware
but not that one"). The Observer is better served by knowing the constraint than
by encountering its symptoms.

This resolves the A5 tension: A5 says the kernel absorbs complexity. But O4 says
essential complexity cannot be eliminated by moving it — only accidental
complexity can be shed. Page-size knowledge is essential for correct address-
space layout. Trying to hide it creates worse outcomes than exposing it. A5 does
not apply to essential complexity that userspace genuinely needs.

### Revisiting the axiom tensions under exposure

With hiding (C) eliminated, the A2/A3/D5 tensions need re-examination under
exposure:

- **A2 (architecture-specific):** Page size IS architecture-specific. But it's a
  queryable runtime constant, not a compile-time assumption. A2 says
  architecture details "live behind trait interfaces" — page size behind a query
  syscall satisfies this. The detail is accessible, not hard-coded.
- **A3 (hardware portability):** Code that queries page size and aligns
  accordingly IS portable across 4K/16K/64K hardware. Code that hard-codes 4096
  is not. The exposure model encourages the former.
- **D5 CHERI note:** Exposing page size is a page-table-specific concept in the
  interface. This tension is real but bounded: on CHERI hardware, the query
  would return the CHERI capability alignment granularity instead of the MMU
  page size. The interface shape (query + align) survives; the value changes.

### What this settles and what it defers

**Settled:** Page size is exposed to userspace. Observers can query page size
and must account for page granularity in memory operations. Full hiding (byte-
addressed with no page concept) is rejected.

**Deferred (one level down):** Whether the interface is fully page-addressed
(Option A — all operations require page-aligned inputs) or implicitly rounded
(Option B — operations accept byte values, kernel rounds, PAGE_SIZE queryable
for Observers that want to optimize). The user's gut favors B (implicit
rounding), but this is one level down from the exposure/hiding decision.

### Convergence check

The archive took the byte-addressed (hiding) position. This derivation rejects
that position on the basis of the adjacent-objects scenario — a failure mode the
archive's derivation did not examine. The rejection is based on D4 (security
violation under sub-page packing) and D24 (auto-unmap tension), neither of which
existed in the archive's derivation context. The archive's axiom-pressure
analysis (A2, A3, A5 favoring hiding) was accurate but incomplete — the decisive
scenario reveals that hiding creates worse problems than it solves.

---

## Decision

**Page size is exposed to userspace.** Observers can query the page size and
must account for page granularity. Full hiding is rejected — it converts
predictable constraints into unpredictable, hardware-dependent failures and
creates security violations under sub-page packing.
