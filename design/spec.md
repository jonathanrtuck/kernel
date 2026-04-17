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
  specific claim has object identity — it is not interchangeable with a
  different claim of the same size). Which physical pages back a claim is a
  kernel-internal concern.

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

### D6 — A Frame is a single schedulable execution unit

A Frame is a single schedulable execution unit: one register state, one program
counter, one Time, one capability table, one address space binding. The kernel
has no "process" concept — "process" is a userspace convention (a group of
Frames sharing a Space). Multi-threaded execution in shared memory is multiple
Frames sharing a Space, each with its own Time. Green threads and cooperative
concurrency are internal to a Frame (userspace, invisible to kernel).

The kernel provides no Frame-grouping mechanism. Grouping is neither essential
complexity (D4 capabilities handle Frame lifecycle without the target's
cooperation) nor workload-universal (A3 — not all workloads need groups).
Userspace builds grouping policy from capabilities; the kernel provides the
mechanism.

Does NOT settle: Frame minimum schema (concrete fields need formal derivation),
Frame-Space binding model (when/how binding occurs), Frame lifecycle operations
(create, destroy, suspend, resume), whether Frames can share capability tables,
or capability table structure.

- **Rests on:** Frame vocabulary (one Time per Frame; SMT paragraph explicitly
  models concurrency as multi-Frame), D2 (scheduler selects Frames — one-level
  selection), D4 (per-Frame capability table; destroy capability works without
  target cooperation), A3 (generic — no workload assumes or requires
  kernel-level grouping), `design/landscape.md` §4.4, §6.1 (seL4 validates
  no-kernel-process; all surveyed systems schedule thread-level entities).
- **Status:** settled — revisit if a downstream derivation (Frame lifecycle)
  reveals that the absence of kernel grouping forces essential complexity into
  userspace that capabilities alone cannot cover. (D8 settled capability table
  structure with per-Frame tables; D10 settled first-class address spaces as the
  sharing mechanism — no grouping pressure found.)
- **Journal:** `journal/006-frame-is-execution-unit.md`.

### D7 — Split interaction model: IPC + typed kernel operations

The kernel's external interface has two mechanism families: a dedicated IPC
mechanism for Frame↔Frame peer communication, and typed kernel operation
syscalls for Frame→Kernel resource management. The two families reflect two
genuinely different relationships. IPC carries peer messages between Frames and
may block, queue, or multiplex. Kernel operations act on resources (Frames,
Spaces, capabilities) and are always synchronous.

The unified model (seL4/EROS — everything through capability invocation, type
determines operation) was rejected because it hides the trust-model asymmetry
that A4 makes explicit: the kernel is the exception handler, not a peer. Full
fragmentation (Zircon — 170+ typed syscalls) was rejected on A5 grounds: large
interface surface, large verification and attack surface.

Does NOT settle: specific syscall surface (names, signatures, count), IPC model
(synchronous vs. asynchronous), notification mechanism, capability transfer
mechanism, or fast-path design.

- **Rests on:** A4 (purely reactive — the kernel is the exception handler, not a
  message server; the Frame→Kernel relationship is asymmetric and the split
  preserves this), D1 (hot-path dispatch — the split model's IPC hot path is
  structurally shorter by one indirection; the kernel knows the operation from
  the syscall number before touching the capability table), D4 (capability-based
  authority — both models satisfy D4; the question is orthogonal to D4's
  "designation = authority" concern), A5 (rejects full fragmentation — large
  syscall surfaces are large interfaces; does not distinguish unified from
  split), `design/research/syscall-landscape.md` §10.2–10.3 (IPC model coupling:
  async buffered IPC introduces behavioral divergence — queuing, blocking,
  multiplexing — that aligns naturally with a split).
- **Status:** settled — revisit if the IPC/kernel-op boundary proves
  unprincipled (too many ambiguous cases degrade the split into two mechanisms
  plus special cases), or if a practical use case requires transparent
  kernel-operation interposition that cannot be served by EL2 hardware,
  capability restriction, or kernel-level mechanisms.
- **Journal:** `journal/007-scope-of-capability-mediation.md`.

### D8 — Kernel-managed flat capability table with typed-memory backing

Each Frame's capability table is a flat array of (kernel object pointer, rights
mask) entries, managed internally by the kernel. Handles are opaque integers;
the kernel handles slot allocation, growth, and reuse. Userspace never sees or
manages the table's structure.

The physical memory backing the table comes from the Frame's memory budget, not
the kernel's pool. The Frame (or its creator) commits physical memory for
capability storage. When the table is full and a new capability must be stored,
the kernel faults the Frame; the fault handler commits more memory, then
retries. This provides explicit resource accounting without exposing table
structure.

The CNode tree model (seL4) was rejected: D7 eliminates the dispatch role that
CNode trees structurally serve, and A5 creates tension with CNode management
pushed to userspace as interface complexity. Per-core replicated tables
(Barrelfish) were rejected on D1 + A2 grounds. Unified cap/page tables
(Composite) were rejected on D5 + A2 grounds.

Each Frame always has its own table. Table sharing between Frames is deferred to
the Frame-Space binding model — it is not a table-structure question.

Does NOT settle: handle numbering/ABA prevention, entry layout (type tag, badge,
generation counter), revocation model, table-full fault protocol, or maximum
table size policy.

- **Rests on:** D7 (split model narrows the table's role to designation/rights
  lookup — not dispatch; CNode tree structure serves dispatch flexibility D7
  eliminated), D4 (per-Frame, O(1) lookup, designation = authority — flat
  indexing satisfies O(1); per-Frame tables are the unit of authority), A5
  (kernel absorbs complexity — CNode management is interface complexity pushed
  to userspace; flat table keeps the interface simple), D1 (hot path — one
  memory access for flat index vs. two+ for CNode tree walk), D3 (one logical
  Space manager — table memory charged to Frame's budget through the Space
  manager), `design/research/authority-models.md` §4, §5.5 (seL4 CNode tree vs.
  Zircon flat table; namespace shape comparison), `design/landscape.md` §1.1
  (capability representation survey).
- **Status:** settled — revisit if D7 is revised (unified model would
  re-motivate CNode dispatch), if Frame-Space binding reveals that per-Frame
  tables force essential sharing complexity into userspace, or if the revocation
  model requires CDT and the absence of tree structure makes it impractical.
- **Journal:** `journal/008-capability-table-structure.md`.

### D9 — Variable-size kernel-managed memory objects

The capability-designated memory resource is a variable-size, kernel-managed
memory object. Frames hold capabilities to memory objects; the kernel allocates
physical pages behind them and maps them into address spaces internally. Memory
objects exist independently of any address space binding (two-step: create, then
bind). Sharing is through capability transfer — multiple Frames holding
capabilities to the same object. Physical backing is drawn from the Frame's
Space; which physical pages back an object is a kernel-internal concern.

The seL4 untyped-memory model (userspace manages physical allocation and
constructs page tables) was rejected: A5 forecloses pushing memory management
complexity into userspace, and D8's precedent (kernel-managed flat capability
table) established the pattern of kernel-internal management with resource
accounting charged to the Frame's Space. Page-granularity objects (one
capability per hardware page) were rejected: they force page size exposure,
violate D5's CHERI forward-compatibility note, and cause capability
proliferation.

Does NOT settle: page size exposure (byte-addressed vs. page-addressed
interface), specific operations on memory objects (create, bind, COW/clone,
resize), object-rights, fault delegation, or precise Space-to-memory-object
accounting relationship. (Frame-Space binding model settled by D10.)

- **Rests on:** A5 (kernel absorbs complexity — same argument that rejected
  CNode trees in D8 applies to memory management), D5 (MMU-backed virtual
  memory; CHERI note requires objects-and-permissions interface, not
  page-table-specific concepts), D4 (capability-designated; sharing through
  capability transfer), D7 (memory operations are typed kernel syscalls, not
  IPC), D8 (precedent: kernel-managed structure with typed-memory backing from
  Frame's budget), D3 (Space manager is the single allocation interface; memory
  object backing flows through it), `design/landscape.md` §2.1–2.3 (four
  families surveyed; two-step create/map dominant).
- **Status:** settled — revisit if A5 is revised (would re-open
  userspace-managed models), or if D5's CHERI note is dropped (would re-open
  page-specific interfaces). (Frame-Space binding model settled by D10 — no
  sharing pattern issues found.)
- **Journal:** `journal/009-memory-object-model.md`.

### D10 — The address space is a first-class kernel object

The address space (page table tree) is a capability-designated kernel object,
separate from the Frame. Frames bind to an address space; multiple Frames can
bind to the same one, sharing the page table tree, TTBR value, and ASID. Memory
objects (D9) are mapped into the address space, not into the Frame directly. The
address space creator's Space budget pays for the page table memory (D8
pattern).

The vocabulary's "Space" remains the budget/resource-claim concept. The address
space is a distinct object type. Working name: "address space" — final naming
deferred to public API.

The emergent model (address space as a Frame attribute, no separate object) was
rejected on three independent paths: A5 (mapping consistency for co-located
Frames is essential complexity pushed to userspace), D1 (TLB capacity pressure
from per-Frame ASIDs), and D4 (cannot delegate address-space access
independently of Frame access). The kernel needs to track shared address spaces
internally regardless (for TLB shootdown); exposing the concept at the interface
is simpler than inferring it.

API design intent (not settled as interface): Frame creation requires an
explicit address space capability; creating a new address space has equal
friction to reusing an existing one; no "share by default."

Does NOT settle: binding mutability (rebindable?), address space lifecycle
(destruction semantics), Frame creation API, capability table sharing (D8
downstream, now reopenable), or address space naming.

- **Rests on:** A5 (mapping consistency is essential complexity; same A5
  argument pattern as D8 and D9 — userspace rebuilds the concept if the kernel
  omits it), D1 (TLB capacity pressure from per-Frame ASIDs on co-located
  workloads; shared TTBR eliminates hot-path cost for same-address-space
  switching), D4 (independent delegation of address-space access vs. Frame
  access), D6 ("binding" language; "sharing a Space" = multiple Frames bound to
  the same object), D5 (CHERI note: address space object abstracts the page
  table), D8 (typed-memory-backing precedent for budget), vocabulary cardinality
  ("one or more Spaces" fits Space-as-budget, not Space-as-address-space),
  `design/landscape.md` §6.5 (all surveyed systems use first-class address
  spaces), `design/research/execution-unit.md` §2 (thread↔address-space
  cardinality across systems).
- **Status:** settled — revisit if A5 is revised (re-opens whether mapping
  consistency belongs in userspace), if D1's hot/cold split is revised (removes
  TLB argument, though A5 and D4 remain), or if a downstream derivation reveals
  that first-class address spaces force essential complexity into userspace.
- **Journal:** `journal/010-address-space-is-first-class.md`.

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

- **Time migration across cores.** When a Frame migrates to a less-loaded core,
  does its Time allocation transfer, or is it re-allocated on the destination?
  Affects D2's migration story.
- **Minimum abstract scheduling properties on a Frame.** D2 says Frames carry
  abstract scheduling properties, but the minimum set (priority? deadline?
  IO-bound flag? period?) is not fixed.
- **Frame-Space cardinality formalization.** The Vocabulary section describes
  Frames as correlating "one or more Spaces." D10 confirms Space (vocabulary)
  and address space are distinct: Space is a budget concept (one or more per
  Frame); the address space is a first-class object (one per Frame, per D6).
  Remaining: formalize the vocabulary's "one or more" — does a Frame hold
  multiple Space claims, or is it one claim subdivided?
- **Revocation model.** Close-only (refcount), authoritative destroy, derivation
  tracking (seL4 CDT), or generation numbers. Each has different cost profiles
  under D1 (hot/cold split) and O2 (cross-core IPIs). D8 (flat table) is
  compatible with refcount, destroy, and generation numbers; CDT would require a
  separate tracking structure.
- **Frame minimum schema.** D6 settles that a Frame is a single execution unit.
  The concrete field set (register state, TTBR, capability table pointer, Time
  binding, scheduling state, fault handler) needs formal derivation in the
  current chain. Archive journal/004 derived a first-principles minimum.
- **Address space binding mutability.** D10 settles the address space as a
  first-class object that Frames bind to. Open: is the binding immutable (set at
  Frame creation) or rebindable at runtime? If rebindable, what happens to TLB
  entries when a Frame changes address space?
- **Frame lifecycle.** Create, destroy, suspend, resume. Whether Frame is a
  capability-held object type (archive journal/013 said yes). Interacts with D4
  and D7 (lifecycle operations are typed kernel syscalls under the split model).
- **Can Frames share capability tables?** D8 settles per-Frame tables with no
  sharing. D10 settles first-class address spaces with multi-Frame binding —
  same-address-space Frame groups are now a supported pattern. Revisit as a D8
  downstream: does same-address-space sharing create sufficient pressure for
  shared capability tables, or is per-Frame authority (with explicit capability
  transfer) sufficient?
- **Interrupt model (device interrupts, not exceptions).** Who owns device
  interrupts? Per-core or routed? Kernel-handled or delegated to userspace
  drivers?
- **Page size exposure.** D5 settles MMU-backed virtual memory; D9 settles
  variable-size kernel-managed memory objects. Open: expose page granularity to
  userspace (proven, universal) or hide it behind byte-addressed objects
  (archive's novel position, no precedent in surveyed systems)? Determines the
  memory object's interface granularity and the Space manager's external
  interface.
- **Fault delegation model.** D5 means page faults occur (MMU generates them on
  unmapped access). Open: kernel resolves faults internally, or forwards to
  userspace pager Frames? Interacts with A4 (reactive), A5 (complexity
  placement).
- **IPC model.** D7 settles the split interaction model but not the IPC
  mechanism itself. Synchronous register-based (L4/seL4 tradition) vs.
  asynchronous buffered (Mach/Zircon tradition) vs. hybrid. Determines message
  format, channel structure, blocking behavior, multiplexing, and the specific
  IPC syscall surface. Tightly coupled with D7 — async IPC was a factor in the
  split decision.
- **Specific syscall surface.** D7 settles two mechanism families but not the
  exact set. The archive's 10-syscall design is a data point. Depends on IPC
  model, Frame lifecycle, and D9 (memory objects).
- **Address space lifecycle.** D10 introduces the address space as a kernel
  object. When is it destroyed? Last capability dropped? Last Frame unbound?
  Interacts with revocation model.
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
- `006-frame-is-execution-unit.md` — reasoning for D6: vocabulary + D2 force
  Frame = single schedulable entity; no kernel grouping because D4 capabilities
  handle lifecycle and A3 makes grouping non-universal; seL4 validates approach.
- `007-scope-of-capability-mediation.md` — reasoning for D7: A4 trust-model
  asymmetry, D1 hot-path dispatch, IPC model coupling all favor split; unified
  hides trust boundary; full fragmentation rejected on A5; archive convergence.
- `008-capability-table-structure.md` — reasoning for D8: D7 narrows table role
  to designation/rights lookup (not dispatch), removing CNode tree
  justification; A5 confirms CNode management is interface complexity;
  typed-memory backing for explicit accounting; table sharing deferred to
  Frame-Space binding.
- `009-memory-object-model.md` — reasoning for D9: D8 precedent (kernel-managed,
  typed-memory backing) extends to memory; A5 rejects seL4 userspace-managed
  model; page-granularity rejected on D5 CHERI note; Space vocabulary provides
  accounting; vocabulary corrected (object identity, not physical address
  binding).
- `010-address-space-is-first-class.md` — reasoning for D10: three independent
  paths (A5 mapping consistency, D1 TLB capacity pressure, D4 independent
  delegation) converge on first-class address space; emergent model rejected;
  vocabulary Space confirmed as budget concept distinct from address space; API
  design intent (no default, equal friction) addresses "right way easy" concern.

---

## Research

See `design/research/` for descriptive prior-art studies and
`design/landscape.md` for the survey of how other kernels resolved each major
design decision.
