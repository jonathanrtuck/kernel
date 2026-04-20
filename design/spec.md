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

- **Observer.** A schedulable execution unit coupling Space and Time — the
  condition under which compute (Time) executes instructions within specific
  memory (Space). Borrowed from physics: in physics a reference frame bundles
  observer + coordinate system; this kernel unbundles them, so Observer is the
  executing entity and Coordinate System is the chart it binds to. Each Observer
  is an independent perspective in which a specific computation unfolds, binding
  to a Coordinate System within which its Spaces are located. An Observer
  correlates one or more Spaces but exactly one Time. SMT-concurrent workloads,
  when hardware supports them, are expressed as multiple Observers sharing a
  Space, each with its own Time on its own logical core — not as a single
  Observer with multiple Times. This keeps the one-Time commitment intact across
  SMT and non-SMT hardware.

- **Coordinate System.** An instance of the framework by which portions of
  memory (Spaces) are located within an Observer's computation — the page table
  tree of D10. Unlike Space and Time, a Coordinate System is not a claim on a
  bounded substance; it is a framework instance, and multiple Observers can bind
  to the same one, sharing its mappings, TTBR value, and ASID. An Observer binds
  to exactly one Coordinate System. Spaces are mapped into a Coordinate System;
  the same Space may be mapped into multiple Coordinate Systems at different
  coordinates. Working name; common-term equivalent from broader OS literature
  is "address space" (D10). No short form — "Coordinate System" stands as-is.

_Term categories:_ The vocabulary has two shapes of term. **Substance names**
(Space, Time) name a bounded substance; an Observer possesses specific,
identifiable portions of it — each an object with identity. Naming the substance
names the possession. **Framework names** (Coordinate System, the address-space
object of D10) name an instance of a framework that generates references —
coordinates — by which substance portions are located. An Observer possesses the
framework instance, not a portion of it. The substance/framework split is a
categorical difference in what the name refers to, not a style inconsistency.
Some framework-shaped concepts have no terse single-word English name that fits
precisely; in those cases the vocabulary accepts a two-word proper noun over
reaching for an obscure technical term (e.g., "Chart" from differential
geometry).

_Capitalized-vs-lowercase convention:_ Capitalized terms (Space, Time, Observer,
Coordinate System) are kernel proper nouns — names of specific concepts in this
kernel's design, with the semantics defined here. Lowercase equivalents from
broader OS literature (memory object, address space, thread) refer to the same
kind of thing but without claiming this kernel's specific semantics. The two are
interchangeable in prose; capitalization signals "speaking of our concept" vs.
"speaking of the general concept." Practical effect: lowercase "coordinate
system" and "reference frame" refer to the physics/general concepts; "Coordinate
System" (capitalized) refers specifically to the D10 address-space object that
Observers bind to.

_Naming note:_ these terms are for internal thinking and will not necessarily
appear in public API names. Public naming is deferred until v0.1. D10's working
name "address space" is the lowercase common-term equivalent of the proper-noun
candidate "Coordinate System"; final choice deferred with the rest of public
naming.

---

## Foundational Observations

Facts and consequences that shape downstream decisions but are not themselves
derived choices. Derivation entries may cite these under "Rests on" when the
observation is load-bearing.

- **O1 — Three output types.** Every kernel invocation produces some combination
  of: (1) updated kernel state, (2) a message delivered to an Observer, (3) a
  choice of which Observer to resume. Descriptive summary of what the kernel
  does; not an exhaustiveness claim with axiom strength. If a future invocation
  appears to need a fourth output type, that is a signal to examine the kernel's
  role definition — not to contort the new mechanism to fit the three.

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
work: exception entry, state update, selecting the next Observer to resume,
resumption. This structure touches no cross-core shared state on the hot path.
Infrequent cross-core concerns — Observer migration, cross-core message
delivery, shared resource allocation — route through an explicitly shared cold
path.

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

The scheduler that selects which Observer resumes on a core is per-core (direct
consequence of D1). Additionally, each core's scheduler may run a _different_
algorithm — throughput-oriented on a big core, simple fixed-priority on a LITTLE
core, deadline-based on a core dedicated to real-time Observers. The Observer
model carries only abstract scheduling properties (priority, CPU/IO
classification, optional deadline); algorithm-specific state (e.g., CFS virtual
runtime, deadline parameters) lives per-core in the scheduler, not in the
Observer. On migration, abstract properties transfer; algorithm-specific state
is re-derived by the destination scheduler.

- **Rests on:** D1, A2 (big.LITTLE asymmetric cores are within target hardware),
  A3 (a generic kernel cannot mandate one scheduling algorithm as the right
  answer), `design/landscape.md` (no surveyed system cleanly separates per-core
  scheduler algorithms as a first-class feature — novel position).
- **Status:** settled — revisit when the minimum abstract-property set on the
  Observer proves unexpressible across the candidate scheduling-algorithm space.
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

An Observer proves it is allowed to perform an operation by presenting an
unforgeable, per-Observer handle (capability) that designates the resource AND
carries the permitted operations. The kernel resolves the handle, checks the
rights, and proceeds or rejects. No identity lookup, no global namespace, no
ambient privilege. Two independent derivation paths converge: (1) A5 forecloses
any interface separating designation from authority (confused deputy); (2) D1
forecloses per-resource authority data on the hot path (ACLs are shared mutable
state). The archived chain reached the same conclusion from a third path.

Does NOT settle: scope of capability mediation (everything vs. resources-only),
capability table structure (kernel-managed vs. CNode-style), or revocation model
(refcount, destroy, CDT, generation numbers). These are one level down. (D7
settled scope via the split interaction model; D8 settled table structure as
kernel-managed flat table; D11 settled the base revocation primitive as
close-only + destroy + ABA tag — add-ons deferred with IPC model.)

- **Rests on:** A5 (confused deputy forces authority-tracking complexity into
  userspace — an A5 violation; capabilities are the only model where designation
  = authority), D1 + O3 (hot-path authority checks must use per-core data;
  per-Observer capability tables are per-core; per-resource ACLs are shared
  mutable state), A4 (no background authority management; capability refcount
  fits explicit-trigger model), A3 (no identity requirement — capabilities work
  across all workloads without assuming an identity scheme),
  `design/landscape.md` §1.2 (confused deputy: Hardy 1988, Miller's
  formalization, "Capability Myths Demolished" 2003).
- **Status:** settled — revisit only if A5 AND D1 are both revised
  simultaneously (either alone leaves at least one derivation path intact).
- **Journal:** `journal/004-capability-based-authority.md`.

### D5 — MMU-backed virtual memory with per-Observer address spaces

The kernel requires the ARM64 MMU to be enabled and uses it for inter-Observer
memory isolation. Each Observer has its own address space (page table tree); the
MMU enforces that an Observer can only access physical memory mapped into its
page tables. Three independent paths converge: (1) A2 hardware requires MMU
enabled for cached memory access — page tables must exist; (2) A3 + A5 require
hardware-enforced inter-Observer isolation, and the MMU is the only such
mechanism on ARM64; (3) philosophy "use what the hardware provides." Every
alternative (physical-only, language-safety isolation, CHERI-only, SFI) is
foreclosed by axioms or hardware facts.

Does NOT settle: address space structure sharing between Observers, page size
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

### D6 — An Observer is a single schedulable execution unit

An Observer is a single schedulable execution unit: one register state, one
program counter, one Time, one capability table, one address space binding. The
kernel has no "process" concept — "process" is a userspace convention (a group
of Observers sharing a Space). Multi-threaded execution in shared memory is
multiple Observers sharing a Space, each with its own Time. Green threads and
cooperative concurrency are internal to an Observer (userspace, invisible to
kernel).

The kernel provides no Observer-grouping mechanism. Grouping is neither
essential complexity (D4 capabilities handle Observer lifecycle without the
target's cooperation) nor workload-universal (A3 — not all workloads need
groups). Userspace builds grouping policy from capabilities; the kernel provides
the mechanism.

Does NOT settle: Observer minimum schema (concrete fields need formal
derivation), Observer-Space binding model (when/how binding occurs), Observer
lifecycle operations beyond the D14 minimum (creation API, rights model,
suspend, clonability), whether Observers can share capability tables. (D8
settled capability table structure; D14 settled Observer as capability-held
object type with resume and destroy as minimum operations.)

- **Rests on:** Observer vocabulary (one Time per Observer; SMT paragraph
  explicitly models concurrency as multi-Observer), D2 (scheduler selects
  Observers — one-level selection), D4 (per-Observer capability table; destroy
  capability works without target cooperation), A3 (generic — no workload
  assumes or requires kernel-level grouping), `design/landscape.md` §4.4, §6.1
  (seL4 validates no-kernel-process; all surveyed systems schedule thread-level
  entities).
- **Status:** settled — revisit if a downstream derivation (Observer lifecycle)
  reveals that the absence of kernel grouping forces essential complexity into
  userspace that capabilities alone cannot cover. (D8 settled capability table
  structure with per-Observer tables; D10 settled first-class address spaces as
  the sharing mechanism — no grouping pressure found.)
- **Journal:** `journal/006-observer-is-execution-unit.md`.

### D7 — Split interaction model: IPC + typed kernel operations

The kernel's external interface has two mechanism families: a dedicated IPC
mechanism for Observer↔Observer peer communication, and typed kernel operation
syscalls for Observer→Kernel resource management. The two families reflect two
genuinely different relationships. IPC carries peer messages between Observers
and may block, queue, or multiplex. Kernel operations act on resources
(Observers, Spaces, capabilities) and are always synchronous.

The unified model (seL4/EROS — everything through capability invocation, type
determines operation) was rejected because it hides the trust-model asymmetry
that A4 makes explicit: the kernel is the exception handler, not a peer. Full
fragmentation (Zircon — 170+ typed syscalls) was rejected on A5 grounds: large
interface surface, large verification and attack surface.

Does NOT settle: specific syscall surface (names, signatures, count), IPC model
(synchronous vs. asynchronous), notification mechanism, capability transfer
mechanism, or fast-path design.

- **Rests on:** A4 (purely reactive — the kernel is the exception handler, not a
  message server; the Observer→Kernel relationship is asymmetric and the split
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

Each Observer's capability table is a flat array of (kernel object pointer,
rights mask) entries, managed internally by the kernel. Handles are opaque
integers; the kernel handles slot allocation, growth, and reuse. Userspace never
sees or manages the table's structure.

The physical memory backing the table comes from the Observer's memory budget,
not the kernel's pool. The Observer (or its creator) commits physical memory for
capability storage. When the table is full and a new capability must be stored,
the kernel faults the Observer; the fault handler commits more memory, then
retries. This provides explicit resource accounting without exposing table
structure.

The CNode tree model (seL4) was rejected: D7 eliminates the dispatch role that
CNode trees structurally serve, and A5 creates tension with CNode management
pushed to userspace as interface complexity. Per-core replicated tables
(Barrelfish) were rejected on D1 + A2 grounds. Unified cap/page tables
(Composite) were rejected on D5 + A2 grounds.

Each Observer always has its own table. Table sharing between Observers is
deferred to the Observer-Space binding model — it is not a table-structure
question.

Does NOT settle: handle numbering/ABA prevention, entry layout (type tag, badge,
generation counter), revocation model, table-full fault protocol, or maximum
table size policy. (D11 settled handle ABA prevention via generational slot tag
and the base revocation primitive as close-only + destroy; entry-layout
specifics beyond the slot tag, revocation add-ons, table-full protocol, and size
policy remain open.)

- **Rests on:** D7 (split model narrows the table's role to designation/rights
  lookup — not dispatch; CNode tree structure serves dispatch flexibility D7
  eliminated), D4 (per-Observer, O(1) lookup, designation = authority — flat
  indexing satisfies O(1); per-Observer tables are the unit of authority), A5
  (kernel absorbs complexity — CNode management is interface complexity pushed
  to userspace; flat table keeps the interface simple), D1 (hot path — one
  memory access for flat index vs. two+ for CNode tree walk), D3 (one logical
  Space manager — table memory charged to Observer's budget through the Space
  manager), `design/research/authority-models.md` §4, §5.5 (seL4 CNode tree vs.
  Zircon flat table; namespace shape comparison), `design/landscape.md` §1.1
  (capability representation survey).
- **Status:** settled — revisit if D7 is revised (unified model would
  re-motivate CNode dispatch), if Observer-Space binding reveals that
  per-Observer tables force essential sharing complexity into userspace, or if
  the revocation model requires CDT and the absence of tree structure makes it
  impractical.
- **Journal:** `journal/008-capability-table-structure.md`.

### D9 — Variable-size kernel-managed memory objects

The capability-designated memory resource is a variable-size, kernel-managed
memory object. Observers hold capabilities to memory objects; the kernel
allocates physical pages behind them and maps them into address spaces
internally. Memory objects exist independently of any address space binding
(two-step: create, then bind). Sharing is through capability transfer — multiple
Observers holding capabilities to the same object. Physical backing is drawn
from the Observer's Space; which physical pages back an object is a
kernel-internal concern.

The seL4 untyped-memory model (userspace manages physical allocation and
constructs page tables) was rejected: A5 forecloses pushing memory management
complexity into userspace, and D8's precedent (kernel-managed flat capability
table) established the pattern of kernel-internal management with resource
accounting charged to the Observer's Space. Page-granularity objects (one
capability per hardware page) were rejected: they force page size exposure,
violate D5's CHERI forward-compatibility note, and cause capability
proliferation.

Does NOT settle: page size exposure (byte-addressed vs. page-addressed
interface), specific operations on memory objects (create, bind, COW/clone,
resize), object-rights, fault delegation, or precise Space-to-memory-object
accounting relationship. (Observer-Space binding model settled by D10.)

- **Rests on:** A5 (kernel absorbs complexity — same argument that rejected
  CNode trees in D8 applies to memory management), D5 (MMU-backed virtual
  memory; CHERI note requires objects-and-permissions interface, not
  page-table-specific concepts), D4 (capability-designated; sharing through
  capability transfer), D7 (memory operations are typed kernel syscalls, not
  IPC), D8 (precedent: kernel-managed structure with typed-memory backing from
  Observer's budget), D3 (Space manager is the single allocation interface;
  memory object backing flows through it), `design/landscape.md` §2.1–2.3 (four
  families surveyed; two-step create/map dominant).
- **Status:** settled — revisit if A5 is revised (would re-open
  userspace-managed models), or if D5's CHERI note is dropped (would re-open
  page-specific interfaces). (Observer-Space binding model settled by D10 — no
  sharing pattern issues found.)
- **Journal:** `journal/009-memory-object-model.md`.

### D10 — The address space is a first-class kernel object

The address space (page table tree) is a capability-designated kernel object,
separate from the Observer. Observers bind to an address space; multiple
Observers can bind to the same one, sharing the page table tree, TTBR value, and
ASID. Memory objects (D9) are mapped into the address space, not into the
Observer directly. The address space creator's Space budget pays for the page
table memory (D8 pattern).

The vocabulary's "Space" remains the budget/resource-claim concept. The address
space is a distinct object type. Working name: "address space" — final naming
deferred to public API.

The emergent model (address space as an Observer attribute, no separate object)
was rejected on three independent paths: A5 (mapping consistency for co-located
Observers is essential complexity pushed to userspace), D1 (TLB capacity
pressure from per-Observer ASIDs), and D4 (cannot delegate address-space access
independently of Observer access). The kernel needs to track shared address
spaces internally regardless (for TLB shootdown); exposing the concept at the
interface is simpler than inferring it.

API design intent (not settled as interface): Observer creation requires an
explicit address space capability; creating a new address space has equal
friction to reusing an existing one; no "share by default."

Does NOT settle: binding mutability (rebindable?), address space lifecycle
(destruction semantics), Observer creation API, capability table sharing (D8
downstream, now reopenable), or address space naming.

- **Rests on:** A5 (mapping consistency is essential complexity; same A5
  argument pattern as D8 and D9 — userspace rebuilds the concept if the kernel
  omits it), D1 (TLB capacity pressure from per-Observer ASIDs on co-located
  workloads; shared TTBR eliminates hot-path cost for same-address-space
  switching), D4 (independent delegation of address-space access vs. Observer
  access), D6 ("binding" language; "sharing a Space" = multiple Observers bound
  to the same object), D5 (CHERI note: address space object abstracts the page
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

### D11 — Base revocation primitive: close-only + authoritative destroy

The base revocation primitive is close-only (refcount on holder-drop) plus
authoritative destroy (an entity with appropriate rights can destroy the
underlying object; outstanding capabilities become dead handles, observable as
errors on next use). Handles carry a generational slot tag — bumped on slot
reuse — to prevent stale-handle aliasing of reused table slots. The slot tag is
ABA defense, not revocation: it does not invalidate live capabilities.

Close-only alone (Base-A) was rejected. Four structural workload patterns under
A3 — adversarial targets, failure-mode targets, pressure response, and
structural cascade — require terminate-by-force. For kernel-owned resources
(Observers, address spaces, memory objects), close-only cannot express this; the
userspace construction that would substitute cannot interpose at the MMU level
and must route through a kernel mechanism that is itself a form of authoritative
destroy under another name. Forcing this construction into userspace violates A5
via O4 (a).

Add-on mechanisms for mass invalidation (generation-as-revocation) and selective
revocation (CDT, badges) are deferred. Their value depends on the IPC model:
endpoint rotation serves mass invalidation only if endpoint-like kernel objects
exist; badges ride on IPC; proxy indirection requires IPC mediation. Committing
to add-ons before the IPC model is settled would either skip a level or
overspend on features whose alternatives may be free.

Does NOT settle: mass invalidation (deferred with IPC), selective revocation
(deferred with IPC), who authorizes destroy, cross-core prompt-effect policy
(strong vs. weak), destroy cleanup protocol (inline vs. preemptible), ABA tag
size and encoding, budget treatment of freed slots, table-full fault ↔
revocation interaction.

- **Rests on:** A5 (close-only alone forces terminate-by-force into userspace
  for kernel-owned resources — O4 (a) violation), D4 (revocation preserves
  designation = authority; close retracts the cap, destroy retracts the target),
  D8 (flat table is compatible with refcount, destroy, and per-slot ABA tags;
  CDT requires separate structure — deferred; D11 closes D8's deferred ABA
  sub-question), D7 (revocation ops are typed kernel syscalls, synchronous), A4
  (no background sweeper; synchronous), D3 (slot tag field flows through Space
  accounting), O2 (cross-core prompt effect costs IPIs; weak observation is
  IPI-free), O4 (a, c — base primitive is essential; add-ons require essential
  justification that depends on IPC-model context), `design/landscape.md` §1.4
  (every surveyed system with authoritative revocation has close + destroy as a
  base; add-on selection varies), `design/research/capability-revocation.md`
  (per-mechanism survey, cost table, stale-capability discovery modes,
  distributed-setting costs), `design/research/authority-models.md` §4.1–4.8,
  §5.2, §6.3 (per-system survey, cost table, seL4 WCET concern).
- **Status:** settled — revisit when the IPC model decision reveals whether
  Base-B plus IPC-level mechanisms (endpoint rotation, badges) cover the
  workloads that would otherwise justify generation-as-revocation or CDT, or if
  a downstream lifecycle derivation (Observer, address space) reveals the base
  primitive is structurally insufficient.
- **Journal:** `journal/011-base-revocation-primitive.md`.

### D12 — Fault delegation to userspace pager Observers

The kernel delegates all page faults to userspace pager Observers. The kernel's
role is fault dispatch: detect the fault, identify the faulting Observer,
deliver a fault notification to the designated pager Observer, and resume the
faulting Observer when the pager replies. The kernel does not contain paging
policy (eviction, prefetch, page source selection, write-back).

Three independent paths converge: (1) A4 forecloses kernel-internal background
paging — pager Observers with their own Time are the only way to do background
page management; (2) A3 forecloses a single hardcoded paging policy — policy
belongs in userspace where each workload implements its own; (3) A5 (net) — the
fault dispatch interface is smaller than a policy-configuration interface, and
policy complexity lives in pager Observers (leaf nodes).

Self-paging (Nemesis/Barrelfish — faults reflected to the faulting Observer) was
rejected: A5 + O4 (a) — pushes fault-handling complexity into every Observer.
Kernel-internal (QNX/Redox) was rejected: A4 + A3 — no background paging, no
single policy fits all. Hybrid (kernel resolves simple, delegates complex) was
rejected as a designed-in interface distinction: the simple/complex boundary is
workload-dependent policy (A3), though a transparent kernel optimization below
the delegation interface is not foreclosed.

IPC overhead on every fault (minimum: 1 IPC + 1 syscall roundtrip per D9's
kernel-managed memory objects) and the requirement that every Observer have a
fault handler are accepted costs. Nothing structural is foreclosed — a
kernel-internal fast path for trivial faults can be added later without
interface changes.

Does NOT settle: ~~fault handler attachment (Observer vs. address space)~~
(settled by D20: per-Observer), pager unavailability protocol (chains vs.
double-fault-kill), root/bootstrap case mechanism, fault message contents, pager
reply/resume mechanism, D7 classification of fault traffic (IPC vs. dedicated
mechanism), Observer minimum schema (fault handler confirmed structurally
required by D12; representation settled by D21 as cap-table entry).

- **Rests on:** A4 (no kernel thread → no background paging; independent path),
  A3 (generic → no single policy; independent path), A5 (net: dispatch interface
  smaller than policy-configuration interface; confirms delegation's interface
  economics), D5 (MMU-backed virtual memory — faults are a hardware fact; makes
  the question unavoidable), D7 (fault traffic must be classified under the
  split model), D9 (kernel-managed memory objects create the structural
  roundtrip: kernel → pager → kernel), `design/landscape.md` §2.4 (three
  patterns surveyed), §5.3 (fault delivery mechanisms),
  `design/research/execution-unit.md` §4 (per-system fault handling),
  `design/research/address-space-as-object.md` §1.3, §1.5, §1.6 (L4 pager,
  Barrelfish self-paging, Mach external pagers).
- **Status:** settled — revisit if A4 is revised (background kernel paging
  becomes possible, weakening the execution-model path), if A3 is revised
  (workload-specific kernel allowed, weakening the policy path), or if the IPC
  model derivation reveals that kernel-as-sender fault traffic cannot be
  accommodated without unacceptable IPC complexity.
- **Journal:** `journal/012-fault-delegation.md`.

### D13 — Queued endpoints with direct-switch fast path

The primary IPC mechanism is bounded queued endpoints. Messages accumulate in a
per-endpoint queue. Sender deposits and continues (non-blocking; behavior on
queue-full is a downstream endpoint-shape question). When the receiver is
already waiting, direct process switch bypasses the queue entirely at rendezvous
speed (~400 cycles ARM64). All information delivery — peer IPC, fault
notifications (D12), interrupt signals, system events — uses the same mechanism.

Sync-only was foreclosed: A3 requires event-driven workload support, and D12
requires the kernel to deposit fault messages without blocking. The queued model
subsumes both patterns: sync (send + block-on-reply endpoint) and async (send +
continue). The archive's "strictly dominates" argument: queued endpoints achieve
rendezvous speed for the same-core, receiver-waiting case AND provide async
fallback — sync rendezvous cannot handle the async case at all.

Sync rendezvous + bitmap notifications (seL4 model) was not foreclosed but not
chosen: sender-always-blocks limitation breaks fan-out patterns; the archive
independently rejected it for the same reason. Sync + queued notifications
(QNX-like) also not chosen: still has sender-blocks, plus two mechanisms.

A coalescing tension exists for shared endpoints with overwrite-oldest overflow:
cross-source data loss when multiple sources share a capacity-1 overwrite
endpoint. Resolution deferred to endpoint-shape exploration. The tension is
documented in journal/013 so it is not rediscovered.

Queue memory charged to creator's Space budget (D8 pattern). Fixed capacity at
creation. Memory per queued message ~48 bytes (register-sized).

Does NOT settle: message format, queue capacity policy, IPC fast-path
conditions, D12 fault delivery specifics. (Endpoint shape settled by D15.
Overflow policy settled by D18. Coalescing dissolved by D18. Reply routing
settled by D16. Badge semantics settled by D17. Multi-endpoint wait resolved by
D19.)

- **Rests on:** A3 (generic — both sync and async patterns required; independent
  path), A4 (purely reactive — no kernel message broker; IPC dispatch within
  syscall handlers), D1 (hot-path — direct-switch fast path achieves D1's
  minimal per-core hot-path requirement), D7 (split model — IPC as a dedicated
  mechanism family; D7 notes "couples naturally with async"), D12 (fault traffic
  is IPC — kernel-as-sender requires non-blocking deposit), D4 (capability-
  mediated — endpoints designated by capabilities), D3 + D8 (queue memory
  budgeted to Space through typed-memory-backing pattern), `design/landscape.md`
  §3.1 (sync vs. async survey), §3.2 ("every production microkernel converges on
  hybrid"), §3.4 (fast-path data), `design/research/syscall-landscape.md` §10
  (IPC as pivot point, performance data, lessons from removals).
- **Status:** tentative — D18 resolves trigger #1 (coalescing gap dissolved — no
  second primitive needed) and settles overflow policy. D19 resolves trigger #3
  (multi-endpoint wait — badge fan-in via D15+D17 covers common patterns;
  multi-receive syscall deferred, not foreclosed). Remaining trigger: bounded
  queue capacity creates unsolvable priority inversion or deadlock patterns
  (trigger #2, a downstream concern of priority/scheduling interaction, D2).
- **Journal:** `journal/013-ipc-model.md`.

### D14 — Observer is a capability-held kernel object type

Observer is a kernel object type designated by capabilities, joining Space,
Time, Coordinate System (D10), and endpoint (D13) as the fifth type. Lifecycle
operations — at minimum resume and destroy — are typed kernel syscalls (D7)
taking Observer capability handles. The capability's rights mask governs
permitted operations. D11's destroy provides termination; outstanding
capabilities become dead handles.

The derivation is forced by a chain of settled decisions: D12 requires resume as
a kernel operation on a suspended Observer (can't participate in IPC); D7
requires it as a typed syscall; D4 requires a capability handle as the noun; D8
accommodates the handle; D11 provides termination. The archive explored the
alternative (lifecycle through IPC indirection — archive/006, archive/011) and
reversed it (archive/013) for the same structural reason. Every surveyed
capability system makes the execution unit a capability-held object type.

Does NOT settle: creation API shape (create-then-configure vs. all-params),
Observer rights model beyond resume and destroy (suspend, inspect, configure),
~~Observer handle clonability~~ (settled by D23: clonable), ~~fault handler
attachment (per-Observer vs. per-address-space)~~ (settled by D20:
per-Observer), Time reclamation on destroy, Observer minimum schema.

- **Rests on:** D12 (resume must exist — suspended Observer can't receive IPC;
  independent path), D7 (lifecycle ops are typed kernel syscalls; provides
  mechanism family), D4 (capability handle required as noun; designation =
  authority), D8 (flat table accommodates Observer handles; rights mask, ABA
  tag), D11 (destroy provides termination; dead-handle semantics), D6 (defines
  what Observer is — the object the handle designates),
  `design/research/execution-unit.md` (100% convergence across surveyed
  capability systems), `design/landscape.md` §4.4, §6.7.
- **Status:** settled — revisit if D7 is revised (unified model changes
  mechanism family) or if D12 is revised (removing fault delegation removes the
  structural demand for resume, though D4 and D6 provide independent support).
- **Journal:** `journal/014-observer-is-capability-held.md`.

### D15 — Unidirectional, many-to-many endpoints with send/receive rights

An endpoint is a single kernel object: bounded queue + waiters list.
Capabilities to the same endpoint carry different rights in the D8 rights mask:
send (enqueue), receive (dequeue), or both. Topology is emergent from capability
distribution — the kernel does not enforce sender/receiver counts. Three usage
patterns arise by convention: server inbox (many:1), worker pool (many:many),
dedicated pipe (1:1).

Three convergent paths: (1) D8 + D11 structural consistency — standard entry
format, symmetric destroy; bidirectional would require structural exceptions to
both; (2) D12 + D13 many-to-one composition — fault delivery, interrupt
delivery, and server patterns are many-to-one; bidirectional requires per-source
channels + aggregation, weakening D13's "one mechanism" commitment; (3) A3 +
capability-distributed topology — diverse patterns served by one mechanism with
capability-mediated access.

Request-reply requires explicit reply-cap transfer per RPC (well-understood
cost; D16 settles the mechanism as send-once cap on a pre-allocated reply
endpoint). Peer disconnection detection requires a badge-closure notification
mechanism (deferred to badge-semantics exploration).

Does NOT settle: overflow policy, multi-endpoint wait, message format, endpoint
naming. (Reply-cap mechanism settled by D16. Badge semantics settled by D17.)

- **Rests on:** D4 (send/receive as independent authorities — confused deputy
  forecloses undifferentiated access), D8 (flat table with rights mask —
  standard entry format; bidirectional would require structural exception), D11
  (symmetric destroy — bidirectional requires asymmetric peer-closure
  signaling), D12 + D13 (kernel-as-sender in many-to-one fault/interrupt
  delivery; one endpoint per receiver, no aggregation needed), A3 (generic —
  diverse topology patterns required; kernel-enforced fixed topology foreclosed;
  QNX constrained model dominated), D7 (creation returns one capability,
  consistent with all other kernel object types), `design/landscape.md` §3.3
  (IPC object model survey: Mach ports, seL4 endpoints, Zircon channels; Zircon
  is the sole bidirectional model among surveyed systems).
- **Status:** settled — revisit if D13 is revised (different IPC model may
  change the many-to-one composition argument), if D8 is revised (non-uniform
  entry format would remove one convergent path), or if the badge-semantics
  exploration reveals that peer disconnection detection cannot be solved within
  the unidirectional model (would reopen bidirectional's peer-closure
  advantage).
- **Journal:** `journal/015-endpoint-shape.md`.

### D16 — Reply via pre-allocated reply endpoint with send-once cap

RPC reply routing uses a pre-allocated reply endpoint per Observer (a regular
endpoint, D15) combined with a send-once capability right in D8's rights mask.
On Call(), the kernel creates a send-once cap to the caller's reply endpoint,
includes it in the request message, and blocks the caller on its reply endpoint.
The server sends the reply to the send-once cap; the cap is consumed on use. The
reply endpoint persists across RPCs; the cap is ephemeral. No new kernel type —
the reply endpoint is a standard endpoint.

Send-once is a general-purpose use-limited attenuation right, not
reply-specific. It extends D4's attenuation hierarchy: a send-once cap is
consumed after one send operation. Independent applications include one-shot
notifications, single-use authorization tokens, and edge-triggered interrupt
delivery. Prior art: Mach send-once rights on ports; EROS resume keys
(effectively send-once).

The kernel is free to optimize the reply fast path behind the endpoint interface
(bypassing the queue structure when the sole waiter is the known caller). This
is an implementation optimization, not an object-model commitment.

Structurally parallel with D14's fault handling: both deliver a caller-specific
response capability in the message. The mechanism families differ per D7 — IPC
reply is send-to-endpoint; fault resume is resume(observer_handle) — but the
message shape is consistent.

A dedicated Reply kernel type (seL4 MCS) was considered and rejected: the
fast-path bypass it enables is an optimization achievable behind the endpoint
interface, not a structural necessity. A persistent send cap without send-once
(archive's approach) was refined: send-once prevents post-reply capability
retention. Badge-based reply was foreclosed by D4 (ambient addressing, not
capability designation).

Does NOT settle: Call()/ReplyRecv() syscall details (part of specific syscall
surface), reply endpoint allocation policy (pre-allocated at creation vs. lazy),
send-once right encoding in D8's rights mask, shared reply endpoint with badge
disambiguation (depends on badge semantics), message format interaction.

- **Rests on:** D15 (unidirectional endpoints require reply-cap transfer — the
  cost this mechanism pays), D14 (fault resume settled separately — decouples
  IPC reply from fault resume; structural parallel in message shape), D7 (split
  model — IPC reply must be in IPC mechanism family), D8 (flat cap table with
  rights mask — send-once extends the mask), D4 (capability-based authority —
  badge-based reply foreclosed), D13 (queued endpoints with direct-switch fast
  path — kernel can optimize reply path), D11 (base revocation — send-once is
  auto-revoked on use; close semantics), `design/research/endpoint-shape.md`
  (Mach send-once rights, seL4 reply cap, EROS resume key),
  `design/research/syscall-landscape.md` §1.1 (seL4 MCS reply object fix).
- **Status:** settled — revisit if D15 is revised (different endpoint shape
  changes the reply-cap constraint), if the send-once right proves insufficient
  for reply semantics (e.g., server needs to reply multiple times), or if the
  fast-path optimization behind the endpoint interface proves unachievable
  without a dedicated Reply type.
- **Journal:** `journal/016-reply-cap-mechanism.md`.

### D17 — Badge semantics: minter-assigned, mint-right-controlled, opt-in lifecycle tracking

A badge is a per-capability field in D8's entry layout, set by the minter at
clone time, immutable after creation, attached by the kernel to every message
sent through that capability. The sender cannot read, choose, or modify its
badge. Badges serve identification (key into receiver state), not merely
distinguishing. The minter chooses the value; the kernel enforces unforgeability
and immutability.

Mint is a third independent right in D8's rights mask (send, receive, mint),
controlling who can assign badges when cloning. The endpoint creator controls
mint-right distribution, aligning badge population growth with budget authority.

Lifecycle visibility is opt-in: the endpoint creator specifies at creation
whether per-badge refcount tracking is enabled. With tracking: when the last
send cap with badge B to endpoint E is closed, the kernel enqueues a closure
notification to E's receive side (through the endpoint queue, D13). Without
tracking: no per-badge state, no notifications, trivial close path. Opt-in
resolves the A3/A4 tension: not all workloads need disconnection detection (A3),
but those that do should not fall back to polling (A4).

Five tensions are accepted for tracked endpoints: D16 send-once consumption vs.
badge-closure (must distinguish consumed-by-use from closed-without-use); D13
bounded queue vs. notification volume; per-badge map growth (controlled by
mint-right distribution and bounded by creation-time capacity); reverse
information flow (receiver observes sender's close — accepted as deliberately
constructed); D14 Observer destroy cascade.

Does NOT settle: badge size (implementation detail, 64-bit default), send-once
exemption encoding, badge on D16 kernel-created send-once caps, max-badge-count
semantics, ~~fault handler representation~~ (settled by D21: cap-table entry —
badge-closure covers child Observer destruction automatically), badge- closure
message format, badge-closure × overflow policy interaction, per-badge tracking
× coalescing interaction.

- **Rests on:** D15 (many-to-one patterns create the structural need for sender
  identification; send/receive rights pattern extended with mint), D8 (flat cap
  table — badge stored in entry; rights mask holds mint right), D4 (designation
  = authority — badge unforgeability prevents confused deputy at IPC layer;
  badge-based addressing foreclosed; mint authority is capability-mediated), D13
  (one delivery mechanism — closure notifications use the endpoint queue), D12
  (fault handler badge structurally required — kernel synthesizes fault messages
  without a sender cap), D11 (base revocation — badge-closure is a revocation
  add-on; close triggers per-badge check on tracked endpoints), D16 (send-once
  caps create the consumed-vs-closed tension), A3 (generic — not all workloads
  need lifecycle tracking; opt-in), A4 (purely reactive — polling-based
  disconnection detection is inconsistent; event-driven notification is
  A4-consistent), `design/research/endpoint-shape.md` (seL4 badge mechanism,
  Mach send-once rights, Zircon peer-closed signal), `design/landscape.md` §1.3,
  §3.5, §3.6, §5.2 (badge and notification mechanisms across surveyed systems).
- **Status:** settled — revisit if D15 is revised (changes the many-to-one
  composition that creates the need), if D8 is revised (changes where badge is
  stored), if D13 is revised (changes the notification delivery mechanism), or
  if the opt-in model proves insufficient (a workload pattern requires
  badge-closure on an endpoint the receiver didn't create and can't replace).
- **Journal:** `journal/017-badge-semantics.md`.

### D18 — Error-to-sender overflow with deferred fault delivery

When a send to a queued endpoint finds the queue at capacity, the kernel returns
an error. No per-endpoint policy modes, no overwrite, no kernel-level
coalescing. Coalescing workloads use shared memory + signaling (D9/D10 +
capacity-1 endpoints) — the standard microkernel architecture (landscape §3.2).

For the kernel-as-sender (D12 fault messages), deferred delivery: the kernel
links the faulting Observer into a per-endpoint pending list. The next receive()
that frees a slot delivers the deferred fault. The pending list is an intrusive
linked list through existing Observer objects — zero additional memory
allocation. D17 badge-closure notifications are dropped on full queue; the
receiver discovers staleness lazily.

The D13 coalescing tension (cross-source data loss on shared endpoints with
overwrite semantics) dissolves: no overwrite means no cross-source data loss.
D13 revisit trigger #1 does not fire — coalescing is achieved through
composition of existing primitives, not through a second IPC primitive.

Does NOT settle: ~~interrupt delivery mechanism (must account for error-on-full
via masking)~~ (settled by D22: delegation with mask-on-delivery; D18 trigger
does not fire — no unsolvable delivery gaps), pager unavailability protocol
(endpoint destroy with pending faults adds a trigger), multi-endpoint wait (D13
revisit trigger #3), Observer minimum schema (pending-list linkage field).

- **Rests on:** A3 (generic — different workloads, but only error is
  irreducible; coalescing is reducible to shared memory + signaling), A4 (purely
  reactive — kernel-as-sender can't block or retry; receive() is the only
  trigger for deferred delivery), D12 (fault delegation — fault messages must be
  delivered; the kernel-as-sender constraint drives deferred delivery), D13
  (bounded queue, fixed capacity — overflow is the question this answers; one
  mechanism — deferred delivery stays within the endpoint, not a second
  primitive), D1 (overflow is cold-path; deferred delivery check on receive is
  cold-path), D9 + D10 (shared memory for coalescing — the existing primitives
  that make kernel-level coalescing reducible), D17 (badge-closure dropped on
  full — not a correctness issue; per-badge tracking × coalescing interaction
  dissolved), `design/landscape.md` §3.2 (every production microkernel converges
  on shared memory + IPC signaling for data-plane communication), §5.1
  (mask-on-delivery for interrupt coalescing).
- **Status:** settled — revisit if D13 is revised (different IPC model may
  change overflow semantics), if a downstream derivation reveals that dropped
  badge-closure notifications create a correctness issue (not just a timeliness
  issue), or if the interrupt model derivation reveals that error-on-full
  combined with interrupt masking creates unsolvable delivery gaps.
- **Journal:** `journal/018-endpoint-overflow-policy.md`.

### D20 — Per-Observer fault handler attachment

The fault handler attaches to the Observer, not the address space. Each Observer
stores a fault handler endpoint reference and a badge. On fault, the kernel
reads both from the faulting Observer's struct and delivers a fault notification
to the handler endpoint with the stored badge, plus the faulting Observer's
capability handle via cap transfer (D14).

Every Observer creation must supply a fault handler endpoint and badge (D12
invariant enforced at creation time). Redundant configuration when N Observers
want the same handler is a userspace ergonomics cost, not kernel complexity — a
library function absorbs it.

Per-address-space attachment was rejected on five independent tensions: D6
(implicit grouping the kernel rejected), D4 (authority coupling), D17
(badge-closure doesn't provide per-Observer lifecycle visibility), D11 (handler
destroy cascades to all bound Observers), D1 (split storage on fault path).
Per-both (Mach/Zircon hierarchical model) was rejected because the hierarchical
model composes with kernel-level grouping that D6 explicitly rejected; it adds
interface surface, branching, mixed cascades, and inconsistent badge-closure
coverage. Per-region (Coyotos GPT model) was foreclosed by D9 + D5 (kernel hides
address space structure).

Does NOT settle: fault handler mutability (part of Observer rights model), fault
handler in Observer creation API shape, pager unavailability protocol,
root/bootstrap fault handling. (Fault handler representation settled by D21:
cap-table entry.)

- **Rests on:** D6 (no kernel grouping — per-address-space re-introduces
  grouping through the side door; independent path), D4 (designation = authority
  — per-Observer allows independent delegation of fault handler configuration
  authority; fault handler control separable from address space authority;
  independent path), D17 (badge-closure lifecycle visibility works only with
  per-Observer reference; fault handler badge is structurally required
  per-Observer regardless of endpoint attachment; independent path), D12 (every
  Observer must have a fault handler — maps to local invariant with
  per-Observer; indirect and fragile invariant with per-address-space), D14
  (Observer as capability-held type — provides the natural configuration noun),
  D10 (first-class address space provides the alternative attachment point; D20
  rejects attaching to it), D1 (hot-path simplicity — single cache-line access),
  `design/research/execution-unit.md` §4 (fault handling across systems),
  `design/research/page-fault-routing.md` §3 (seL4 per-TCB fault handler),
  `design/landscape.md` §5.3 (fault delivery mechanisms — seL4 per-endpoint,
  Mach dual-level, Zircon hierarchical).
- **Status:** settled — revisit if D6 is revised (kernel grouping would reopen
  per-address-space attachment as a natural companion to group policy), if D17
  is revised (removing badge-closure would remove the strongest structural
  advantage of per-Observer cap-table entry), or if a downstream derivation
  reveals that per-Observer configuration creates essential complexity that a
  userspace library cannot absorb.
- **Journal:** `journal/020-fault-handler-attachment.md`.

### D21 — Fault handler is a cap-table entry

The per-Observer fault handler reference (D20) is a regular capability in the
Observer's D8 flat table at a kernel-reserved slot index. The entry carries send
rights to the handler endpoint, the per-Observer badge, and a generational slot
tag (D11). On fault, the kernel reads the entry at the known index and delivers
a fault message to the designated endpoint with the stored badge.

Three independent arguments converge: (1) D11 authoritative destroy of the
handler endpoint must invalidate the reference — the cap-table walk handles this
automatically; kernel-internal requires a parallel tracking structure; (2) D17
badge-closure on Observer destroy fires generically via cap-close — kernel-
internal requires explicit coupling between Observer-destroy and badge-closure;
(3) D8 ABA slot-tag protection prevents stale references after endpoint destroy

- slot reuse — kernel-internal requires separate dangling-pointer prevention.

The archive chose kernel-internal ("(wormhole_ref, badge) pair, not a handle").
This divergence is explained by the archive's absence of D17 opt-in per-badge
lifecycle tracking — without badge-closure, the strongest cap-table argument did
not exist.

The sole cost is one extra dependent memory access on the fault path (Observer
struct → cap table → entry at known index). This is marginal relative to the IPC
delivery that follows (~400 cycles ARM64).

Does NOT settle: reserved slot index value (implementation detail), rights on
the handler cap (likely send-only; checked at configuration, not fault time),
address space binding representation (parallel question — same D11/D8 arguments
apply but different access frequency), pager unavailability protocol (D21 makes
detection clear: dead cap-table entry).

- **Rests on:** D11 (destroy-invalidation: cap-table walk finds and invalidates
  the handler entry automatically; kernel-internal requires parallel tracking
  structure — don't rebuild what the existing system handles), D17
  (badge-closure lifecycle visibility: Observer destroy closes the cap,
  triggering notification generically; kernel-internal requires explicit
  coupling — D17 journal: "no equivalent substitute"), D8 (ABA slot-tag
  protection: prevents stale handler references; flat table makes cap-table
  entry cheap — one slot, O(1) lookup, no structural management cost), D4 (the
  handler participates in the capability system uniformly; kernel-internal is a
  special case), D20 (per-Observer attachment provides the per-Observer
  cap-table slot), D1 (one extra memory access on fault path — tension,
  accepted: marginal relative to IPC delivery cost).
- **Status:** settled — revisit if D11 is revised (changes the
  destroy-invalidation mechanism that provides the strongest structural
  argument), if D17 is revised (removing badge-closure removes the
  lifecycle-visibility advantage), or if D8 is revised (changes cap-table entry
  cost or structure).
- **Journal:** `journal/021-fault-handler-representation.md`.

### D22 — Device interrupt delegation through endpoints

The kernel delegates device interrupt handling to userspace driver Observers.
The kernel's role is interrupt dispatch: detect the interrupt (read GIC IAR),
mask it, enqueue a message to the driver Observer's endpoint with a
per-interrupt badge (D17) and a send-once ack cap (D16), send EOI, return. The
driver does everything else. Three independent paths converge, paralleling D12
(fault delegation): (1) A4 forecloses background interrupt processing; (2) A3
forecloses a single hardcoded interrupt policy; (3) A5 — the dispatch interface
(mask, signal, EOI) is smaller than a policy-configuration interface.

No separate IRQ kernel object type. The interrupt namespace maps onto the
endpoint namespace. The kernel maintains an internal IRQ→endpoint routing table.
At boot, device interrupts (discovered from device tree / GIC configuration)
route to a root interrupt endpoint. The initial Observer receives this endpoint
(same mechanism as initial Space distribution — one unsettled boot protocol, one
answer for both). To delegate, the holder splits the endpoint by IRQ range: a
new endpoint receives the subset, the original loses it. The new endpoint cap is
transferred to a driver Observer. Dynamically-discovered interrupts (LPIs via
ITS) are added to the appropriate endpoint by the kernel.

The driver handles interrupts identically to IPC: receive a message, do work,
respond. Each interrupt message carries a badge (identifying the IRQ) and a
send-once ack cap (D16). Using the ack cap unmasks the interrupt. The cap is
consumed on use (D16 send-once semantics). If the driver crashes and the cap is
closed without use, the interrupt stays masked (D18 safety). No IRQ-specific
operations — the driver uses receive() and send-once, exactly like RPC.

Both delivery and ack are IPC-family under D7: delivery is kernel-as-sender
depositing to endpoint; ack is driver using a send-once cap. No typed kernel
operations specific to interrupts.

Scope: SPIs (32–1019), LPIs (8192+), and delegatable PPIs. The preemption timer
is kernel-internal (D2 scheduling mechanism). IPIs are kernel-internal (O2
cross-core coordination). Landscape §5.1 confirms: "No microkernel delegates the
preemption timer."

Two endpoint operations emerge: split (create new endpoint, move IRQ routes to
it) and combine (merge N endpoints into one receiving all sources). Both are
cold-path. Both are potentially general endpoint operations — split for
structured load distribution, combine as an alternative to multi-wait (D19).
Details downstream of the endpoint model.

An IRQ object type (parallel to Space) and a factory model (IRQControl, seL4
precedent) were both considered and rejected. Every concern identified with the
endpoint-only model — send-once performance, crash recovery, split/combine
complexity — traces to a parent decision (D16, general lifecycle, D13/D15) and
is not introduced by D22.

Does NOT settle: endpoint split semantics (automatic return on destroy for crash
recovery? generalization to badge-range partitioning?), endpoint combine
semantics (transparent forwarding vs. dead handles for existing send caps), boot
distribution of IRQ authority, interrupt priority exposure (GICv3 8-bit priority
— deferred), IRQ routing policy (which core receives a given SPI — deferred),
userspace timer mechanism, GICv4 forward-compatibility (direct virtual
injection).

- **Rests on:** A4 (no background interrupt processing; independent path), A3
  (no single interrupt policy; independent path), A5 (net: dispatch interface
  smaller than policy-configuration interface; confirms delegation's interface
  economics), D12 (structural precedent — three convergent paths parallel
  exactly), D13 (all information delivery through queued endpoints — interrupt
  delivery committed; the endpoint IS the delivery mechanism, no additional type
  needed), D16 (send-once ack cap — D16 explicitly lists "edge-triggered
  interrupt delivery" as an application; the ack mechanism already exists), D17
  (badges identify which interrupt fired; fan-in onto one endpoint), D18
  (overflow settled: mask-on-delivery, GIC holds pending state; D18 revisit
  trigger does not fire — no unsolvable delivery gaps), D4 (capability-mediated
  authority — endpoint receive cap IS the authority over its interrupt sources;
  integer IRQ IDs and file-descriptor models foreclosed), D7 (both delivery and
  ack are IPC-family; no interrupt-specific typed kernel operations), D8 (flat
  table accommodates send-once ack caps per interrupt message), D11 (endpoint
  destroy masks associated IRQs; dead-handle semantics), D1 (hot path: GIC CPU
  interface registers are per-core; no shared mutable state on interrupt
  handling path; routing configuration and split/combine are cold-path), O3
  (interrupts taken on targeted core), `design/landscape.md` §5.1 (four
  interrupt ownership patterns surveyed; universal kernel-internal: masking,
  EOI, preemption timer), §5.2 (six interrupt object models surveyed), §5.6
  (microkernels dissolve deferred processing), §5.7 (GICv3/v4 specifics),
  `design/research/syscall-landscape.md` (seL4 IRQControl/IRQHandler, Zircon
  interrupt objects, L4Re IRQ objects, EROS IrqCtl/IrqWait).
- **Status:** settled — revisit if D13 is revised (different IPC model changes
  the delivery mechanism), if D16 is revised (changes the send-once mechanism
  that provides ack), or if a downstream derivation reveals that the
  endpoint-only model creates essential complexity that a separate IRQ type
  would not (e.g., split/combine prove unimplementable without per-endpoint IRQ
  state that breaks D15 uniformity).
- **Journal:** `journal/022-interrupt-model.md`.

### D23 — Observer capabilities are clonable

Observer handles follow uniform capability rules: clone, attenuate, transfer —
identically to every other kernel object type (endpoints, address spaces, memory
objects). Multiple entities can hold capabilities to the same Observer, each
with independent rights masks. No type-specific exceptions in D8's table
management.

Non-clonable was rejected on five convergent structural arguments: D4
attenuation requires cloning (foreclosed), D8 uniformity requires no
type-specific exceptions (broken), D12/D20 fault delivery requires cap-copy
(requires new mechanism), D11 close creates orphan risk (requires new
mechanism), and type consistency (Observer would be the sole non-clonable type
among five). Non-clonable's sole benefit — kernel-enforced single-manager — is
achievable through capability discipline under clonable.

The archive's "handle = handler unification" concept (if non-clonable, the
handle holder is necessarily the fault handler) is dissolved by D20/D21: the
fault handler is a separate endpoint cap at a reserved slot, not the Observer
handle holder.

A duplicate-control right (Zircon's ZX_RIGHT_DUPLICATE model) can be added later
as a rights-mask extension without affecting this decision. Deferred to the
Observer rights model derivation.

Does NOT settle: Observer rights model (which rights go in the mask), Observer
creation API shape, Observer minimum schema, whether the duplicate-control right
is adopted. These are one level down.

- **Rests on:** D4 (attenuation requires cloning — foreclosed by non-clonable;
  independent path), D8 (uniform flat table — non-clonable breaks uniformity
  with type-specific enforcement; independent path), D12 + D20 (fault messages
  include Observer cap via cap transfer — non-clonable requires new mechanism;
  independent path), D11 (close under non-clonable creates orphan risk — alive
  Observer unreachable through cap graph; independent path), D10 + D15 + D9
  (type consistency — all other kernel object types are clonable; Observer would
  be sole exception), `design/research/execution-unit.md` (100% landscape
  convergence — all surveyed capability systems make execution-unit handles
  clonable), `design/research/authority-models.md` §4 (seL4 CNode_Copy, Zircon
  handle_duplicate — uniform capability copying for all object types).
- **Status:** settled — revisit if D11 is revised (changes the refcount/destroy
  model that makes multi-holder safe), if D20/D21 are revised (reopens handle =
  handler unification), or if the Observer rights model derivation reveals that
  clonability creates essential complexity that non-clonable would have avoided.
- **Journal:** `journal/024-observer-handle-clonability.md`.

### D24 — Cap-mapping invariant: no cap → no mapping

The kernel maintains synchronization between capability ownership and MMU
mappings. When an Observer's last capability to a mapped memory object is
removed (via close, move, or destroy), the kernel automatically unmaps that
object from the Observer's address space. Map is explicit (Observer chooses
address); unmap is both explicit (Observer can unmap while retaining the cap)
and automatic (last- cap-close triggers unmap).

The invariant strengthens D4: the capability table is the source of truth for
memory access. An Observer cannot access memory it has no capability for — the
MMU state follows the cap state. The two-step model (D9: create, then bind) is
preserved for mapping creation; the invariant adds automatic cleanup on the
removal side.

Ownership-transfer IPC (the PLOS 2023 concept flagged by journal 023) is not a
separate mechanism. It falls out naturally: "move" is clone-to-receiver +
close-on-sender. The close triggers auto-unmap. No IPC-level changes, no
message-format changes, no D7 classification ambiguity.

The exploration evaluated four IPC-level ownership-transfer mechanisms (full,
dedicated syscall, optional, none) and found that all IPC-level approaches place
page-table work on the IPC hot path. The reframe: the safety property (sender
can't access after send) is better achieved as a cap-system invariant at the
cold-path cap-close layer than as an IPC mechanism at the hot-path send layer.

For single-Observer address spaces (common case under D6), auto-unmap is always
local to the Observer's own core — no cross-core broadcast needed. For shared
address spaces (D10), the broadcast cost is identical to explicit unmap().

The invariant requires per-(address-space, memory-object) cap counting and a
per-memory-object reverse mapping list (the latter likely needed regardless for
destroy cleanup). In shared address spaces, Observers that use a mapped memory
object must each hold their own cap — piggybacking on another Observer's mapping
without holding a cap is explicitly disallowed.

IPC-level ownership transfer was rejected: the invariant provides the same
safety property with strictly less cost (cold-path vs. hot-path, no IPC changes,
no DoS vector from sender-controlled page-table work in the send path, no D13
queue cost disruption). No invariant (cap-table/MMU independence, the standard
model) was rejected for inconsistency with D4's "designation = authority"
commitment.

Does NOT settle: explicit unmap() semantics (likely available — unmap while
retaining cap for later remap), sub-page packing strategy (kernel-internal
implementation concern — the invariant makes it load-bearing), Space budget
transfer on cap move, D9 memory object operations.

- **Rests on:** D4 (designation = authority — the invariant extends D4 to MMU
  access; the MMU mapping is a form of authority that should be governed by the
  capability system), D9 (variable-size kernel-managed memory objects — the
  invariant operates on D9 objects; two-step create/bind preserved), D10
  (first-class address spaces — shared address spaces create the cascade
  behavior; per-AS cap counting needed), D8 (flat cap table — cap-table
  mutations trigger the counter updates; the implementation cost is per-mutation
  bookkeeping), D11 (base revocation — close triggers auto-unmap as a new
  consequence; destroy of a memory object still requires cross-AS mapping
  cleanup regardless of invariant), D5 (MMU-backed virtual memory — the
  invariant synchronizes the two enforcement layers; CHERI forward-compatible —
  on CHERI hardware, capability pointers could replace MMU enforcement, and the
  invariant's interface is not page-table-specific), A1 (Rust ownership — the
  invariant makes the kernel's external interface consistent with Rust's "if you
  don't own it, you can't use it" model; not a mandate from A1 but a natural
  alignment), `design/research/bleeding-edge-os-landscape.md` §9 (PLOS 2023
  ownership-transfer IPC, Singularity linear types, LionsOS data/metadata
  separation — prior art on the safety property this invariant provides),
  `design/landscape.md` §2.7 (page size exposure survey), §3.2 (shared memory as
  universal data plane — the invariant does not replace this pattern; shared
  memory + signaling remains the data plane, with the invariant providing
  automatic cleanup).
- **Status:** settled — revisit if D4 is revised (weakening "designation =
  authority" removes the strongest motivation), if D9 is revised (different
  memory object model may change the cap/mapping relationship), if the sub-page
  packing question reveals that the invariant creates unacceptable internal
  fragmentation for small objects, or if a downstream derivation reveals that
  "cap without mapping" patterns (resource managers holding caps they don't map)
  are insufficient and "mapping without cap" is structurally needed.
- **Journal:** `journal/025-cap-mapping-invariant.md`.

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

- **Time migration across cores.** When an Observer migrates to a less-loaded
  core, does its Time allocation transfer, or is it re-allocated on the
  destination? Affects D2's migration story. (Journal 023 notes that
  time-as-capability — seL4 MCS, S3K — would make migration a capability
  transfer: close on source core, create on destination. See also Time
  reclamation.)
- **Minimum abstract scheduling properties on an Observer.** D2 says Observers
  carry abstract scheduling properties, but the minimum set (priority? deadline?
  IO-bound flag? period?) is not fixed.
- **Observer-Space cardinality formalization.** The Vocabulary section describes
  Observers as correlating "one or more Spaces." D10 confirms Space (vocabulary)
  and address space are distinct: Space is a budget concept (one or more per
  Observer); the address space is a first-class object (one per Observer, per
  D6). Remaining: formalize the vocabulary's "one or more" — does an Observer
  hold multiple Space claims, or is it one claim subdivided?
- **Revocation add-ons.** D11 settles the base primitive (close-only + destroy
  - ABA slot tag). D17 settles badge semantics (minter-assigned, opt-in
    per-badge tracking with closure notifications). Remaining add-ons: endpoint
    rotation via destroy (D11 provides destroy; endpoint lifecycle needed);
    generation-as-revocation (O(1) mass invalidation; alternative is endpoint
    rotation). Still deferred: CDT (selective revocation of a subtree); who
    authorizes destroy; strong vs. weak cross-core prompt-effect policy; destroy
    cleanup protocol (inline vs. preemptible — D17's destroy cascade tension
    amplifies this question).
- **Observer minimum schema.** D6 settles that an Observer is a single execution
  unit. D14 settles that Observer is a capability-held kernel object type. D20
  settles per-Observer fault handler. D21 settles the handler as a cap-table
  entry (not a separate Observer struct field). The concrete field set (register
  state, TTBR, capability table pointer, Time binding, scheduling state,
  pending-list linkage (D18), Observer state: runnable/blocked/faulted) needs
  formal derivation in the current chain. Note: the fault handler is NOT in this
  list — it lives in the cap table at a reserved slot, not the Observer struct.
  Archive journal/004 derived a first-principles minimum. D12 confirms the fault
  handler is structurally required. D14 confirms lifecycle state tracking is
  required. D20 confirms per-Observer attachment. D21 confirms cap-table
  representation.
- **Address space binding mutability.** D10 settles the address space as a
  first-class object that Observers bind to. Open: is the binding immutable (set
  at Observer creation) or rebindable at runtime? If rebindable, what happens to
  TLB entries when an Observer changes address space?
- **Observer creation API shape.** D14 settles Observer as capability-held but
  not the creation interface. Create-then-configure (seL4 — inert Observer,
  configured via cap ops, started separately) vs. all-params-upfront (archive —
  one syscall). Minimum inputs: Space, Time, address space (D10), fault handler
  endpoint + badge (D12, D20). Open: initial PC/SP, initial capabilities, create
  vs. start as separate operations.
- **Observer rights model.** D14 settles resume and destroy as minimum. D23
  settles clonability, enabling rights separation across multiple caps. Open:
  suspend (external pause), inspect register state (debugging), modify
  scheduling properties (D2), change fault handler (D20 — per-Observer, so this
  is an Observer-cap right), change address space binding (D10 binding
  mutability), duplicate-control right (Zircon ZX_RIGHT_DUPLICATE model,
  deferred from D23). Each right = a typed kernel syscall under D7.
- ~~**Observer handle clonability.**~~ Settled by D23: clonable. Observer
  handles follow uniform capability rules (clone, attenuate, transfer)
  identically to all other kernel object types. Non-clonable rejected on five
  convergent structural arguments. Archive's "handle = handler unification"
  dissolved by D20/D21. Duplicate-control right deferred to Observer rights
  model.
- **Suspend as distinct from faulted.** Is there external suspension (not caused
  by fault)? If yes, Observer state has four values (runnable, blocked, faulted,
  externally-suspended). Use cases: debugging, checkpointing, resource pressure.
- **Time reclamation on Observer destroy.** Observer holds one Time (D6). On
  destroy: return to destroyer? To creator? Destroy the Time? Interacts with
  Time's non-clonable property. (Journal 023 notes that time-as-capability — D4
  consistency — would dissolve this: close returns to delegator via D11
  semantics.)
- **Can Observers share capability tables?** D8 settles per-Observer tables with
  no sharing. D10 settles first-class address spaces with multi-Observer binding
  — same-address-space Observer groups are now a supported pattern. Revisit as a
  D8 downstream: does same-address-space sharing create sufficient pressure for
  shared capability tables, or is per-Observer authority (with explicit
  capability transfer) sufficient?
- ~~**Interrupt model (device interrupts, not exceptions).**~~ Settled by D22:
  delegation to userspace driver Observers through endpoints. No separate IRQ
  object type — the interrupt namespace maps onto the endpoint namespace. The
  kernel routes hardware interrupts to endpoints; authority = receive cap. Ack
  via D16 send-once cap in each interrupt message. Split/combine endpoint
  operations for IRQ range delegation. Preemption timer and IPIs excluded
  (kernel-internal).
- **Endpoint split semantics.** D22 introduces split-by-IRQ-range: create a new
  endpoint, move IRQ routes to it. Open: does the parent endpoint retain a
  reference for automatic return on destroy (crash recovery)? Does split
  generalize to badge-range partitioning for IPC sources?
- **Endpoint combine semantics.** D22 introduces combine: merge N endpoints into
  one. Open: what happens to existing send caps on the originals? Transparent
  forwarding, dead handles (D11), or explicit migration?
- **Interrupt priority and routing.** D22 defers both. GICv3 8-bit priority:
  kernel-managed vs. exposed. SPI routing: kernel-managed vs. exposed. Both are
  kernel-internal GIC configuration, not tied to any object model.
- **Userspace timers.** Preemption timer is kernel-internal (D2). Userspace
  timer callbacks: kernel programs timer on behalf of Observer and deposits
  message when it fires. Connects to D2 scheduling model and D13 delivery.
- **Page size exposure.** D5 settles MMU-backed virtual memory; D9 settles
  variable-size kernel-managed memory objects. Open: expose page granularity to
  userspace (proven, universal) or hide it behind byte-addressed objects
  (archive's novel position, no precedent in surveyed systems)? Determines the
  memory object's interface granularity and the Space manager's external
  interface.
- ~~**Fault handler attachment.**~~ Settled by D20: per-Observer. Each Observer
  stores its own fault handler endpoint reference and badge. Per-address-space
  rejected (D6 grouping tension, D4 authority coupling, D17 badge-closure
  doesn't work, D11 cascade, D1 split storage). Per-both rejected (composes with
  kernel grouping D6 rejected).
- **Pager unavailability protocol.** What happens when a pager Observer is
  destroyed, blocked, or unresponsive while an Observer is faulting? Archive
  used fault handler chains (Observer → handler → … → kernel as root).
  Alternative: double fault = kill the faulting Observer. D18 adds a trigger:
  endpoint destroy with pending deferred faults — those Observers need cleanup.
- **Root/bootstrap fault handling.** D12 requires every Observer to have a fault
  handler, but the initial Observer has no userspace pager yet. The archive used
  "kernel as root fault handler" — the one place the kernel does internal
  resolution. Alternatives exist.
- **Pager reply/resume mechanism.** D14 settles the resume half:
  resume(observer_handle) as a typed kernel syscall. The Observer handle reaches
  the pager via capability transfer in the fault message (D13). Remaining: how
  does the pager signal that it has resolved the fault and prepared the address
  space? Is resume() alone sufficient, or does the pager also need to perform
  memory operations (map page, etc.) before calling resume()? The sequence (map
  → resume) vs. (resume-with-mapping) shapes the pager's syscall pattern.
- **D7 classification of fault traffic.** D12 says fault notifications go to
  pager Observers. D13 says all information delivery uses queued endpoints.
  Fault delivery is through normal IPC endpoints (kernel-as-sender). D18 settles
  the overflow case (deferred via pending list). Remaining: the specific
  mechanism by which the kernel enqueues fault messages in the normal (non-full)
  case, and fault message contents.
- ~~**Endpoint overflow policy.**~~ Settled by D18: error-to-sender, deferred
  fault delivery for kernel-as-sender. No per-endpoint policy modes.
- ~~**Coalescing / notification mechanism.**~~ Dissolved by D18: no overwrite
  means no cross-source data loss. Coalescing lives in shared memory + signaling
  (D9/D10), not in the endpoint mechanism.
- ~~**Multi-endpoint wait.**~~ Resolved by D19: badge fan-in (D15+D17) covers
  the common multi-source patterns (clients, faults, timers, replies on one
  endpoint). Residual cases (structurally distinct endpoints) use
  thread-per-source. A stateless multi-receive syscall is explicitly not
  foreclosed — Observer wait-state internals should accommodate N-endpoint
  blocking for future addition.
- **Badge downstream details.** D17 settles badge semantics (minter-assigned,
  mint right, opt-in per-badge tracking). Remaining: badge size (implementation
  detail, 64-bit default), send-once exemption encoding (consumed-by-use vs.
  closed-without-use — deferred with D16's send-once right encoding), badge on
  D16 kernel-created send-once caps (Call() badge assignment), max-badge-count /
  capacity semantics for tracked endpoints, badge-closure message format.
  (Badge-closure × overflow: resolved by D18 — dropped on full queue. Per-badge
  tracking × coalescing: dissolved by D18 — coalescing is not an endpoint
  mechanism; per-badge map serves tracking only.)
- ~~**Fault handler representation.**~~ Settled by D21: cap-table entry. The
  handler is a regular capability in the Observer's D8 flat table at a
  kernel-reserved slot index. D11 destroy-invalidation, D17 badge-closure, and
  D8 ABA protection all operate automatically. Archive divergence: archive chose
  kernel-internal, explained by absence of D17 badge-closure in the archive's
  derivation context.
- **Message format.** Size, slot count, capability transfer encoding, badge
  placement. The archive chose 4 slots (32 bytes), cap_mask bitmask, badge from
  capability. Interacts with D8 (cap transfer from table to table), D15 (badge
  must fit in message), and D16 (send-once reply cap must fit in message cap
  slots; Call() must encode "include my reply cap in slot N"). ~~Journal 023's
  ownership-transfer timing concern is dissolved by D24: ownership transfer is a
  cap-system invariant, not an IPC mechanism, so the message format is fully
  independent.~~
- **Send-once right encoding.** D16 introduces send-once as a general-purpose
  right in D8's rights mask. How it is represented (a right bit, a modifier on
  the send right, or a separate field) is an entry-layout detail deferred with
  D8's open entry-layout questions.
- **IPC fast-path conditions.** When does direct process switch occur? Receiver
  waiting? Priority check? seL4 fastpath requires no higher-priority runnable.
- **Specific syscall surface.** D7 settles two mechanism families but not the
  exact set. D14 adds resume() and confirms destroy() applies to Observers. The
  archive's 10-syscall design is a data point. Depends on IPC model, Observer
  creation API, Observer rights model, and D9 (memory objects).
- **Address space lifecycle.** D10 introduces the address space as a kernel
  object. When is it destroyed? Last capability dropped? Last Observer unbound?
  Interacts with revocation model.
- **Boot / bring-up model.** BSP-then-APs vs symmetric bring-up. Touches A2 but
  not derived.
- **Explicit unmap() semantics.** D24 settles auto-unmap on last-cap-close.
  Open: does explicit unmap() still exist? Likely yes — remap at a different
  address requires unmap + map while retaining the cap. The cap is not affected
  by explicit unmap; only the mapping moves. The invariant's auto-unmap is a
  supplement to explicit unmap, not a replacement.
- **Sub-page packing under D24.** D24's auto-unmap operates at page granularity
  (the MMU works in pages). If the kernel packs multiple small memory objects
  onto one physical page, closing the last cap to one object can't unmap the
  shared page without affecting the other. Resolution options: no packing (each
  object gets its own page — internal fragmentation), copy co-located objects on
  unmap (expensive), or accept that sub-page objects don't benefit from auto-
  unmap. Kernel-internal implementation concern, but D24 makes it load-bearing.
- **Space budget transfer on cap move.** When a memory-object cap is moved from
  one Observer to another, does the budget charge for the object's physical
  backing transfer to the receiver's Space? Stay with the original creator?
  Interacts with D3 (one logical Space manager) and D8 (typed-memory backing).

---

## Journal index

- `001-per-core-hot-path.md` — reasoning for D1: hot/cold split, IPI
  coordination, landscape check vs seL4 BKL and Barrelfish multikernel.
- `002-per-core-schedulers.md` — reasoning for D2: per-core algorithms, abstract
  vs algorithm-specific properties on the Observer, migration implications.
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
- `006-observer-is-execution-unit.md` — reasoning for D6: vocabulary + D2 force
  Observer = single schedulable entity; no kernel grouping because D4
  capabilities handle lifecycle and A3 makes grouping non-universal; seL4
  validates approach.
- `007-scope-of-capability-mediation.md` — reasoning for D7: A4 trust-model
  asymmetry, D1 hot-path dispatch, IPC model coupling all favor split; unified
  hides trust boundary; full fragmentation rejected on A5; archive convergence.
- `008-capability-table-structure.md` — reasoning for D8: D7 narrows table role
  to designation/rights lookup (not dispatch), removing CNode tree
  justification; A5 confirms CNode management is interface complexity;
  typed-memory backing for explicit accounting; table sharing deferred to
  Observer-Space binding.
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
- `011-base-revocation-primitive.md` — reasoning for D11: two-level
  decomposition (base primitive vs. add-ons); four workload patterns
  (adversarial, failure-mode, pressure response, structural cascade) establish
  terminate-by-force as essential under A3, rejecting Base-A; generational slot
  tag closes D8's deferred ABA question; add-ons (generation-as-revocation, CDT,
  badges) deferred jointly with IPC model because their alternatives depend on
  IPC-level mechanisms.
- `012-fault-delegation.md` — reasoning for D12: three independent paths (A4 no
  background paging, A3 no single policy, A5 dispatch interface smaller than
  policy-configuration interface) converge on delegation; self-paging foreclosed
  by A5; kernel-internal foreclosed by A4+A3; hybrid boundary is
  workload-dependent policy (A3); archive convergence on full delegation.
- `013-ipc-model.md` — reasoning for D13: sync-only foreclosed by A3+D12; queued
  endpoints with direct-switch fast path subsume both sync and async patterns;
  archive convergence on same model via independent paths (Time transfer,
  message unification); coalescing tension documented; tentative pending
  downstream cluster (overflow, coalescing, notification, multi-wait, message
  format).
- `014-observer-is-capability-held.md` — reasoning for D14: derivation chain
  D12→D7→D4→D8→D11 forces Observer as capability-held type; archive explored
  alternative (IPC indirection) and reversed it; 100% landscape convergence;
  settles fault resume as resume(observer_handle) typed syscall; six downstream
  questions opened (creation API, rights, clonability, suspend, fault handler
  attachment, Time reclamation).
- `015-endpoint-shape.md` — reasoning for D15: three convergent paths (D8+D11
  structural consistency, D12+D13 many-to-one composition, A3+capability
  topology) settle unidirectional many-to-many with send/receive rights;
  bidirectional (Zircon) rejected for structural exceptions to D8+D11 and
  aggregation requirement weakening D13; QNX constrained model dominated; peer
  disconnection gap addressable via badge-closure notifications (deferred to
  badge semantics).
- `016-reply-cap-mechanism.md` — reasoning for D16: pre-allocated reply endpoint
  (regular endpoint) with send-once cap; D14 decouples fault resume from IPC
  reply, removing archive's unification argument; send-once is general-purpose
  use-limited attenuation (Mach precedent), not reply-specific; dedicated Reply
  type rejected (optimization achievable behind endpoint interface); archive
  convergence on same object model, refined with send-once.
- `017-badge-semantics.md` — reasoning for D17: D15's many-to-one patterns
  require sender identification; minter-assigned because identification (key
  into receiver state) requires receiver-controlled values; mint right as third
  independent right in D8's rights mask (D4 consistency, budget alignment);
  opt-in per-badge lifecycle tracking resolves A3/A4 tension (not all workloads
  need it, but those that do should not fall back to polling); five tensions
  accepted for tracked endpoints; archive convergence on representation and
  assignment, mint right and lifecycle tracking are new.
- `018-endpoint-overflow-policy.md` — reasoning for D18: workload decomposition
  shows only error-to-sender is irreducible; coalescing is reducible to shared
  memory + signaling (landscape §3.2 standard pattern); D13 coalescing tension
  dissolves (no overwrite = no cross-source data loss); kernel-as-sender (D12)
  fault delivery via deferred pending list (intrusive linked list through
  Observer objects, zero allocation); badge-closure dropped on full queue
  (receiver discovers staleness lazily); archive convergence on error-to-sender.
- `019-multi-endpoint-wait.md` — resolves D13 trigger #3: badge fan-in (D15+D17)
  covers common multi-source patterns (clients, faults, timers, replies
  consolidated onto one endpoint); four mechanisms evaluated (no primitive, port
  set, multi-receive, endpoint binding); no kernel primitive needed now;
  multi-receive syscall explicitly not foreclosed; Observer wait-state should
  accommodate N-endpoint blocking for future addition.
- `020-fault-handler-attachment.md` — reasoning for D20: five tensions with
  per-address-space (D6 grouping, D4 authority coupling, D17 badge-closure, D11
  cascade, D1 hot path) all favor per-Observer; per-both rejected (composes with
  kernel grouping D6 rejected); per-region foreclosed by D9+D5; representation
  (cap-table entry vs. kernel-internal) strongly indicated toward cap-table
  entry by D17 badge-closure but not formally settled; archive convergence.
- `021-fault-handler-representation.md` — reasoning for D21: three convergent
  arguments (D11 destroy-invalidation via cap-table walk, D17 badge-closure via
  generic cap-close, D8 ABA slot-tag protection) settle the fault handler as a
  cap-table entry at a kernel-reserved slot index; kernel-internal rejected
  (requires parallel tracking structure, explicit badge-closure coupling,
  dangling-pointer prevention — rebuilds what the capability system provides);
  archive divergence explained by absence of D17 in archive's chain.
- `022-interrupt-model.md` — reasoning for D22: three convergent paths (A4 no
  background processing, A3 no single policy, A5 dispatch < policy interface)
  parallel D12 exactly; no separate IRQ object type — interrupts are endpoint
  traffic; D13 commits delivery, D16 provides ack via send-once, D17 provides
  badge identification; endpoint split/combine for IRQ range delegation;
  derivation trail: IRQControl factory → IRQ objects → endpoints-only, each
  revision eliminating a proposed type by applying D4/D13/D16 more thoroughly;
  every identified downside traces to a parent decision; archive convergence on
  unification principle ("all information delivery is one mechanism").
- `023-research-implications.md` — analysis of 2022–2026 bleeding-edge OS
  research against settled decisions. Not a derivation. Identifies: framekernel
  pattern (Asterinas) as systematic realization of A1 trust boundaries;
  verification readiness (Verus/Flux) enabled by framekernel; ownership-transfer
  IPC (PLOS 2023) as timing-sensitive input to message format; capability graph
  completeness (TreeSLS) as architectural discipline; time-as-capability (seL4
  MCS, S3K) as frame for open Time questions. Records explicit non-fits
  (Theseus, MnemOS, Hubris, io_uring) and research validation of D5, D13, D18.
- `024-observer-handle-clonability.md` — reasoning for D23: five convergent
  structural arguments (D4 attenuation, D8 uniformity, D12/D20 fault delivery,
  D11 orphan risk, type consistency) settle Observer handles as clonable;
  non-clonable rejected on structural costs exceeding narrow benefit; archive's
  handle=handler unification dissolved by D20/D21; duplicate-control right
  deferred to Observer rights model; landscape convergence (100%).
- `025-cap-mapping-invariant.md` — reasoning for D24: exploration started as "is
  ownership-transfer IPC in scope?" and reframed to cap-system invariant. Four
  IPC-level ownership-transfer mechanisms evaluated and rejected (all place
  page-table work on IPC hot path or require IPC changes). The invariant (no cap
  → no mapping) achieves the same safety property at the cold-path cap-close
  layer. Performance analysis: shared memory is already zero-copy, ownership
  transfer adds cost, benefit is safety not performance. Cross-core analysis:
  single-Observer address space requires no broadcast. Novel position: no
  surveyed system auto-unmaps on cap close; justified by D4's "designation =
  authority" commitment. Archive divergence: archive proposed IPC-level
  ownership transfer; this derivation dominates it.

---

## Research

See `design/research/` for descriptive prior-art studies and
`design/landscape.md` for the survey of how other kernels resolved each major
design decision.
