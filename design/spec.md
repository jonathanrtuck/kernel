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
  memory (Space). Each Observer is an independent perspective in which a
  specific computation unfolds. An Observer holds capabilities to one or more
  Spaces and exactly one Time. Memory is accessed through capability-addressed
  (Space, offset) pairs; the kernel manages the underlying virtual address
  mapping (D26). The Observer never chooses or manages virtual addresses.
  SMT-concurrent workloads, when hardware supports them, are expressed as
  multiple Observers sharing a Space, each with its own Time on its own logical
  core — not as a single Observer with multiple Times. This keeps the one-Time
  commitment intact across SMT and non-SMT hardware.

_Capitalized-vs-lowercase convention:_ Capitalized terms (Space, Time, Observer)
are kernel proper nouns — names of specific concepts in this kernel's design,
with the semantics defined here. Lowercase equivalents from broader OS
literature (memory object, thread) refer to the same kind of thing but without
claiming this kernel's specific semantics. The two are interchangeable in prose;
capitalization signals "speaking of our concept" vs. "speaking of the general
concept."

_Naming note:_ these terms are for internal thinking and will not necessarily
appear in public API names. Public naming is deferred until v0.1.

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

### D5 — MMU-backed virtual memory for Space isolation

The kernel requires the ARM64 MMU to be enabled and uses it for inter-Observer
memory isolation. The MMU enforces that an Observer can only access Spaces it
holds capabilities to. Each Observer has a per-Observer page table (L0 root);
page table subtrees for individual Spaces are shared across Observers holding
the same Space cap (D26). Three independent paths converge: (1) A2 hardware
requires MMU enabled for cached memory access — page tables must exist; (2) A3 +
A5 require hardware-enforced inter-Observer isolation, and the MMU is the only
such mechanism on ARM64; (3) philosophy "use what the hardware provides." Every
alternative (physical-only, language-safety isolation, CHERI-only, SFI) is
foreclosed by axioms or hardware facts.

Does NOT settle: page size exposure vs. hiding (settled by D25: exposed), memory
object model (what capabilities designate as memory), fault delegation
(kernel-internal vs. userspace pager), or CHERI forward-compatibility. These are
one level down. The memory interface should be shaped around objects and
permissions, not page-table-specific concepts, to avoid foreclosing CHERI as a
future complementary enforcement layer.

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
program counter, one Time, one capability table. The kernel has no "process"
concept — "process" is a userspace convention (a group of Observers sharing
Space caps). Multi-threaded execution in shared memory is multiple Observers
holding caps to the same Space, each with its own Time. Green threads and
cooperative concurrency are internal to an Observer (userspace, invisible to
kernel).

The kernel provides no Observer-grouping mechanism. Grouping is neither
essential complexity (D4 capabilities handle Observer lifecycle without the
target's cooperation) nor workload-universal (A3 — not all workloads need
groups). Userspace builds grouping policy from capabilities; the kernel provides
the mechanism.

Does NOT settle: Observer minimum schema (concrete fields need formal
derivation), Observer lifecycle operations beyond the D14 minimum (creation API,
rights model, suspend, clonability), whether Observers can share capability
tables. (D8 settled capability table structure; D14 settled Observer as
capability-held object type with resume and destroy as minimum operations.)

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
  structure with per-Observer tables; D26 settled capability-addressed memory
  with Space sharing through caps — no grouping pressure found.)
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

The physical memory backing the table is drawn from the Observer's Spaces, not
from a kernel-internal pool. When the table is full and a new capability must be
stored, the kernel faults the Observer; the fault handler provides more memory,
then retries.

The CNode tree model (seL4) was rejected: D7 eliminates the dispatch role that
CNode trees structurally serve, and A5 creates tension with CNode management
pushed to userspace as interface complexity. Per-core replicated tables
(Barrelfish) were rejected on D1 + A2 grounds. Unified cap/page tables
(Composite) were rejected on D5 + A2 grounds.

Each Observer always has its own table. Table sharing between Observers is
deferred — it is not a table-structure question.

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
  Space manager — table memory drawn from the Observer's Spaces),
  `design/research/authority-models.md` §4, §5.5 (seL4 CNode tree vs. Zircon
  flat table; namespace shape comparison), `design/landscape.md` §1.1
  (capability representation survey).
- **Status:** settled — revisit if D7 is revised (unified model would
  re-motivate CNode dispatch), if the capability-addressed memory model (D26)
  reveals that per-Observer tables force essential sharing complexity into
  userspace, or if the revocation model requires CDT and the absence of tree
  structure makes it impractical.
- **Journal:** `journal/008-capability-table-structure.md`.

### D9 — Variable-size kernel-managed memory objects

The capability-designated memory resource is a variable-size, kernel-managed
memory object (Space). Observers hold capabilities to Spaces; the kernel
allocates physical pages behind them and manages the MMU mappings that make them
accessible. An Observer accesses a Space through capability-addressed (Space,
offset) pairs (D26) — holding a Space cap is sufficient for access. Sharing is
through capability transfer — multiple Observers holding capabilities to the
same Space. Which physical pages back a Space is a kernel-internal concern.

The seL4 untyped-memory model (userspace manages physical allocation and
constructs page tables) was rejected: A5 forecloses pushing memory management
complexity into userspace, and D8's precedent (kernel-managed flat capability
table) established the pattern of kernel-internal management with resource
accounting charged to the Observer's Space. Page-granularity objects (one
capability per hardware page) were rejected: they force page size exposure,
violate D5's CHERI forward-compatibility note, and cause capability
proliferation.

Does NOT settle: ~~page size exposure (byte-addressed vs. page-addressed
interface)~~ (settled by D25: exposed; hiding rejected), specific operations on
Spaces (split, COW/clone, resize), Space rights, fault delegation, or how an
Observer acquires additional Spaces at runtime.

- **Rests on:** A5 (kernel absorbs complexity — same argument that rejected
  CNode trees in D8 applies to memory management), D5 (MMU-backed virtual
  memory; CHERI note requires objects-and-permissions interface, not
  page-table-specific concepts), D4 (capability-designated; sharing through
  capability transfer), D7 (memory operations are typed kernel syscalls, not
  IPC), D8 (precedent: kernel-managed structure with typed-memory backing from
  the Observer's Spaces), D3 (Space manager is the single allocation interface;
  memory object backing flows through it), `design/landscape.md` §2.1–2.3 (four
  families surveyed; two-step create/map dominant).
- **Status:** settled — revisit if A5 is revised (would re-open
  userspace-managed models), or if D5's CHERI note is dropped (would re-open
  page-specific interfaces). (Capability-addressed memory model settled by D26 —
  Space access through caps, no binding step.)
- **Journal:** `journal/009-memory-object-model.md`.

### ~~D10 — The address space is a first-class kernel object~~

**Superseded by D26** (capability-addressed memory). The address space is no
longer a user-visible kernel object type. The page table is a kernel-internal
mechanism that materializes each Observer's Space cap holdings for the MMU.

D10's three original derivation paths (A5 mapping consistency, D1 TLB pressure,
D4 independent delegation) are all satisfied by the capability-addressed model:
Space caps provide consistent access (A5), per-Space VA bases enable shared page
table subtrees (D1), and Space caps are the delegation unit (D4). The concerns
that motivated D10 are real; the response is different — Space caps replace the
address space object.

- **Original journal:** `journal/010-address-space-is-first-class.md`.
- **Supersession journal:** `journal/027-capability-addressed-memory.md`.

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
(Observers, Spaces, endpoints), close-only cannot express this; the userspace
construction that would substitute cannot interpose at the MMU level and must
route through a kernel mechanism that is itself a form of authoritative destroy
under another name. Forcing this construction into userspace violates A5 via O4
(a).

Add-on mechanisms for mass invalidation (generation-as-revocation) and selective
revocation (CDT, badges) are deferred. Their value depends on the IPC model:
endpoint rotation serves mass invalidation only if endpoint-like kernel objects
exist; badges ride on IPC; proxy indirection requires IPC mediation. Committing
to add-ons before the IPC model is settled would either skip a level or
overspend on features whose alternatives may be free.

Does NOT settle: mass invalidation (deferred with IPC), selective revocation
(deferred with IPC), who authorizes destroy, cross-core prompt-effect policy
(strong vs. weak), destroy cleanup protocol (inline vs. preemptible), ABA tag
size and encoding, memory reclamation of freed slots, table-full fault ↔
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
  a downstream lifecycle derivation (Observer, Space) reveals the base primitive
  is structurally insufficient.
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

Queue memory drawn from the creator's Spaces (D8 pattern). Fixed capacity at
creation. Memory per queued message ~48 bytes (register-sized).

Does NOT settle: ~~message format~~ (settled by D28), queue capacity policy, IPC
fast-path conditions, D12 fault delivery specifics. (Endpoint shape settled by
D15. Overflow policy settled by D18. Coalescing dissolved by D18. Reply routing
settled by D16. Badge semantics settled by D17. Multi-endpoint wait resolved by
D19. Message format settled by D28.)

- **Rests on:** A3 (generic — both sync and async patterns required; independent
  path), A4 (purely reactive — no kernel message broker; IPC dispatch within
  syscall handlers), D1 (hot-path — direct-switch fast path achieves D1's
  minimal per-core hot-path requirement), D7 (split model — IPC as a dedicated
  mechanism family; D7 notes "couples naturally with async"), D12 (fault traffic
  is IPC — kernel-as-sender requires non-blocking deposit), D4 (capability-
  mediated — endpoints designated by capabilities), D3 + D8 (queue memory drawn
  from creator's Spaces; typed-memory-backing pattern), `design/landscape.md`
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
Time, and endpoint (D13) as the fourth type. Lifecycle operations — at minimum
resume and destroy — are typed kernel syscalls (D7) taking Observer capability
handles. The capability's rights mask governs permitted operations. D11's
destroy provides termination; outstanding capabilities become dead handles.

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
disambiguation (depends on badge semantics). ~~Message format interaction~~
settled by D28: reply cap is a kernel-injected dedicated field (not a user cap
slot), paralleling badge.

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
mint-right distribution, aligning badge population growth with resource
authority.

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
coalescing. Coalescing workloads use shared memory + signaling (D9 shared Space
caps + capacity-1 endpoints) — the standard microkernel architecture (landscape
§3.2).

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
  cold-path), D9 (shared memory for coalescing — sharing Space caps makes
  kernel-level coalescing reducible), D17 (badge-closure dropped on full — not a
  correctness issue; per-badge tracking × coalescing interaction dissolved),
  `design/landscape.md` §3.2 (every production microkernel converges on shared
  memory + IPC signaling for data-plane communication), §5.1 (mask-on-delivery
  for interrupt coalescing).
- **Status:** settled — revisit if D13 is revised (different IPC model may
  change overflow semantics), if a downstream derivation reveals that dropped
  badge-closure notifications create a correctness issue (not just a timeliness
  issue), or if the interrupt model derivation reveals that error-on-full
  combined with interrupt masking creates unsolvable delivery gaps.
- **Journal:** `journal/018-endpoint-overflow-policy.md`.

### D20 — Per-Observer fault handler attachment

The fault handler attaches to the Observer. Each Observer stores a fault handler
endpoint reference and a badge. On fault, the kernel reads both from the
faulting Observer's struct and delivers a fault notification to the handler
endpoint with the stored badge, plus the faulting Observer's capability handle
via cap transfer (D14).

Every Observer creation must supply a fault handler endpoint and badge (D12
invariant enforced at creation time). Redundant configuration when N Observers
want the same handler is a userspace ergonomics cost, not kernel complexity — a
library function absorbs it.

Does NOT settle: fault handler mutability (part of Observer rights model), fault
handler in Observer creation API shape, pager unavailability protocol,
root/bootstrap fault handling. (Fault handler representation settled by D21:
cap-table entry.)

- **Rests on:** D6 (no kernel grouping — per-Observer is the natural attachment
  level; independent path), D4 (designation = authority — per-Observer allows
  independent delegation of fault handler configuration authority; independent
  path), D17 (badge-closure lifecycle visibility works only with per-Observer
  reference; fault handler badge is structurally required per-Observer
  regardless of endpoint attachment; independent path), D12 (every Observer must
  have a fault handler — maps to local invariant with per-Observer), D14
  (Observer as capability-held type — provides the natural configuration noun),
  D1 (hot-path simplicity — single cache-line access),
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
pager unavailability protocol (D21 makes detection clear: dead cap-table entry).

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
identically to every other kernel object type (Spaces, endpoints). Multiple
entities can hold capabilities to the same Observer, each with independent
rights masks. No type-specific exceptions in D8's table management.

Non-clonable was rejected on five convergent structural arguments: D4
attenuation requires cloning (foreclosed), D8 uniformity requires no
type-specific exceptions (broken), D12/D20 fault delivery requires cap-copy
(requires new mechanism), D11 close creates orphan risk (requires new
mechanism), and type consistency (Observer would be the sole non-clonable type
among four). Non-clonable's sole benefit — kernel-enforced single-manager — is
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
  Observer unreachable through cap graph; independent path), D15 + D9 (type
  consistency — all other kernel object types are clonable; Observer would be
  sole exception), `design/research/execution-unit.md` (100% landscape
  convergence — all surveyed capability systems make execution-unit handles
  clonable), `design/research/authority-models.md` §4 (seL4 CNode_Copy, Zircon
  handle_duplicate — uniform capability copying for all object types).
- **Status:** settled — revisit if D11 is revised (changes the refcount/destroy
  model that makes multi-holder safe), if D20/D21 are revised (reopens handle =
  handler unification), or if the Observer rights model derivation reveals that
  clonability creates essential complexity that non-clonable would have avoided.
- **Journal:** `journal/024-observer-handle-clonability.md`.

### D24 — Cap-mapping invariant: no cap → no mapping

Under capability-addressed memory (D26), the cap-mapping invariant is a
structural property, not an enforced invariant. An Observer's page table
contains entries only for Spaces it holds capabilities to. When an Observer
loses its last capability to a Space (via close, move, or destroy), the kernel
removes the corresponding page table entries. The Observer cannot access memory
it has no capability for — the MMU state is a materialized view of the cap
state.

There is no separate map() or unmap() operation. Holding a Space cap is
sufficient for access (D26); losing the cap removes access. Both directions are
driven by capability state.

Ownership-transfer IPC (the PLOS 2023 concept flagged by journal 023) is not a
separate mechanism. It falls out naturally: "move" is clone-to-receiver +
close-on-sender. The close removes the sender's page table entries for that
Space. No IPC-level changes, no message-format changes, no D7 classification
ambiguity. The safety property (sender can't access after send) is achieved as a
cap-system structural property at the cold-path cap-close layer rather than as
an IPC mechanism at the hot-path send layer.

Per-Space VA bases (D26) mean that page table subtree cleanup on cap loss is
local to the losing Observer's page table — no cross-core broadcast needed. The
kernel maintains per-Observer Space cap reference counts; when the count for a
Space reaches zero, the page table subtree for that Space is detached from the
Observer's L0 table.

Does NOT settle: sub-page packing strategy (kernel-internal implementation
concern), kernel-internal memory cost on cap transfer (page table entries for
new holder), D9 Space operations.

- **Rests on:** D4 (designation = authority — the invariant extends D4 to MMU
  access; the MMU mapping is a form of authority governed by the capability
  system), D26 (capability-addressed memory — the page table is a materialized
  view of cap holdings; the invariant is structural rather than enforced), D9
  (variable-size kernel-managed Spaces — the invariant operates on D9 Spaces),
  D8 (flat cap table — cap-table mutations drive page table updates), D11 (base
  revocation — close triggers page table cleanup; destroy of a Space requires
  cleanup across all holders), D5 (MMU-backed virtual memory — the invariant
  synchronizes the two enforcement layers; CHERI forward-compatible), A1 (Rust
  ownership — the invariant makes the kernel's external interface consistent
  with Rust's "if you don't own it, you can't use it" model; not a mandate from
  A1 but a natural alignment), `design/research/bleeding-edge-os-landscape.md`
  §9 (PLOS 2023 ownership-transfer IPC, Singularity linear types, LionsOS
  data/metadata separation — prior art on the safety property this invariant
  provides), `design/landscape.md` §3.2 (shared memory as universal data plane —
  the invariant does not replace this pattern; shared Space caps + signaling
  remains the data plane, with the invariant providing automatic cleanup).
- **Status:** settled — revisit if D4 is revised (weakening "designation =
  authority" removes the strongest motivation), if D9 is revised (different
  memory model may change the cap/mapping relationship), or if the sub-page
  packing question reveals that the invariant creates unacceptable internal
  fragmentation for small Spaces.
- **Journal:** `journal/025-cap-mapping-invariant.md`,
  `journal/027-capability-addressed-memory.md`.

### D25 — Page size is exposed to userspace

Observers can query the page size and must account for page granularity in
memory operations. Full hiding (byte-addressed memory objects with no page
concept in the interface) is rejected.

The exploration began with four axioms (A2, A3, A5, D5 CHERI note) pushing
toward hiding and one settled decision (D24 cap-mapping invariant) pushing
toward exposure. A concrete scenario resolved the tension: two separate 4KB
Spaces on 16KB hardware. Every hiding strategy fails — through unpredictable
errors, security violations (sub-page packing lets an Observer access memory it
has no cap for — D4 and D24 violation), or hardware-dependent behavior.

Page-size knowledge is essential complexity (O4). Hiding it does not eliminate
it — it converts predictable constraints into unpredictable, hardware-dependent
failures. The Observer is better served by knowing the constraint.

The A2/A3/D5 tensions under exposure are bounded: page size is a queryable
runtime constant (not hard-coded), code that queries and aligns is portable
across 4K/16K/64K hardware, and the query interface survives on CHERI hardware
(the value changes to capability alignment granularity, the interface shape
persists).

Does NOT settle: whether the interface is fully page-addressed (all operations
require page-aligned inputs) or implicitly rounded (operations accept byte
values, kernel rounds, PAGE_SIZE queryable for Observers that want to optimize).
This is one level down.

- **Rests on:** D4 (designation = authority — sub-page packing under hiding
  creates unauthorized access, the decisive security argument), D24 (cap-mapping
  invariant — auto-unmap at page granularity makes sub-page packing
  load-bearing; hiding + D24 is structurally incompatible for shared objects),
  D9 (variable-size memory objects — the interface granularity depends on this;
  D9 deferred it as "one level down"), D5 (MMU-backed virtual memory — the MMU
  operates in pages; CHERI note tension accepted as bounded), A2 (ARM64 supports
  4K/16K/64K — multi-granule is the hardware reality that hiding attempts to
  absorb), A3 (generic — queryable page size is portable; hidden page size
  creates hardware-dependent failures), O4 (essential complexity — page-size
  knowledge cannot be eliminated by hiding, only made worse),
  `design/landscape.md` §2.7 (page size hiding appears nowhere in surveyed
  systems — universal exposure is not coincidence but a consequence of the same
  essential-complexity argument).
- **Status:** settled — revisit if D24 is revised (removing auto-unmap
  eliminates the sub-page packing argument, though D4 security argument
  remains), if the CHERI note in D5 is strengthened to require byte-granularity
  interfaces (would reopen the tension), or if a downstream derivation reveals
  that the implicit-rounding model (deferred) effectively re-hides page size in
  practice.
- **Journal:** `journal/026-page-size-exposure.md`.

### D26 — Capability-addressed memory

Observers access memory through (Space, offset) pairs. Holding a Space
capability is sufficient for access; the kernel manages all virtual address
assignment and page table maintenance internally. There is no separate map() or
unmap() operation. The Observer never chooses, manages, or observes virtual
addresses.

The kernel assigns each Space a VA base at creation time. The base is a property
of the Space — all holders see the same Space at the same VA. Each Observer has
its own L0 page table (root, pointed to by TTBR0) containing entries only for
Spaces it holds caps to. Page table subtrees (L1/L2/L3) for individual Spaces
are shared across Observers holding the same Space cap (reference-counted). This
provides O(Observers + Spaces) page table memory rather than O(Observers ×
Spaces).

On the hardware bridge: ARM64 instructions use flat virtual addresses. The
Observer stores a per-Space-cap base VA (provided by the kernel on cap
acquisition). Memory access is `base_of(Space) + offset` — one table load, one
add. The base table is small (one u64 per Space cap) and L1-hot. Per-access
overhead is ~2–5 cycles, negligible against memory latency and strictly cheaper
than the map() syscall the model eliminates.

Supersedes D10. The page table is no longer a user-visible concept; it is a
kernel-internal mechanism that materializes Space cap holdings for the MMU.
D24's cap-mapping invariant becomes a structural property of this model rather
than an enforced invariant.

Does NOT settle: base table management (kernel-maintained read-only page vs.
Observer-managed), cap rights for memory access (separate "access" right?),
demand fault vs. eager page table population, page table memory ownership (whose
Spaces back per-Observer page table structures), VA base reclamation policy for
long-lived systems.

- **Rests on:** D4 (designation = authority — holding a Space cap IS the
  authority; the model eliminates the gap between holding authority and
  exercising it), D5 (MMU-backed virtual memory — the MMU is the enforcement
  mechanism; the model uses it without exposing it), A3 (generic — runtime base
  lookup imposes no workload limits; the fixed bit-partition alternative was
  rejected as an A3 violation), A5 (kernel absorbs complexity — VA management
  moves into the kernel; Observers work with the simpler (cap, offset)
  abstraction), D8 (flat cap table — Space caps in the cap table drive page
  table state; cap mutations trigger page table updates), D9 (variable-size
  kernel-managed Spaces — the objects that get VA assignments), D24 (cap-mapping
  invariant — strengthened from enforced to structural), D12 (fault delegation —
  demand faults carry Space identity + offset to the pager, giving richer
  semantic information), `design/journal/027-capability-addressed-memory.md`
  (full exploration of the model, alternatives considered, hardware bridge
  analysis, performance data, impact analysis across all settled decisions).
- **Status:** settled — revisit if A3 is revised (removing generic-workload
  requirement would allow fixed bit-partition addressing), if D5 is revised to
  include CHERI (CHERI hardware capabilities could replace the runtime base
  lookup with hardware-native capability addressing), or if a downstream
  derivation reveals that the absence of explicit map()/unmap() creates
  essential complexity that the (cap, offset) model cannot absorb.
- **Journal:** `journal/027-capability-addressed-memory.md`.

### D27 — Flat Space cardinality

An Observer holds multiple independent Space caps directly in its D8 capability
table. Each Space cap is an independent entry — no kernel-tracked parent/child,
hierarchical, or structural relationships between Spaces. "Related Spaces" (a
program's code, data, heap) are a userspace convention, paralleling D6's
treatment of Observer grouping as userspace convention.

The hierarchical alternative (parent Space subdivided into children) was
rejected on five convergent grounds: D8 (first inter-entry structural
relationship in the flat table), D6 (grouping is userspace policy), D4
(hierarchy introduces implicit structural authority beyond designation), D11
(close/destroy would require cascade or orphan semantics), A3 (tree assumption
forecloses non-tree memory patterns such as shared libraries or peer ring
buffers).

Does NOT settle: Space operations (split, resize, COW/clone — D9 downstream),
provenance tracking (deferred as a potential kernel-internal optimization,
orthogonal to user-facing cardinality).

- **Rests on:** D8 (flat cap table — no inter-entry relationships; Space caps
  follow existing independent-entry pattern), D6 (no kernel grouping — Space
  grouping is policy, same as Observer grouping), D4 (designation = authority —
  each Space cap designates one resource; hierarchy would introduce implicit
  structural authority), D11 (close removes one cap; destroy invalidates one
  object — hierarchy would require cascade or orphan semantics), D26 (per-Space
  VA bases assigned independently at creation), A3 (generic — hierarchy forces
  tree assumption that not all workloads fit).
- **Status:** settled — revisit if D8 is revised to support inter-entry
  relationships, if D6 is revised to add kernel grouping, or if a downstream
  derivation (Space split, memory accounting) reveals that the absence of
  hierarchy forces essential complexity into userspace.
- **Journal:** `journal/028-space-cardinality.md`.

### D28 — Fixed-size IPC message format

An IPC message is a fixed-size control packet with structurally separate fields:
4 untyped data words (32 bytes), 1 user capability slot, a label in a dedicated
header field, a badge in a dedicated kernel-injected field, and a reply cap in a
dedicated kernel-injected field (present only on Call()).

Sender provides: label + 4 data words + 0-1 cap handle. Receiver sees: badge +
label + 4 data words + 0-1 remapped cap handle + reply cap (if Call). The kernel
transforms the message in transit: injects badge from the sending capability
(D17), translates cap handles from source table to destination table (D8), and
injects the reply cap for Call() (D16).

Data words and capability slots are structurally separate (not a shared budget).
Cap transfer is a categorically different kernel operation from data copying
(D8: validation, allocation, ABA tag management). Zero-cap vs. cap-bearing is
cheaply distinguishable (one field check), gating the fast path.

The reply cap is a dedicated kernel-injected field — not a user cap slot —
because D16 settles that the kernel creates it (parallel to badge). This keeps
the user's 1 cap slot free for payload caps during Call(), supporting the common
"request + delegated authority" RPC pattern.

Fault messages use the same format: the kernel generates label (fault type) + 4
data words (fault descriptor: Space identity, offset, access type) + 1 cap
(Observer handle for resume, D14). Full Observer state (registers, PC, PSTATE)
is accessible via inspect(observer_handle) — a D7 typed kernel operation. The
fault message carries the notification; state inspection is a separate
operation. This decomposition follows from D7's split model: IPC is one
mechanism family, resource operations are another.

Variable-length messages (seL4 model) were rejected: two copy paths, length
validation on every message, variable queue slot sizes — complexity serving
workloads already better served by shared-Space bulk transfer (D26). The
bitmask-over-unified-slots encoding (archive's cap_mask) was rejected: conflates
data copying with cap transfer, requires mask inspection even for zero-cap
messages.

Does NOT settle: fault message content details per fault type (VM fault,
cap-table-full, invalid syscall), badge-closure notification content, interrupt
message content, inspect() syscall shape, sender-side syscall encoding (which
registers carry what — A2 implementation detail), send-right gating of cap
transfer (Grant right), IPC fast-path conditions.

- **Rests on:** D13 (queued endpoints — message is what the queue holds; ~48
  byte per-slot estimate anchors the size), D16 (reply cap mechanism — the
  kernel creates the reply cap, motivating the dedicated field), D17 (badge is
  kernel-injected — motivates badge as a separate field outside data words), D8
  (flat cap table with typed entries — cap transfer is structurally distinct
  from data copying; motivates dedicated cap fields), D12 (fault delegation —
  kernel generates fault messages that must fit this format; fault descriptor
  completeness establishes 4 words as the natural data size), D14 (Observer
  handle in fault messages — requires 1 cap slot), D22 (interrupt messages carry
  send-once ack cap — requires 1 cap slot; D16 provides the ack mechanism), D26
  (capability-addressed memory — bulk data through shared Spaces, not
  in-message; messages are control plane only), D7 (split model — fault message
  is IPC notification; inspect() is typed kernel operation; label is
  pass-through, kernel doesn't dispatch on it), D15 (unidirectional endpoints —
  message flows one direction; badge identifies sender), D24 (cap-mapping
  invariant — ownership-transfer IPC dissolved; message format independent),
  `design/research/ownership-transfer-ipc.md` (message format survey across
  Mach, Zircon, seL4, EROS, KeyKOS), `design/research/page-fault-routing.md` §3
  (seL4 fault message: 4 MRs), `design/landscape.md` §3.2 ("IPC should never
  carry bulk data"), §3.4 (seL4 fastpath: message in registers, ~400 cycles
  ARM64).
- **Status:** settled — revisit if D13 is revised (different IPC model changes
  queue and fast-path assumptions), if D16 is revised (changes the reply-cap
  mechanism that motivated the dedicated field), if D26 is revised (removing
  capability-addressed memory would reopen bulk-data-in-message), or if a
  downstream derivation reveals that 4 data words are insufficient for a
  structurally required kernel message type.
- **Journal:** `journal/029-message-format.md`.

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

### D29 — Time is a capability-held kernel object type

Time is a kernel object type designated by capabilities, joining Space,
endpoint, and Observer as the fourth type. The Observer's Time reference is a
capability in the Observer's D8 flat table at a reserved slot (D21 pattern).
Time objects represent claims to scheduling allocation on a specific logical
core.

Three convergent paths: (1) D4 — Time is "a claim to a portion of a specific
logical core's scheduling time," a bounded resource. D4 requires capability
mediation for bounded resources. Kernel-internal Time binding is ambient
privilege. (2) D21 precedent — D11 destroy-invalidation, D17 badge-closure, and
D8 ABA protection require the Time reference to be a cap-table entry, not a
struct field. (3) Journal 023 cap-graph completeness — Time as kernel-internal
would be the sole resource outside the capability graph.

Dissolves two open questions. Time reclamation on Observer destroy: Observer
destroy closes the Time cap (D11 close semantics). Time migration across cores:
close Time cap on source core, acquire Time cap on destination (cold-path
capability operation, D1-consistent).

Does NOT settle: Time cardinality (one or many per Observer — vocabulary's
"exactly one Time" is an unexamined assumption, not derived; the Space parallel
under D27 suggests multi-Time may be consistent), Time parameters
(budget/period, fraction, or claim-to-participate), Time clonability (D23
uniformity suggests clonable but not derived), Time creation authority (per-core
Time manager), Time donation mechanism (seL4 MCS pattern — deferred), D2
scheduling property split (which abstract properties live on Time vs. Observer).

- **Rests on:** D4 (designation = authority — Time is a bounded resource;
  ambient scheduling privilege foreclosed; independent path), D21 (cap-table
  entry precedent — D11 destroy-invalidation, D17 badge-closure, D8 ABA
  protection apply identically to Time reference; independent path), journal 023
  (cap-graph completeness — Time as kernel-internal would be the sole hole;
  independent path), D8 (flat cap table — Time cap at reserved slot, O(1)
  lookup), D11 (close semantics dissolve Time reclamation), D6 (one Time per
  Observer — vocabulary commitment carried forward; cardinality not yet
  re-examined), `design/landscape.md` §4.4 (scheduling parameters on thread
  object vs. separate first-class capability — seL4 MCS scheduling contexts),
  `design/research/syscall-landscape.md` (seL4 MCS SchedContext/SchedControl).
- **Status:** settled — revisit if D4 is revised (removing designation =
  authority removes the primary path), if D21 is revised (removing cap-table
  entry precedent removes the representation argument), or if a downstream
  derivation (Time cardinality, Time parameters) reveals that capability-held
  Time creates essential complexity that kernel-internal would not.
- **Journal:** `journal/030-time-is-capability-held.md`.

---

## Open questions

- ~~**Time migration across cores.**~~ Dissolved by D29: Time is a
  capability-held kernel object. Migration is a cold-path capability operation:
  close Time cap on source core, acquire Time cap on destination. Observer's
  abstract scheduling properties (D2) transfer; the Time object is
  core-specific.
- **Minimum abstract scheduling properties on an Observer.** D2 says Observers
  carry abstract scheduling properties, but the minimum set (priority? deadline?
  IO-bound flag? period?) is not fixed.
- ~~**Observer-Space cardinality formalization.**~~ Settled by D27: flat. An
  Observer holds multiple independent Space caps directly in its D8 table. No
  kernel-tracked hierarchy between Spaces. Grouping is userspace convention (D6
  parallel). Hierarchy rejected on D8, D6, D4, D11, A3. Provenance tracking
  deferred as kernel-internal optimization.
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
  entry (not a separate Observer struct field). D29 settles Time as a cap-table
  entry at a reserved slot (same D21 pattern). The concrete field set (register
  state, L0 page table pointer, capability table pointer, scheduling state,
  pending-list linkage (D18), Observer state: runnable/blocked/faulted) needs
  formal derivation in the current chain. Note: neither the fault handler NOR
  the Time reference are in this list — both live in the cap table at reserved
  slots, not in the Observer struct. Archive journal/004 derived a
  first-principles minimum. D12 confirms the fault handler is structurally
  required. D14 confirms lifecycle state tracking is required. D20 confirms
  per-Observer attachment. D21 confirms cap-table representation. D29 confirms
  Time cap-table representation.
- ~~**Address space binding mutability.**~~ Dissolved by D26: no address space
  object, no binding. Observers access Spaces through capabilities; the page
  table is updated automatically as caps are acquired and lost.
- **Observer creation API shape.** D14 settles Observer as capability-held but
  not the creation interface. Create-then-configure (seL4 — inert Observer,
  configured via cap ops, started separately) vs. all-params-upfront (archive —
  one syscall). Minimum inputs: Space, Time cap (D29), fault handler endpoint +
  badge (D12, D20). Open: initial PC/SP, initial capabilities, create vs. start
  as separate operations.
- **Observer rights model.** D14 settles resume and destroy as minimum. D23
  settles clonability, enabling rights separation across multiple caps. Open:
  suspend (external pause), inspect register state (debugging), modify
  scheduling properties (D2), change fault handler (D20 — per-Observer, so this
  is an Observer-cap right), duplicate-control right (Zircon ZX_RIGHT_DUPLICATE
  model, deferred from D23). Each right = a typed kernel syscall under D7.
- ~~**Observer handle clonability.**~~ Settled by D23: clonable. Observer
  handles follow uniform capability rules (clone, attenuate, transfer)
  identically to all other kernel object types. Non-clonable rejected on five
  convergent structural arguments. Archive's "handle = handler unification"
  dissolved by D20/D21. Duplicate-control right deferred to Observer rights
  model.
- **Suspend as distinct from faulted.** Is there external suspension (not caused
  by fault)? If yes, Observer state has four values (runnable, blocked, faulted,
  externally-suspended). Use cases: debugging, checkpointing, resource pressure.
- ~~**Time reclamation on Observer destroy.**~~ Dissolved by D29: Time is a
  capability-held kernel object. Observer destroy closes the Time cap (D11 close
  semantics). If this was the last reference, the Time object is destroyed and
  its scheduling allocation returns to the per-core pool. If other caps exist
  (e.g., the creator retained a reference), the object persists.
- **Time cardinality.** D29 settles Time as capability-held but does NOT settle
  whether an Observer holds one or many Time caps. The vocabulary's "exactly one
  Time" (D6) is an unexamined assumption, not derived from axioms. The Space
  parallel (D27 — flat cardinality, multiple independent Space caps) suggests
  multi-Time may be the consistent position: each Time cap represents a portion
  of scheduling allocation, the total is the sum. Observer's abstract scheduling
  properties (D2) are hints about how the total allocation should be
  distributed. Requires its own exploration. Interacts with Time parameters and
  D2.
- **Time parameters.** What does a Time object carry? Budget/period (seL4 MCS
  sporadic server model — hard-RT capable), a fraction of core scheduling
  capacity (vocabulary-literal), or just a claim-to-participate with quantity
  determined by the scheduler. Interacts with cardinality: if multi-Time, each
  cap's parameters define its quantum; the per-core scheduler works with the
  Observer's total allocation.
- **Time clonability.** D23 settled all other types as clonable. The uniformity
  argument suggests clonable (multiple references to the same scheduling
  allocation, not a second allocation). Journal 014's assumption of
  "non-clonable" was never derived.
- **Time creation authority.** Who creates Time objects? Per-core Time manager
  (graph.d2 already has this box). Parallels D3 (one logical Space manager). How
  is initial Time distributed at boot?
- **Time donation on IPC.** seL4 MCS donates scheduling context during IPC for
  priority inversion prevention. If adopted, likely a kernel-internal
  optimization on Call() paralleling D16's reply-cap injection. The caller is
  blocked (doesn't need Time); the server uses the caller's Time. Compatible
  with D6's constraint at any instant. Deferred.
- **Can Observers share capability tables?** D8 settles per-Observer tables with
  no sharing. Under D26, Observers sharing Spaces hold independent caps to the
  same Spaces. Revisit as a D8 downstream: does the
  multi-Observer-sharing-Spaces pattern create sufficient pressure for shared
  capability tables, or is per-Observer authority (with explicit capability
  transfer) sufficient?
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
- ~~**Page size exposure.**~~ Settled by D25: page size is exposed. Hiding
  rejected — creates unpredictable hardware-dependent failures and security
  violations under sub-page packing. Remaining: whether the interface is fully
  page-addressed (all operations require page-aligned inputs) or implicitly
  rounded (byte values accepted, kernel rounds, PAGE_SIZE queryable). One level
  down from D25.
- ~~**Fault handler attachment.**~~ Settled by D20: per-Observer. Each Observer
  stores its own fault handler endpoint reference and badge.
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
  memory operations (provide physical backing for the faulting Space) before
  calling resume()? The sequence shapes the pager's syscall pattern.
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
  (D9 shared Space caps), not in the endpoint mechanism.
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
- ~~**Message format.**~~ Settled by D28: fixed-size. 4 untyped data words + 1
  user cap slot + label in header + badge as kernel-injected field + reply cap
  as kernel-injected field (Call() only). Cap slots structurally separate from
  data words. Variable-length rejected (D26 bulk-data-through-Spaces makes it
  unnecessary). Archive's cap_mask bitmask replaced by dedicated cap fields
  (D8's structural distinction between data copying and cap transfer).
  Remaining: fault message content per type, badge-closure content, interrupt
  content, inspect() shape, fast-path conditions.
- **Send-once right encoding.** D16 introduces send-once as a general-purpose
  right in D8's rights mask. How it is represented (a right bit, a modifier on
  the send right, or a separate field) is an entry-layout detail deferred with
  D8's open entry-layout questions. D28 confirms the reply cap (a send-once cap)
  is kernel-injected in a dedicated message field, not a user cap slot.
- **IPC fast-path conditions.** When does direct process switch occur? Receiver
  waiting? Priority check? seL4 fastpath requires no higher-priority runnable.
- **Specific syscall surface.** D7 settles two mechanism families but not the
  exact set. D14 adds resume() and confirms destroy() applies to Observers. D28
  establishes inspect(observer_handle) as a typed kernel operation for reading
  Observer state (fault message decomposition). The archive's 10-syscall design
  is a data point. Depends on IPC model, Observer creation API, Observer rights
  model, and D9 (memory objects).
- ~~**Address space lifecycle.**~~ Dissolved by D26: no address space kernel
  object. The page table is kernel-internal; per-Observer L0 tables are
  destroyed with the Observer; per-Space subtrees are reference-counted and
  freed when the last holder's cap is closed.
- **Boot / bring-up model.** BSP-then-APs vs symmetric bring-up. Touches A2 but
  not derived.
- ~~**Explicit unmap() semantics.**~~ Dissolved by D26: no explicit map() or
  unmap(). The page table is managed by the kernel based on Space cap holdings.
  Holding a cap grants access; losing a cap removes access.
- **Sub-page packing under D24.** D24's page table cleanup operates at page
  granularity (the MMU works in pages). If the kernel packs multiple small
  Spaces onto one physical page, closing the last cap to one Space can't remove
  the shared page table entry without affecting the other. Resolution options:
  no packing (each Space gets its own page — internal fragmentation), copy
  co-located Spaces on cleanup (expensive), or accept that sub-page Spaces don't
  benefit from automatic cleanup. Kernel-internal implementation concern, but
  D24 makes it load-bearing.
- **Space acquisition at runtime.** Observers cannot create Spaces from nothing;
  they receive Spaces through capability transfer or split existing Spaces. How
  does an Observer that needs more memory acquire it? Candidates: request from
  fault handler Observer, request from a memory server, protocol-level
  negotiation. Connects to D12 (fault delegation) and D3 (Space manager as
  allocation interface).
- **Kernel-internal memory on cap transfer.** When an Observer acquires a Space
  cap, the kernel creates page table entries and base table entries for the new
  holder. Whose Spaces back these structures? Interacts with D8
  (typed-memory-backing pattern) and D26 (per-Observer page tables).

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
  typed-memory backing for explicit accounting; table sharing deferred.
- `009-memory-object-model.md` — reasoning for D9: D8 precedent (kernel-managed,
  typed-memory backing) extends to memory; A5 rejects seL4 userspace-managed
  model; page-granularity rejected on D5 CHERI note; Space vocabulary provides
  accounting; vocabulary corrected (object identity, not physical address
  binding).
- `010-address-space-is-first-class.md` — original reasoning for D10 (superseded
  by D26/journal 027): three independent paths converged on first-class address
  space; those concerns are now satisfied by capability-addressed memory
  instead.
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
  independent right in D8's rights mask (D4 consistency, resource alignment);
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
- `026-page-size-exposure.md` — reasoning for D25: page size is exposed to
  userspace. Four axioms initially favored hiding; a concrete scenario (two 4KB
  objects adjacent on 16KB hardware) demonstrated that every hiding strategy
  fails — unpredictable errors, D4/D24 security violations under sub-page
  packing, or hardware-dependent behavior. O4 resolved: page-size knowledge is
  essential complexity. Archive divergence: archive took byte-addressed (hiding)
  position; rejected here based on D4/D24 arguments absent from archive's
  context. Includes detour reaffirming D24's "map is explicit" (auto-map
  rejected for D10 cascade and cap-without-mapping patterns). Note: D10 cascade
  ground is dissolved by D26; journal 027 revisits the auto-map rejection.
- `027-capability-addressed-memory.md` — reasoning for D26: capability-addressed
  memory with (Space, offset) access model. Observer accesses memory by
  presenting a Space cap and offset; the kernel manages VA assignment
  internally. Runtime base lookup bridges to ARM64 flat-VA hardware. Per-Space
  VA bases enable page table subtree sharing and cross-Observer pointer sharing.
  Dissolves D10 (address space object); strengthens D24 to structural property;
  dissolves map/unmap asymmetry. Supersedes journal 010.
- `028-space-cardinality.md` — reasoning for D27: flat Space cardinality. An
  Observer holds multiple independent Space caps directly in its D8 flat table.
  Five convergent grounds reject hierarchy: D8 (first inter-entry structural
  relationship), D6 (grouping is userspace policy), D4 (hierarchy introduces
  implicit authority beyond designation), D11 (cascade/orphan extends close
  semantics), A3 (tree assumption forecloses non-tree patterns). Provenance
  tracking deferred as kernel-internal optimization.
- `029-message-format.md` — reasoning for D28: fixed-size IPC message format. 4
  data words + 1 user cap slot + label/badge/reply-cap as dedicated fields. Data
  word count from fault-descriptor completeness (4 words = natural fault
  descriptor size; gap to full Observer state too large for any message to
  bridge — inspect() provides full state). Dedicated cap fields from D8's
  structural distinction between data copying and cap transfer. Reply cap as
  kernel-injected field from D16's kernel-creates-it parallel to badge. Archive
  convergence on size and fixed format; divergence on cap encoding (bitmask →
  dedicated) and reply-cap placement (shared slot → dedicated field), explained
  by D8 and D16 settled after archive.
- `030-time-is-capability-held.md` — reasoning for D29: three convergent paths
  (D4 designation = authority for bounded resources, D21 cap-table entry
  precedent, journal 023 cap-graph completeness) settle Time as capability-held
  kernel object type. Dissolves Time reclamation (D11 close semantics) and Time
  migration (cap close + acquire). Discovery: vocabulary's "exactly one Time" is
  an unexamined assumption, not derived — flagged for exploration. Archive
  convergence: archive treated Time as first-class object with dynamic bindings
  and Time donation via IPC.

---

## Research

See `design/research/` for descriptive prior-art studies and
`design/landscape.md` for the survey of how other kernels resolved each major
design decision.
