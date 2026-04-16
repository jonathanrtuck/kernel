# Kernel Design Specification

The current state of the kernel's design. Settled decisions with brief
rationale. See `design/graph.d2` for the structural map and `design/journal/`
for full exploration history.

This document is intentionally sparse. It was reset on 2026-04-15 to re-derive
contingent decisions from first principles. The previous derivation chain is
preserved under `design/archive/restart-1/` for convergence-checking — consult
it only after a fresh derivation has arrived at an answer.

---

## Project stance

These are not design inputs — they do not appear under any derivation's "Rests
on" line and do not filter options inside derivations. Listed once so they do
not need to be re-litigated.

- **Public domain.** The kernel source and every design artifact in this
  repository — `spec.md`, `journal/`, `graph.d2`, research documents, all of it
  — are dedicated to the public domain via `UNLICENSE`. The intended
  contribution is the derivation trail alongside the code: the reasoning that
  produced each decision, in the commons, so others can fork from any point and
  continue. This is a values commitment, not a strategic move toward novelty or
  adoption. It is a property of the artifact, not a lever on the logic that
  produces it.

---

## Axioms

These are design inputs, not decisions. They constrain everything that follows.
Labeled for reference from derivation entries' "Rests on" lines.

- **A1 — Rust (nightly, no_std).** Not a language preference — a design input.
  Ownership maps to resource lifecycle. Traits map to architecture abstraction.
  Unsafe boundaries map to trust boundaries.

- **A2 — ARM64 target.** Generic timer, GIC, EL0/EL1. The codebase is structured
  for portability (`src/arch/`); architecture-specific details live behind trait
  interfaces and do not shape the design.

- **A3 — The kernel is generic.** No assumptions about the OS or workload.
  Personal devices, servers, embedded — all viable. Workload-specific policy
  belongs in userspace.

- **A4 — The kernel is purely reactive.** The kernel runs only in response to
  hardware exceptions: syscalls, faults, interrupts, timer events. There is no
  kernel thread, no event loop, no polling. The exception vector is the sole
  entry point.

- **A5 — The kernel is a leaf node.** Complexity placement principle: the kernel
  presents a simple interface and absorbs complexity behind it, rather than
  exposing primitives that force complexity into userspace. This is a design
  input — it determines which side of the kernel|userspace boundary a concern
  belongs on whenever the question arises.

---

## Vocabulary

The language used to think about the kernel's concerns. Vocabulary is neither
true nor false — only useful or not. Derivation entries do not list vocabulary
under "Rests on"; it exists so later sections can be precise.

- **Space.** A claim to a portion of the system's bounded memory resource. Space
  is bounded as a quantity: at any instant, the system has a finite pool of
  memory from which Space claims are allocated. Space is not cumulative (a claim
  is a snapshot, not an accumulation) and not fungible once allocated (a
  specific claim binds to specific addresses).

- **Time.** A claim to a portion of a specific logical core's scheduling time.
  Compute flows without a finite pool, but the _rate_ of scheduling time per
  logical core is bounded at 100%. A Time claim is a fraction of a specific
  logical core's scheduling allocation (e.g., 10% of logical core 0). Time is
  cumulative over wall-clock (scheduling time consumed accumulates) and fungible
  within a logical core. Fungibility across cores is weaker — moving Time across
  cores has structural cost, handled as migration. On hardware without SMT,
  scheduling time and delivered compute are equivalent. On SMT hardware,
  delivered compute additionally depends on sibling logical-core contention for
  shared pipeline resources; the kernel guarantees scheduling allocation, not
  physical-compute-rate delivery.

- **Frame.** Coupled Space and Time: the condition under which compute (Time)
  executes instructions within specific memory (Space). Borrowed from physics'
  reference frame — each Frame is an independent coordinate system in which a
  specific computation unfolds. A Frame correlates one or more Spaces but
  exactly one Time. SMT-concurrent workloads, when hardware supports them, are
  expressed as multiple Frames sharing a Space, each with its own Time on its
  own logical core — not as a single Frame with multiple Times. This keeps the
  one-Time commitment intact across SMT and non-SMT hardware.

_Naming note:_ these terms are for internal thinking and will not necessarily
appear in public API names. Public naming is deferred until v0.1.

---

## Foundational Observations

Facts and consequences that shape downstream decisions but are not themselves
derived choices. Derivation entries may cite these under "Rests on" when the
observation is load-bearing.

- **O1 — Three output types.** Every kernel invocation produces some combination
  of: (1) updated kernel state, (2) a message delivered to a Frame, (3) a choice
  of which Frame to resume. Descriptive summary of what the kernel does; not an
  exhaustiveness claim with axiom strength. If a future invocation appears to
  need a fourth output type, that is a signal to examine the kernel's role
  definition — not to contort the new mechanism to fit the three.

- **O2 — Cross-core coordination requires IPIs.** A4 applied to A2 has a
  hardware consequence: the only mechanism to wake another core's kernel is an
  inter-processor interrupt (SGI on ARM64). A core that needs another core to
  run kernel code must send an IPI — there is no mailbox a kernel thread could
  poll, because no kernel thread exists.

- **O3 — Exceptions are taken on the causing core.** Hardware fact on A2:
  synchronous exceptions (syscalls, faults) are delivered to the core that
  caused them; device interrupts are delivered to the targeted core. The
  software handler runs on that core unless explicitly forwarded.

- **O4 — Simplification moves under A5.** Essential complexity is conserved — it
  can only be moved, not eliminated. Once the kernel is at its designed minimum
  under A1–A5, any proposed "simplification" of the kernel must fit one of three
  categories: (a) moving essential complexity to userspace — **violates A5,
  rejected**; (b) moving essential complexity to hardware — **allowed** if the
  hardware provides the suitable primitive (A5 concerns the kernel|userspace
  boundary, not the kernel|hardware boundary); (c) shedding accidental
  complexity that prior design was carrying without axiomatic justification —
  **always allowed**. A proposal that fits none of these is an A5 violation
  wearing the badge of "simpler is better." This observation gives derivation
  entries a citable target when rejecting such proposals.

---

## Derivations

Throughout derivations, "core" means _logical core_. On SMT hardware each
physical core presents multiple logical cores, and each logical core has its own
Core manager and Time manager. On non-SMT hardware (current A2 target), logical
and physical cores are one-to-one. Concerns that are genuinely per-physical-core
(power state, thermal, shared cache/TLB) belong to components not yet derived
and are orthogonal to D1/D2.

### D1 — Per-core hot path, shared cold path

Each hardware core has its own kernel-side structure for handling high-frequency
work: exception entry, state update, selecting the next Frame to resume,
resumption. This structure touches no cross-core shared state on the hot path.
Infrequent cross-core concerns — Frame migration, cross-core message delivery,
shared resource allocation — route through an explicitly shared cold path.

- **Rests on:** A4 (no kernel thread means per-core exception handlers are the
  only way to respond to per-core exceptions), A5 (cross-core coordination
  complexity belongs kernel-side; the Barrelfish alternative pushes it into
  userspace), A2 (cache-coherent ARM64 makes shared cold-path reads cheap), O2,
  O3, `design/landscape.md` (seL4 big-kernel-lock rejected for ~23% ARM overhead
  from barrier instructions alone; Barrelfish full per-core rejected for
  userspace-coordination burden).
- **Status:** tentative — accepted to enable further derivation.
- **Journal:** `journal/001-per-core-hot-path.md`.

### D2 — Per-core schedulers may run different algorithms

The scheduler that selects which Frame resumes on a core is per-core (direct
consequence of D1). Additionally, each core's scheduler may run a _different_
algorithm — throughput-oriented on a big core, simple fixed-priority on a LITTLE
core, deadline-based on a core dedicated to real-time Frames. The Frame model
carries only abstract scheduling properties (priority, CPU/IO classification,
optional deadline); algorithm-specific state (e.g., CFS virtual runtime,
deadline parameters) lives per-core in the scheduler, not in the Frame. On
migration, abstract properties transfer; algorithm-specific state is re-derived
by the destination scheduler.

- **Rests on:** D1, A2 (big.LITTLE asymmetric cores are within target hardware),
  A3 (a generic kernel cannot mandate one scheduling algorithm as the right
  answer), `design/landscape.md` (no surveyed system cleanly separates per-core
  scheduler algorithms as a first-class feature — novel position).
- **Status:** settled — revisit when the minimum abstract-property set on the
  Frame proves unexpressible across the candidate scheduling-algorithm space.
- **Journal:** `journal/002-per-core-schedulers.md`.

### D3 — One logical Space manager

The kernel exposes a single interface for physical memory management to its
other components: one place to allocate, free, and account. The _implementation_
of that interface — whether a single global allocator, per-NUMA-node allocators
with cross-node fallback, per-core caches backed by a global reserve, or some
other structure — is a leaf-node concern inside the Space manager, swappable
without affecting the rest of the kernel.

Explicitly does NOT commit to: shared global allocator, per-CPU caches,
NUMA-partitioning, or any other specific internal strategy. Those are leaf-node
decisions recorded where the implementation is chosen.

- **Rests on:** A2 (ARM64 target covers NUMA hardware — committing the skeleton
  to non-NUMA would foreclose supported configurations), A3 (generic across
  hardware topologies — we cannot assume away NUMA by workload convention), D1
  (allocation is cold-path; interface-level simplicity is not paid for on the
  hot path). The structural argument that topology-specific complexity belongs
  behind an interface in a leaf rather than in the kernel skeleton applies the
  "push complexity to the leaves" principle fractally within the kernel's
  internal tree; per the axiom/philosophy split above, it is named inline in the
  journal rather than listed as an axiom-level predecessor.
- **Status:** settled — revisit when a workload makes allocation hot-path
  (violating the D1-cold assumption) OR if the single-interface commitment
  itself starts costing.
- **Journal:** `journal/003-space-manager-interface.md`.

### D4 — Capability-based authority

A Frame proves it is allowed to perform an operation by presenting an
unforgeable, per-Frame handle (capability) that designates the resource AND
carries the permitted operations. The kernel resolves the handle, checks the
rights, and proceeds or rejects. No identity lookup, no global namespace, no
ambient privilege. Two independent derivation paths converge: (1) A5 forecloses
any interface separating designation from authority (confused deputy); (2) D1
forecloses per-resource authority data on the hot path (ACLs are shared mutable
state). The archived chain reached the same conclusion from a third path.

Does NOT settle: scope of capability mediation (everything vs. resources-only),
capability table structure (kernel-managed vs. CNode-style), or revocation model
(refcount, destroy, CDT, generation numbers). These are one level down.

- **Rests on:** A5 (confused deputy forces authority-tracking complexity into
  userspace — an A5 violation; capabilities are the only model where designation
  = authority), D1 + O3 (hot-path authority checks must use per-core data;
  per-Frame capability tables are per-core; per-resource ACLs are shared mutable
  state), A4 (no background authority management; capability refcount fits
  explicit-trigger model), A3 (no identity requirement — capabilities work
  across all workloads without assuming an identity scheme),
  `design/landscape.md` §1.2 (confused deputy: Hardy 1988, Miller's
  formalization, "Capability Myths Demolished" 2003).
- **Status:** settled — revisit only if A5 AND D1 are both revised
  simultaneously (either alone leaves at least one derivation path intact).
- **Journal:** `journal/004-capability-based-authority.md`.

### D5 — MMU-backed virtual memory with per-Frame address spaces

The kernel requires the ARM64 MMU to be enabled and uses it for inter-Frame
memory isolation. Each Frame has its own address space (page table tree); the
MMU enforces that a Frame can only access physical memory mapped into its page
tables. Three independent paths converge: (1) A2 hardware requires MMU enabled
for cached memory access — page tables must exist; (2) A3 + A5 require
hardware-enforced inter-Frame isolation, and the MMU is the only such mechanism
on ARM64; (3) philosophy "use what the hardware provides." Every alternative
(physical-only, language-safety isolation, CHERI-only, SFI) is foreclosed by
axioms or hardware facts.

Does NOT settle: address space structure sharing between Frames, page size
exposure vs. hiding, memory object model (what capabilities designate as
memory), fault delegation (kernel-internal vs. userspace pager), or CHERI
forward-compatibility. These are one level down. The memory interface should be
shaped around objects and permissions, not page-table-specific concepts, to
avoid foreclosing CHERI as a future complementary enforcement layer.

- **Rests on:** A2 (MMU must be enabled for cached operation — hardware fact,
  not design choice), A3 + A5 (generic workloads require hardware isolation;
  kernel absorbs that complexity), D1 (TTBR switching is hot-path cost,
  accepted; TLB shootdown is cold-path, consistent with hot/cold split),
  `design/landscape.md` §2 (all surveyed systems with hardware isolation use
  MMU-backed virtual memory), `design/philosophy.md` "use what the hardware
  provides."
- **Status:** settled — revisit only if A2 changes to include non-MMU isolation
  hardware (e.g., CHERI in silicon), which would open the question of whether
  MMU remains the sole enforcement mechanism or becomes one of two.
- **Journal:** `journal/005-memory-translation-model.md`.

### Entry template

Each derivation entry names three things: what rests on what, how settled the
entry is, and where to find the reasoning. Format:

> **Name.** One-sentence statement of what was derived.
>
> - **Rests on:** the load-bearing predecessors — axiom labels (A1, A2, …),
>   prior derivation names, and any `design/research/` docs that directly shaped
>   the derivation. Only entries the reasoning _actually invokes_, not every
>   entry that might be related. Completeness is not the goal; honesty is. If a
>   predecessor moves, this entry must be revisited.
> - **Status:** `tentative` (accepted to enable downstream exploration, may
>   move), `settled` (reasoning reviewed, revisit only on explicit trigger), or
>   `settled — revisit when X` (settled now but with a named trigger to reopen).
> - **Journal:** link to the numbered journal entry containing the full
>   reasoning. Spec entries state the _conclusion_; journals carry the
>   _argument_.

No confidence numbers. Numeric scores in this kind of work turn into vibes
within a session and then start being treated as load-bearing. Qualitative
language above is the substitute.

### Relationship to philosophy

`design/philosophy.md` is not in the axioms list and is not a predecessor listed
under "Rests on." Axioms are _what we derive from_; philosophy provides
_strategies for how to derive_. When a journal entry applies a philosophy
principle to make a derivation move, it should name that principle ("applying
'push complexity to the leaves' here…") so the principle's role is visible
without collapsing it into the dependency graph.

### Disclaiming non-load-bearing axioms

When an axiom feels relevant to a derivation but is not actually load-bearing —
the reasoning doesn't pass through it, or the question it answers is settled
elsewhere — disclaim it explicitly in the journal rather than omitting it
silently. Pattern: _"A<n> is not load-bearing here. A<n> answers [X]; this entry
answers [Y]. The work is done by [actual predecessors] alone."_

Silent omission invites a future reader — including the designer returning after
a gap — to reach for the axiom reflexively and either miscredit it or extend the
derivation on the assumption that the axiom constrains something it does not.
This is especially likely where axiom vocabulary overlaps with a philosophy
principle: A5 "leaf node" vs. the fractal "push complexity to the leaves"
principle share a word but differ in scope, and the overlap is where miscitation
slips in.

### Template revisit

This template itself is tentative. After 3-5 entries have landed under it,
review whether the shape fits what actually needs to be captured. Adjust if not.

---

## Open questions

- **Capability table structure.** D4 settles capability-based authority; the
  per-Frame capability table is the hot-path authority structure
  (D1-compatible). Open: kernel-managed opaque handle table (Zircon) vs.
  capability to a table object (seL4 CNode). Determines who controls the
  authority-space structure and how table sizing/growth works.
- **Time migration across cores.** When a Frame migrates to a less-loaded core,
  does its Time allocation transfer, or is it re-allocated on the destination?
  Affects D2's migration story.
- **Minimum abstract scheduling properties on a Frame.** D2 says Frames carry
  abstract scheduling properties, but the minimum set (priority? deadline?
  IO-bound flag? period?) is not fixed.
- **Frame-Space cardinality formalization.** The Vocabulary section describes
  Frames as correlating "one or more Spaces." D5 grounds this concretely: each
  Frame has its own virtual address space. Open: can Frames share address space
  structure (shared page table subtrees)? Is one-to-many a property of Frames or
  a separate decision?
- **Scope of capability mediation.** D4 settles capabilities as the authority
  model. Open: everything through capability invocation (seL4/EROS — universal
  invoke, capability type determines operation) vs. resources through
  capabilities with operations as direct syscalls (Zircon-style). Shapes syscall
  surface and composability.
- **Revocation model.** Close-only (refcount), authoritative destroy, derivation
  tracking (seL4 CDT), or generation numbers. Each has different cost profiles
  under D1 (hot/cold split) and O2 (cross-core IPIs). Interacts with capability
  table structure.
- **What unit runs in a Frame.** Thread, process, capability-holder, actor — a
  name and shape are needed before scheduling and isolation can be fully
  specified. Needs its own derivation.
- **Interrupt model (device interrupts, not exceptions).** Who owns device
  interrupts? Per-core or routed? Kernel-handled or delegated to userspace
  drivers?
- **Page size exposure.** D5 settles MMU-backed virtual memory. Open: expose
  page granularity to userspace (proven, universal) or hide it behind
  byte-addressed objects (archive's novel position, no precedent in surveyed
  systems)? Determines the Space manager's external interface granularity.
- **Memory object model.** D5 settles virtual memory; D4 settles capabilities.
  Open: what is the capability-designated memory resource? seL4-style typed
  frames, Zircon-style VMOs, or something new shaped by the Space vocabulary?
  Determines create/map separation.
- **Fault delegation model.** D5 means page faults occur (MMU generates them on
  unmapped access). Open: kernel resolves faults internally, or forwards to
  userspace pager Frames? Interacts with A4 (reactive), A5 (complexity
  placement).
- **Boot / bring-up model.** BSP-then-APs vs symmetric bring-up. Touches A2 but
  not derived.

---

## Journal index

- `001-per-core-hot-path.md` — reasoning for D1: hot/cold split, IPI
  coordination, landscape check vs seL4 BKL and Barrelfish multikernel.
- `002-per-core-schedulers.md` — reasoning for D2: per-core algorithms, abstract
  vs algorithm-specific properties on the Frame, migration implications.
- `003-space-manager-interface.md` — reasoning for D3: single interface
  commitment with topology-aware implementation as leaf node; why the archive's
  "small cache-coherent SoC" framing was not load-bearing for the allocator
  decision.
- `004-capability-based-authority.md` — reasoning for D4: two independent paths
  (A5 + confused deputy; D1 + hot-path data organization) converge on
  capabilities; ambient and pure ACLs foreclosed; archive convergence.
- `005-memory-translation-model.md` — reasoning for D5: three independent paths
  (A2 hardware requires MMU; A3+A5 require hardware isolation; philosophy) all
  converge on MMU-backed virtual memory; all alternatives foreclosed by axioms
  or hardware facts; CHERI forward-compatibility noted.

---

## Research

See `design/research/` for descriptive prior-art studies and
`design/landscape.md` for the survey of how other kernels resolved each major
design decision.
