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

Does NOT settle: fault handler attachment (Observer vs. address space), pager
unavailability protocol (chains vs. double-fault-kill), root/bootstrap case
mechanism, fault message contents, pager reply/resume mechanism, D7
classification of fault traffic (IPC vs. dedicated mechanism), Observer minimum
schema (though fault handler field is now confirmed structurally required).

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

Does NOT settle: overflow policy (error/overwrite/fault), coalescing mechanism,
multi-endpoint wait, endpoint shape (uni/bidirectional, topology), message
format, reply routing, queue capacity policy, IPC fast-path conditions, D11
badge semantics, D12 fault delivery specifics.

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
- **Status:** tentative — accepted to enable derivation of the downstream
  endpoint-shape cluster (overflow, coalescing, notification, multi-wait,
  message format). Revisit if: the coalescing gap cannot be solved without a
  full second primitive (undermining the "one mechanism" value); bounded queue
  capacity creates unsolvable priority inversion or deadlock patterns; the
  multi-endpoint wait problem has no clean solution.
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
Observer handle clonability, fault handler attachment (per-Observer vs.
per-address-space), Time reclamation on destroy, Observer minimum schema.

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
  destination? Affects D2's migration story.
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
  - ABA slot tag). D13 (queued endpoints) now enables exploring: badges
    (per-capability, attached by kernel to messages — natural fit for queued
    model); endpoint rotation via destroy (D11 provides destroy; endpoint
    lifecycle needed); generation-as-revocation (O(1) mass invalidation;
    alternative is endpoint rotation). Still deferred: CDT (selective revocation
    of a subtree); who authorizes destroy; strong vs. weak cross-core
    prompt-effect policy; destroy cleanup protocol (inline vs. preemptible).
- **Observer minimum schema.** D6 settles that an Observer is a single execution
  unit. D14 settles that Observer is a capability-held kernel object type. The
  concrete field set (register state, TTBR, capability table pointer, Time
  binding, scheduling state, fault handler, Observer state: runnable/blocked/
  faulted) needs formal derivation in the current chain. Archive journal/004
  derived a first-principles minimum. D12 confirms the fault handler field is
  structurally required. D14 confirms lifecycle state tracking is required.
- **Address space binding mutability.** D10 settles the address space as a
  first-class object that Observers bind to. Open: is the binding immutable (set
  at Observer creation) or rebindable at runtime? If rebindable, what happens to
  TLB entries when an Observer changes address space?
- **Observer creation API shape.** D14 settles Observer as capability-held but
  not the creation interface. Create-then-configure (seL4 — inert Observer,
  configured via cap ops, started separately) vs. all-params-upfront (archive —
  one syscall). Minimum inputs: Space, Time, address space (D10), fault handler
  endpoint (D12). Open: initial PC/SP, initial capabilities, create vs. start as
  separate operations.
- **Observer rights model.** D14 settles resume and destroy as minimum. Open:
  suspend (external pause), inspect register state (debugging), modify
  scheduling properties (D2), change fault handler, change address space binding
  (D10 binding mutability). Each right = a typed kernel syscall under D7.
- **Observer handle clonability.** Clonable: multiple independent lifecycle
  managers, flexible delegation (parent delegates kill to sibling). Non-clonable
  (like Time): exactly one manager, enables handle=handler unification.
- **Suspend as distinct from faulted.** Is there external suspension (not caused
  by fault)? If yes, Observer state has four values (runnable, blocked, faulted,
  externally-suspended). Use cases: debugging, checkpointing, resource pressure.
- **Time reclamation on Observer destroy.** Observer holds one Time (D6). On
  destroy: return to destroyer? To creator? Destroy the Time? Interacts with
  Time's non-clonable property.
- **Can Observers share capability tables?** D8 settles per-Observer tables with
  no sharing. D10 settles first-class address spaces with multi-Observer binding
  — same-address-space Observer groups are now a supported pattern. Revisit as a
  D8 downstream: does same-address-space sharing create sufficient pressure for
  shared capability tables, or is per-Observer authority (with explicit
  capability transfer) sufficient?
- **Interrupt model (device interrupts, not exceptions).** Who owns device
  interrupts? Per-core or routed? Kernel-handled or delegated to userspace
  drivers?
- **Page size exposure.** D5 settles MMU-backed virtual memory; D9 settles
  variable-size kernel-managed memory objects. Open: expose page granularity to
  userspace (proven, universal) or hide it behind byte-addressed objects
  (archive's novel position, no precedent in surveyed systems)? Determines the
  memory object's interface granularity and the Space manager's external
  interface.
- **Fault handler attachment.** D12 settles delegation but not where the fault
  handler attaches. D14 (Observer as capability-held type) provides the Observer
  handle as a natural per-Observer configuration noun. Per-Observer (archive
  choice) allows different fault policies for Observers sharing an address
  space. Per-address-space (D10 natural fit) avoids redundant handling. Possibly
  both (address space default, Observer override).
- **Pager unavailability protocol.** What happens when a pager Observer is
  destroyed, blocked, or unresponsive while an Observer is faulting? Archive
  used fault handler chains (Observer → handler → … → kernel as root).
  Alternative: double fault = kill the faulting Observer.
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
  Fault delivery is through normal IPC endpoints (kernel-as-sender). Remaining:
  the specific mechanism by which the kernel enqueues fault messages.
- **Endpoint overflow policy.** D13 defers: what happens when the queue is full?
  Error to sender (archive), overwrite-oldest (ring buffer / coalescing), fault
  the sender? Per-endpoint policy at creation? Determines whether the coalescing
  gap (journal/013) is solvable within the queued model.
- **Coalescing / notification mechanism.** D13 documents a three-way tension:
  queued endpoints + capacity-1 overwrite + shared endpoints = cross-source data
  loss. May require a separate lightweight primitive, per-badge slots, one
  endpoint per source, or acceptance of the tension. Connected to overflow
  policy.
- **Multi-endpoint wait.** How does an Observer wait on multiple endpoints
  simultaneously? Port aggregator (Zircon), multi-receive syscall, notification
  binding to Observer? The "select/epoll" problem for the queued model.
- **Endpoint shape.** Unidirectional vs. bidirectional. Many-to-many vs.
  constrained topology. The archive chose unidirectional, many-to-many, topology
  via capabilities. Send/receive as object-rights.
- **Message format.** Size, slot count, capability transfer encoding, badge
  placement. The archive chose 4 slots (32 bytes), cap_mask bitmask, badge from
  capability. Interacts with D8 (cap transfer from table to table).
- **Reply routing.** Reply cap in message (archive for IPC), resume() syscall
  (archive for faults). Does D7's split model require the distinction?
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

---

## Research

See `design/research/` for descriptive prior-art studies and
`design/landscape.md` for the survey of how other kernels resolved each major
design decision.
