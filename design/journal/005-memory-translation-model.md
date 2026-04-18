# 005 — Memory translation model

**Question:** Does the kernel require MMU-backed virtual memory, or should the
design accommodate physical-only addressing, tagged pointers, or other
translation models?

**Answer:** The kernel requires MMU-backed virtual memory with per-Observer
address spaces. This is a derived consequence — every alternative is foreclosed
by axioms or hardware facts, not by preference.

---

## Prior work

The archive (restart-1) stated the MMU as a requirement without deriving it:
"The kernel requires an MMU for memory isolation"
(`archive/restart-1/spec.md:62`). The current chain reopened this as an open
question to derive it from first principles.

No journal entry in either chain had previously explored this question. The
landscape (§2, §6.6, §7.4) covers memory management models across 18+ systems;
all surveyed systems with hardware isolation use MMU-backed virtual memory. The
two exceptions — Singularity (language-safety isolation) and CHERI/Morello
(hardware-tagged capabilities) — are analyzed below.

---

## Derivation

Three independent paths converge on the same answer.

### Path 1: A2 hardware fact — MMU must be enabled

On ARMv8-A, disabling the MMU (`SCTLR_EL1.M = 0`) forces all memory accesses to
Device-nGnRnE attributes: uncached, non-gathering, non-reordering. Main memory
latency is ~50-100+ ns per access vs. ~1-4 ns for an L1 cache hit — roughly
50-200x slower. This is not a tunable tradeoff; it is how the hardware works.

The MMU performs two jobs with one mechanism: (1) virtual-to-physical address
translation and (2) memory attribute assignment (cacheability, permissions,
access flags). The caching system depends on attributes assigned by page table
entries. You cannot have caching without attributes, and you cannot have
attributes without page tables.

Therefore: page tables must exist and the MMU must be enabled. The remaining
question is what the kernel does with the translation capability the MMU
provides.

### Path 2: A3 + A5 — hardware-enforced inter-Observer isolation

A3 (generic kernel) means Observers may execute arbitrary, untrusted,
potentially adversarial code. There are no workload assumptions that constrain
what runs in an Observer. Inter-Observer isolation — preventing one Observer
from accessing another's memory — is mandatory.

A5 (kernel is leaf node) means the kernel absorbs isolation complexity rather
than pushing it to userspace or constraining what code is allowed to run.

On ARM64, the available isolation mechanisms are:

1. **MMU page tables.** Per-Observer page tables map only authorized physical
   memory. The MMU enforces access permissions on every memory access. This is
   the standard mechanism, available on all ARM64 hardware.

2. **Language-safety isolation (Singularity model).** Software Isolated
   Processes (SIPs) in a single shared address space, relying on a verified
   language runtime to prevent cross-boundary access. **Foreclosed by A3:**
   requiring all code to be in a verified language is a workload assumption. A
   generic kernel cannot dictate what language Observers run.

3. **CHERI hardware capabilities.** 128-bit tagged pointers with hardware-
   enforced bounds and permissions, enabling compartmentalization without MMU
   switching. **Foreclosed by A2:** current ARM64 silicon does not include
   CHERI. The CHERI Alliance signals future intent, but A2 names the target
   hardware as it exists.

4. **Software Fault Isolation (SFI).** Code instrumentation at load time to
   enforce memory bounds. **Foreclosed by philosophy + A3:** software
   reimplementation of a hardware guarantee is always weaker (philosophy: "use
   what the hardware provides"). Also requires trusting the instrumentor and
   constraining loadable code formats, which tensions with A3.

Only option 1 is available and unconstrained. Per-Observer page tables are the
mechanism.

### Path 3: Philosophy — use what the hardware provides

"When hardware already enforces a property, the kernel programs that mechanism —
it doesn't reimplement the enforcement in software." The ARM64 MMU provides
exactly the property needed: per-access permission enforcement based on the
current page table. The kernel's job is to program the page tables correctly for
each Observer, not to build a parallel enforcement system.

### Convergence

Each path is independently sufficient:

- Path 1 alone: MMU must be enabled, so page tables exist and translation
  occurs.
- Path 2 alone: isolation requires per-Observer page tables.
- Path 3 alone: hardware provides the mechanism; use it.

Together they leave no design space for alternatives. The archive reached the
same conclusion (convergence confirmed), though without deriving it.

---

## Costs

These are accepted costs, not arguments against the derivation:

- **TTBR switch on every Observer transition (D1 hot path).** Writing TTBR_EL1
  requires an `isb` barrier. ARM64 ASIDs (8 or 16 bits, implementation-
  dependent) allow TLB entries to survive TTBR switches, mitigating the flush
  cost. ASID management (allocation, rollover) is added kernel complexity.

- **Cross-core TLB shootdown on unmap (O2).** When a page is unmapped from a
  Observer that may be running on another core, stale TLB entries must be
  invalidated via IPI + TLBI instruction + DSB barrier. This is cold-path
  (consistent with D1) but expensive when it occurs.

- **Page table management complexity.** Multi-level page tables (up to 4 levels
  on ARM64) are substantial data structures. Under A5, this complexity belongs
  kernel-side. Under O4, it is essential complexity — isolation requires it. Per
  the "push complexity to the leaves" philosophy, page table management is
  contained inside the Space manager (D3), behind its single interface.

---

## What this does NOT settle

- **Address space structure per Observer.** Whether each Observer has a fully
  independent page table tree, or Observers can share structure (e.g., shared
  upper-level tables). One level down.

- **Page size exposure.** Whether the kernel exposes page granularity to
  userspace (universal among surveyed systems) or hides it behind byte-addressed
  objects (archive's novel position). One level down.

- **Memory object model.** What capability-designated (D4) memory resources look
  like: seL4-style typed frames, Zircon-style VMOs, or something shaped by the
  Space vocabulary. One level down.

- **Fault delegation.** Whether the kernel resolves page faults internally or
  forwards them to userspace pager Observers. Interacts with A4 (reactive) and
  A5 (complexity placement). One level down.

- **CHERI forward-compatibility.** CHERI is foreclosed as a replacement for MMU
  isolation (A2 — hardware doesn't exist), but not as a future complement. CHERI
  would provide finer-grained isolation _within_ an Observer's address space
  (the CheriBSD "co-processes" model: multiple compartments sharing one address
  space, separated by CHERI tags instead of MMU, with 1-2 orders of magnitude
  faster switching). The memory interface should be shaped to not foreclose
  this: design around memory objects and permissions rather than
  page-table-specific concepts, so that CHERI-as-additional-enforcement-layer
  can slot in without interface changes.

---

## Rejected alternatives (summary)

| Alternative                 | Foreclosed by   | Reason                                  |
| --------------------------- | --------------- | --------------------------------------- |
| Physical-only (MMU off)     | A2 hardware     | Uncached memory — non-viable            |
| Language-safety isolation   | A3              | Workload assumption (verified language) |
| CHERI-only                  | A2              | Hardware not present in target          |
| Software Fault Isolation    | Philosophy + A3 | Weaker than hardware; constrains code   |
| Single shared address space | A3 + A5         | No inter-Observer isolation mechanism   |

---

## Axioms not load-bearing here

A1 (Rust) is not load-bearing. Rust's ownership model is compatible with any
translation approach — it does not push toward or away from virtual memory. The
derivation rests on hardware facts (A2), genericity (A3), complexity placement
(A5), and per-core structure (D1). A1 will become relevant one level down when
the page table implementation interacts with Rust's type system, but that is not
this entry's concern.

A4 (purely reactive) is not directly load-bearing for the top-level question. It
becomes relevant through implication (demand paging fits the reactive model
naturally), but the derivation does not pass through A4. A4's role is
downstream: it shapes fault handling, not whether translation exists.
