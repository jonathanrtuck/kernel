# Kernel Design Specification

The current state of the kernel's design. Settled decisions with brief
rationale. See `design/graph.d2` for the structural map and `design/journal/`
for full exploration history.

This document was reset on 2026-04-15 to re-derive contingent decisions from
first principles. The current derivation chain (D1–D53) has fully superseded the
previous chain, which was deleted after systematic convergence checking
confirmed coverage across all topics.

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

- **Time.** A claim to a portion of the system's compute capacity, denominated
  in normalized compute units. Each Time cap carries an integer quantity of
  compute units calibrated to hardware-described core capacity factors (ARM
  `capacity-dmips-mhz`, ACPI CPPC, or equivalent). A given number of compute
  units represents approximately the same amount of work regardless of which
  core executes it — the kernel translates to per-core scheduling time
  internally. Total system capacity = sum of all core capacities; the kernel
  cannot over-allocate. Time is fungible — multiple Time caps are additive
  (D30). Time caps are linear: at most one capability reference per Time object,
  non-clonable (D38). Authority delegation uses split (new object with a portion
  of the original's quantity), not clone. The Observer holds abstract compute
  capacity without knowing which core it runs on or what that core's capability
  is; core assignment, migration, algorithm selection, and the
  compute-unit-to-time translation are kernel-internal concerns (D31, D36),
  parallel to how physical addresses and virtual addresses are kernel-internal
  for Space (D9, D26). The Observer provides a three-value scheduling profile
  (D42): responsiveness, throughput, and precision, sharing a fixed per-Observer
  budget. Spending on one dimension takes from the other two. One set of values
  — no base/effective split; scheduling adjustment during IPC is a userspace
  concern via modify-scheduling (D43). The kernel places the Observer on an
  appropriate core and enforces the compute allocation. On SMT hardware,
  delivered compute additionally depends on sibling logical-core contention for
  shared pipeline resources; the kernel guarantees compute allocation, not
  physical-compute-rate delivery. (Vocabulary revised by D36 — previously
  "abstract scheduling capacity" as a per-core fraction (D31). Per-core
  fractions leak core identity through the provisioning chain on heterogeneous
  hardware (A2 big.LITTLE). Normalized compute units restore the Space parallel:
  Space = bytes, Time = compute units — both hardware-independent quantities
  with kernel-internal placement.)

- **Observer.** A schedulable execution unit coupling Space and Time — the
  condition under which compute (Time) executes instructions within specific
  memory (Space). Each Observer is an independent perspective in which a
  specific computation unfolds. An Observer holds capabilities to one or more
  Spaces and one or more Times (D30). Memory is accessed through
  capability-addressed (Space, offset) pairs; the kernel manages the underlying
  virtual address mapping (D26). The Observer never chooses or manages virtual
  addresses, physical addresses, or core assignments — these are kernel-internal
  (D9, D26, D31). Multiple Time caps are additive — the kernel maintains a
  cached scheduling aggregate. SMT-concurrent workloads, when hardware supports
  them, are expressed as multiple Observers sharing a Space, each with its own
  Time(s) — not as a single Observer with multiple execution points. The
  Observer always has one register state, one PC, one execution stream
  regardless of how many Time caps it holds.

- **Field.** The medium through which Observers communicate. A Field is a
  queued, unidirectional, many-to-many IPC object: multiple Observers may send
  to the same Field, and multiple may receive from it, with access governed by
  capability rights (send, receive, mint). All information delivery — peer
  messages, fault notifications, interrupt signals, timer fires, badge-closure
  events — flows through Fields. The metaphor is from physics: a field mediates
  interaction between observers, and any number of participants can disturb or
  sense the same field. Which queue slots are occupied, the waiters list, and
  optional per-badge tracking state are kernel-internal concerns.

- **Pulsar.** A timer that the kernel programs on behalf of an Observer and
  delivers as a Field message when it fires. A Pulsar is a capability-held
  kernel object (D44) created from Space (D32) with a delivery Field, badge,
  duration, and period. Period = 0 means one-shot; period > 0 means repeating
  with kernel-managed re-arm and drift compensation. The kernel enqueues a
  message to the designated Field with the designated badge when the duration
  elapses — the same kernel-as-sender pattern used for fault notifications (D12)
  and interrupt delivery (D22). The Pulsar's period is an explicit input to D42
  EDF admission (T in the C/T test). Overflow: when the delivery Field is full,
  the kernel stops re-arming until a slot opens, then delivers with an overrun
  count. Observers needing adaptive timing use one-shot Pulsars in a loop. The
  metaphor is from astrophysics: a pulsar emits regular, precisely-timed signals
  — an Observer listens.

_Capitalized-vs-lowercase convention:_ Capitalized terms (Space, Time, Observer,
Field, Pulsar) are kernel proper nouns — names of specific concepts in this
kernel's design, with the semantics defined here. Lowercase equivalents from
broader OS literature (memory object, thread) refer to the same kind of thing
but without claiming this kernel's specific semantics. The two are
interchangeable in prose; capitalization signals "speaking of our concept" vs.
"speaking of the general concept."

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

#### D1 IPI interface (settled 2026-04-26)

Cross-core coordination mechanism for D1's per-core hot path. Three decisions:

- **Fire-and-forget semantics.** Core A sends SGI and continues. No
  acknowledgment, no synchronous wait. Eventual consistency by next scheduler
  round. D56 work-stealing checks are stale by definition — scheduling quality
  issue, not correctness.
- **Per-core circular queue.** Not a single-entry mailbox. Multiple IPIs can be
  in-flight simultaneously (TLB invalidation + work steal + Observer migration).
  Queue depth bounded by request types, not traffic.
- **Typed enum requests.**
  `IpiRequest { WorkSteal, ObserverMigration, TlbInvalidation, RoutingEntryCleanup }`.
  Type-safe, no encoding overhead at exception level. The enum is
  kernel-internal (O2) and can grow without ABI impact.

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
- **Status:** settled — D42 settles the minimum property set as a three-value
  profile (responsiveness, throughput, precision). The revisit trigger (property
  set proves unexpressible) does not fire: the three values are interpretable by
  fixed-priority, fair-share, and deadline-based algorithms. Revisit if D42 is
  revised.
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
close-only + destroy + ABA tag; D67 settled add-on as universal generation
counters, CDT rejected.)

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
program counter, one or more Time caps (D30), one capability table. The kernel
has no "process" concept — "process" is a userspace convention (a group of
Observers sharing Space caps). Multi-threaded execution in shared memory is
multiple Observers holding caps to the same Space, each with its own Time(s).
Green threads and cooperative concurrency are internal to an Observer
(userspace, invisible to kernel). Note: D6 originally said "one Time"
(vocabulary assumption). D30 settles multi-Time as additive resource claims —
the Observer still has one execution stream.

The kernel provides no Observer-grouping mechanism. Grouping is neither
essential complexity (D4 capabilities handle Observer lifecycle without the
target's cooperation) nor workload-universal (A3 — not all workloads need
groups). Userspace builds grouping policy from capabilities; the kernel provides
the mechanism.

Does NOT settle: Observer minimum schema (concrete fields need formal
derivation), Observer lifecycle operations beyond the D14 minimum (creation API,
rights model, suspend, clonability). ~~Whether Observers can share capability
tables~~ — settled: per-Observer only (D8 confirmed, sharing revisit condition
resolved). (D8 settled capability table structure; D14 settled Observer as
capability-held object type with resume and destroy as minimum operations.)

- **Rests on:** Observer vocabulary (SMT paragraph explicitly models concurrency
  as multi-Observer, not multi-execution-point; D30 revised Time cardinality
  from "exactly one" to "one or more" without affecting the execution-stream
  commitment), D2 (scheduler selects Observers — one-level selection), D4
  (per-Observer capability table; destroy capability works without target
  cooperation), A3 (generic — no workload assumes or requires kernel-level
  grouping), `design/landscape.md` §4.4, §6.1 (seL4 validates no-kernel-process;
  all surveyed systems schedule thread-level entities).
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
and the base revocation primitive as close-only + destroy; D67 settled
revocation add-on as universal generation counters, CDT rejected; entry-layout
specifics beyond the slot tag, table-full protocol, and size policy remain
open.)

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
  re-motivate CNode dispatch) or if the revocation model requires CDT and the
  absence of tree structure makes it impractical. ~~The D26 sharing revisit
  condition is resolved: D26 handles memory sharing at the page-table level, and
  the remaining authority-sharing ergonomics are userspace-library complexity,
  not essential kernel complexity under A5.~~
- **Journal:** `journal/008-capability-table-structure.md`,
  `journal/065-shared-cap-tables.md` (sharing revisit discharged).

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
interface)~~ (settled by D25: exposed; hiding rejected), ~~specific operations
on Spaces (split, COW/clone, resize)~~ (merge and split settled by D41;
COW/clone remains open), Space rights, ~~fault delegation~~ (settled by D12), or
~~how an Observer acquires additional Spaces at runtime~~ (settled by D31).

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
(Observers, Spaces, fields), close-only cannot express this; the userspace
construction that would substitute cannot interpose at the MMU level and must
route through a kernel mechanism that is itself a form of authoritative destroy
under another name. Forcing this construction into userspace violates A5 via O4
(a).

Add-on mechanisms for mass invalidation (generation-as-revocation) and selective
revocation (CDT, badges) are deferred. Their value depends on the IPC model:
field rotation serves mass invalidation only if field-like kernel objects exist;
badges ride on IPC; proxy indirection requires IPC mediation. Committing to
add-ons before the IPC model is settled would either skip a level or overspend
on features whose alternatives may be free.

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
- **Status:** settled. The IPC model deferral condition is discharged by D67:
  generation counters adopted (universal, all object types); CDT rejected.
  Revisit if a downstream lifecycle derivation (Observer, Space) reveals the
  base primitive is structurally insufficient.
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
(settled by D20: per-Observer), ~~pager unavailability protocol (chains vs.
double-fault-kill)~~ (settled by D68: three failure modes — supervision
notification, cooperative escalation + Pulsar watchdog, kernel-autonomous
destroy at chain terminus), root/bootstrap case mechanism, ~~fault message
contents~~ (settled by D61: four fault types with specific data word
assignments), ~~pager reply/resume mechanism~~ (settled by D61/D40: resume via
Observer handle cap in fault message), ~~D7 classification of fault traffic~~
(settled by D61: faults ARE IPC, kernel-as-sender), Observer minimum schema
(fault handler confirmed structurally required by D12; representation settled by
D21 as cap-table entry).

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

### D13 — Queued fields with direct-switch fast path

The primary IPC mechanism is bounded queued fields. Messages accumulate in a
per-field queue. Sender deposits and continues (non-blocking; behavior on
queue-full is a downstream field-shape question). When the receiver is already
waiting, direct process switch bypasses the queue entirely at rendezvous speed
(~400 cycles ARM64). All information delivery — peer IPC, fault notifications
(D12), interrupt signals, system events — uses the same mechanism.

Sync-only was foreclosed: A3 requires event-driven workload support, and D12
requires the kernel to deposit fault messages without blocking. The queued model
subsumes both patterns: sync (send + block-on-reply field) and async (send +
continue). The archive's "strictly dominates" argument: queued fields achieve
rendezvous speed for the same-core, receiver-waiting case AND provide async
fallback — sync rendezvous cannot handle the async case at all.

Sync rendezvous + bitmap notifications (seL4 model) was not foreclosed but not
chosen: sender-always-blocks limitation breaks fan-out patterns; the archive
independently rejected it for the same reason. Sync + queued notifications
(QNX-like) also not chosen: still has sender-blocks, plus two mechanisms.

A coalescing tension exists for shared fields with overwrite-oldest overflow:
cross-source data loss when multiple sources share a capacity-1 overwrite field.
Resolution deferred to field-shape exploration. The tension is documented in
journal/013 so it is not rediscovered.

Queue memory drawn from the creator's Spaces (D8 pattern). Fixed capacity at
creation. Memory per queued message ~48 bytes (register-sized).

Does NOT settle: ~~message format~~ (settled by D28), queue capacity policy,
~~IPC fast-path conditions~~ (settled by D50), D12 fault delivery specifics.
(Field shape settled by D15. Overflow policy settled by D18. Coalescing
dissolved by D18. Reply routing settled by D16. Badge semantics settled by D17.
Multi-field wait resolved by D19. Message format settled by D28.)

- **Rests on:** A3 (generic — both sync and async patterns required; independent
  path), A4 (purely reactive — no kernel message broker; IPC dispatch within
  syscall handlers), D1 (hot-path — direct-switch fast path achieves D1's
  minimal per-core hot-path requirement), D7 (split model — IPC as a dedicated
  mechanism family; D7 notes "couples naturally with async"), D12 (fault traffic
  is IPC — kernel-as-sender requires non-blocking deposit), D4 (capability-
  mediated — fields designated by capabilities), D3 + D8 (queue memory drawn
  from creator's Spaces; typed-memory-backing pattern), `design/landscape.md`
  §3.1 (sync vs. async survey), §3.2 ("every production microkernel converges on
  hybrid"), §3.4 (fast-path data), `design/research/syscall-landscape.md` §10
  (IPC as pivot point, performance data, lessons from removals).
- **Status:** tentative — D18 resolves trigger #1 (coalescing gap dissolved — no
  second primitive needed) and settles overflow policy. D19 resolves trigger #3
  (multi-field wait — badge fan-in via D15+D17 covers common patterns;
  multi-receive syscall deferred, not foreclosed). Remaining trigger: bounded
  queue capacity creates unsolvable priority inversion or deadlock patterns
  (trigger #2, a downstream concern of priority/scheduling interaction, D2).
- **Journal:** `journal/013-ipc-model.md`.

### D14 — Observer is a capability-held kernel object type

Observer is a kernel object type designated by capabilities, joining Space,
Time, and field (D13) as the fourth type. Lifecycle operations — at minimum
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

Does NOT settle: ~~creation API shape (create-then-configure vs. all-params)~~
(settled by D35: minimal create + separate start), ~~Observer rights model
beyond resume and destroy (suspend, inspect, configure)~~ (settled by D39: nine
rights), ~~Observer handle clonability~~ (settled by D23: clonable), ~~fault
handler attachment (per-Observer vs. per-address-space)~~ (settled by D20:
per-Observer), ~~Time reclamation on destroy~~ (dissolved by D29: cap-table
close), Observer minimum schema.

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

### D15 — Unidirectional, many-to-many fields with send/receive rights

A Field is a single kernel object: bounded queue + waiters list. Capabilities to
the same field carry different rights in the D8 rights mask: send (enqueue),
receive (dequeue), or both. Topology is emergent from capability distribution —
the kernel does not enforce sender/receiver counts. Three usage patterns arise
by convention: server inbox (many:1), worker pool (many:many), dedicated pipe
(1:1).

Three convergent paths: (1) D8 + D11 structural consistency — standard entry
format, symmetric destroy; bidirectional would require structural exceptions to
both; (2) D12 + D13 many-to-one composition — fault delivery, interrupt
delivery, and server patterns are many-to-one; bidirectional requires per-source
channels + aggregation, weakening D13's "one mechanism" commitment; (3) A3 +
capability-distributed topology — diverse patterns served by one mechanism with
capability-mediated access.

Request-reply requires explicit reply-cap transfer per RPC (well-understood
cost; D16 settles the mechanism as send-once cap on a pre-allocated reply
field). Peer disconnection detection requires a badge-closure notification
mechanism (deferred to badge-semantics exploration).

Does NOT settle: overflow policy, multi-field wait, message format, field
naming. (Reply-cap mechanism settled by D16. Badge semantics settled by D17.)

- **Rests on:** D4 (send/receive as independent authorities — confused deputy
  forecloses undifferentiated access), D8 (flat table with rights mask —
  standard entry format; bidirectional would require structural exception), D11
  (symmetric destroy — bidirectional requires asymmetric peer-closure
  signaling), D12 + D13 (kernel-as-sender in many-to-one fault/interrupt
  delivery; one field per receiver, no aggregation needed), A3 (generic —
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
- **Journal:** `journal/015-field-shape.md`.

### D16 — Reply via pre-allocated reply field with send-once cap

RPC reply routing uses a pre-allocated reply field per Observer (a regular
field, D15) combined with a send-once capability right in D8's rights mask. On
Call(), the kernel creates a send-once cap to the caller's reply field, includes
it in the request message, and blocks the caller on its reply field. The server
sends the reply to the send-once cap; the cap is consumed on use. The reply
field persists across RPCs; the cap is ephemeral. No new kernel type — the reply
field is a standard field.

Send-once is a general-purpose use-limited attenuation right, not
reply-specific. It extends D4's attenuation hierarchy: a send-once cap is
consumed after one send operation. Independent applications include one-shot
notifications, single-use authorization tokens, and edge-triggered interrupt
delivery. Prior art: Mach send-once rights on ports; EROS resume keys
(effectively send-once).

The kernel is free to optimize the reply fast path behind the field interface
(bypassing the queue structure when the sole waiter is the known caller). This
is an implementation optimization, not an object-model commitment.

Structurally parallel with D14's fault handling: both deliver a caller-specific
response capability in the message. The mechanism families differ per D7 — IPC
reply is send-to-field; fault resume is resume(observer_handle) — but the
message shape is consistent.

A dedicated Reply kernel type (seL4 MCS) was considered and rejected: the
fast-path bypass it enables is an optimization achievable behind the field
interface, not a structural necessity. A persistent send cap without send-once
(archive's approach) was refined: send-once prevents post-reply capability
retention. Badge-based reply was foreclosed by D4 (ambient addressing, not
capability designation).

Does NOT settle: Call()/ReplyRecv() syscall details (part of specific syscall
surface), reply field allocation policy (pre-allocated at creation vs. lazy),
~~send-once right encoding in D8's rights mask~~ (settled by D51: boolean flag
on Entry, not a rights bit), shared reply field with badge disambiguation
(depends on badge semantics). ~~Message format interaction~~ settled by D28:
reply cap is a kernel-injected dedicated field (not a user cap slot),
paralleling badge.

- **Rests on:** D15 (unidirectional fields require reply-cap transfer — the cost
  this mechanism pays), D14 (fault resume settled separately — decouples IPC
  reply from fault resume; structural parallel in message shape), D7 (split
  model — IPC reply must be in IPC mechanism family), D8 (flat cap table with
  rights mask — send-once extends the mask), D4 (capability-based authority —
  badge-based reply foreclosed), D13 (queued fields with direct-switch fast path
  — kernel can optimize reply path), D11 (base revocation — send-once is
  auto-revoked on use; close semantics), `design/research/field-shape.md` (Mach
  send-once rights, seL4 reply cap, EROS resume key),
  `design/research/syscall-landscape.md` §1.1 (seL4 MCS reply object fix).
- **Status:** settled — revisit if D15 is revised (different field shape changes
  the reply-cap constraint), if the send-once right proves insufficient for
  reply semantics (e.g., server needs to reply multiple times), or if the
  fast-path optimization behind the field interface proves unachievable without
  a dedicated Reply type.
- **Journal:** `journal/016-reply-cap-mechanism.md`.

### D17 — Badge semantics: minter-assigned, mint-right-controlled, opt-in lifecycle tracking

A badge is a per-capability field in D8's entry layout, set by the minter at
clone time, immutable after creation, attached by the kernel to every message
sent through that capability. The sender cannot read, choose, or modify its
badge. Badges serve identification (key into receiver state), not merely
distinguishing. The minter chooses the value; the kernel enforces unforgeability
and immutability.

Mint is a third independent right in D8's rights mask (send, receive, mint),
controlling who can assign badges when cloning. The field creator controls
mint-right distribution, aligning badge population growth with resource
authority.

Lifecycle visibility is opt-in: the field creator specifies at creation whether
per-badge refcount tracking is enabled. With tracking: when the last send cap
with badge B to field E is closed, the kernel enqueues a closure notification to
E's receive side (through the field queue, D13). Without tracking: no per-badge
state, no notifications, trivial close path. Opt-in resolves the A3/A4 tension:
not all workloads need disconnection detection (A3), but those that do should
not fall back to polling (A4).

Five tensions are accepted for tracked fields: D16 send-once consumption vs.
badge-closure (must distinguish consumed-by-use from closed-without-use); D13
bounded queue vs. notification volume; per-badge map growth (controlled by
mint-right distribution and bounded by creation-time capacity); reverse
information flow (receiver observes sender's close — accepted as deliberately
constructed); D14 Observer destroy cascade.

Does NOT settle: ~~badge size~~ (settled by D58: u64, forced by D47/D49 ABI),
~~send-once exemption encoding~~ (settled by D73: structural code-path
separation, reply Field always-tracked), ~~badge on D16 kernel-created send-once
caps~~ (settled by D65: caller-supplied reply_badge), max-badge-count semantics,
~~fault handler representation~~ (settled by D21: cap-table entry —
badge-closure covers child Observer destruction automatically), ~~badge-closure
message format~~ (settled by D64: badge B + LABEL_CLOSURE + zero data + no
caps), badge-closure × overflow policy interaction, per-badge tracking ×
coalescing interaction.

- **Rests on:** D15 (many-to-one patterns create the structural need for sender
  identification; send/receive rights pattern extended with mint), D8 (flat cap
  table — badge stored in entry; rights mask holds mint right), D4 (designation
  = authority — badge unforgeability prevents confused deputy at IPC layer;
  badge-based addressing foreclosed; mint authority is capability-mediated), D13
  (one delivery mechanism — closure notifications use the field queue), D12
  (fault handler badge structurally required — kernel synthesizes fault messages
  without a sender cap), D11 (base revocation — badge-closure is a revocation
  add-on; close triggers per-badge check on tracked fields), D16 (send-once caps
  create the consumed-vs-closed tension), A3 (generic — not all workloads need
  lifecycle tracking; opt-in), A4 (purely reactive — polling-based disconnection
  detection is inconsistent; event-driven notification is A4-consistent),
  `design/research/field-shape.md` (seL4 badge mechanism, Mach send-once rights,
  Zircon peer-closed signal), `design/landscape.md` §1.3, §3.5, §3.6, §5.2
  (badge and notification mechanisms across surveyed systems).
- **Status:** settled — revisit if D15 is revised (changes the many-to-one
  composition that creates the need), if D8 is revised (changes where badge is
  stored), if D13 is revised (changes the notification delivery mechanism), or
  if the opt-in model proves insufficient (a workload pattern requires
  badge-closure on a field the receiver didn't create and can't replace).
- **Journal:** `journal/017-badge-semantics.md`.

### D18 — Error-to-sender overflow with deferred fault delivery

When a send to a queued field finds the queue at capacity, the kernel returns an
error. No per-field policy modes, no overwrite, no kernel-level coalescing.
Coalescing workloads use shared memory + signaling (D9 shared Space caps +
capacity-1 fields) — the standard microkernel architecture (landscape §3.2).

For the kernel-as-sender (D12 fault messages), deferred delivery: the kernel
links the faulting Observer into a per-field pending list. The next receive()
that frees a slot delivers the deferred fault. The pending list is an intrusive
linked list through existing Observer objects — zero additional memory
allocation. D17 badge-closure notifications are dropped on full queue; the
receiver discovers staleness lazily.

The D13 coalescing tension (cross-source data loss on shared fields with
overwrite semantics) dissolves: no overwrite means no cross-source data loss.
D13 revisit trigger #1 does not fire — coalescing is achieved through
composition of existing primitives, not through a second IPC primitive.

Does NOT settle: ~~interrupt delivery mechanism (must account for error-on-full
via masking)~~ (settled by D22: delegation with mask-on-delivery; D18 trigger
does not fire — no unsolvable delivery gaps), ~~pager unavailability protocol
(field destroy with pending faults adds a trigger)~~ (settled by D68: D33 hook
point provides Case A notification), multi-field wait (D13 revisit trigger #3),
Observer minimum schema (pending-list linkage field).

- **Rests on:** A3 (generic — different workloads, but only error is
  irreducible; coalescing is reducible to shared memory + signaling), A4 (purely
  reactive — kernel-as-sender can't block or retry; receive() is the only
  trigger for deferred delivery), D12 (fault delegation — fault messages must be
  delivered; the kernel-as-sender constraint drives deferred delivery), D13
  (bounded queue, fixed capacity — overflow is the question this answers; one
  mechanism — deferred delivery stays within the field, not a second primitive),
  D1 (overflow is cold-path; deferred delivery check on receive is cold-path),
  D9 (shared memory for coalescing — sharing Space caps makes kernel-level
  coalescing reducible), D17 (badge-closure dropped on full — not a correctness
  issue; per-badge tracking × coalescing interaction dissolved),
  `design/landscape.md` §3.2 (every production microkernel converges on shared
  memory + IPC signaling for data-plane communication), §5.1 (mask-on-delivery
  for interrupt coalescing).
- **Status:** settled — revisit if D13 is revised (different IPC model may
  change overflow semantics), if a downstream derivation reveals that dropped
  badge-closure notifications create a correctness issue (not just a timeliness
  issue), or if the interrupt model derivation reveals that error-on-full
  combined with interrupt masking creates unsolvable delivery gaps.
- **Journal:** `journal/018-field-overflow-policy.md`.

### D20 — Per-Observer fault handler attachment

The fault handler attaches to the Observer. Each Observer stores a fault handler
field reference and a badge. On fault, the kernel reads both from the faulting
Observer's struct and delivers a fault notification to the handler field with
the stored badge, plus the faulting Observer's capability handle via cap
transfer (D14).

Every Observer creation must supply a fault handler field and badge (D12
invariant enforced at creation time). Redundant configuration when N Observers
want the same handler is a userspace ergonomics cost, not kernel complexity — a
library function absorbs it.

Does NOT settle: ~~fault handler mutability (part of Observer rights model)~~
(settled by D39: change-handler is a separate right from install-cap), ~~fault
handler in Observer creation API shape~~ (settled by D35), ~~pager
unavailability protocol~~ (settled by D68), root/bootstrap fault handling.
(Fault handler representation settled by D21: cap-table entry.)

- **Rests on:** D6 (no kernel grouping — per-Observer is the natural attachment
  level; independent path), D4 (designation = authority — per-Observer allows
  independent delegation of fault handler configuration authority; independent
  path), D17 (badge-closure lifecycle visibility works only with per-Observer
  reference; fault handler badge is structurally required per-Observer
  regardless of field attachment; independent path), D12 (every Observer must
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
rights to the handler field, the per-Observer badge, and a generational slot tag
(D11). On fault, the kernel reads the entry at the known index and delivers a
fault message to the designated field with the stored badge.

Three independent arguments converge: (1) D11 authoritative destroy of the
handler field must invalidate the reference — the cap-table walk handles this
automatically; kernel-internal requires a parallel tracking structure; (2) D17
badge-closure on Observer destroy fires generically via cap-close — kernel-
internal requires explicit coupling between Observer-destroy and badge-closure;
(3) D8 ABA slot-tag protection prevents stale references after field destroy

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
~~pager unavailability protocol (D21 makes detection clear: dead cap-table
entry)~~ (settled by D68: dead cap detection feeds Case A supervision
notification).

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

### D22 — Device interrupt delegation through fields

The kernel delegates device interrupt handling to userspace driver Observers.
The kernel's role is interrupt dispatch: detect the interrupt (read GIC IAR),
mask it, enqueue a message to the driver Observer's field with a per-interrupt
badge (D17) and a send-once ack cap (D16), send EOI, return. The driver does
everything else. Three independent paths converge, paralleling D12 (fault
delegation): (1) A4 forecloses background interrupt processing; (2) A3
forecloses a single hardcoded interrupt policy; (3) A5 — the dispatch interface
(mask, signal, EOI) is smaller than a policy-configuration interface.

No separate IRQ kernel object type. The interrupt namespace maps onto the field
namespace. The kernel maintains an internal IRQ→field routing table. At boot,
device interrupts (discovered from device tree / GIC configuration) route to a
root interrupt field. The initial Observer receives this field (same mechanism
as initial Space distribution — one unsettled boot protocol, one answer for
both). To delegate, the holder splits the field by IRQ range: a new field
receives the subset, the original loses it. The new field cap is transferred to
a driver Observer. Dynamically-discovered interrupts (LPIs via ITS) are added to
the appropriate field by the kernel.

The driver handles interrupts identically to IPC: receive a message, do work,
respond. Each interrupt message carries a badge (identifying the IRQ) and a
send-once ack cap (D16). Using the ack cap unmasks the interrupt. The cap is
consumed on use (D16 send-once semantics). If the driver crashes and the cap is
closed without use, the interrupt stays masked (D18 safety). No IRQ-specific
operations — the driver uses receive() and send-once, exactly like RPC.

Both delivery and ack are IPC-family under D7: delivery is kernel-as-sender
depositing to field; ack is driver using a send-once cap. No typed kernel
operations specific to interrupts.

Scope: SPIs (32–1019), LPIs (8192+), and delegatable PPIs. The preemption timer
is kernel-internal (D2 scheduling mechanism). IPIs are kernel-internal (O2
cross-core coordination). Landscape §5.1 confirms: "No microkernel delegates the
preemption timer."

Two field operations emerge: split and combine. Both are cold-path. D45 settles
split as badge-range routing with fallback-on-destroy, generalizing beyond IRQ
to all badge-range traffic. Combine dissolves into split-to-existing + destroy
(D45). Split-to-existing enables drivers to receive interrupts and IPC on one
Field without multi-wait.

An IRQ object type (parallel to Space) and a factory model (IRQControl, seL4
precedent) were both considered and rejected. Every concern identified with the
field-only model — send-once performance, crash recovery, split/combine
complexity — traces to a parent decision (D16, general lifecycle, D13/D15) and
is not introduced by D22.

Does NOT settle: ~~field split semantics~~ (settled by D45: badge-range routing
with fallback-on-destroy; generalizes beyond IRQ to all badge-range traffic),
~~field combine semantics~~ (dissolved by D45: combine decomposes into
split-to-existing + destroy), ~~boot distribution of IRQ authority~~ (settled by
D99: kernel populates IrqRoutingTable at boot with all device INTIDs routing to
root interrupt Field; delegation via FieldSplit updates routing entries),
~~interrupt priority exposure~~ (settled by journal 066: flat absorption,
forward-compatible with future exposure), ~~IRQ routing policy~~ (settled by
journal 066: kernel-automatic GICD_IROUTER tracking on migration and receive-cap
transfer), ~~userspace timer mechanism~~ (settled by D44), GICv4
forward-compatibility (direct virtual injection).

- **Rests on:** A4 (no background interrupt processing; independent path), A3
  (no single interrupt policy; independent path), A5 (net: dispatch interface
  smaller than policy-configuration interface; confirms delegation's interface
  economics), D12 (structural precedent — three convergent paths parallel
  exactly), D13 (all information delivery through queued fields — interrupt
  delivery committed; the field IS the delivery mechanism, no additional type
  needed), D16 (send-once ack cap — D16 explicitly lists "edge-triggered
  interrupt delivery" as an application; the ack mechanism already exists), D17
  (badges identify which interrupt fired; fan-in onto one field), D18 (overflow
  settled: mask-on-delivery, GIC holds pending state; D18 revisit trigger does
  not fire — no unsolvable delivery gaps), D4 (capability-mediated authority —
  field receive cap IS the authority over its interrupt sources; integer IRQ IDs
  and file-descriptor models foreclosed), D7 (both delivery and ack are
  IPC-family; no interrupt-specific typed kernel operations), D8 (flat table
  accommodates send-once ack caps per interrupt message), D11 (field destroy
  masks associated IRQs; dead-handle semantics), D1 (hot path: GIC CPU interface
  registers are per-core; no shared mutable state on interrupt handling path;
  routing configuration and split/combine are cold-path), O3 (interrupts taken
  on targeted core), `design/landscape.md` §5.1 (four interrupt ownership
  patterns surveyed; universal kernel-internal: masking, EOI, preemption timer),
  §5.2 (six interrupt object models surveyed), §5.6 (microkernels dissolve
  deferred processing), §5.7 (GICv3/v4 specifics),
  `design/research/syscall-landscape.md` (seL4 IRQControl/IRQHandler, Zircon
  interrupt objects, L4Re IRQ objects, EROS IrqCtl/IrqWait).
- **Status:** settled — D45 settles split/combine semantics (the D22 revisit
  trigger "split/combine prove unimplementable" does not fire — D45's
  badge-range routing preserves D15 uniformity). Revisit if D13 is revised
  (different IPC model changes the delivery mechanism), if D16 is revised
  (changes the send-once mechanism that provides ack).
- **Journal:** `journal/022-interrupt-model.md`.

### D23 — Observer capabilities are clonable

Observer handles follow standard capability rules: clone, attenuate, transfer.
Multiple entities can hold capabilities to the same Observer, each with
independent rights masks. Clone is a per-type right (D38), not a universal
meta-operation; Observer's rights set includes clone. No type-specific
exceptions in D8's table management for Observer.

Non-clonable was rejected on five convergent structural arguments: D4
attenuation requires cloning (foreclosed), D8 uniformity requires no
type-specific exceptions (broken), D12/D20 fault delivery requires cap-copy
(requires new mechanism), D11 close creates orphan risk (requires new
mechanism), and type consistency. Non-clonable's sole benefit — kernel-enforced
single-manager — is achievable through capability discipline under clonable.
Note: D38 shows that non-clonable is correct for Time, where different
structural arguments apply (D30 aggregate soundness). D23's reasoning is
Observer-specific, not a universal law.

The archive's "handle = handler unification" concept (if non-clonable, the
handle holder is necessarily the fault handler) is dissolved by D20/D21: the
fault handler is a separate field cap at a reserved slot, not the Observer
handle holder.

A duplicate-control right (Zircon's ZX_RIGHT_DUPLICATE model) can be added later
as a rights-mask extension without affecting this decision. Deferred to the
Observer rights model derivation.

Does NOT settle: ~~Observer rights model (which rights go in the mask)~~
(settled by D39: nine rights), ~~Observer creation API shape~~ (settled by D35),
Observer minimum schema, whether the duplicate-control right is adopted
(deferred to D8 derivation). These are one level down.

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
  handler unification). D39 confirms clonability is load-bearing — rights
  separation via attenuated clones is the primary access control mechanism for
  the nine-right Observer model.
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

Does NOT settle: ~~sub-page packing strategy (kernel-internal implementation
concern)~~ (settled by D70: per-type slab with page return), kernel-internal
memory cost on cap transfer (page table entries for new holder), D9 Space
operations.

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

Does NOT settle: ~~whether the interface is fully page-addressed or implicitly
rounded~~ (settled by D60: byte-addressed inputs, kernel rounds to PAGE_SIZE
internally; forced by A5 + D26 + D9).

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

Does NOT settle: ~~Space operations (split, resize, COW/clone — D9 downstream)~~
(merge and split settled by D41; COW/clone remains open), provenance tracking
(deferred as a potential kernel-internal optimization, orthogonal to user-facing
cardinality).

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
is accessible via observer_read_registers(observer_handle) — a D7 typed kernel
operation (D39 formalizes; D28's tentative "inspect()" name superseded). The
fault message carries the notification; state inspection is a separate
operation. This decomposition follows from D7's split model: IPC is one
mechanism family, resource operations are another.

Variable-length messages (seL4 model) were rejected: two copy paths, length
validation on every message, variable queue slot sizes — complexity serving
workloads already better served by shared-Space bulk transfer (D26). The
bitmask-over-unified-slots encoding (archive's cap_mask) was rejected: conflates
data copying with cap transfer, requires mask inspection even for zero-cap
messages.

Does NOT settle: ~~fault message content details per fault type~~ (settled by
D61: four fault types with specific data word assignments), ~~badge-closure
notification content~~ (settled by D64: badge B + LABEL_CLOSURE + zero data),
interrupt message content, ~~inspect() syscall shape~~ (reconciled by D48:
observer_read_registers, D39's name), sender-side syscall encoding (which
registers carry what — A2 implementation detail), send-right gating of cap
transfer (Grant right), ~~IPC fast-path conditions~~ (settled by D50).

- **Rests on:** D13 (queued fields — message is what the queue holds; ~48 byte
  per-slot estimate anchors the size), D16 (reply cap mechanism — the kernel
  creates the reply cap, motivating the dedicated field), D17 (badge is
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
  pass-through, kernel doesn't dispatch on it), D15 (unidirectional fields —
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

Time is a kernel object type designated by capabilities, joining Space, field,
and Observer as the fourth type. Time capabilities are regular entries in the
Observer's D8 flat table (D30 settles multi-Time, so the D21 reserved-slot
pattern does not apply — Time caps use regular slots like Space caps). Time
objects represent claims to scheduling capacity (D31 revised the vocabulary:
abstract, not per-core; core assignment is kernel-internal).

Three convergent paths: (1) D4 — Time is "a claim to a portion of a specific
logical core's scheduling time," a bounded resource. D4 requires capability
mediation for bounded resources. Kernel-internal Time binding is ambient
privilege. (2) D21 precedent — D11 destroy-invalidation, D17 badge-closure, and
D8 ABA protection require the Time reference to be a cap-table entry, not a
struct field. (3) Journal 023 cap-graph completeness — Time as kernel-internal
would be the sole resource outside the capability graph.

Dissolves two open questions. Time reclamation on Observer destroy: Observer
destroy closes the Time cap (D11 close semantics). ~~Time migration across
cores: close Time cap on source core, acquire Time cap on destination (cold-path
capability operation, D1-consistent).~~ Superseded by D31: Time is abstract
scheduling capacity; core assignment is kernel-internal; migration is the kernel
moving an Observer between cores, not a capability operation.

Does NOT settle: ~~Time cardinality~~ (settled by D30: one or more), ~~Time
parameters~~ (settled by D36: normalized compute units), ~~Time clonability~~
(settled by D38: non-clonable — D30 aggregate soundness), ~~Time creation
authority~~ (settled by D31: kernel holds pool, allocates via pager chain),
~~Time donation mechanism~~ (settled by D37: explicit cap transfer via user cap
slot on Call()), ~~D2 scheduling property split~~ (settled by D36: Time carries
quantity (compute units), Observer carries scheduling hints (D2)).

- **Rests on:** D4 (designation = authority — Time is a bounded resource;
  ambient scheduling privilege foreclosed; independent path), D21 (cap-table
  entry precedent — D11 destroy-invalidation, D17 badge-closure, D8 ABA
  protection apply identically to Time reference; independent path), journal 023
  (cap-graph completeness — Time as kernel-internal would be the sole hole;
  independent path), D8 (flat cap table — Time caps in regular slots; D30
  settles multi-Time), D11 (close semantics dissolve Time reclamation),
  `design/landscape.md` §4.4 (scheduling parameters on thread object vs.
  separate first-class capability — seL4 MCS scheduling contexts),
  `design/research/syscall-landscape.md` (seL4 MCS SchedContext/SchedControl).
- **Status:** settled — revisit if D4 is revised (removing designation =
  authority removes the primary path), if D21 is revised (removing cap-table
  entry precedent removes the representation argument), or if a downstream
  derivation (Time cardinality, Time parameters) reveals that capability-held
  Time creates essential complexity that kernel-internal would not.
- **Journal:** `journal/030-time-is-capability-held.md`.

### D30 — Multi-Time: an Observer holds one or more Time capabilities

An Observer holds one or more Time capabilities in its D8 flat capability table
as regular entries (not reserved slots). Each Time cap represents a portion of
scheduling capacity (D31 revised the vocabulary: abstract, not per-core).
Multiple Time caps are additive — the kernel maintains a cached per-Observer
scheduling aggregate, updated on Time cap acquisition/loss (cold-path, O(1) per
mutation). The per-core scheduler reads the cached aggregate (hot-path, O(1)).

The D27 parallel (flat Space cardinality) is suggestive but not mechanically
forcing: Space is not fungible (each Space is a distinct object), while Time is
fungible within a core (additive, not independently useful). The deciding
argument is the server multi-client scenario: a server receiving Time caps from
clients A and B holds both simultaneously and returns each to the correct client
on reply. Under single-Time, this requires either kernel-internal donation
(breaks cap-graph completeness), explicit merge/unmerge protocol (pushes
coordination complexity to userspace — A5 tension), or a replace-on-receive
mechanism (new kernel machinery). Multi-Time absorbs multi-source delegation
automatically.

D6's rejected alternative "Multi-Time Observers" addressed multiple execution
streams (the SMT paragraph). Multi-Time as additive resource claims on a single
execution stream is a different concern. The Observer still has one register
state, one PC, one execution stream.

Does NOT settle: ~~Time parameters~~ (settled by D36: normalized compute units),
Time clonability (D37 constrains — D30 aggregate double-counts clones), ~~Time
creation authority~~ (settled by D31), ~~Time donation mechanism~~ (settled by
D37: explicit cap transfer via user cap slot), cross-core Time holding semantics
(reservation for migration).

- **Rests on:** A5 (kernel absorbs complexity — single-Time pushes multi-source
  coordination to userspace; multi-Time absorbs it via cached aggregate), D29
  (Time is capability-held — cardinality is a downstream question), D8 (flat cap
  table — Time caps are regular entries like Space caps; no inter-entry
  hierarchy; additive aggregation is a kernel-internal materialization, not a
  structural inter-entry relationship), D27 (suggestive parallel — flat Space
  cardinality established the pattern of multiple independent resource caps per
  Observer), D11 (close removes one Time cap, reducing the aggregate — no
  cascade), D1 (cached aggregate is O(1) on hot path; aggregate update is
  cold-path), `design/research/time-as-kernel-object.md` (no surveyed system
  provides multi-time per execution unit — novel position, justified by server
  scenario).
- **Status:** settled — revisit if D8 is revised (changes cap-table entry
  model), (D36 confirms additive aggregation is compatible — compute units
  compose via integer addition), or if the cached-aggregate approach proves
  unimplementable without hot-path cost.
- **Journal:** `journal/031-time-cardinality.md`.

### D31 — Resource acquisition through pager chain; boot architecture

Observers acquire bounded resources (Space, Time) through the pager chain. A
resource request syscall is routed by the kernel to the Observer's fault handler
(D20/D21), using the same mechanism as page fault delivery (D12). The Observer
does not know who its handler is — the kernel mediates. The handler can grant
(from its own holdings), deny, or escalate (its own handler receives the
request). The chain terminates at the kernel, which holds unallocated resources
as a root Space (all physical memory not yet granted) and per-core root Time
objects (scheduling capacity not yet granted). The kernel's internal pools are
Space and Time objects subject to the same split invariants — the kernel cannot
over-allocate.

Structural objects (Fields, Observers) are created by presenting a Space cap to
back them. The kernel allocates from that Space and returns a cap to the new
object. The Space shrinks by the allocation cost. Conservation holds: physical
bytes change purpose, not quantity. A "create" right in the Space rights mask
(D8) distinguishes creation authority from memory access.

The kernel is root pager for hand-picked root Observer(s). For resource
requests: allocate from pool or deny (trivially simple policy — real policy
lives in userspace pagers). For page faults on initial memory: cannot occur
(D26 + D24 — initial Spaces are fully backed), so any fault is a bug →
terminate.

At boot, the kernel creates the root Observer with minimal resources: initial
Space(s) (fully backed), initial Time (enough to run), interrupt field (D22
receive cap), and fault handler = kernel (D21 reserved slot). Post-boot, the
root Observer acquires additional resources through the normal pager-chain
mechanism. No special boot protocol.

Time vocabulary revised: Time is abstract scheduling capacity. Observers do not
see core identity; core assignment, migration, and algorithm selection are
kernel-internal (A5 parallel with D9/D26 — Observers don't see PA/VA either).
This dissolves the "Time migration across cores" open question (migration is the
kernel's internal scheduling decision, not a capability operation).

Two convergent paths: (1) D4 + D9 — the kernel manages physical memory (D9) and
no authority is ambient (D4). The pager chain provides capability-mediated
resource acquisition without factory caps or omnipotent root Observers. (2) D12
structural reuse — resource requests use the same mechanism as page fault
delivery. D8 already describes this pattern for cap table growth ("the kernel
faults the Observer; the fault handler provides more memory, then retries").

The split model (root Observer holds all resources, subdivides) was rejected for
security: all resources in a userspace Observer (EL0) is a poor security
posture. The pager-chain model puts unallocated resources in the kernel (EL1,
behind the hardware trust boundary). Conservation is identical — the kernel's
root Space + all granted Spaces = total physical memory, enforced by the same
split invariants. Security is gained without losing conservation.

Factory caps were rejected: D4 says "holding a cap to a resource IS the
authority over it." Factory caps separate authority-to-create from the created
object — an indirection D4 doesn't require. (Journal 022 rejected the same
pattern for interrupts.)

Does NOT settle: ~~resource request fault message format~~ (settled by D100:
four fault types with register-level layout; ResourceRequest uses x0 = resource
type, x1 = quantity), Space "create" right encoding, ~~pager unavailability
protocol (chains committed but unavailability handling still open)~~ (settled by
D68), ~~secondary core bring-up mechanism~~ (settled by D46: core lifecycle is
kernel-internal; all cores activate at boot), ~~Observer creation API config
parameters~~ (settled by D95: CreateObserver protocol — space_cap,
handler_field_cap, badge; composable setup via D35 operations), Time parameters,
Time clonability.

- **Rests on:** D4 (designation = authority — the pager chain provides
  capability-mediated resource acquisition; factory caps add indirection D4
  doesn't require; ambient creation foreclosed), D9 (kernel-managed memory — the
  kernel IS the resource manager; retaining the pool is consistent; giving
  everything to root Observer partially undoes D9), D12 (fault delegation —
  resource requests use the same routing mechanism; D8's cap-table-full pattern
  generalizes; independent path), A5 (kernel absorbs complexity — resource pool
  management, core assignment, VA/PA mapping all kernel-internal; split model
  pushes allocation management to root Observer), D20 + D21 (per-Observer fault
  handler at reserved slot — the routing target for resource requests), D7
  (resource request is a typed kernel syscall), D8 (Space-backed creation —
  typed-memory-backing pattern extended; "create" right in rights mask), D26 +
  D24 (initial Spaces fully backed — root Observer page faults dissolved), D22
  (interrupt field at boot — same distribution mechanism), A4 (kernel goes
  dormant after boot — all post-boot resource flow requires Observer syscalls),
  A3 (boot creates minimal initial graph — no workload assumptions; real policy
  in userspace pagers), `design/landscape.md` §1.7 (bootstrapping models), §2.2
  (allocation authority), §7.2 (initial process patterns),
  `design/research/memory-resource-capability.md` (initial distribution survey),
  `design/research/authority-models.md` §5.6 (bootstrapping problem).
- **Archive convergence:** Strong. Archive journal 013 derived the same model
  independently through "supervision trees": kernel retains pools, root Context
  gets minimal resources, faults escalate through handler chain to kernel.
  Archive claims.toml: "Boot is not a special protocol... retains unallocated
  resources and grants on request." Same architecture, same security argument,
  different derivation path. Divergence: Time vocabulary revision (abstract,
  core-agnostic) is novel — archive had per-Context core affinity bookkeeping.
- **Status:** settled — revisit if D12 is revised (changes the fault routing
  mechanism that resource acquisition depends on), if D9 is revised (reopening
  userspace-managed memory changes the pool model), if D4 is revised (weakening
  designation = authority reopens factory caps), or if a downstream derivation
  reveals that the kernel-as-root-pager creates essential complexity that a
  simpler boot model avoids.
- **Journal:** `journal/032-resource-acquisition-and-boot.md`.

### D32 — Kernel-internal memory accounting: type conversion model

Object creation is type conversion: a Space is consumed entirely and becomes the
object's functional backing. `create_field(space_cap) → field_cap`;
`create_observer(space_cap, config) → observer_cap`. The Space is gone; the
object exists. Destruction is the reverse: `destroy(object_cap) → space_cap`.
The freed pages become a new Space returned to the destroyer. Conservation is
structural: physical pages change purpose, not quantity.

Per-object kernel metadata (queue headers, scheduling aggregates, tracking
structs) is allocated from the kernel's root Space (D31). This cost is invisible
to userspace, bounded per object (fixed-size), and small relative to the
object's functional backing. Total system capacity is already opaque (D31 — root
Space is kernel-internal), so metadata overhead introduces no new opacity.

Page table subtree cost (L1/L2/L3 entries for a Space under D26) is baked into
the Space at split time. The parent Space shrinks by
`child_size + subtree_overhead`. The overhead is a deterministic function of
Space size and page granularity. First holder: subtree populated from the
Space's reserved capacity. Subsequent holders: reference count increment, no
allocation. The cost is a property of the Space, not the holder.

Cap table growth (D8 table-full fault): the handler provides a Space cap in the
fault reply. The kernel allocates table pages from it (Space consumed — type
conversion into cap table backing). The handler controls which Space pays. Most
general protocol; optimizations (designated growth Space) can be added behind
the same interface.

Time is asymmetric: Time comes from the kernel's per-core pool (D31), not from
Space. Destroying Time returns capacity to the pool. No Space involved.

Boot structures (root Observer, initial objects) from kernel's root Space.
Fixed, predictable cost.

Observer destruction returns structural backing as Space cap. What happens to
the Observer's held capabilities (caps in its table) is the destroy cascade
question — deferred.

Does NOT settle: ~~destroy cascade protocol~~ (settled by D33: preemptible
cascade, structural-backing-only return, cascade-freed to root Space), Space
"create" right in the rights mask, overhead reporting (does the Observer see
subtree overhead?), ~~merge/join operation (reverse of split)~~ (settled by D41:
merge is the reverse of split).

- **Rests on:** D8 (typed-memory backing — the Observer pays for its structures
  from its Spaces; unaccounted kernel allocation foreclosed), D31 (kernel holds
  root Space — metadata charged there; pager chain for resource acquisition;
  type conversion established for Field/Observer creation), D9 (kernel-managed
  memory — kernel handles physical backing internally; non-contiguous physical
  pages behind one Space is standard), D26 (per-Space shared subtrees —
  reference-counted; subtree cost baked into Space at creation), D11 (close on
  destroy — freed backing returns as Space or to root Space if cap closed), D25
  (expose real constraints — visible Space consumption, no hidden overhead in
  Observer Spaces), D13 (queue memory from creator's Spaces — extended to full
  type conversion), A4 (synchronous accounting — charge at alloc, refund at
  dealloc), A5 (kernel absorbs complexity — metadata invisible, accounting
  kernel-internal).
- **Archive convergence:** Strong. Archive journal 013:
  `open_wormhole → wormhole_handle`, `close_wormhole → space_handle`. Same
  type-conversion model, same conservation argument.
- **Status:** settled — revisit if D8 is revised (changes the typed-memory
  backing principle), if D31 is revised (changes the root Space model or
  creation mechanism), or if the destroy cascade derivation reveals that
  type-conversion reversal creates essential complexity for multi-Space-backed
  objects.
- **Journal:** `journal/033-kernel-memory-accounting.md`.

### D33 — Preemptible destroy cascade with structural-backing-only return

Object destruction cascades through held capabilities: each cap is closed, and
objects reaching refcount zero are destroyed too. Only Observers cascade (only
Observers hold caps; Spaces, Fields, Times don't). Single Observer destroy is
O(N + M): N cap table entries closed, M badge-closure checks. Cascade depth is
bounded by exclusively-held Observer chains.

The object is dead before cleanup begins (D11 — dead handles created at destroy
time). The cascade is cleanup of an already-dead object. No partially-alive
state is externally visible.

The cascade is preemptible: the kernel processes cleanup in bounded steps.
Between steps, the timer interrupt can preempt and the scheduler can run
higher-priority Observers. The kernel saves continuation state: position in cap
table iteration plus a stack of cascading objects. seL4 MCS demonstrates
feasibility. Inline (run-to-completion) is a special case — preemptible
forecloses nothing; inline forecloses bounded destroy time.

The top-level destroy returns one Space cap: the destroyed object's structural
backing (D32). Cascade-freed backing (refcount-zero objects destroyed during the
cascade) goes to the kernel's root Space (D31). Three arguments: shared
resources break the return model (last holder is arbitrary), internal
reorganization makes returns unpredictable, and structural backing is
predictable.

Destroy requires a "destroy" right in D8's per-cap rights mask. D4 requires
capability-mediated authority. Same pattern as send/receive/mint (D17). Badge-
closure during cascade is best-effort (D18 applies unchanged). Pending fault
list cleanup is O(1) per linkage (D18 intrusive list).

Does NOT settle: cap table close ordering (whether ordering within the table
matters), cross-core prompt-effect policy (strong vs. weak — D11 deferred), TLB
shootdown batching (optimization, deferred), ~~Observer "extract" operation
(pulling caps from child's table before destroy)~~ (evaluated and excluded by
D39 — proactive cap sharing via D23 + D28 covers the use cases; deferred).

- **Rests on:** D11 (base revocation — close-only + destroy provides the
  mechanism; dead-handle semantics ensure no visible intermediate state; ABA
  tags protect reused slots), D17 (badge-closure — up to M checks per Observer
  destroy; T5 tension accepted), D18 (overflow — badge-closure dropped on full
  queue; deferred faults cleaned up on field destroy), D32 (type conversion —
  structural backing returned to destroyer; cascade-freed has no caller), D31
  (root Space — cascade-freed backing destination; pager chain for
  re-acquisition), D8 (flat cap table — iteration is O(N); rights mask holds
  destroy right), D4 (capability-mediated authority — destroy right is D4-
  consistent), D26 (page table cleanup — last holder triggers subtree detach;
  cross-core TLB shootdown on last system-wide holder), A4 (purely reactive — no
  background cleanup; preemptible cascade uses timer preemption within syscall
  context), A3 (generic — RT workloads require bounded preemption latency;
  preemptible cascade is RT-compatible), D1 (cold-path — destroy is infrequent
  but must not block hot-path work indefinitely),
  `design/research/capability-revocation.md` (seL4 MCS preemptible revocation,
  Barrelfish cross-core costs, cost table), `design/landscape.md` §1.4
  (revocation approaches across surveyed systems).
- **Archive convergence:** Partial. Archive converges on cascade through owned
  resources ("subtree destroyed, resources reclaimed"). Diverges on return
  destination: archive returns resources to supervisor (ownership tree model);
  this derivation returns structural backing to destroyer and cascade-freed to
  root Space (flat caps + refcounting, no ownership tree). Divergence explained
  by D6 (no kernel grouping) and D27 (flat cardinality). Archive does not
  discuss preemptibility.
- **Status:** settled — revisit if D11 is revised (changes the base
  destroy/close mechanism), if D32 is revised (changes the type-conversion
  return model). D39 evaluated "extract before destroy" and excluded it —
  cascade protocol stands as the complete cleanup story.
- **Journal:** `journal/034-destroy-cascade-protocol.md`.

### D35 — Observer creation API: minimal create, separate start, composable operations

Observer creation is a minimal typed kernel syscall:
`create_observer(space_cap, handler_field_cap, badge) → observer_cap`. The Space
cap is consumed entirely (D32 type conversion). The handler field cap and badge
are installed at the reserved cap-table slot (D21). The Observer is created in
an inert state — structure exists (cap table, page table root, register save
area, fault handler), but the Observer is not scheduled.

The caller configures the Observer before starting it using general-purpose
operations: `observer_install_cap(observer_cap, source_cap) → slot` installs a
capability into the Observer's table (kernel manages slot allocation,
D8-consistent); `observer_write_registers(observer_cap, pc, sp, ...)` sets
register state; `observer_resume(observer_cap)` transitions from inert to
runnable (D14). Each requires a corresponding right on the Observer cap.

Cap installation is a general-purpose Observer operation, not creation-specific.
The same primitive serves fault resolution (supervisor installs Space caps after
page fault), dynamic delegation (granting new capabilities at runtime), and
pre-start setup. Time caps may be optionally installed before start — D31's
pager chain is the fallback acquisition mechanism, not a prohibition on early
provision.

Five creation models foreclosed: fork+exec (D6/D4/D27), constructor/image-stamp
(D31/D4), manifest-based (A3), VSpace binding (D26), Time as required parameter
(D31). The all-params-upfront alternative was rejected: separate create and
start forecloses nothing (all-params is a userspace library wrapping the
sequence; the reverse decomposition cannot be built outside the kernel), every
post-creation operation exists independently of creation (no new kernel
surface), and the syscall overhead (~1,000–2,000 extra cycles for 4–6 calls) is
negligible on this cold path relative to Observer creation's structural weight.

Does NOT settle: ~~Observer rights model~~ (settled by D39: nine rights),
Observer minimum schema (must support inert state), specific syscall encoding,
reply field allocation timing (D16 downstream — compatible with either pre-
allocated or lazy), cap-install slot selection policy (kernel-chosen default,
D8-consistent).

- **Rests on:** D32 (type conversion — Space consumed entirely as structural
  backing; creation Space does not provide executable memory), D20 + D21 (fault
  handler field cap + badge mandatory at creation time; cap-table write at
  reserved slot), D31 (Time via pager chain — not a creation parameter; pager
  chain is fallback, not prohibition), D26 (no address space object — no VSpace
  binding; Observer needs code Space cap before PC is meaningful), D14 (Observer
  as capability-held type; resume as typed syscall), D7 (creation is a typed
  kernel syscall; install-cap and write-registers are typed kernel syscalls), D8
  (flat cap table with kernel-managed allocation — install-cap returns slot
  number; D8-consistent), D12 (fault resolution requires cap installation into
  another Observer's table — same primitive as pre-start setup), A4 (synchronous
  creation within exception handler), A5 (kernel absorbs complexity — composable
  primitives with userspace library for common patterns; all-params is a
  library, not a kernel concern), D6 (Observer creation is structurally
  heavyweight — extra syscalls negligible on cold path), D1 (creation is
  cold-path), `design/landscape.md` §6.7 (task lifecycle survey —
  create-then-configure, spawn, fork+exec, constructor, manifest),
  `design/research/bootstrap-authority.md` (creation authority models),
  `design/research/execution-unit.md` (execution unit structure),
  `design/research/syscall-landscape.md` (seL4 TCB operations, Zircon
  thread_create/thread_start).
- **Archive convergence:** Partial. Archive used all-params-upfront:
  `create_context(space, time, fault_handler, ...)`. Divergence explained by D31
  (removes Time from creation), D26 (removes VSpace binding), and the
  cap-install reuse argument (fault resolution shares the primitive with
  pre-start setup — archive lacked this because pager protocol was explored
  after creation model).
- **Status:** settled — revisit if D32 is revised (changes type conversion
  model), if D20/D21 are revised (changes fault handler requirement at
  creation). D39 confirms install-cap / write-registers decomposition — the
  nine-right model builds on these composable operations without revealing
  essential complexity that a richer create call would have avoided.
- **Journal:** `journal/035-observer-creation-api.md`.

### D36 — Time parameters: normalized compute units

A Time object carries a single numerical value: a quantity of normalized compute
units. The unit is calibrated to hardware-described core capacity factors (ARM
`capacity-dmips-mhz`, ACPI CPPC `highest_perf`, or equivalent), so that a given
number of compute units represents approximately the same amount of work
regardless of which core executes it. The kernel translates compute units to
per-core scheduling time internally using precomputed capacity factors.

This is the Time parallel to Space's bytes: Space = bytes (hardware-independent
quantity, kernel manages physical placement), Time = compute units
(hardware-independent quantity, kernel manages core placement). The Observer
reasons about absolute compute quantities ("I need N compute units per frame"),
not fractions of system capacity or core-specific time.

The kernel charges consumed compute as `elapsed_time × core_capacity_factor`,
making empirical measurement core-independent: an Observer that measures its
per-frame compute requirement on any core gets a result valid on every core of
the system.

Conservation: total system capacity = sum of all core capacities. Per-core
admission: sum of compute units of all Observers on a core ≤ that core's
capacity. The kernel cannot over-allocate. On homogeneous hardware, all capacity
factors are equal and compute units degenerate to per-core fractions.

The capacity factor is a first-order approximation — actual speedup ratios vary
by workload type (~1.2x to ~3.5x for a stated 2x factor). Hard-RT precision is
achieved through D2's per-core algorithm heterogeneity: dedicated RT cores run
RT schedulers where the kernel knows the exact core and capacity factor. Best-
effort scheduling absorbs the approximation.

Does NOT settle: unit encoding (integer representation, bit width, global
scale), minimum Time quantum, Time split syscall surface, Time clonability (D37
constrains — D30 aggregate double-counts clones), ~~Time donation on IPC~~
(settled by D37: explicit cap transfer via user cap slot on Call()), ~~minimum
abstract scheduling properties on the Observer~~ (settled by D42: three-value
profile — responsiveness, throughput, precision; D37's priority-level
inheritance becomes scheduling inheritance), capacity factor source (A2
implementation detail).

- **Rests on:** D30 (multi-Time additive aggregate requires composable numerical
  quantity — budget/period pairs don't compose for different periods), D2
  (quantity/policy split — algorithm-specific parameters forbidden on Time;
  per-core scheduler derives enforcement from compute units + Observer hints),
  D31 (abstract scheduling capacity, core assignment kernel-internal — extends
  to core capability now also kernel-internal), D31 conservation (bounded total
  requires absolute quantity, not relative weight), A2 (big.LITTLE asymmetric
  cores — per-core fractions break the Space parallel on heterogeneous hardware;
  ARM DT provides `capacity-dmips-mhz`), A5 (kernel absorbs hardware-dependent
  translation — Observer never sees core capacities), D9 (Space parallel — Space
  = bytes, Time = compute units; same pattern, different resource), D1 (cached
  aggregate stores precomputed per-core fraction; cold-path conversion on cap
  mutation), `design/research/time-as-kernel-object.md` (seL4 MCS budget/period
  model, scheduling algorithm state placement), `design/landscape.md` §4
  (scheduling algorithm survey, heterogeneous scheduling).
- **Archive convergence:** Strong on the resource/requirements split and
  fraction-as-quantity. Archive journal 008 independently derived: Time =
  fraction (single value), conservation, fungibility, Space parallel. Divergence
  on per-core fraction (archive) vs. normalized compute units (this derivation),
  explained by D31's core-independence requirement and A2's asymmetric hardware
  — the archive did not address big.LITTLE normalization.
- **Status:** settled — revisit if D31 is revised (changes the core-independence
  requirement that motivates compute units over raw fractions), if D2 is revised
  (changes the quantity/policy split that forbids algorithm-specific parameters
  on Time), if D30 is revised (changes the composability requirement), or if the
  capacity factor approximation proves too imprecise for the workloads the
  kernel must support (the ~1.2x–3.5x range for a stated 2x factor is accepted
  as adequate for scheduling; a downstream derivation could add
  per-workload-class capacity factors if needed).
- **Journal:** `journal/036-time-parameters.md`.

### D37 — Time donation on IPC: explicit cap transfer

Time donation on IPC is explicit capability transfer via the D28 user cap slot.
On Call(), the caller may include a Time cap in the user cap slot. Standard move
semantics: the Time cap transfers from the caller's table to the message, then
to the server's table on Receive(). The server holds the donated Time alongside
its own Time caps (D30 multi-Time additive). The server returns the Time cap in
the reply message's cap slot.

Donation is opt-in. A Call() without a Time cap works identically to today's
model. The caller chooses whether to donate, and which of its Time caps to
include. The kernel does not track or enforce Time return — if the server keeps
the Time cap, that is a userspace protocol concern.

Crash safety is not a kernel concern: if the server dies, D33's cascade destroys
the donated Time (D32 asymmetry — capacity returns to kernel pool), but the
caller is already stuck on its reply field with no one to reply. The Time loss
is a symptom, not a separate catastrophe. Partial donation mitigates: the caller
donates one Time cap and retains others (D30).

Time transfer is necessarily a move, not a copy. D30's aggregate sums quantities
across held caps (`total += cap.amount`). If two caps reference the same Time
object (clone), the aggregate double-counts — violating "the kernel cannot
over-allocate." This constrains the open Time clonability question.

Scope: this transfers scheduling capacity (D36 compute units). It does not
transfer scheduling priority (D2 hints). Priority-level inheritance during IPC
is orthogonal, deferred to the D2 scheduling-hint exploration.

Cases requiring both Time donation and a payload cap in the same Call()
decompose: grant the authority via Send() first, then Call() with Time and data
words — the same pattern D26 establishes for data (shared Space as data plane,
message as signal). Adding more cap slots was rejected: the same conflict
re-appears at N+1. One slot for the atomic authority-per-request is structurally
right.

Kernel-internal donation (seL4 MCS pattern) was rejected: creates a Time
reference outside the capability graph for the Call/reply round-trip duration,
contradicting D29's cap-graph completeness rationale. Kernel-injected dedicated
Time field was rejected: solves a crash-safety problem that doesn't exist,
requires D28 revision, grows every message slot.

Does NOT settle: priority-level inheritance during IPC (D2 downstream), ~~Time
clonability~~ (settled by D38: non-clonable), server-side return protocol
(userspace convention), Send() with Time caps (naturally follows from standard
cap transfer).

- **Rests on:** D28 (fixed-size message with 1 user cap slot — Time donation is
  the natural use of the cap slot during Call(); "request + delegated authority"
  pattern decomposes via D26), D30 (multi-Time — server holds multiple clients'
  donated Time caps simultaneously; this was D30's motivating scenario), D36
  (normalized compute units — donation transfers a concrete integer quantity;
  composable via D30's aggregate), D31 (abstract capacity — donated compute
  units are core-independent; no rebinding needed; structurally simpler than
  seL4 MCS per-core donation), D4 (designation = authority — donation visible in
  cap graph; kernel-internal donation rejected on cap-graph completeness), D16
  (Call/reply — caller blocks on reply field; reply carries the returned Time
  cap), D13 (queued fields — cap-in-queue follows D24 precedent; direct-switch
  fast path eliminates transit for the common case), D24 (cap-in-transit
  precedent — Space caps in transit are unmapped for both parties; Time caps in
  transit are unscheduled for both parties), D32 + D33 (Time asymmetry + destroy
  cascade — crash-loss analysis; not a kernel concern because the caller is
  already stuck), D29 (Time is capability-held — donation uses existing cap
  transfer; kernel-internal rejected on D29's own rationale),
  `design/research/time-as-kernel-object.md` §2 (seL4 MCS donation: zero
  fastpath overhead, passive server model), `design/landscape.md` §4.5 (priority
  inversion handling — donation is one of four approaches surveyed).
- **Archive convergence:** Strong. Archive claims.toml "events-carry-resources":
  "Events can carry resource handles (Time, Space, routing capabilities). A
  sender that donates Time cannot run (effectively blocked)." Same conclusion,
  different path.
- **Status:** settled — revisit if D28 is revised (different cap slot count or
  message format changes the constraint), if D30 is revised (changes multi-Time
  semantics), if D2 scheduling-hint exploration reveals that priority
  inheritance requires coupling with Time donation (would reopen mechanism
  choice), or if the Send()+Call() decomposition proves unworkable for a
  critical workload class.
- **Journal:** `journal/037-time-donation-on-ipc.md`.

### D38 — Time capabilities are non-clonable

Time caps are linear: at most one capability reference exists per Time object.
Clone is structurally forbidden. Time caps can be transferred (moved) but not
duplicated.

D30's cached scheduling aggregate maintains a conservation invariant:
`total += cap.amount` on acquisition, `total -= cap.amount` on loss. Each cap
must reference a distinct Time object. Clone creates two references to the same
object, double-counting its compute units — the kernel believes more capacity
exists than is real. This violates "the kernel cannot over-allocate."

D37's move-only donation reinforces independently: if Time were clonable, the
caller could clone before donating, retaining scheduling capacity while the
server also counts it. D16 send-once provides precedent for non-clonable caps
(single-use invariant, structurally same pattern as Time's conservation
invariant).

D23's "identically to every other kernel object type" is narrowed: clone is a
per-type right, not a universal meta-operation. Each object type defines its
valid rights; clone appears in the rights sets of Space, Field, and Observer but
not Time. From the Observer's perspective, the distinction between
"meta-operations" and "type-specific rights" does not surface — a cap has a type
and a set of things you can do with it (A5: simple interface, kernel absorbs
dispatch complexity).

Authority delegation for Time uses split (creating a new Time object with a
portion of the original's quantity), not clone. Split creates two objects whose
quantities sum to the original — conservation holds. This is the Time analog of
Space split.

A1 parallel: linear Time caps map to Rust's ownership model — a move-only type
with no `Clone` impl.

Does NOT settle: Observer rights model (clone is confirmed as a per-type right,
shaping how rights masks are structured), Time split syscall surface (D36
remaining), duplicate-control right for other types (D23 deferred).

- **Rests on:** D30 (multi-Time additive aggregate — the conservation invariant
  that clone violates; load-bearing), D37 (move-only donation — clone would
  defeat capacity transfer; independent reinforcement), D29 (Time is
  capability-held — clone is a cap-level operation that must be evaluated per
  type), D16 (send-once precedent — non-clonable caps already exist in the
  design for structural soundness reasons), D23 (Observer clonability — correct
  for Observer, but universality framing narrowed; D23's structural arguments do
  not apply to Time because Time's soundness constraint is different), A5
  (per-type rights set is simpler interface than meta-operation/type-specific
  split).
- **Archive convergence:** Archive journal/014 assumed non-clonable Time but
  never derived it. Same conclusion, different path — archive assumed it as a
  property; this chain derives it from D30 soundness.
- **Status:** settled — revisit if D30 is revised (changes the aggregate model
  that makes clone unsound), or if a downstream derivation reveals that
  non-clonable Time creates essential complexity (orphan risk, authority
  delegation gaps) analogous to what D23 found for Observer — noting that Time's
  split operation provides the delegation path that clone provides for Observer.
- **Journal:** `journal/038-time-clonability.md`.

### D39 — Observer rights model: nine rights

The Observer capability carries nine rights, each corresponding to a typed
kernel syscall (D7) and a bit in D8's per-cap rights mask:

| Right             | Syscall                                        | Source   |
| ----------------- | ---------------------------------------------- | -------- |
| resume            | observer_resume(cap)                           | D14, D35 |
| destroy           | destroy(cap)                                   | D14, D33 |
| install-cap       | observer_install_cap(cap, source_cap) → slot   | D35      |
| write-registers   | observer_write_registers(cap, state)           | D35      |
| clone             | clone(cap, reduced_rights) → new_cap           | D38      |
| read-registers    | observer_read_registers(cap) → state           | D28, D39 |
| suspend           | observer_suspend(cap)                          | D39      |
| change-handler    | observer_change_handler(cap, field_cap, badge) | D39      |
| modify-scheduling | observer_set_scheduling(cap, hints)            | D39      |

Read-registers is derived mechanically from D28 (which assumes
`inspect(observer_handle)` exists) and is the structural dual of write-registers
(D35). Suspend adds a fifth Observer state (externally-suspended) alongside
inert, runnable, blocked, and faulted. Change-handler is separate from
install-cap because the handler slot is already structurally special (D21
kernel-reserved) and D12 establishes the fault handler as the root of the
Observer's supervision relationship — a fundamentally different authority from
routine cap provisioning. Modify-scheduling gates external modification of D2
scheduling hints (settled by D42: three-value profile — responsiveness,
throughput, precision).

Extract-cap (reading caps from another Observer's table) was evaluated and
excluded: the primary use case (pre-destroy resource recovery) is served by
proactive cap sharing through IPC (D23 clonable + D28 user cap slot).
Extract-cap compensates for userspace policy failures, not kernel mechanism
gaps. Deferred — reconsider if a structural need emerges. Duplicate-control
(Zircon ZX_RIGHT_DUPLICATE model, deferred from D23) is not Observer-specific;
it belongs in a D8 derivation applicable to all types uniformly.

Observer state machine: inert (D35), runnable, blocked (D13), faulted (D12),
externally-suspended (D39). Suspended can co-occur with blocked or faulted.
Resume is a single right covering all stopped→runnable transitions (landscape
consensus: seL4, Zircon, Mach).

Does NOT settle: Observer minimum schema (concrete struct fields — D39
constrains: must track five states including co-occurrence), ~~D2 minimum
scheduling properties~~ (settled by D42: three-value profile — responsiveness,
throughput, precision; modify-scheduling gates these values), self-reference
capabilities (whether an Observer holds a cap to itself), duplicate-control
right (D8 derivation), extract-cap (deferred), specific syscall encoding,
concurrent scheduling modification semantics (external vs. kernel-internal
responsiveness inheritance ordering).

- **Rests on:** D14 (resume and destroy as minimum; Observer as capability-held
  type), D35 (install-cap, write-registers, resume as creation rights;
  composable operations pattern), D38 (per-type rights; clone in Observer's
  set), D33 (destroy in rights mask), D28 (assumes inspect(observer_handle)
  exists — read-registers is the fulfillment), D7 (each right = typed kernel
  syscall), D8 (per-cap rights mask; nine bits), D23 (clonable — rights
  separation via attenuated clones is the access control mechanism), D20 + D21
  (per-Observer fault handler at reserved cap-table slot — handler change is
  separate authority from routine cap provisioning because D12 makes the handler
  structurally critical), D2 (abstract scheduling properties on Observer —
  external modification requires a right under D4), D36 (Time/ Observer split —
  scheduling hints are Observer properties), D37 (priority- level inheritance
  deferred to D2; modify-scheduling enables external complement), A3 (generic —
  suspend required for debugging, checkpointing, resource pressure workloads),
  D4 (designation = authority — handler change and scheduling modification are
  meaningful authorities requiring capability mediation), D6 (no kernel grouping
  — extract-cap's use case rests on supervision trees that are userspace policy;
  proactive cap sharing via D23 + D28 serves the same patterns),
  `design/research/syscall-landscape.md` (seL4 TCB operations, Zircon
  thread/task operations, Mach thread operations, L4 ExchangeRegisters —
  universal convergence on suspend, read-registers, scheduling modification),
  `design/landscape.md` §1.5 (capability granularity — rights masks), §4.4
  (scheduling parameters on thread object).
- **Archive convergence:** Strong. Archive's 10-syscall set maps closely:
  context_resume → resume, destroy_context → destroy, context_suspend → suspend,
  read_context_state → read-registers, write_context_state → write-registers,
  context_install_cap → install-cap, set_fault_handler → change-handler,
  set_scheduling_params → modify-scheduling. Divergence: archive bundled
  creation and Time-grant as Observer rights; D35/D30 settle those differently
  (creation on Space cap, Time as regular install-cap). Convergence on handler
  as separate operation.
- **Status:** settled — revisit if D35 is revised (changes the composable
  operations model), if D20/D21 are revised (changes the handler representation
  that motivates separate change-handler), if D2 is revised (affects
  modify-scheduling's scope), or if a downstream derivation reveals that
  extract-cap serves a structural need that proactive cap sharing cannot.
- **Journal:** `journal/039-observer-rights-model.md`.

### D40 — Pager fault resolution protocol: per-fault-type resolution via typed kernel syscalls

The pager's fault resolution sequence is per-fault-type, using existing typed
kernel syscalls (D7). No new kernel surface is introduced.

1. **Resource requests (D31):** `observer_install_cap(obs, cap)` +
   `observer_resume(obs)`. The Observer explicitly requested resources; the
   handler provides; the Observer adapts to the new VA base. install_cap IS the
   mapping operation under D26 (holding a Space cap = having access; cap-table
   mutation triggers page table update via D24).

2. **Cap-table-full (D8):** `observer_install_cap(obs, growth_space)` to a
   reserved growth slot + `observer_resume(obs)`. The kernel consumes the Space
   for table growth (D32 type conversion) and retries the original operation.
   The reserved growth slot follows D21's pattern (kernel-reserved cap-table
   slot). The slot is always writable regardless of regular-slot fullness. D32
   (line 1667): "optimizations (designated growth Space) can be added behind the
   same interface."

3. **VM page faults (out-of-bounds):** Dispatched to handler per D12 as error
   notification. The handler cannot resolve by providing a new Space — D26's
   kernel-assigned VA bases mean a new Space doesn't cover the faulting VA
   (`base_of(S) + offset` where offset exceeds S's size). Handler's response:
   destroy, or cooperative recovery via write-registers (D39) to redirect the
   Observer's PC to a pre-arranged trampoline. Memory growth uses the explicit
   D31 resource request path. Transparent demand paging requires Space resize
   (D9 open question).

4. **Lazy PTE population:** Kernel-internal per D12's preserved fast-path
   optimization. If the kernel lazily populates page table entries (D26 open
   sub-question), faults within owned Spaces are resolved by the kernel directly
   — no pager involvement. D9 guarantees physical backing exists.

5. **No kernel validation** of fault resolution before resume(). The kernel
   trusts the handler (D12's trust model). If the handler calls resume() without
   resolving the fault, the Observer re-faults (self-correcting).

install_cap + resume is the general-purpose pattern. D35's structural reuse
holds: the same operations serve Observer creation, resource request resolution,
and cap-table growth.

Does NOT settle: ~~Observer handle rights in fault message~~ (settled by D100:
exactly 5 of 9 rights — resume, destroy, install_cap, write_registers,
read_registers; kernel constructs TransferredCap directly), ~~Space resize (D9
open — would enable transparent demand paging)~~ (settled by D41: merge enables
demand paging), ~~fault message content per type~~ (settled by D100:
register-level layout for all four fault types; D28 downstream discharged),
~~pager unavailability protocol (separate question)~~ (settled by D68), VA
assignment policy details (D26 open).

- **Rests on:** D12 (fault delegation — kernel dispatches, doesn't contain
  policy), D14 (resume as typed kernel syscall; Observer handle in fault
  message), D26 + D24 (capability-addressed memory — holding Space cap = access;
  page table as materialized cap state; kernel-assigned VA bases create the
  demand-paging constraint), D35 (install_cap as general-purpose primitive —
  serves creation, fault resolution, dynamic delegation; no new kernel surface),
  D32 (type conversion — Space consumed for table growth; D8 table-full fault
  pattern), D21 (reserved cap-table slot precedent — growth slot follows same
  pattern), D28 (fault message format — type label enables per-fault-type
  dispatch by handler), D31 (resource requests through fault mechanism — the
  primary use case for install_cap + resume), D9 (Spaces always physically
  backed — lazy-population faults are kernel-internal), D39 (write-registers
  right enables cooperative recovery via PC surgery), D7 (typed kernel syscalls
  — resolution is syscall sequence, not IPC),
  `design/research/page-fault-routing.md` (seL4 map+reply, L4 fpage-in-reply,
  Mach data_provided, Zircon supply_pages — all assume pager-controlled VA; this
  kernel's D26 model diverges), `design/landscape.md` §2.4 (three fault handling
  patterns), §5.3 (fault delivery mechanisms).
- **Archive convergence:** Divergent. Archive (journal/011) unified IPC reply
  and fault resume as "send to reply/control endpoint" — IPC-based resolution.
  Current chain: D14 decoupled fault resume as typed syscall, D16 settled IPC
  reply as send-to-field. Archive did not have D26 and implicitly assumed
  VA-controlled mapping (traditional demand paging). Divergence explained by D7
  (typed syscalls for Observer operations), D14 (resume is not IPC), and D26
  (kernel-assigned VA bases limit demand paging).
- **Status:** settled — D41 settles Space merge, enabling transparent demand
  paging. The OOB fault path gains a resolution option: handler merges a source
  Space into the faulting Space, then resumes. The error-notification path
  remains available for cases where the handler chooses not to grow. Revisit if
  D26 VA assignment policy allows pager-influenced placement (changes the
  demand-paging constraint), or if D41 is revised.
- **Journal:** `journal/040-pager-fault-resolution-protocol.md`.

### D41 — Space merge and split

Spaces support two topology-changing operations: merge (two → one) and split
(one → two). These are the only operations that change Space boundaries.

**Merge:** a source Space is absorbed into a target Space. The source ceases to
exist as an independent Space. The target's VA range extends upward from its
fixed base (D26 — base is stable). All holders see the extended range
immediately (D24 — page table is materialized cap state). The source's physical
pages and page table subtree memory are absorbed. Follows D32 conservation:
pages change membership, not quantity. Merge can fail if no adjacent VA space is
available (kernel-internal VA layout policy, D26).

**Split:** a portion of a target Space is extracted into a new independent
Space. The new Space receives its own kernel-assigned VA base (D26). The
target's VA range contracts. Holders of the target may lose access to the
extracted portion (no automatic cap to the new Space — parallels D11 destroy
visibility). Follows D32 conservation: total pages unchanged.

Both are typed kernel syscalls (D7), cold-path (D1), require dedicated rights in
the Space rights mask (D4/D8), and operate at page granularity (D25). Split
requires cross-core TLB invalidation for shared Spaces (O2); merge likely does
not on ARM64 (the architecture does not cache translation faults for unmapped
ranges).

The conceptual model: Space is physical memory — it persists. Objects _occupy_
Space (D32 type conversion); merge and split change boundaries, not material.
"Grow" and "shrink" are the wrong framing because they imply creation or
destruction of material. Nothing appears or vanishes — boundaries move.

Split was already established as a pattern: D31/D32/D33 all use "the parent
Space shrinks by the allocation cost" — that is split. Merge is the genuinely
new primitive. It resolves D40's demand-paging gap: a pager handling an
out-of-bounds fault can merge a source Space into the faulting Space to cover
the offset, then resume the Observer. The existing pager protocol (install_cap +
resume) is unchanged — merge is an additional step before resume.

D32's unsettled "merge/join operation (reverse of split)" is resolved: merge is
the reverse of split.

Does NOT settle: syscall signatures (all-or-nothing merge vs. partial; split
extraction end), ~~separate merge/split rights or single topology right~~
(settled by D52: separate SPLIT and MERGE rights), VA headroom policy
(kernel-internal), Space vocabulary refinement ("not cumulative" wording),
COW/clone (D9 deferred, orthogonal), ~~complete Space rights mask~~ (settled by
D52: split, merge, destroy, clone).

- **Rests on:** D9 (variable-size kernel-managed Spaces — merge and split
  operate on D9 Spaces), D26 (capability-addressed memory — kernel-assigned VA
  bases create the demand-paging constraint that motivates merge; VA base
  stability determines that merge extends upward from fixed base; VA layout is
  kernel-internal), D32 (type conversion / conservation — merge follows the
  consumption pattern; split follows the existing delegation pattern; pages
  change membership, not quantity), D24 (cap-mapping invariant — all holders see
  topology changes immediately via page table updates), D40 (pager fault
  resolution — merge resolves the OOB demand-paging gap; the pager protocol is
  unchanged), D33 (page table subtree cost — extends from one-shot to
  incremental; source Space provides subtree memory on merge), D25 (page size
  exposed — operations are page-aligned), D4 + D8 (authority — dedicated rights
  in Space rights mask), D7 (typed kernel syscalls, not IPC), D1 (cold-path),
  A5 + O4 (merge absorbs demand-paging complexity into the kernel; the
  cooperative recovery alternative pushes essential complexity to userspace),
  D27 (flat cardinality — no hierarchy between Spaces; merge/split operate on
  individual Spaces without cascade), D11 (split visibility parallels destroy —
  holders learn of access loss via fault), `design/landscape.md` §2.1 (memory
  object models — Zircon VMOs resizable via `zx_vmo_set_size`; Barrelfish lists
  `resize` as capability operation), `design/research/syscall-landscape.md`
  §Barrelfish (resize in capability operations).
- **Archive convergence:** Partial. Archive (claims.toml
  "space-non-fungibility") noted "Space splitting produces distinguishable
  children" — same conclusion for split. Archive did not derive merge; its
  VA-addressed Space model ("space-named-by-virtual-address") allowed
  traditional demand paging without merge. Divergence explained by D26
  (kernel-assigned VA bases).
- **Status:** settled — revisit if D26 is revised (different VA model may change
  the demand-paging constraint), if D32 is revised (changes the conservation
  model), or if a downstream derivation reveals that the VA adjacency constraint
  on merge creates essential complexity (merge failure rates unacceptable under
  realistic workloads).
- **Journal:** `journal/041-space-merge-and-split.md`.

### D42 — Three-value scheduling profile: responsiveness, throughput, precision

The minimum abstract scheduling properties on an Observer are three values
sharing a fixed per-Observer budget: **responsiveness**, **throughput**, and
**precision**. The Observer distributes points across the three dimensions (R +
T + P ≤ budget). Each dimension controls an aspect of how the Observer's Time
allocation is delivered:

- **Responsiveness:** how quickly the Observer is scheduled when runnable.
- **Throughput:** how long the Observer runs when scheduled (uninterrupted
  slices).
- **Precision:** how accurately the scheduler hits timing targets (low jitter,
  deadline accuracy).

Spending on one dimension takes from the other two. The trade-offs are
physically grounded: high responsiveness costs context-switch overhead
(~1000–5000 cycles per switch on ARM64), high throughput costs scheduling
latency, high precision constrains the scheduler's flexibility for the other
two. Every Observer gets the same budget. No dimension is strictly better — the
right distribution depends on the workload. This dissolves the priority
inflation problem: there is no single value to max out, and maximizing any
dimension costs real capability in the other two. No MCP-style delegation bound
is needed.

D2's parenthetical "(priority, CPU/IO classification, optional deadline)" is
replaced entirely. There is no priority integer. CPU/IO classification is
kernel-inferred from the profile. Core placement is kernel-internal
(D31/D36/A5). Deadline is kernel-derived from timer-programmed periods + the
precision value.

Hard real-time scheduling uses Time allocation (compute budget, D36) +
kernel-programmed timer period (the kernel knows T because it set the timer) +
the precision value (how tight the guarantee must be). On a dedicated RT core
(D2 per-core scheduler), EDF admission uses these for the schedulability test.
The precision value provides what the archive's tolerance parameters provided,
but with a self-enforcing per-Observer cost (spending on precision takes from
responsiveness and throughput).

Scheduling inheritance during IPC is a userspace concern (D43). The Observer
struct holds one set of R/T/P values — no base/effective split. The server's
scheduling profile is determined by the server's work, not by who requested it.
If a supervisor wants to adjust a server's profile before dispatching a
latency-sensitive request, it uses modify-scheduling (D39). The kernel provides
the mechanism; userspace provides the policy.

Does NOT settle: ~~budget size and encoding~~ (settled by D57: budget 128, store
R and T as u8, derive P), ~~default profile for newly-created Observers~~
(settled by D57: R=43, T=43, P=42), timer syscall surface, ~~Observer minimum
schema~~ (settled by D43: three scheduling fields, no effective variants —
inheritance is userspace policy), admission control details on RT cores (how
precision + Time + timer period compose).

- **Rests on:** D2 (per-core schedulers may run different algorithms — the
  three-value profile must be interpretable by all algorithm families; per-core
  RT schedulers use precision + Time + timer period for admission), D36 (Time
  carries compute quantity, Observer carries scheduling hints — the profile
  values are scheduling hints; qualitative, not quantitative), D37 (Time
  donation transfers compute not scheduling properties — scheduling inheritance
  during IPC is the complement; deferred from D37), D39 (modify-scheduling right
  gates external modification of the profile; three structural use cases:
  supervisor adjustment, scheduling inheritance, load-balancing policy), D1
  (hot-path — scheduler reads effective profile from Observer struct; three
  integers, O(1)), D31 (core assignment kernel-internal — the kernel uses the
  profile + Time to inform placement decisions without exposing core identity),
  D13 (queued fields — timer delivery through the existing field mechanism; the
  kernel-as-sender pattern provides period information to the scheduler), A2
  (big.LITTLE — the profile provides core-placement information without naming
  core types), A3 (generic — the three dimensions span all workload types from
  interrupt handlers to batch compute to RT control loops), A5 (kernel absorbs
  complexity — three values from Observer, kernel derives timing parameters from
  timer requests and Time allocation; structured timing declarations would push
  parameter management to userspace), `design/landscape.md` §4.2 (scheduling
  algorithm survey), §4.5 (priority inversion — scheduling inheritance is the
  analog), §4.6 (real-time guarantees — precision dimension covers the RT
  spectrum), §4.7–4.8 (energy-aware scheduling, interactive responsiveness —
  kernel-internal, informed by the profile),
  `design/research/time-object-content.md` (Observer vs. Time object split
  taxonomy — priority/QoS on execution unit, quantity on Time object).
- **Archive convergence:** Strong. Both derivations separate resource (Time)
  from scheduling preference, reject priority integers, and require every
  parameter to have a cost. The archive's six parameters (mode + d + dt +
  denom + tol) collapse to three values because the kernel already knows period
  (timer-programmed) and compute budget (Time allocation). The precision
  dimension captures the archive's tolerance spectrum (tight = hard-RT, loose =
  best-effort) with a self-enforcing per-Observer cost. See journal for full
  convergence analysis.
- **Status:** settled — revisit if D2 is revised (changes the per-core algorithm
  heterogeneity that allows RT cores), if D36 is revised (changes the
  Time/Observer split), if the timer mechanism derivation reveals that the
  kernel cannot derive sufficient timing information from timer requests (would
  reopen explicit timing declarations on the Observer), or if a downstream
  derivation reveals that three values are insufficient to express a
  structurally required scheduling scenario.
- **Journal:** `journal/042-scheduling-properties.md`.

### D43 — Observer minimum schema

The Observer metadata struct contains eight field clusters, mechanically derived
from settled decisions. The Observer physically splits into two regions: a small
metadata struct (root Space, D32 "bounded, small") and large structural backing
(consumed Space, D35 type conversion — register save area, cap table pages, L0
page table root).

**Forced fields (metadata struct):**

| Field                         | Type                                                           | Source      | Path                |
| ----------------------------- | -------------------------------------------------------------- | ----------- | ------------------- |
| Register save pointer         | pointer                                                        | D6/D35/D32  | Hot (ctx switch)    |
| TTBR0 value                   | u64                                                            | D5/D26/D1   | Hot (ctx switch)    |
| Cap table pointer             | pointer                                                        | D4/D8       | Hot (syscall entry) |
| Scheduling state              | enum {inert, runnable, blocked, faulted} + suspended flag      | D39         | Hot (scheduler)     |
| Cached compute-unit aggregate | integer                                                        | D30/D31/D36 | Hot (scheduler)     |
| Responsiveness                | u8 (0–128)                                                     | D42/D57     | Hot (scheduler)     |
| Throughput                    | u8 (0–128)                                                     | D42/D57     | Hot (scheduler)     |
| Wait-state linkage            | enum {None, Single(prev/next/field), Multi(allocated entries)} | D18/D19     | Cold (block/wake)   |
| Reference count               | integer                                                        | D11/D33     | Cold (cap ops)      |

**Excluded from the struct:**

- Fault handler → cap-table reserved slot (D21)
- Reply field → cap-table reserved slot (D16 + D21 pattern)
- Time caps → cap-table regular slots (D30)
- Algorithm-specific scheduler state → per-core (D2)
- Core assignment → transient, re-decided per runnable transition (D31)
- Effective/base scheduling split → none; one set of R/T/P values, userspace
  manages dynamic adjustment via modify-scheduling (D39)

**Key design decisions:**

1. _No kernel-side scheduling inheritance._ The scheduling profile values are
   the Observer's current values, period. No base/effective split. Scheduling
   adjustment during IPC (priority inheritance) is a userspace policy concern —
   the server's optimal profile is determined by the server's work, not by who
   requested it. The kernel provides mechanism (modify-scheduling); userspace
   provides policy.

2. _Transient core assignment._ No core ID field. The kernel makes a fresh
   placement decision each time an Observer transitions to runnable. Every
   wake-up is an implicit migration opportunity. Cache affinity is a per-core
   scheduler hint, not a per-Observer field.

3. _Wait-state as Rust enum._ Inline single-field variant (zero allocation for
   the common case) with allocated multi-field variant (supports future
   multi-receive without schema rework). A1-idiomatic enum-as-state-machine.

4. _Reply field follows D21 pattern._ The three arguments that settled D21 (D11
   destroy-invalidation, D17 badge-closure, D8 ABA protection) apply
   identically. Second reserved cap-table slot.

Does NOT settle: wait-state allocation source for multi-field, cap table
capacity tracking placement (implementation optimization), ~~register save area
layout within structural backing~~ (already implemented:
`src/arch/aarch64/register_state.rs`, 816 bytes), ~~budget size/encoding~~
(settled by D57: budget 128, store R and T as u8, derive P = 128 - R - T),
~~default scheduling profile~~ (settled by D57: R=43, T=43, P=42),
~~self-reference capabilities~~ (settled by D57: kernel-installed at reserved
slot 2 with full rights).

- **Rests on:** D32 (type conversion — metadata from root Space, structural
  backing from consumed Space; "bounded, small" forces pointer indirection for
  registers), D35 (creation — consumed Space becomes cap table + L0 root +
  register save area; inert state), D6 (one register state, one PC), D5/D26
  (per-Observer L0 page table, TTBR0), D4/D8 (flat cap table, pointer in
  struct), D39 (five-state machine with co-occurrence), D30/D31/D36 (cached
  aggregate of compute units), D42 (three scheduling profile values), D18/D19
  (intrusive wait-state linkage, multi-field accommodation), D11/D33 (reference
  count from close/destroy semantics), D21 (fault handler and reply field as
  cap-table entries — not struct fields), D2 (algorithm-specific state
  per-core), D31 (core assignment kernel-internal — transient), A1 (Rust enum
  for wait state), A2 (register file size → pointer indirection), A4 (reactive —
  struct must contain everything for resumption), A5 (modify-scheduling is
  mechanism; inheritance policy is userspace).
- **Archive convergence:** Strong on register state, TTBR, scheduling state,
  wait-state linkage. Divergent on fault handler placement (D21 moved it),
  cached aggregate (D30 multi-Time created the need), scheduling profile (D42
  vs. archive's timing declarations), reference count (D11/D33). See journal.
- **Status:** settled — revisit if D32 is revised (changes the metadata/backing
  split), if D39 is revised (changes the state machine), if D42 is revised
  (changes the scheduling profile), if D18/D19 is revised (changes wait-state
  requirements), or if a downstream derivation reveals a structurally required
  field not present in this set.
- **Journal:** `journal/043-observer-minimum-schema.md`.

### D44 — Pulsar: capability-held timer object with kernel-managed delivery

Pulsar is the fifth kernel object type (Space, Time, Observer, Field, Pulsar). A
Pulsar is a timer that the kernel programs on behalf of an Observer and delivers
as a field message when it fires. Creation consumes Space (D32 type conversion).
The Pulsar is armed on creation with a delivery field cap, badge, duration, and
period.

**Delivery:** kernel-as-sender to the designated field with the designated badge
(D13/D17 pattern, parallel to D22 interrupts and D12 faults). Message includes
actual fire time in a data word.

**Repeating:** For period > 0, the kernel re-arms automatically with
drift-compensated deadlines (`next = scheduled + period`). The Observer does not
participate in re-arm. Period = 0 means one-shot.

**Overflow:** When the delivery field is full, the kernel stops re-arming. On
the next receive that frees a slot, the kernel re-arms and includes an overrun
count. Parallels D22 mask-on-delivery.

**Scheduler input:** The Pulsar's period is T in D42's EDF admission test (Σ
C_i/T_i ≤ 1.0). Setting or destroying a Pulsar triggers re-evaluation.

**Manual control:** Observers needing adaptive timing (variable period, drift
compensation, tick skipping) use one-shot Pulsars in a loop.

**Clock access:** Per-Observer controlled. The kernel manages
CNTKCTL_EL1.EL0VCTEN per context switch. Observers with clock-access authority
read CNTVCT_EL0 directly (~1 cycle). Others use a typed kernel operation.

Does NOT settle: ~~Pulsar rights mask~~ (settled by D52: destroy + clone),
~~creation API shape~~ (settled by D62: single-call, armed-at-creation), ~~full
message content layout~~ (settled by D63: badge + LABEL_TIMER_FIRE + fire_time +
overrun_count + reserved + empty cap), ~~duration vs. absolute deadline API~~
(settled by D72: relative duration in nanoseconds; absolute mode not
foreclosed), ~~clock access mechanism~~ (settled by D66: per-Observer bool,
CNTKCTL_EL1.EL0VCTEN on context switch), clock access authority mechanism and
default policy (genuine choices, decoupled from G09 by D72), ~~badge-filtered
receive~~ (closed by D71: not needed — D45 routing serves the use case;
receive-time filtering tensions D13 queue semantics, D18 overflow, and D50
fast-path).

- **Rests on:** D4 (capability-based authority — Pulsar caps in cap table,
  cancel = destroy via D11), D7 (split model — timer operations are typed kernel
  syscalls), D13 (queued fields — timer delivery through existing field
  mechanism; kernel-as-sender pattern), D17 (badges — timer messages carry
  minter-assigned badges for identification; multiple Pulsars to same field
  distinguished by badge), D22 (interrupt delegation — structural parallel;
  kernel detects hardware event, enqueues message, returns; overflow parallels
  mask-on-delivery), D28 (fixed-size message format — timer message fits
  existing format), D32 (type conversion — Pulsar creation consumes Space;
  self-limiting resource accounting), D42 (scheduling profile — Pulsar period is
  T for EDF admission; precision value modulates delivery guarantee), A2 (ARM64
  generic timer — per-core, one-shot, kernel multiplexes; CNTVCT_EL0 for
  per-Observer clock access), A3 (generic — timer interface serves all
  workloads; per-Observer clock access control serves both precision-sensitive
  and security-sensitive workloads), A4 (purely reactive — timer fire is a
  hardware exception; re-arm is exception-triggered processing using persistent
  state), A5 (leaf node — kernel absorbs timer multiplexing, drift compensation,
  overflow handling, and scheduling integration; one-shot Pulsars provide escape
  hatch when Observer-managed timing is needed).
- **Archive convergence:** No convergence or divergence on Pulsar as an object
  type — the archive did not reach this question. The archive has no userspace
  timer concept; periodic behavior is declared via scheduling parameters on the
  Context (d, dt, p, pt). Divergence explained by same factor as D42: the
  current design derives T from the Pulsar's period rather than requiring
  explicit scheduling declarations. See journal.
- **Status:** settled — revisit if D42 is revised (changes the scheduling
  profile or timer period's role in EDF admission), if D13 is revised (changes
  the delivery mechanism), if D32 is revised (changes the type conversion
  model), if D22 is revised (changes the interrupt delivery pattern that Pulsar
  parallels), or if a downstream derivation reveals that kernel-managed re-arm
  cannot serve a structurally required timer pattern.
- **Journal:** `journal/044-userspace-timer-interface.md`.

### D45 — Field split: badge-range routing with fallback-on-destroy

Field split is a typed kernel operation (D7) that installs a badge-range routing
rule on a source Field, directing matching messages to a destination Field. The
current syscall surface exposes split-to-new only: the destination is always a
newly created Field (backed by caller's Space, D32). Split-to-existing (routing
into an existing Field) is deferred, not foreclosed (journal 071). Split is a
receive-side operation — senders are oblivious. Their caps still designate the
source Field; the kernel routes internally by badge before enqueuing.

The destination is a separate Field object with standard lifecycle (D11). When
the destination is destroyed, the routing rule on the source is removed and
traffic falls back to the source's queue — automatic crash recovery without
parent→child tracking.

Routing is composable across Field boundaries: a message routed to a destination
passes through that Field's own routing rules before reaching its queue. The
kernel may flatten routing chains into a direct badge→leaf-Field lookup as a
kernel-internal optimization (parallels D24: page tables as materialized views
of cap state).

Per-send cost: O(log N) binary search over non-overlapping badge ranges on split
Fields, where N is the number of splits on that specific Field. Unsplit Fields
pay zero (null routing table → existing fast path unchanged). The D13
direct-switch optimization must follow routing before attempting the match:
determine the destination Field, then check that Field's waiters list.

**Combine does not exist as a separate primitive.** Reversing a split = destroy
the destination Field (routing rule removed, traffic falls back). Merging
unrelated Fields would decompose into split-to-existing + destroy, but
split-to-existing is deferred (journal 071); Field merge is therefore not
currently expressible.

**Entrance management** (send-side) uses existing capability operations (D17
mint, D23 clone, D11 close). No new mechanism — split is exclusively about
receive-side routing.

Split-to-existing would enable a key use case: a driver wanting interrupts and
IPC on one Field. With split-to-new only, the driver receives a second Field and
uses multi-receive (D19, planned) to wait on both. Split-to-existing remains
deferred until the authority coherence question (routing-target consent via send
cap) is explored (journal 071).

Authority: receive cap with split right on the source Field; Space cap consumed
for the new Field's backing (split-to-new, D32). Split-to-existing authority
(send cap on the destination) deferred with that variant.

Rejected alternatives: transparent forwarding (D1 hot-path violation), port sets
/ non-destructive aggregation (D13 one mechanism, D19 already rejected),
internal sub-queues (no independent lifecycle, D4/D8 sub-object tension), IRQ-
only restriction (badge-range routing works identically for IRQs and IPC — no
architectural reason to restrict).

Does NOT settle: ~~badge condition form~~ (settled by D71: range
`low <= badge <= high`; bitmask foreclosed by D54 binary search incompatibility;
predicate foreclosed by A5 + D1), ~~whether split-to-new and split-to-existing
are one syscall or two~~ (settled by journal 071: split-to-new only;
split-to-existing deferred, not foreclosed; multi-receive planned per D19 covers
the two-Field case), Field rights mask (split right details, complete Field
rights set), queued message handling at split time, D17 badge-closure tracking
partitioning on split, ~~routing table structure~~ (settled by D54: nullable
pointer to external sorted array, allocated from root Space), flattened routing
table update protocol.

- **Rests on:** D22 (interrupt delegation through fields — introduces split as
  the delegation mechanism; IRQ routing is the canonical use case), D15
  (unidirectional, many-to-many fields — topology via capability distribution;
  senders are oblivious to receive-side changes; the "allow shape, don't enforce
  it" principle applied to reconfiguration), D13 (all information delivery
  through queued fields — routing routes through the existing mechanism; one
  mechanism preserved; direct-switch fast path must follow routing), D17 (badges
  — routing condition matches on minter-assigned badges; badge fan-in is the
  send-side composition mechanism, D19), D4 (designation = authority — senders'
  caps designate the source Field; the kernel routes internally without
  invalidating caps), D8 (flat cap table with rights mask — split right in the
  Field's rights set), D32 (type conversion — split-to-new consumes Space;
  conservation holds), D11 (base revocation — destination Field destroy triggers
  routing rule cleanup; fallback-on-destroy follows from D11's dead-handle
  protocol applied to the kernel's internal reference), D33 (destroy cascade —
  Field destroy doesn't cascade; routing rule cleanup is O(1) per source that
  routes to the destroyed Field), D1 (hot-path — per-send routing cost is O(log
  N) on split Fields, zero on unsplit; transparent forwarding foreclosed), D7
  (typed kernel syscall — split is a kernel operation on Field objects), D44
  (Pulsar precedent — split-to-existing uses the same pattern as Pulsar delivery
  Field: caller provides a Field cap as a message destination), D41 (Space
  merge/split — structural analogue for topology-changing operations; D41
  informed the framing but Field split diverges: Space merge/split changes
  boundaries of a single resource; Field split installs routing rules between
  independent objects), D19 (multi-field wait — combine-as-multi-wait dissolves;
  split-to-existing serves the IRQ+IPC-on-one-Field use case; badge fan-in
  covers cooperative cases), `design/landscape.md` §3.3 (IPC object model survey
  — no surveyed kernel provides split/combine on IPC endpoints; Mach port sets
  are non-destructive aggregation, seL4 uses badge fan-in, Zircon uses port
  aggregation), `design/research/field-overflow-and-multi-wait.md` (multi-source
  wait patterns confirm aggregation landscape).
- **Archive convergence:** None. The archive does not contain a concept of field
  split or combine. The archive routes interrupts through a supervision tree
  (claims.toml: "events routed by the supervision tree"). The current design
  routes through field topology. No convergence or divergence — the archive
  didn't reach this question.
- **Status:** settled — revisit if D15 is revised (changes the Field shape or
  many-to-many model), if D13 is revised (changes the IPC mechanism that split
  routes through), if D1 is revised (changes the hot-path constraint that shapes
  routing cost analysis), if D22 is revised (changes the interrupt model that
  motivated split), or if a downstream derivation reveals that per-send routing
  cost is unacceptable for a structurally required workload pattern.
- **Journal:** `journal/045-field-split.md`.

### D46 — Core lifecycle is kernel-internal

Core activation, idle management, and deactivation are fully kernel-internal.
Observers do not know what cores exist, how many are active, or when one
activates or deactivates. This extends D31/D36's "core assignment is
kernel-internal" to "core existence is kernel-internal." The Space parallel:
cores are to Time what physical pages are to Space — implementation details of
the kernel's resource management, invisible to Observers.

**Activation:** The kernel activates all discovered cores during boot, before
creating the root Observer. Each core initializes its per-core kernel structures
(D1), configures its MMU (D5), enables its GIC redistributor (D22), and enters
the scheduling loop. PSCI `CPU_ON` is the hardware mechanism (A2). No userspace
syscall triggers activation.

**Idle:** Cores with no runnable Observers enter an idle state. The specific
power state (WFI, PSCI CPU_SUSPEND, or platform-specific sleep) is an
architecture-specific implementation detail behind `src/arch/`. The kernel wakes
idle cores via IPI (O2) when work arrives.

**Deactivation:** The kernel may fully deactivate an idle core (PSCI CPU_OFF)
when the D36 conservation invariant permits. Because Time is fungible (D36
normalized compute units), deactivation requires no per-core-origin tracking or
Time cap revocation — only a bookkeeping check:
`unallocated_time_pool ≥ core_capacity`. If true, the kernel shrinks the pool by
that amount and powers off the core. Re-activation follows the boot-time
initialization path.

No "activate core" syscall. No Core kernel object type. No capability for cores.
Core management is an implementation concern, parallel to physical page
management (D9) and VA assignment (D26).

Does NOT settle: specific idle power state policy per platform, interrupt
routing policy across cores (which core receives a given SPI), per-core
scheduler algorithm selection policy, ~~boot ordering for secondary cores
(parallel vs. sequential PSCI CPU_ON)~~ (settled by D93: BSP completes
init_kernel_state before any PSCI CPU_ON; secondaries activate after global is
live; existing activate_secondaries pattern preserved), deactivation decision
thresholds.

- **Rests on:** D31 (core assignment kernel-internal — extends to core
  existence; boot architecture — secondary core bring-up is the remaining
  question this settles), D36 (normalized compute units — Time fungibility makes
  deactivation a bookkeeping check, not a revocation problem; core capacity
  factors enable conservation invariant on heterogeneous hardware), D1 (per-core
  kernel structures — activation initializes these; idle/wake preserves them),
  D2 (per-core schedulers — algorithm assignment is kernel-internal; newly
  activated cores get scheduler instances chosen by the kernel based on D42
  profiles of runnable Observers), A4 (purely reactive — no kernel thread to
  autonomously manage cores; activation at boot sidesteps the trigger problem;
  idle cores wake via IPI on the exception-handling path), A2 (ARM64 — PSCI
  CPU_ON/CPU_OFF/ CPU_SUSPEND; GIC redistributor per core), A3 (generic —
  boot-time activation with idle power management avoids workload assumptions;
  lazy activation rejected for ~1ms latency and runtime complexity), D5 (MMU —
  per-core TTBR/MAIR/TCR/ SCTLR initialization), D22 (GIC redistributor per core
  — interrupt dispatch is kernel-internal; new core configures its own
  redistributor), D43 (transient core assignment — Observers migrate naturally
  on deactivation; no struct field for core identity), O2 (IPI — idle core wake
  mechanism), `design/landscape.md` §7.3 (PSCI, spin tables, ACPI parking —
  firmware mechanisms for ARM64 multicore bringup), §5.7 (GICv3 redistributor
  per core).
- **Status:** settled — revisit if D31 is revised (changes the core-independence
  model), if D36 is revised (changes Time fungibility or conservation model —
  deactivation check depends on fungible compute units), if A4 is revised
  (background kernel activity would enable runtime-triggered activation), or if
  a downstream derivation reveals that boot-time activation creates essential
  complexity (e.g., embedded workloads where core power draw during boot is
  unacceptable even briefly).
- **Journal:** `journal/046-core-activation.md`.

### D47 — Syscall ABI: SVC immediate, IPC-optimized registers, two-level numbering

The syscall ABI has three components:

**Trap mechanism:** SVC #imm16. The operation is encoded in the SVC
instruction's 16-bit immediate field, readable from ESR_EL1[15:0] at zero
additional cost (the kernel already reads ESR_EL1 to confirm EC=0x15). This
frees all 8 argument registers (x0–x7) for payload — critical because D28's
message format exactly fills 8 registers on both send and receive.

**Register convention:** IPC-optimized, uniform for both mechanism families.
Registers are mapped to D28's message format:

- x0–x3: primary payload (4 IPC data words / typed-operation args)
- x4: label (IPC) / operation code (typed ops, SVC #0 only)
- x5: field handle (IPC send) / badge (IPC receive) / target handle (typed ops)
- x6: cap handle (IPC) / secondary arg (typed ops)
- x7: flags (IPC send) / reply handle (IPC receive) / additional arg (typed ops)

This layout enables a fast-path optimization: on direct switch (D13), x0–x3 pass
through in physical registers without save or restore. The kernel's fast-path
code must not use x0–x3 as scratch — an invariant maintained in the exception
entry assembly.

**Numbering:** Two-level, reflecting D7's split. IPC operations are nonzero SVC
immediates (SVC #1 through #N, one per IPC operation). Typed kernel operations
are SVC #0, with the specific operation code in x4. The kernel dispatches IPC
operations from ESR_EL1 alone — before reading any GPR.

Does NOT settle: ~~error signaling convention~~ (settled by D49: carry flag for
IPC, negative-x0 for typed ops), ~~cap-present indicator encoding~~ (settled by
D49: sentinel u64::MAX), ~~specific SVC number assignments~~ (settled by D49:
#1–#5), ~~typed operation code assignments within x4~~ (settled by D49: grouped
sequential 0–19), ~~large return value convention~~ (settled by D49: userspace
buffer pointer), ~~IPC fast-path conditions~~ (settled by D50: scheduler
callback, 0-cap gate, Call + ReplyRecv scope).

- **Rests on:** D28 (fixed-size message format — 4 data words + 1 cap + label
  exactly fills 8 ARM64 registers, making the discriminator's placement
  load-bearing), D7 (split interaction model — two families with different
  performance profiles; the two-level numbering encodes this split), D13
  (direct-switch fast path — the IPC-optimized register layout enables zero-copy
  data word pass-through, saving ~20–30% of the fast-path cycle budget), D4/D8
  (capability handles are small integers — one register per handle), D17 (badge
  is kernel-injected — send side has one fewer value than receive, leaving room
  for the field handle), A2 (ARM64 — SVC #imm16 is a hardware feature; ESR_EL1
  decoding is free), A4 (SVC is the sole kernel entry mechanism), D1 (per-core
  hot path — fast-path savings are per-core with no lock contention),
  `design/research/syscall-landscape.md` §8 (minimal kernel: 5–7 operations;
  seL4's 8 syscalls as pragmatic minimum), §10 (IPC as pivot point — IPC syscall
  is the most-executed entry point by orders of magnitude).
- **Status:** settled — revisit if D28 is revised (message size change alters
  the register budget), if D7 is revised (unified model removes the two-level
  motivation), if D13 is revised (different IPC model changes fast-path
  assumptions), or if the fast-path register pass-through proves impractical
  (kernel fast-path code requires x0–x3 as scratch).
- **Journal:** `journal/047-syscall-abi.md`.

### D48 — Syscall enumeration: 5 IPC + 20 typed = 25 operations

The kernel's complete syscall surface, collected from all settled derivations.

**IPC operations (Family 1 — nonzero SVC immediates, 5 operations):**

| Operation | Behavior                                                                                                       | Source              |
| --------- | -------------------------------------------------------------------------------------------------------------- | ------------------- |
| Send      | Non-blocking deposit into Field. Error on full (D18). Also serves as Reply (D16 send-once consumed by kernel). | D13                 |
| Receive   | Blocking wait on Field. Returns message in registers.                                                          | D13                 |
| Call      | Send + block on reply field. Kernel creates send-once reply cap.                                               | D16                 |
| ReplyRecv | Send reply via send-once + receive next on same field. Server fast path.                                       | D16                 |
| Yield     | Voluntary CPU relinquishment. Scheduling hint, not communication.                                              | D48 (A3, landscape) |

NBSend rejected: Send never blocks (D13 queued + D18 error-on-full). Reply
rejected: Send to a send-once cap IS Reply (D16 right-based, not
mechanism-based). NBRecv deferred: not foreclosed, D19 pattern; if
polling/event-loop patterns prove painful, add as a flag on Receive or a
separate SVC number.

**Typed kernel operations (Family 2 — SVC #0, operation code in x4, 20
operations):**

Observer operations (D39 — nine rights):

| Operation                | Signature                | Source   |
| ------------------------ | ------------------------ | -------- |
| observer_resume          | (cap)                    | D14, D35 |
| observer_install_cap     | (cap, source_cap) → slot | D35      |
| observer_write_registers | (cap, state)             | D35      |
| observer_read_registers  | (cap) → state            | D28, D39 |
| observer_suspend         | (cap)                    | D39      |
| observer_change_handler  | (cap, field_cap, badge)  | D39      |
| observer_set_scheduling  | (cap, hints)             | D39      |

Generic cap operations (cross-type):

| Operation | Signature                              | Source   |
| --------- | -------------------------------------- | -------- |
| destroy   | (cap) → space_cap                      | D11, D33 |
| clone     | (cap, reduced_rights) → new_cap        | D23, D39 |
| close     | (slot)                                 | D11      |
| mint      | (cap, badge, reduced_rights) → new_cap | D17      |

Space operations:

| Operation   | Signature                | Source |
| ----------- | ------------------------ | ------ |
| space_split | (cap, size) → new_cap    | D41    |
| space_merge | (target_cap, source_cap) | D41    |

Field operations:

| Operation    | Signature                                     | Source           |
| ------------ | --------------------------------------------- | ---------------- |
| create_field | (space_cap) → field_cap                       | D32              |
| field_split  | (cap, badge_range, space_cap) → new_field_cap | D45, journal 071 |

Time operations:

| Operation  | Signature               | Source |
| ---------- | ----------------------- | ------ |
| time_split | (cap, amount) → new_cap | D38    |

Pulsar operations:

| Operation     | Signature                                             | Source        |
| ------------- | ----------------------------------------------------- | ------------- |
| create_pulsar | (space_cap, field_cap, badge, duration, period) → cap | D44, D32, D72 |
| clock_read    | () → timestamp                                        | D44           |

Observer creation:

| Operation       | Signature                                   | Source   |
| --------------- | ------------------------------------------- | -------- |
| create_observer | (space_cap, handler_field_cap, badge) → cap | D35, D32 |

Resource acquisition:

| Operation        | Signature | Source |
| ---------------- | --------- | ------ |
| resource_request | (type)    | D31    |

The 25-operation total places this kernel between Coyotos (~25 effective) and
seL4 (~60 effective). All irreducible categories from
`design/research/syscall-landscape.md` §8 are covered: IPC, object creation,
resource management, lifecycle control, capability operations, scheduling
control. Interrupt delivery flows through Fields (D22) with no dedicated
operation.

Does NOT settle: ~~specific SVC number assignments~~ (settled by D49: #1–#5),
~~typed operation code assignments within x4~~ (settled by D49: grouped
sequential 0–19), ~~error signaling convention~~ (settled by D49: carry flag for
IPC, negative-x0 for typed), ~~cap-present indicator~~ (settled by D49: sentinel
u64::MAX), ~~large return value convention~~ (settled by D49: userspace buffer
pointer). ~~Pending additions from Space rights mask, Field rights mask, and
Pulsar rights mask~~ (settled by D52: complete per-type rights masks for all
four remaining types). time_merge not included (no functional need — D30
additive aggregate makes holding multiple Time caps equivalent; not foreclosed).

- **Rests on:** D7 (split interaction model — two families, each right = typed
  kernel operation), D47 (ABI framework — two-level numbering, register
  convention), D13 (queued fields — Send is non-blocking, eliminating NBSend;
  direct-switch fast path), D16 (send-once caps — Reply is Send, eliminating
  standalone Reply; Call/ReplyRecv compound operations), D18 (error-on-full —
  completes Send's non-blocking guarantee), D11 (close-only + destroy — close
  and destroy as explicit typed operations), D17 (mint as third Field right —
  typed kernel operation), D23 (clone as per-type right), D38 (Time non-clonable
  — delegation via split), D39 (nine Observer rights = nine operations), D35
  (Observer creation API — create + install_cap + write_registers + resume), D41
  (Space merge and split), D44 (Pulsar create + clock_read; cancel = destroy),
  D45 (Field split), D32 (type conversion — create operations consume Space),
  D31 (resource request as typed kernel syscall triggering fault routing), D28
  (inspect reconciled as observer_read_registers), A3 (generic — Yield included
  for compute-bound workload support; 100% landscape convergence),
  `design/research/syscall-landscape.md` §8 (irreducible set verification).
- **Status:** settled — revisit if a downstream rights mask derivation (Space,
  Field, Pulsar) reveals operations that change the typed set, if D13 is revised
  (changes Send's blocking behavior and may restore NBSend), if D16 is revised
  (changes send-once semantics and may restore standalone Reply), or if NBRecv
  proves necessary (add as flag on Receive or separate SVC number — no
  structural change needed).
- **Journal:** `journal/048-syscall-enumeration.md`.

### D49 — Syscall ABI encoding: error signaling, cap-present, SVC assignments, typed op codes, large return values

Five encoding details completing the syscall ABI (D47 framework + D48
enumeration).

**Error signaling — split convention:**

IPC operations use the ARM64 carry flag in SPSR_EL1. The kernel modifies
SPSR_EL1 before eret: carry clear = success (registers carry normal message
payload), carry set = error (x0 = error code, x1–x7 undefined). This is the only
approach that preserves all 8 registers for D28's message data — error-in-x0 is
impossible because data words are arbitrary 64-bit values. NZCV flags are
caller-saved per AAPCS64; clobbering on syscall return is legitimate. XNU
production precedent. Cost: ~1 BIC/ORR instruction on SPSR, piggybackable on
existing restore.

Typed kernel operations use negative-x0 (x0 < 0 = error code, x0 ≥ 0 =
success/return value). Typed operation return values are bounded non-negative
integers (cap-table slot indices, timestamps, zero-for-void). Negative values
are unambiguous. This is the Zircon convention.

The split (condition flag for IPC, x0 for typed) is consistent with D7 — the two
families already have different register semantics. Forcing uniform
condition-flag checking on typed operations would be gratuitous when x0 already
carries the return value.

**Cap-present indicator — sentinel u64::MAX:**

u64::MAX in x6 = no user cap present. u64::MAX in x7 = no reply cap present.
Cap-table slot indices are small non-negative integers; u64::MAX cannot be a
valid slot (D8 tables are bounded by typed-memory backing). Uniform for send and
receive sides. Self-contained in one register — no flags consumed.

**SVC number assignments:**

| SVC # | Operation |
| ----- | --------- |
| #1    | Send      |
| #2    | Receive   |
| #3    | Call      |
| #4    | ReplyRecv |
| #5    | Yield     |

Primitives before compounds, IPC before scheduling. Convention, not structural.

**Typed operation codes (x4) — grouped sequential:**

| Range | Type              | Operations                                                                                    |
| ----- | ----------------- | --------------------------------------------------------------------------------------------- |
| 0–6   | Observer          | resume, install_cap, write_registers, read_registers, suspend, change_handler, set_scheduling |
| 7–10  | Generic cap       | destroy, clone, close, mint                                                                   |
| 11–12 | Space             | split, merge                                                                                  |
| 13–14 | Field             | create, split                                                                                 |
| 15    | Time              | split                                                                                         |
| 16–17 | Pulsar            | create, clock_read                                                                            |
| 18    | Observer creation | create_observer                                                                               |
| 19    | Resource          | resource_request                                                                              |

Dense table dispatch. Type grouping is self-documenting; the kernel already
knows the target type from the cap in x5. Future rights mask additions append to
their respective type groups.

**Large return value convention — userspace buffer pointer:**

For operations exceeding the register budget (observer_read_registers,
observer_write_registers), the caller provides x0 = buffer pointer, x1 = buffer
length. The kernel validates the VA (D24 cap-mapping invariant) and reads/writes
using LDTR/STTR instructions (respecting ARMv8.1 PAN). No per-Observer
allocation. No well-known addresses (D26 preserved). Symmetric for read and
write operations.

**ReplyRecv register assignment:**

Entry: x0–x3 = reply data, x4 = reply label, x5 = receive field handle, x6 =
user cap for reply (u64::MAX if none), x7 = send-once reply cap handle (from
previous Receive's x7). Two targets (reply cap in x7, receive field in x5) use
two separate registers.

Does NOT settle: ~~IPC fast-path conditions~~ (settled by D50: scheduler
callback, 0-cap gate, Call + ReplyRecv scope), specific error code values (error
domain), buffer alignment and size constraints for large returns, ReplyRecv
send-side flags (whether x7 can carry flags in addition to the reply cap handle,
or whether flags are unnecessary for reply sends).

- **Rests on:** D47 (ABI framework — register convention and two-level numbering
  are the foundation all encoding details build on), D48 (25-operation
  enumeration — the set being encoded), D28 (fixed-size message — all 8
  registers used on receive, forcing the carry-flag error convention), D7 (split
  interaction model — justifies different error conventions per family), A2
  (ARM64 — SPSR_EL1/eret mechanism enables condition-flag signaling; PAN
  constrains user-memory access), D8 (flat cap table with bounded size —
  u64::MAX sentinel validity), D26 (capability-addressed memory — no well-known
  addresses, Observer knows VA bases; motivates buffer-pointer over
  kernel-allocated region), D24 (cap-mapping invariant — kernel can validate
  buffer VA), D13 (fast-path direct switch — carry-flag cost is ~1 cycle on
  ~400-cycle path), `design/research/syscall-abi.md` §5 (error convention survey
  — XNU carry-flag precedent, Zircon status-type precedent), §7 (large return
  value approaches).
- **Status:** settled — revisit if D47 is revised (register convention change
  alters the register budget), if D48 is revised (operation set change may
  require re-numbering), if D28 is revised (message size change may free a
  register for error status, removing the carry-flag motivation), or if PAN
  constraints on user-memory access prove problematic for the buffer-pointer
  convention.
- **Journal:** `journal/049-syscall-encoding.md`.

### D50 — IPC fast-path conditions: scheduler callback, 0-cap gate, Call + ReplyRecv scope

Six conditions, all of which must hold for the kernel to take the direct-switch
fast path on IPC:

1. **Operation is Call (SVC #3) or ReplyRecv (SVC #4).** The sender voluntarily
   blocks. Send, Receive, and Yield do not qualify. Send is a different
   interaction shape (fire-and-forget; sender continues per D13). If
   Send-to-waiting-receiver needs optimization, it is a separate "fast enqueue"
   mechanism, not direct switch.
2. **Same core.** Structural — O3 guarantees the SVC handler runs on the issuing
   core; the receiver must be on this core. Cross-core IPC goes through
   enqueue + IPI (O2).
3. **Target field has a waiting receiver.** An Observer is blocked on Receive on
   the target field (post-D45 routing resolution). D13's fundamental trigger.
4. **No user cap in message.** x6 = u64::MAX (D49 sentinel). Zero-cap detection
   is a single field check (D28 line 1359: "cheaply distinguishable"). Cap
   transfer goes through the general IPC path — rights validation, destination
   table allocation, ABA tag management are categorically more expensive than
   data-word pass-through (D47 x0–x3).
5. **Scheduler approves the switch.** The per-core scheduler's
   `should_switch_to(receiver)` callback returns true. The scheduler is the
   authority on "who runs next" regardless of code path. This is the D42 analog
   of seL4's "no higher-priority runnable" check, generalized to work with any
   D2 algorithm.
6. **Field routing resolved.** D45 routing evaluation completes. Unsplit fields:
   null table skip (~0 cost). Split fields: ~10–20 cycles within budget.

If any condition fails → slow path. The slow path can still direct-switch
through the general IPC code; it handles all cases uniformly at higher cost
(~600–800 vs. ~400 cycles).

D28's fixed-size message format eliminates the "message too large" condition
that seL4 must check — every message fits in registers by construction. D43's
"no kernel-side scheduling inheritance" eliminates profile manipulation from the
fast path. Run queues stay consistent per the Benno scheduling lesson (no lazy
scheduling).

The scheduler callback isolates an uncertain decision behind an interface
(philosophy: "isolate uncertain decisions behind interfaces"). D2 allows
different algorithms per core; the callback is algorithm-agnostic. Cost ~20–50
cycles (5–12% of ~400 budget). This is not extra work — A4 means the kernel must
choose which Observer to resume after every SVC; the callback unifies the
fast-path check with the slow-path scheduling decision.

The 0-cap gate makes D37's Time donation on Call() always slow-path. D37 chose
cap-graph visibility over seL4 MCS's zero-fastpath-overhead kernel-internal
approach; this is where that tradeoff materializes. The slow path still bypasses
the queue when the receiver is waiting — it uses the general code path, not the
optimized fast-path assembly.

Does NOT settle: scheduler callback interface (function signature, constant-time
requirement — depends on scheduler internals), ~~Send-to-waiting-receiver "fast
enqueue" optimization~~ (confirmed as implementation-only — no separate
mechanism; D13 + D16 precedent; ~15 cycle saving gated on profiling; see journal
055), ~~scheduler callback signature~~ (settled by D59: two traits — Scheduler
with 5 methods + Placement; NonNull\<Observer\> arguments; lock discipline from
D53), ~~interrupt masking during fast path~~ (settled by D69: DAIF.I masking for
full fast-path window).

- **Rests on:** D13 (queued fields with direct-switch fast path — establishes
  the mechanism and ~400-cycle target), D28 (fixed-size message — eliminates
  message-size check; "cheaply distinguishable" 0-cap gate), D47 (IPC-optimized
  registers — x0–x3 pass-through is the core fast-path optimization), D49
  (cap-present sentinel u64::MAX — the 0-cap gate mechanism), D42 (three-value
  scheduling profile — no single priority integer; motivates scheduler callback
  over hard-coded check), D43 (no scheduling inheritance — fast path does no
  profile manipulation), D2 (per-core schedulers with different algorithms —
  callback must be algorithm-agnostic), D1 (per-core hot path — same-core
  requirement; no cross-core shared state), D45 (field split — routing
  evaluation before direct-switch check), D48 (syscall enumeration — Call and
  ReplyRecv are the fast-path operations), D16 (reply via send-once —
  ReplyRecv's reply side), D37 (Time donation — explicitly slow-path; cap-graph
  tradeoff accepted), O3 (exceptions on causing core — same-core is structural),
  A4 (purely reactive — scheduler decision is part of every exception return),
  A5 (kernel is leaf node — scheduler authority preserved),
  `design/research/reply-cap-mechanism.md` §seL4 fast-path conditions (seL4
  disqualifying conditions), `design/landscape.md` §3.4 (fast-path data,
  cache-working-set principle), `design/research/syscall-landscape.md` §9.4
  (Benno scheduling — lazy scheduling abandoned).
- **Status:** settled — revisit if D13 is revised (different IPC model), if D42
  is revised (different scheduling model changes callback semantics), if D2 is
  revised (unified scheduler removes need for algorithm-agnostic callback), if
  D37 is revised (different Time donation mechanism changes cap-transfer
  tradeoff), or if the scheduler callback proves too expensive in practice
  (consistently >50 cycles, >12% of fast-path budget).
- **Journal:** `journal/050-ipc-fast-path-conditions.md`.

### D51 — Send-once right encoding: boolean flag on capability entry, not a rights bit

Send-once (D16) is a use-limited property of a capability: after one send
operation, the cap is consumed (removed from the holder's table). D16 settles
the mechanism but explicitly defers the encoding: "send-once right encoding in
D8's rights mask."

The encoding is a boolean flag (`send_once: bool`) on the capability entry,
separate from the rights bitmask. The flag is set at creation time and immutable
thereafter. The kernel checks the flag on Send; if true, the cap is removed from
the table after delivery.

Why a flag and not a rights bit: attenuation can only clear bits, never set
them. If send-once were a bit in the rights mask, a holder with "send +
send-once" could attenuate away the "send-once" bit, producing a plain "send"
cap — defeating the use-limited guarantee. A flag outside the rights mask is not
subject to attenuation. The flag is copied through attenuation (struct update
syntax copies all fields), preserving the send-once property regardless of
rights narrowing.

The flag adds 1 byte to the Entry struct (within padding on 64-bit alignment).
No measurable performance impact — the check is one branch on the send path,
correctly predicted as not-taken in the common case (most caps are not
send-once).

Does NOT settle: whether send-once should compose with other operations beyond
Send (e.g., send-once mint, send-once receive), or whether multi-use-limited
caps (send-N-times) are needed. These are not foreclosed — the boolean flag
could be widened to a u32 use count if demand emerges.

- **Rests on:** D16 (send-once mechanism — the property being encoded), D8 (cap
  table entry layout — the structure being extended), D4 (attenuation hierarchy
  — attenuation must not defeat send-once; motivates flag over bit).
- **Status:** settled — revisit if D4's attenuation model changes (e.g., to
  support "additive" attenuation that could set bits), or if multi-use caps are
  needed (widen flag to counter).

### D52 — Per-type rights masks: complete assignment for Space, Time, Field, Pulsar

D39 settles the nine Observer rights. D48 enumerates all typed operations. This
derivation settles the complete rights mask for the remaining four types, drawn
from D48's operation table and earlier derivations.

**Space rights (4 bits):** split (D41), merge (D41), destroy (D11/D33), clone
(D23). Both split and merge require dedicated rights (D41: "require dedicated
rights in the Space rights mask"). Destroy and clone follow the cross-type
pattern. No additional rights — Space has no read/write operations (memory
access is through the MMU, governed by the page table, not by per-operation
rights checks).

**Time rights (2 bits):** split (D38), destroy (D11/D33). No clone — D38 settles
Time as non-clonable (linear). No merge — D48 excludes time_merge (D30's
additive aggregate makes holding multiple Time caps equivalent to merging).
Delegation uses split, not clone.

**Field rights (6 bits):** send (D15), receive (D15), mint (D17), split (D45),
destroy (D11/D33), clone (D23). Send and receive are the fundamental IPC
operations. Mint controls badge assignment. Split (D45) enables routing table
construction. No separate "create" right — field creation operates on a Space
cap (D32 type conversion), not on an existing Field.

**Pulsar rights (2 bits):** destroy (D11/D33), clone (D23). No modify or rearm
right — Pulsars are configured at creation and managed by the kernel thereafter
(D44: "kernel programs the timer and delivers messages"). clock_read is capless
(D48: no cap argument). create_pulsar operates on a Space cap (D32), not on an
existing Pulsar.

Shared rights occupy the same bit positions across all types: DESTROY (bit 1),
CLONE (bit 4), SPLIT (bit 12). Type-specific rights occupy non-overlapping
positions. This is the complete assignment — no reserved bits are needed beyond
the 14 currently allocated (bits 0–13 of the u16 Rights value).

Does NOT settle: COW/snapshot rights for Space (D9 deferred), revocation right
(D67 settles generation counters; right encoding deferred), interrupt delivery
rights (D22 — interrupts flow through Fields, no separate rights).

- **Rests on:** D39 (Observer rights — establishes the 9-right pattern), D48
  (operation enumeration — every typed operation implies a right), D41 (Space
  merge and split — "require dedicated rights"), D38 (Time non-clonable —
  excludes CLONE from Time), D45 (Field split — implies SPLIT right), D17 (mint
  as third Field right), D23 (clone as per-type right — present for Space,
  Field, Pulsar; excluded for Time per D38), D11/D33 (destroy as universal
  right), D4/D8 (rights mask as the enforcement mechanism), D44 (Pulsar
  create/cancel — cancel is destroy).
- **Status:** settled — revisit if D48 is extended with new typed operations
  that imply new rights, if D9's COW/snapshot is settled (adds Space rights), or
  if D22's interrupt delivery settles rights beyond the existing Field
  send/receive.

### D53 — Arena lock ordering: Field before Observer

Under the global-arena concurrency model (one SpinLock per Arena<T>, five arenas
total), any operation that accesses objects of two different types must acquire
both arena locks. Deadlock freedom requires a total ordering on lock
acquisition.

The ordering is: **Arena<Field> before Arena<Observer>**. This follows from the
IPC data flow — the most common cross-type operation:

1. Sender resolves its own cap table entry (sender's Observer, via CoreLocal —
   no arena lock needed for the running Observer's hot fields).
2. Lock Arena<Field>: access the target Field to check for waiters or enqueue.
3. Lock Arena<Observer>: if a waiter exists, modify its state (Blocked →
   Runnable), clear its wait_target, update run queue linkage.
4. Release Arena<Observer>, then release Arena<Field>.

The ordering ensures that no IPC path acquires Observer before Field. Cross-core
IPC follows the same order: the IPI handler on the receiver's core acquires
Field then Observer in the same order.

Other cross-type operations:

- **Object creation** (create_field, create_observer, create_pulsar): acquires
  only the target type's arena. No ordering concern.
- **Destroy cascade** (D33): iterates one Observer's cap table, closing caps to
  various types. Acquires one arena at a time per close operation (never holds
  two simultaneously during iteration). No ordering concern.
- **Fault handling** (D40): acquires Arena<Observer> to inspect/modify the
  faulting Observer. May acquire Arena<Field> to enqueue a fault message. This
  requires Field-before-Observer ordering, which is satisfied by releasing
  Observer, acquiring Field, then re-acquiring Observer if needed. The fault
  path is cold (D1) — the release-reacquire cost is acceptable.

The ordering extends to future arenas: Arena<Space> and Arena<Time> are accessed
independently (Space/Time operations do not cross into Field or Observer arenas
during normal operation). Arena<Pulsar> crosses into Arena<Field> (timer fire
enqueues a message), so the extended ordering is: Field < Observer < Pulsar
(Pulsar acquires Field, never Observer). Space and Time are unordered with
respect to the others (no cross-arena operations).

Does NOT settle: per-core sharding of arenas (future SMP optimization), lock
ordering for operations that touch three or more arenas simultaneously (not
currently possible given the operation set).

- **Rests on:** D1 (hot/cold split — same-core IPC is uncontended; cross-core is
  cold), D13 (IPC data flow — send checks Field then wakes Observer; determines
  the natural ordering), D50 (fast-path conditions — same-core requirement means
  the fast path avoids contention entirely), D33 (destroy cascade — preemptible,
  one arena at a time), D44 (Pulsar timer fire — enqueues into Field, extending
  the ordering to Pulsar), A1 (Rust — SpinLock<T> prevents data races; the
  ordering prevents deadlocks).
- **Status:** settled — revisit if arena sharding is implemented (per-core
  arenas change the locking model), if a new operation requires holding three or
  more arena locks simultaneously, or if the fault-path release-reacquire cost
  proves too high (would motivate a different locking strategy).

### D54 — Routing table structure: nullable pointer to external sorted array

The per-Field routing table (D45 badge-range → destination-Field mappings) is a
nullable pointer to an externally-allocated sorted array of routing entries.
Null when unsplit — zero hot-path cost (null check in the cache line already
loaded for `waiters`/`queue_len`). On first split, the kernel allocates the
array from root Space (D31).

Each routing entry holds: badge range condition, destination Field ObjectId, and
intrusive-list linkage for the destination's back-pointer cleanup list.
Destination Fields gain a back-pointer list head (intrusive list paralleling
waiters). When a destination is destroyed, the kernel walks its back-pointer
list and removes each corresponding routing entry from the source Field's table.
O(1) per source.

Growth via geometric doubling on the split path (cold, amortized O(1) per
split). The array is contiguous for binary-search cache-friendliness.

Memory accounting: root Space (D31). Each split adds ~40–48 bytes — bounded per
operation, small, invisible to userspace. This extends D32's metadata pattern
from "bounded per object" to "bounded per operation" — a new category
(kernel-internal variable-size infrastructure) that also covers the D22
IRQ→Field routing table.

Rejected alternatives: small inline array + overflow pointer (64–192 bytes arena
bloat on all Fields for ~10-cycle savings on a minority; two code paths; inline
count is a global commitment); routing entries in queue pages (liveness coupling
— full queue blocks split; type safety violation on dual-typed pages; routing
and queue capacity cannot scale independently).

Does NOT settle: exact routing entry layout, initial array capacity, sub-page
allocation strategy for routing arrays, flattened routing table structure
(D24-parallel optimization).

- **Rests on:** D45 (badge-range routing — this derivation settles one of D45's
  open items: routing table structure), D32 (type conversion / memory accounting
  — routing table memory comes from root Space, extending the metadata pattern),
  D31 (root Space — pays for routing table allocations), D1 (hot-path — null
  check in hot cache line; pointer dereference only on split Fields), D50
  (routing evaluation is a fixed cost on every send — the presence check must be
  in the hot partition), D33 (destroy cascade — destination destroy must find
  and remove routing rules on source Fields; back-pointer list enables
  O(1)-per-source cleanup), D43 (Observer minimum schema — precedent for
  nullable/optional structures: inline common case, allocated uncommon case),
  D15 (Field as single object — routing table expands the Field's internal
  structure while preserving the single-object model), A1 (Rust — Option pointer
  for null safety; intrusive list for back-pointers follows established kernel
  pattern).
- **Status:** settled — revisit if D45 is revised (changes the routing
  mechanism), if D32 is revised (changes the memory accounting model), if D1 is
  revised (changes the hot-path constraint), or if profiling reveals the pointer
  dereference cost is unacceptable for a structurally required workload pattern.
- **Journal:** `journal/051-routing-table-structure.md`.

### D55 — Field destroy routing-cleanup protocol: preemptible walk, generation check, IPI-requested removal

When a destination Field is destroyed, the kernel walks its back-pointer list
(D54) to remove routing entries from source Fields. The protocol has three
components:

**Preemptible walk (D33 extension).** The back-pointer walk is O(K) where K =
number of sources routing to this destination. D33's structural argument applies
identically: "inline forecloses bounded destroy time; preemptible forecloses
nothing." The walk proceeds in bounded steps; between steps, the timer can
preempt. Continuation state extends D33's per-core framework (back-pointer list
position plus pending cross-core IPIs).

**Generation check for stale-rule detection.** D11 requires the object to be
dead before cleanup begins — creating a window where source Fields have routing
rules pointing to a dead destination. Each routing entry stores the
destination's ObjectId generation at installation time. On routing evaluation,
the kernel compares generations; a mismatch means the destination is dead (or
reused) — the entry is treated as absent and the send falls back to the source
queue (D45 fallback). This extends D11's ABA tag pattern from userspace
capability slots to kernel-internal routing references. Cost: one comparison for
the matching entry per send, in the same cache line as the badge range —
effectively free (branch predictor learns the always-taken path).

**IPI-requested removal for cross-core sources.** D1 requires no shared mutable
state on the hot path. The source Field's routing table is hot-path data (D50).
When the destroying core needs to remove a routing entry from a source on
another core, it sends an IPI (O2). The IPI handler on the target core performs
the removal in its own execution context — no lock on the send path, no
concurrent modification. During IPI delay, the generation check (above) handles
stale entries transparently.

Same-core sources are cleaned inline within the syscall context (no IPI needed —
no concurrent access possible within one core's exception handler).

Rejected alternatives: inline walk (forecloses bounded destroy time,
inconsistent with D33), liveness check via pointer dereference (cache miss vs.
same-cache-line generation comparison), fail-the-send on stale entry (no
precedent for transient userspace-visible errors during kernel-internal
cleanup), lock on source routing table (D1 hot-path violation), deferred-on-send
removal (polling, not reactive — but noted as a viable Verus verification
stepping stone before the IPI protocol is formally specified).

Does NOT settle: generation field placement in routing entry layout, IPI
batching for multi-source cleanup on the same remote core, IPI acknowledgment
protocol, flattened routing table invalidation (gated on flattened tables being
adopted), continuation state layout (extends D33).

- **Rests on:** D54 (back-pointer intrusive list — the mechanism this protocol
  operates), D33 (preemptible destroy cascade — structural argument transfers;
  continuation framework extended), D11 (dead-before-cleanup guarantee creates
  the stale window; ABA tag pattern extended to routing entries), D45 (fallback-
  on-destroy — the semantic that stale-rule detection implements), D50 (routing
  evaluation on every send — source routing table is hot-path data; generation
  check must be cache-friendly), D1 (per-core hot path — no shared mutable
  state; IPI-requested removal avoids cross-core writes to hot-path data), O2
  (cross- core coordination requires IPIs), A3 (generic/RT — bounded preemption
  latency requires preemptible cleanup), A4 (purely reactive — IPI is reactive;
  no background cleanup), `design/philosophy.md` ("find the abstraction that
  absorbs the edge cases" — generation check unifies dead-handle and
  dead-routing- destination handling; "react to reality, don't poll for it" —
  IPI over deferred-on-send).
- **Status:** settled — revisit if D54 is revised (changes the back-pointer
  mechanism), if D33 is revised (changes the preemptibility framework), if D11
  is revised (changes the dead-before-cleanup guarantee or ABA tag pattern), if
  O2 is revised (changes the cross-core coordination mechanism), or if Verus
  verification reveals the IPI protocol is infeasible to specify (would motivate
  deferred-on-send as the permanent solution).
- **Journal:** `journal/052-field-destroy-routing-cleanup.md`.

### D56 — Cross-core logic: scored placement, steal-then-idle, push+pull rebalance

The kernel-internal mechanisms for Observer placement, cross-core wake, core
idle management, and rebalancing. Settles the protocols that implement D43's
"fresh placement decision each wake-up" across multiple cores.

**Per-core run queues.** Each core owns a local run queue. The scheduler pick
(D1 hot path) reads only local state. Migration between queues is cold-path.
Global run queue foreclosed by D1 (shared mutable state on hot path).

**Scored placement.** On every runnable transition, the placement function
scores candidate cores using: idle status (atomic bitmap, boot-sized array of
AtomicU64), queue depth (atomic per-core counter), profile compatibility
(Observer's R/T/P matched against core's current scheduler algorithm per D2),
capacity factor (D36), and cache affinity with decay (~1–5ms half-life). The
function returns "local" (hot path, no IPI) or "remote(core_id)" (cold path,
mailbox + IPI). The scoring function is behind a trait — weights are tunable,
and the implementation is a leaf node swappable without affecting the rest of
the kernel.

**Cross-core wake protocol.** When placement returns "remote": (1) causing core
writes Observer reference to per-core mailbox for target, (2) causing core sends
SGI, (3) target core's IPI handler reads mailbox, (4) handler acquires arena
locks (D53 ordering: Field < Observer), (5) handler enqueues Observer on local
run queue. The causing core does not touch the target's run queue directly.

**Idle entry: steal-then-idle.** When a core's last runnable Observer blocks:
scan other cores' queue depths (atomic reads). If any core has queue depth > 1,
steal its lowest-affinity runnable Observer (D53 arena locks). If no work found,
set idle bit and enter WFI (or CPU_SUSPEND per D46 platform policy).

**Rebalancing: push + pull.** Push on timer tick: each core's preemption timer
handler checks local queue depth against fair-share target, pushes excess
Observers to least-loaded core via mailbox + IPI. Pull on idle entry:
steal-then-idle as above. Both are A4-consistent (hardware exception handlers).

**Dynamic core-type classification.** Scheduler algorithm assignment per core
adapts to workload — the kernel reclassifies based on the (R, T, P) profiles of
Observers currently on each core. Reclassification rebuilds algorithm-specific
state from abstract properties (D2). Eliminates wasted capacity from static
classification.

**Cache affinity: per-core tracker with decay.** Each per-core scheduler
maintains a tracker (ring buffer of recent Observer IDs with timestamps).
Affinity weight decays over ~1–5ms (L2 cache lifetime). D43's "no core ID on
Observer" preserved — affinity state lives in per-core structures. Affinity is a
tiebreaker, not a binding constraint.

**Boot-sized per-core arrays.** All per-core data structures (idle bitmap, queue
depths, algorithm tags, capacity factors, affinity trackers) are sized at boot
based on discovered core count. A3 forbids compile-time core count limits.

Does NOT settle: scoring weights (tuning parameter), mailbox structure
(implementation detail), reclassification thresholds (tuning parameter),
affinity decay curve, work stealing synchronization mechanism (lock-based vs.
lock-free), NUMA awareness (deferred until NUMA hardware is tested), IPC
locality tracking (would address D50 fast-path locality tension — requires its
own derivation if needed), admission control on heterogeneous migration (D2
journal 002 open sub-question — depends on specific scheduler algorithms).

- **Rests on:** D1 (hot/cold split — per-core run queues forced; cold-path
  migration; "shared cold-path reads cheap on cache-coherent ARM64" enables
  scored placement), D2 (per-core algorithm heterogeneity — creates the matching
  problem that scored placement solves; trait boundary for scheduler enables
  dynamic reclassification), D42 (three-value profile — provides placement
  signal without core identity; R/T/P → core-type preference is
  kernel-internal), D43 (transient core assignment — "fresh placement decision
  each wake-up" is the requirement this derivation implements; "cache affinity
  is a per-core scheduler hint" constrains affinity tracking), D46 (core
  lifecycle — idle via WFI, wake via IPI; boot-time activation; deactivation
  conservation check), D50 (cross-core IPC = enqueue + IPI — establishes the
  slow-path pattern; same-core fast-path creates a tension with scored
  placement), D53 (arena lock ordering — governs cross-core IPI handler lock
  acquisition and work stealing), D36 (normalized compute units — fungibility
  makes migration costless at cap level; capacity factors feed the scoring
  function), A4 (purely reactive — forecloses background rebalancer; constrains
  to four piggy-back triggers), A3 (generic — forbids compile-time core count
  limits; boot-sized arrays), A5 (leaf node — placement complexity
  kernel-internal; Observers don't see cores), A2 (ARM64 — cache coherency, SGI
  for IPI, WFI/CPU_SUSPEND, big.LITTLE motivates heterogeneous placement), A1
  (Rust — trait boundaries, AtomicU64, ownership model for per-core vs shared),
  O2 (IPIs for cross-core coordination — the mechanism), O3 (exceptions on
  causing core — placement runs on the causing core), `design/research/smp.md`
  (synchronization models, IPI latency, "Wasted Cores" failure modes,
  heterogeneous scheduling), `design/landscape.md` §4.3 (multicore scheduling —
  per-core queues universal, work stealing secondary to affinity), §4.7
  (energy-aware scheduling — EAS maps to scoring function pattern).
- **Landscape divergence:** No surveyed system combines per-core algorithm
  heterogeneity, scored placement with profile matching, dynamic
  reclassification, and reactive-only rebalancing. Linux EAS is closest (scored
  placement with energy model) but assumes single algorithm. Novel position
  arises from D2 × D43 × A4 intersection.
- **Status:** settled — revisit if D1 is revised (changes hot/cold split), if D2
  is revised (unified scheduler eliminates matching), if D43 is revised (static
  binding replaces placement), if D50 is revised (changes fast-path locality
  tradeoff), if scoring overhead proves consistently >500 cycles (>60% of slow-
  path budget — would motivate fallback to two-tier), or if cache-line bouncing
  on per-core queue depth proves measurably worse than less-frequent cross-core
  reads (would motivate Pragmatic variant).
- **Journal:** `journal/053-cross-core-logic.md`.

### D57 — Observer schema downstream: budget encoding, default profile, self-reference cap

Settles three deferred items from D43 (Observer minimum schema) and D42
(scheduling profile).

**Budget encoding: store two, derive the third.** The Observer stores
responsiveness (R) and throughput (T) as `u8` fields. Precision is derived:
`P = 128 - R - T`. Budget is 128 (power of 2 — scheduler math uses shift, not
division). Validation: `R + T <= 128`. Storing two values eliminates the
three-way sum invariant by construction — invalid states are unrepresentable (A1
applied to data representation).

**Default profile: R = 43, T = 43, P = 42.** Closest equal distribution on
budget 128. Serves A3 (no workload type favored) and A5 (zero configuration for
reasonable behavior).

**Self-reference cap: kernel-installed at reserved slot 2.** D4 + D7 + D8 rule
out magic self-handles — an Observer must hold a real cap to itself. The kernel
installs it at creation, following the D35/D21 pattern (fault handler at slot 0,
reply field at slot 1). Full rights mask — the Observer can attenuate and
delegate. Eliminates the "forgot to install self-cap" bug class (A5).

Reserved cap-table slots: 0 = fault handler (D21), 1 = reply field (D43), 2 =
self-cap (D57). User slots start at index 3.

Does NOT settle: self-cap rights attenuation conventions (userspace policy).

- **Rests on:** D42 (three-value profile — budget size and encoding were
  deferred), D43 (Observer minimum schema — self-reference caps, budget
  encoding, default profile were deferred), D8 (flat cap table — self-reference
  requires a real slot), D4 (designation = authority — no magic handles), D7
  (only capabilities designate), D35/D21 (kernel-installed reserved slots
  pattern), D39 (modify-scheduling right — self-cap enables self-directed
  scheduling changes), A1 (store-two-derive-third exploits Rust's type system to
  eliminate runtime invariant), A3 (equal default serves all workload types), A5
  (kernel absorbs self-cap installation and budget rounding).
- **Status:** settled — revisit if D42 is revised (changes the profile model),
  if D43 is revised (changes the metadata struct), if D8 is revised (changes cap
  table structure), or if a downstream derivation reveals that 128 is
  insufficient resolution for a structurally required scheduling distinction.
- **Journal:** `journal/054-observer-schema-downstream.md`.

### D58 — Badge size: u64

Badge is a 64-bit (u64) value in the cap-table entry and in the delivered
message. Forced by the ABI: D47/D49 place badge in x5, a 64-bit ARM64 register.
The cap-table entry stores what the ABI delivers — no masking, no
zero-extension, no conventions about which bits are meaningful. The minter
provides a u64 at clone time; the receiver reads a u64 in x5.

Prior art unanimous: every 64-bit capability system (seL4/64, L4/Fiasco.OC,
Zircon) uses the full machine word. No benefit from narrowing — no entry-size
budget, no value-space ceiling, immeasurable fast-path cost difference.

Does NOT settle: whether badge value zero is reserved as "unbadged" (downstream
convention question — should be settled alongside M10).

- **Rests on:** D47 (IPC-optimized register convention — badge in x5; primary
  forcing constraint), D49 (confirmed register assignments), A2 (ARM64 — 64-bit
  registers), D8 (cap-table entry — badge is a per-entry field; entry stores
  what ABI delivers), D17 (badge is minter-assigned — minter must express
  arbitrary u64 values).
- **Status:** settled — revisit only if D47 is revised (different ABI register
  layout).
- **Journal:** `journal/055-badge-size.md`.

### D59 — Scheduler callback signature: two traits, five methods

The kernel-to-scheduler interface decomposes into two traits: **Scheduler**
(per-core, hot-path) and **Placement** (cross-core scoring). They must be
separate because D1 (per-core hot path) conflicts with D56's placement function
reading cross-core state. Separation makes the boundary a type-level guarantee.

**Scheduler trait — five methods:** `enqueue` (Observer joins run queue; under
Arena\<Observer\> lock), `dequeue` (Observer leaves; under lock), `pick_next`
(select next; no locks), `should_switch_to` (D50 fast-path predicate; read-only,
no locks, ≤50 cycle budget), `on_preempt` (timer tick accounting; no locks). All
take `NonNull<Observer>` — arena lookup by ID exceeds the fast-path budget.

**Placement trait:**
`fn place(&self, observer: &Observer, snapshot: &CoreSnapshot) -> PlacementDecision`
returning Local or Remote(CoreId). CoreSnapshot populated once before scoring to
avoid cache-line bouncing. One instance per system, not per-core.

**Lock discipline:** enqueue/dequeue called while holding Arena\<Observer\> (D53
ordering). pick_next, should_switch_to, on_preempt called without arena locks.

Does NOT settle: internal run queue structure, affinity decay curve, scoring
weights, reclassification thresholds, CoreSnapshot layout, admission control
failure handling, work stealing synchronization mechanism.

- **Rests on:** D2 (per-core algorithm heterogeneity — trait must be
  algorithm-agnostic), D50 (fast-path predicate — forces should_switch_to as
  read-only ≤50 cycle method; Benno lesson forces enqueue/dequeue consistency),
  D56 (cross-core placement — forces separate Placement trait; CoreSnapshot
  pattern), D53 (arena lock ordering — governs which methods run under locks),
  D43 (no core ID on Observer — placement is kernel-internal), D46 (idle entry —
  pick_next returns None signals WFI), D1 (per-core hot path — Scheduler trait
  touches only local state), A1 (Rust traits — algorithm families implement
  Scheduler; NonNull for arena-allocated pointers), A4 (reactive — scheduler
  decisions happen inside exception handlers).
- **Status:** settled — revisit if D50 is revised (changes fast-path predicate
  semantics), if D2 is revised (unified scheduler removes need for
  algorithm-agnostic trait), if D56 is revised (changes placement model), or if
  should_switch_to proves consistently >50 cycles (would motivate inlining the
  check instead of a trait call).
- **Journal:** `journal/056-scheduler-callback-signature.md`.

### D60 — Space byte-addressing: byte inputs, kernel rounds

Space operations accept byte-count inputs. The kernel rounds up to PAGE_SIZE
internally. `space_split(cap, size) → new_cap`: `size` is a byte count; the
returned Space has `round_up(size, PAGE_SIZE)` bytes. `size = 0` is an error.

Forced by A5 (kernel absorbs the alignment computation — pushing page-size
rounding to every userspace caller is accidental complexity), D25 (page size is
exposed and queryable — observability satisfied without mandatory per-call
alignment), D26 (capability-addressed memory eliminates the map() scenario where
byte-addressing was risky), D9 (rejected seL4's page-addressed model on A5
grounds — closer comparators Zircon/Genode/Mach all use byte inputs).

D41's "operate at page granularity" describes the kernel's internal action
quantum, not the API unit. D25's risk note (implicit rounding re-hides page
size) does not apply at the split interface because D26 eliminates the adjacent-
mapping scenario that motivated the concern.

Does NOT settle: how the actual rounded size is communicated back to the caller
(second return register vs. query syscall), subtree overhead visibility, size =
0 error code, merge interaction (unaffected — merge is all-or-nothing).

- **Rests on:** A5 (kernel absorbs rounding — the primary forcing axiom), D25
  (page size exposed — observability requirement satisfied by queryable
  PAGE_SIZE), D26 (capability-addressed memory — eliminates the map() scenario),
  D9 (variable-size kernel-managed Spaces — rejected seL4's model), D41 (Space
  merge/split — "page granularity" is internal quantum), A2 (ARM64 multi-granule
  4K/16K/64K — byte inputs more portable), A3 (generic — byte inputs impose no
  granularity assumption), O4 (essential complexity — rounding cannot be
  eliminated, only moved; A5 moves it to the kernel).
- **Status:** settled — revisit if D25 is revised (changes page-size exposure
  model), if D26 is revised (reintroduces explicit map() where byte-addressing
  creates sub-page packing risk), or if D9 is revised (moves toward
  userspace-managed memory).
- **Journal:** `journal/057-space-byte-addressing.md`.

### D61 — Fault message content and delivery mechanism

Fault delivery is standard queued-Field IPC with the kernel as sender. No
separate mechanism. Four fault types:

| Type               | data[0]                        | data[1]     | data[2]                   | data[3] |
| ------------------ | ------------------------------ | ----------- | ------------------------- | ------- |
| VM_FAULT           | Space slot index               | byte offset | access type (0=R,1=W,2=X) | 0       |
| RESOURCE_REQUEST   | resource type (0=Space,1=Time) | quantity    | 0                         | 0       |
| CAP_TABLE_FULL     | 0                              | 0           | 0                         | 0       |
| HARDWARE_EXCEPTION | ESR_EL1                        | ELR_EL1     | FAR_EL1                   | 0       |

All carry: badge from D21 handler cap, fault-type label, Observer handle cap
with 5 of 9 rights (resume + destroy + install_cap + write_registers +
read_registers). Faulted is a distinct Observer state (D39), descheduled, reuses
D43 wait-state linkage for D18 pending list. D50's 0-cap fast-path gate does not
apply (fault messages always carry a cap).

VM_FAULT diverges from all prior art: Space slot index + byte offset instead of
raw VA (D26 makes VA kernel-internal).

Does NOT settle: hardware exception label taxonomy (one vs many), debug fault
delivery, ~~pager unavailability (G04)~~ (settled by D68), label numeric values,
lazy vs eager PTE population policy.

- **Rests on:** D12 (fault delegation via IPC), D13 (queued fields — delivery
  mechanism), D18 (deferred delivery — pending list), D20/D21 (fault handler at
  reserved slot 0), D26 (capability-addressed memory — forces slot+offset
  instead of VA), D28 (fixed-size message format), D39 (faulted state + 9
  rights), D40 (fault resolution actions — determines required rights), D43
  (wait-state linkage reuse), D7 (split model — faults are IPC, resolution is
  typed ops), A2 (ARM64 — ESR_EL1, FAR_EL1).
- **Status:** settled — revisit if D26 is revised (changes VA exposure model,
  would change VM_FAULT content), if D39 is revised (changes rights model), if
  D28 is revised (changes message format).
- **Journal:** `journal/058-fault-messages.md`.

### D62 — Pulsar creation API: single-call, armed-at-creation

`create_pulsar(space_cap, field_cap, badge, duration, period) → pulsar_cap`.
Armed on creation. No separate arm, configure, or modify call. Cancel =
`destroy(pulsar_cap)`. Forced by D44 ("armed on creation"), D48 (no `arm_pulsar`
in syscall table), D52 (no modify/rearm right — 2-bit mask).

D35's composable pattern does not apply: Pulsars have no structural gap
requiring an inert state (unlike Observers, which need code Space caps installed
post-creation). D35's independent-utility test rejects `arm_pulsar` (would exist
solely to serve creation). Modify = destroy + create. One-shot loop is the
manual-control escape hatch.

D53 carve-out: `create_pulsar` briefly increments the delivery Field's refcount
(cross-arena write, safe but should be documented as an exception to D53's
"creation acquires only target arena" claim).

Does NOT settle: ~~deadline parameter form~~ (settled by D72: relative duration
in nanoseconds), minimum Space size for Pulsar creation, Field refcount
acquisition protocol details.

- **Rests on:** D44 (Pulsar semantics — "armed on creation"), D48 (syscall table
  — no arm operation), D52 (rights mask — no modify/rearm right), D32 (type
  conversion — space_cap consumed), D35 (composable creation — does NOT apply;
  no structural gap), D17 (badge immutable), D13/D45 (Field send right), D53
  (arena lock ordering — carve-out for Field refcount).
- **Status:** settled — revisit only if D44 is revised (changes Pulsar
  semantics) or D52 is revised (adds modify right).
- **Journal:** `journal/059-pulsar-creation-api.md`.

### D63 — Pulsar message content layout

When a Pulsar fires: badge (minter-assigned ID), label (LABEL_TIMER_FIRE),
data[0] (actual fire time: CNTVCT_EL0 at interrupt entry), data[1] (overrun
count: 0 for normal, N for missed periods), data[2..3] (reserved zero), cap
(empty — ack-to-re-arm rejected by D44), reply_cap (absent — kernel deposit).

Fire time in raw CNTVCT_EL0 ticks (not nanoseconds): cheaper at interrupt time,
directly comparable to Observer counter reads. Overrun always present (D28 fixed
format). Empty cap slot satisfies D50 fast-path 0-cap condition. No surveyed
system includes a firing timestamp — D44 deliberately departs from consensus.

Does NOT settle: LABEL_TIMER_FIRE numeric value, data[2] disposition (reserved
zero vs scheduled deadline — medium confidence), one-shot field-full behavior.

- **Rests on:** D28 (fixed-size format), D17 (badge injection), D44 (fire time +
  overrun count + ack-to-re-arm rejection), D16 (no reply cap), D50 (0-cap
  fast-path eligibility), A2 (ARM64 — CNTVCT_EL0), A5 (kernel manages re-arm).
- **Status:** settled — revisit if D44 is revised (changes delivery content) or
  D28 is revised (changes message format).
- **Journal:** `journal/060-pulsar-message-layout.md`.

### D64 — Badge-closure message format

When the last send cap with badge B to a tracked Field is closed: badge (B),
label (LABEL_CLOSURE), data[0..3] (all zero), cap (absent), reply_cap (absent).

Badge identifies which client disconnected (D17). Label distinguishes
kernel-synthesized closure from user messages (D4). Data words are zero: the
reaction (free per-badge state) is the same regardless of closure reason; badge
assignment discipline (D17 minter-assigned) lets servers self-encode capability
types in badge ranges, making reason codes unnecessary (fails A5 test).

Closest to QNX disconnect pulses. Better than Mach (no per-right registration).
Unlike Zircon (many-to-many Fields vs 1:1 peer-closed).

Does NOT settle: LABEL_CLOSURE numeric value, routing interaction (does closure
notification for a badge in a routed range follow D45 routing or bypass it?),
~~T1 kernel detection mechanism (consumed-by-use vs closed-without-use)~~
(settled by D73: structural code-path separation).

- **Rests on:** D17 (badge-closure concept + minter-assigned), D28 (fixed-size
  format), D4 (distinguishability), D13 (delivery via Field queue), D18
  (overflow — dropped if full, not a correctness issue), D29 (journal 029:
  "badge-closure notifications, 1 word at most").
- **Status:** settled — revisit if D17 is revised (changes badge-closure
  semantics) or D28 is revised (changes message format).
- **Journal:** `journal/061-badge-closure-format.md`.

### D65 — Send-once reply badge: caller-supplied

Call() takes a `reply_badge` parameter. The kernel embeds the caller-provided
value into the send-once cap entry. When the server replies, the message arrives
at the caller's reply field carrying that badge, allowing the caller to identify
which outstanding RPC is being answered.

Forced by D16 (single pre-allocated reply field per Observer — multiple
concurrent Calls share it, badge-discrimination required per journal 019) + D17
(receiver-controls-badge principle — the caller IS the receiver of its own reply
field). Kernel-auto-assigned explicitly foreclosed by D17 ("opaque values...
translation layer"). Fixed sentinel foreclosed by journal 019's multi-RPC
pattern.

Request badges (caller→server namespace) and reply badges (server→caller
namespace) are independent — no correlation.

Downstream: Call()'s syscall encoding (D49) needs a `reply_badge` register
parameter.

Does NOT settle: reply_badge register assignment in D49, zero-badge reservation
policy (connects to D58's adjacent question).

- **Rests on:** D16 (single reply field, send-once reply caps), D17
  (receiver-controls-badge, kernel-auto-assigned rejected), D14 (Call = Send +
  Receive), D28 (reply cap in message), journal 019 (multi-wait resolution:
  badge-distinguished replies).
- **Status:** settled — revisit only if D16 is revised (changes reply field
  model) or D17 is revised (changes badge assignment principle).
- **Journal:** `journal/062-send-once-reply-badge.md`.

### D66 — Clock access mechanism: per-Observer CNTKCTL_EL1.EL0VCTEN

The mechanism for per-Observer clock access authority. D43 gains a ninth field:
`clock_access: bool`. The kernel writes CNTKCTL_EL1.EL0VCTEN on every context
switch based on this flag. EL0PCTEN statically denied (physical counter leaks
host timing in hypervisor contexts; A3). EVNTEN irrelevant. Per-Observer
granularity required (A3 forecloses static grant or deny). `clock_read()` (D48)
remains as capless fallback for Observers without direct access.

Does NOT settle: authority mechanism (how the flag is set/changed — graft onto
modify-scheduling right vs new 10th right vs creation parameter), default policy
(grant by default vs deny by default). These are genuine choices. D72 settles
G09 as relative duration, decoupling the authority choices from the timer API
form — either default policy works with relative durations.

- **Rests on:** D44 (CNTKCTL_EL1 per-Observer — primary source), D43 (Observer
  metadata struct — gains clock_access field), D48 (clock_read syscall — capless
  fallback), A2 (ARM64 generic timer — CNTKCTL_EL1 register), A3 (generic —
  forecloses static policy; per-Observer required), A5 (clock_read absorbs
  complexity for access-denied Observers).
- **Status:** mechanism settled, authority model open — revisit if D44 is
  revised (changes timer mechanism) or D43 is revised (changes metadata struct).
  Authority choices can now be settled independently (G09 settled by D72).
- **Journal:** `journal/063-clock-access-mechanism.md`.

### D67 — Revocation add-on: universal generation counters

Every kernel object carries a `generation: AtomicU64` counter. Every capability
table entry stores the generation value at time of creation or clone. On
explicit revocation: the object's counter is atomically incremented — O(1). On
capability use: the entry's stored generation is compared against the object's
live generation; mismatch means the cap is stale and the operation returns an
error. Stale slots are lazily rewritten to Null on next access (Coyotos
lazy-rewrite pattern), maintaining A4 compliance.

Universal: applies to all five kernel object types (Space, Observer, Field,
Time, Pulsar) uniformly. Scoped application (generation counters on non-IPC
types only) was rejected: it bifurcates the revocation API by object type (two
mechanisms for the same semantic operation — tension with D4's uniform
capability semantics), forecloses "invalidate Field caps while preserving queued
state" (field rotation destroys the object), and optimizes for a per-use cost
that is likely zero (the generation field shares a cache line with fields
already loaded on the syscall path; the branch predictor correctly predicts
"match" in the common case).

CDT (Capability Derivation Tree) is not adopted. The gaps CDT would address —
per-client selective revocation and transitive delegation tracking — are handled
through existing primitives: field-per-client for IPC (D19 multi-field wait),
bump-and-reissue for non-IPC, userspace conventions for delegation depth. CDT
adds a separate kernel structure (intrusive linkage, 16 bytes per derivation
node), O(N) revocation time, unresolved cross-type lock ordering with D53, and
cross-core costs orders of magnitude above IPC fast-path latency. No deployed
system uses both CDT and generation counters; the Coyotos lineage explicitly
replaced CDT-style link chains with generation counters.

Discharges D11's deferral: "revisit when the IPC model decision reveals whether
Base-B plus IPC-level mechanisms cover the workloads that would otherwise
justify generation-as-revocation or CDT." The IPC model (D13–D17) is settled.
Field rotation covers session invalidation for Fields. Badges (D17) cover sender
identification and opt-in lifecycle tracking. The remaining gap — revoking all
caps to a non-IPC object (Space, Observer, Time, Pulsar) without destroying it —
is essential under A3 (temporary access tokens for shared memory regions are a
standard microkernel workload pattern). Generation counters close this gap.

Does NOT settle: cap entry layout (generation field placement relative to
existing fields), revocation syscall surface (new typed operation vs. modifier),
cross-core prompt-effect policy (strong vs. weak — generation counters are
naturally lazy/weak; prompt revocation requires IPI), stale slot reclamation
(slots occupied by stale caps until next access; sweep mechanism deferred).

- **Rests on:** D11 (base primitive — generation counters extend, not replace),
  D8 (flat table — generation field in cap entry), A3 (non-IPC cap gap is a
  genuine workload need across generic workloads), A4 (lazy on-use detection is
  purely reactive), A5 (CDT adds a separate kernel structure for gaps
  addressable through existing primitives — incidental), O2 (cache-coherent
  ARM64 — generation bump is eventually visible without IPI), D4 (uniform
  capability semantics — universal mechanism, not type-conditional), D13/D15/D17
  (IPC model settles field rotation + badges as IPC-cap alternatives), D19
  (multi-field wait enables field-per-client as CDT alternative), D33
  (preemptible cascade — generation counters avoid CDT's O(N) WCET concern), D53
  (arena lock ordering — generation counters don't create cross-type traversal),
  Coyotos allocation count (primary precedent), seL4 CDT (rejected precedent for
  CDT-only).
- **Status:** settled — revisit if measurement shows generation check cost on
  IPC hot path exceeds 5 cycles (re-opens scoped-vs-universal), or if a
  downstream userspace framework derivation reveals transitive delegation chains
  are essential and field-per-client + bump-and-reissue are structurally
  insufficient (re-opens CDT).
- **Journal:** `journal/064-revocation-addons.md`.

### D68 — Pager unavailability: three failure modes, three mechanisms

G04 decomposes into three structurally distinct failure modes, each forced to a
different mechanism by the constraint graph.

**Case A — handler Field destroyed.** D33's Field-destroy hook walks the pending
list and wakes faulting Observers with an error. The kernel transitions the
Observer to an error-faulted sub-state and sends a notification to a
pre-configured supervision Field. The supervisor (holding appropriate D39
rights) decides: destroy, re-assign handler via change_handler, or resume with
resolution. The kernel does not autonomously destroy (D4). The half-open variant
(handler received fault message, then died before resume) is structurally
identical — D33's cascade reaches the handler Field, same notification fires.

**Case B — handler Field alive, receiver unresponsive.** Cooperative escalation
chain (D31 model): each handler that cannot resolve a fault forwards it to its
own handler via standard IPC, passing the faulting Observer's handle cap
through. Chain continues until resolution or root pager (Case C). Timeout
enforcement via Pulsar watchdog (D44): supervision Observer arms a Pulsar,
checks Observer state via read_registers (D39) on fire, acts if still faulted.
No kernel-internal timeout — A3 says timeout value is workload-dependent; A4
says no background scanning. Pulsar watchdog also covers silent chain breakage
(handler drops escalation without forwarding).

**Case C — chain terminus.** When the escalation chain terminates at the kernel
(root pager) without resolution, the kernel destroys the faulting Observer.
Kernel-autonomous destroy is justified here: the kernel IS the final authority
as root pager. Parking in error-faulted state is not viable — no higher-level
supervisor exists to act.

Supervision Field is a creation-time configuration parameter (optional). The
escalation message format is standard D28 IPC — forwarding is a userspace
convention, not a kernel-enforced protocol.

Does NOT settle: supervision Field mandatory vs. optional at creation (Observer
creation API refinement), escalation protocol standardization (userspace
convention vs. kernel-defined format), error-faulted sub-state encoding in D39's
state machine.

- **Rests on:** D31 (fault handler chains — forecloses standalone let-it-hang
  and standalone double-fault-kill), D33 (Field destroy cascade — Case A hook
  point), D21 (handler at reserved slot 0 — dead cap detection O(1)), D18
  (deferred delivery — pending list is fault queue), D44 (Pulsar — Case B
  watchdog without kernel-internal timeout), D39 (Observer rights —
  read_registers for state checking, change_handler for re-assignment), D40
  (fault resolution — Observer handle in fault message enables chain
  forwarding), D11 (destroy-invalidation — dead Field detection automatic), A3
  (generic — no embedded timeout policy), A4 (purely reactive — no background
  scanning), A5 (kernel absorbs detection/notification, userspace provides
  policy), D4 (designation = authority — kernel-autonomous destroy only at chain
  terminus).
- **Status:** settled. Closes G04. Revisit if cooperative escalation's
  silent-failure mode proves structurally unacceptable (re-opens
  kernel-automatic traversal with back-pointers), or if Pulsar watchdog proves
  too complex for practical supervision hierarchies (re-opens kernel-internal
  timeout with A3 tension acknowledged).
- **Journal:** `journal/067-pager-unavailability.md`.

### D69 — Interrupt masking during IPC fast path: DAIF.I for full window

The IPC fast path masks IRQ (DAIF I-bit) for its ~400-cycle window.
`msr daifset, #2` at fast-path entry, `msr daifclr, #2` at exit. ~2–8 cycles
overhead. DAIF.I only — FIQ (F-bit) is not masked (routes to EL3 at Non-Secure
EL1; not ours to mask).

Five convergences from the design graph: (1) D50 TOCTOU elimination — no
interrupt can invalidate the scheduler callback's decision between
`should_switch_to` returning true and the context switch completing; (2) journal
023 Verus readiness — a non-preemptible section is orders of magnitude easier to
specify and verify than a preemptible or restartable one; (3) A4 alignment — the
fast path is a single non-nested exception handler execution, avoiding TrapFrame
nesting, serial lock deadlock, and nested-exception handling; (4) D1 hot-path
simplicity — straight-line section with no interrupt-nesting branches; (5)
Blackham et al. (EuroSys 2012) quantitative grounding — the fast path's
~400-cycle window is 0.4–4% of measured worst-case interrupt latency in
non-preemptible seL4; the fast path is not where interrupt latency is primarily
spent.

Prior art is unanimous: every surveyed microkernel with an IPC fast path (seL4
classic, seL4 MCS, L4Ka::Pistachio, Fiasco.OC, EROS/Coyotos, Barrelfish) masks
interrupts during the equivalent window. 30+ years, zero counterexamples.

Three alternatives rejected. Don't mask (Option B): D50 TOCTOU, nested exception
handling, no prior art. Priority masking via ICC_PMR_EL1 (Option C): requires
settling D22's deferred priority exposure, partial TOCTOU, contradicts journal
066's flat-priority settlement. Restartable fast path (Option D): extreme
complexity, no hardware support, no prior art in any kernel.

D42 tension accepted: high-R, high-P Observers experience up to ~400 cycles
(~200 ns at 2 GHz) added interrupt delivery latency per concurrent fast-path
invocation on their core. Accepted because the floor is bounded and
deterministic, is <4% of total worst-case, millisecond-scale RT deadlines
tolerate it, and ultra-low-latency RT uses dedicated cores (D2) where IPC
fast-path frequency is low.

- **Rests on:** D50 (fast-path conditions — scheduler callback creates TOCTOU
  that masking closes), D1 (hot-path simplicity — straight-line section), A4
  (purely reactive — no kernel threads, no preemption infrastructure), A2 (ARM64
  DAIF mechanism — I-bit masks IRQ, ~1–4 cycles), D42 (three-value profile — D42
  tension accepted, not eliminated), D22 (interrupt delivery through fields —
  masking delays, does not lose, delivery), D33 (preemptible destroy cascade —
  the dominant contributor to worst-case interrupt latency is already addressed
  by preemption points in long paths), journal 023 (framekernel/Verus readiness
  — non-preemptible fast path is the verification prerequisite), journal 066
  (flat interrupt priority — priority-based masking contradicts).
- **Status:** settled. Closes G05. Revisit if a concrete workload requires <200
  ns interrupt latency AND cannot use dedicated-core isolation (D2), or if a
  formally verified restartable fast path is demonstrated in any kernel.
- **Journal:** `journal/068-interrupt-masking-fastpath.md`.

### D70 — Arena internal structure: per-type slab with page return

Each per-type arena (D53) is internally a slab allocator: hardware pages divided
into N fixed-size slots, with an intrusive freelist through freed slots. When
all N slots on a page are free, the page returns to the root Space pool.

This question applies only to kernel-internal fixed-size allocations (D32
category 2: per-object metadata from root Space). D24's cap-mapping invariant
does not constrain it — kernel structs are never mapped into Observer address
spaces. The D24-driven sub-page concern (journals 025, 026) applies to userspace
Space objects, which are a separate problem already resolved (full pages per
Space, D25).

The slab is chosen over two alternatives:

- **One-per-page** (seL4 model): 97.6% memory waste per object (96-byte Observer
  in 4 KB page), TLB miss per object on sequential access, root Space budget
  dominated by waste. Rejected under A3 (generic kernel cannot absorb the cost).
- **Grows-never-shrinks arena**: same dense packing as slab, but memory
  proportional to peak allocation, not steady-state. After a workload peak,
  pages are stranded permanently. Rejected under A3 (long-lived servers require
  steady-state-proportional memory).

Copy-on-compact is foreclosed by A4 (synchronous — no background compaction),
D33 (preemptible cascade — concurrent access during copy), D4 (pointer =
capability — moving objects creates dangling pointers), and SMP (stop-the-world
pause required). Object addresses are stable for life.

Buddy allocator for fixed-size types is foreclosed by D32 (fixed-size per type —
buddy degenerates to power-of-two freelist with roundup waste, no benefit over
direct freelist).

Implementation properties: intrusive freelist uses `MaybeUninit<T>` with freed
slot bytes reused as `*mut NextSlot` (all current types ≥ 48 bytes, well above
the 8-byte pointer minimum). Unsafe lives in the framekernel core (journal 023).
Page size read at boot via D25 query; slots-per-page configured accordingly.

Does NOT settle: variable-size auxiliary allocation (D54 routing arrays, D43
multi-field WaitEntries — different sub-problem, not fixed-size), per-core arena
sharding (D53's flagged SMP optimization — compatible with per-core magazines
but not required; D1 cold-path makes global per-type locks acceptable), object
zeroing policy on slot reuse, root Space pool internal recycling behavior.

- **Rests on:** D32 (fixed-size per object type — slab's native operating
  condition), D53 (per-type arena with SpinLock — slab is the arena's internal
  structure), D33 (preemptible destroy cascade — partial-page retention is
  benign; freed slots immediately reusable), D1 (cold-path allocation — slab
  setup cost amortized; hot-path benefits from cache locality of packed
  same-type objects), A3 (generic kernel — steady-state-proportional memory
  required for server workloads), A1 (Rust — MaybeUninit slab is
  well-established unsafe pattern; contained in framekernel core), D31 (root
  Space — slab pages drawn from and returned to root Space pool), D25 (page size
  exposed — slab configures slots-per-page at boot), journal 023 (framekernel
  discipline — all slab unsafe at the core boundary).
- **Status:** settled. Closes G06. Revisit if a concrete workload demonstrates
  slab page-return overhead is significant (would motivate grows-never-shrinks),
  if formal verification requires one-per-page simplicity (would revisit the
  seL4 trade-off), or if per-core arena sharding is implemented (internal slab
  structure may need per-core magazines).
- **Journal:** `journal/069-sub-page-packing.md`.

### D71 — Badge condition form: range; receive-time filter: closed

The badge condition embedded in a D45 routing rule is a closed range:
`low <= badge <= high`. Each routing entry stores
`(low: u64, high: u64, destination)`. Exact match is `low == high`. The
condition is evaluated via two comparisons per candidate entry during O(log N)
binary search over D54's sorted array.

Three independent paths converge on range:

1. **D54 binary search compatibility.** D54's sorted array requires conditions
   with a natural total order. Range conditions sort on `low` and support binary
   search. Bitmask conditions (`badge & mask == expected`) have no orderable
   key, forcing O(N) linear scan — at 20 splits, ~40 cycles for condition checks
   alone; at 100 splits, ~200 cycles, half the ~400-cycle fast-path budget.
   Structurally incompatible with D1.

2. **Expressive sufficiency.** Common badge allocation patterns — sequential
   client IDs, IRQ number ranges, category-per-range partitions — are naturally
   range-expressible. Bit-structured badge spaces (category in high nibble,
   instance in low nibble) are also expressible: category K occupies
   `[K << shift, K << shift + (2^width - 1)]`. Non-contiguous badge sets require
   multiple entries, but under D17 the minter controls badge values and can
   structure allocation to produce contiguous ranges.

3. **Incumbent.** D45 is called "badge-range routing." D54's entry layout stores
   range fields. Every journal entry and spec entry uses range language.

Foreclosed alternatives: predicate (A5 — computational model in kernel
interface; D1 — unpredictable cost), bitmask alone (D54 binary search
incompatibility), exact match (dominated by range — same cost, strictly less
expressive).

**Receive-time filter:** D44's deferred "badge-filtered receive" is closed. D45
routing serves the primary use case (routing subsets of messages to dedicated
Fields). Receive-time filtering tensions D13 (O(queue_len) scan vs. O(1) front
dequeue), D15/D18 (skipped messages fill queue, blocking legitimate senders),
and D50 (filter condition evaluation on every arrival adds fast-path cost). No
surveyed kernel has badge-range filtering on Receive for queue-based IPC.

Does NOT settle: range representation (closed `[low, high]` vs. half-open
`[low, high)` — implementation detail), exact routing entry layout (D54 open
item), catch-all entry semantics (expressible as normal `[0, u64::MAX]` range,
no special status needed).

- **Rests on:** D45 (badge-range routing — this derivation settles D45's
  deferred badge condition form), D54 (sorted-array routing table — binary
  search requires orderable conditions; the structural constraint that
  forecloses bitmask), D1 (hot-path — O(N) linear scan for bitmask is
  unacceptable at moderate split counts), D50 (routing evaluation is a fixed
  cost within the fast path — condition check must be extremely cheap), D17
  (minter-assigned badges — the minter controls badge values and can structure
  allocation to produce contiguous ranges), D13 (queued fields — receive-time
  filtering changes O(1) front-dequeue to O(queue_len) scan), D15 (senders
  oblivious — skipped messages occupy queue slots), D18 (error-to-sender
  overflow — queue fills with skipped messages), A3 (generic kernel — condition
  form must work for all badge allocation strategies), A5 (leaf node — predicate
  interpreter in kernel is complexity in the wrong place; bitmask expressiveness
  gap addressable by userspace allocation strategy or multiple routing entries).
- **Status:** settled. Closes D45's "badge condition form" and D44's
  "badge-filtered receive." Revisit if D54 is revised (changes the routing table
  structure), if D1 is revised (changes the hot-path constraint), if a concrete
  workload demonstrates that contiguous badge allocation is structurally
  impossible (would reopen the bitmask question), or if a downstream derivation
  identifies a use case for receive-time filtering that D45 routing cannot
  serve.
- **Journal:** `journal/070-badge-condition-form.md`.

### D72 — Pulsar deadline form: relative duration in nanoseconds

The `create_pulsar` duration parameter is a relative offset in nanoseconds
("fire in N nanoseconds from now"). The kernel reads the current counter,
converts nanoseconds to ticks using CNTFRQ_EL0, computes the absolute CVAL
comparator, and programs the timer. Internally the kernel works in absolute
space — D44's `next = scheduled + period` arithmetic is unchanged.

Settled by applying D66's anti-pattern: an absolute-only API forces callers to
provide the current counter value (via `clock_read` or direct CNTVCT_EL0
access), which the kernel already knows. Relative duration accepts the caller's
natural expression and has the kernel absorb the trivial conversion — consistent
with D66's routing resolution (kernel-automatic, no new API for information the
kernel already has) and A5 (absorb complexity).

Common-case timer patterns ("sleep for N ms," "retry in 5s") are one syscall for
all Observers regardless of clock access authority (M11). Precision one-shot
loops (adaptive timing) compute `next_duration = desired - now`, requiring one
clock read — the same cost absolute-only imposes on the common case for
Observers without clock access. The cost is borne by the minority precision use
case rather than the majority common case.

Forward-compatible: absolute mode (flag bit in duration field or second
operation) is additive and non-breaking. Not foreclosed.

Nanoseconds as the API unit follows from A5: the kernel knows CNTFRQ_EL0 and
absorbs the frequency conversion. Callers express intent in human-meaningful
units.

Does NOT settle: minimum/maximum duration bounds, duration = 0 semantics
(immediate fire vs. error — implementation detail).

- **Rests on:** D44 (Pulsar semantics — deferred G09; kernel re-arm uses
  absolute internals), D66 (clock access mechanism — established
  "kernel-already-knows" anti-pattern; per-Observer clock access means some
  Observers lack direct counter access), D49 (ABI encoding — duration fits in
  x2; no flag bit needed), D62 (creation API — single-call, armed-at-creation;
  duration is the fourth parameter), D63 (message layout — fire time in raw
  ticks enables drift-free one-shot loops with relative API), A5 (absorb
  complexity — kernel absorbs relative-to-absolute and ns-to-tick conversion),
  A3 (generic — relative serves all workloads; absolute mode not foreclosed), A2
  (ARM64 — TVAL/CVAL symmetric; no hardware preference).
- **Status:** settled. Closes G09. Revisit if a defined workload demonstrates
  that `clock_read` cost in precision one-shot loops is a correctness or
  performance bottleneck not addressable by granting clock access authority —
  add the flag-bit absolute mode (additive, non-breaking).
- **Journal:** `journal/072-pulsar-deadline-form.md`.

### D73 — Send-once exemption encoding: structural code-path separation, reply Field always-tracked

D17's T1 tension requires the kernel to distinguish "consumed by use" (send-once
cap used for a successful Send — no badge-closure) from "closed without use"
(cap dropped or cascade-closed — badge-closure fires). The encoding mechanism is
structural code-path separation: the kernel has two distinct operations that
remove a send-once cap from the table, and badge-closure checking lives in only
one of them.

1. **Consume-on-delivery** (used path): kernel-triggered post-delivery removal.
   Clears the slot and decrements the refcount directly. Does not enter D11's
   close logic. Badge-closure is never reached.
2. **D11 close** (unused path): userspace-triggered or cascade-triggered (D33).
   Runs the full badge-closure check on tracked Fields. This is the path that
   fires the "reply will never come" notification.

No extra data field, no conditional branch on the used path. The exemption is a
structural property of the code, not a runtime check. Under D53's arena-lock
model, concurrent close and consume-on-delivery on the same cap are serialized —
an explicit `consumed` flag adds no safety beyond what the structural separation
provides.

The reply Field (D16's pre-allocated per-Observer Field for RPC replies) is
always created with badge-closure tracking enabled. This is a specialization of
D17's general opt-in rule: the reply Field's structural purpose (reply routing)
requires tracking for correctness — without it, a caller whose reply cap is
dropped has no reactive signal and is permanently blocked (violates A4). General
Fields remain opt-in per D17. The cost is one bit per reply Field.

Badge-closure notification on the reply Field is self-discriminating: its
presence means "reply will never come." No reason code is needed in the message
body — D64's all-zero data words are correct for this case.

Does NOT settle: reply Field creation timing (pre-allocated vs. lazy — D16
defers), voluntary Call() cancellation by the caller (not foreclosed), per-Field
exemption policy for non-reply authorization-audit use cases (Option C from
exploration — not foreclosed, additive if a concrete workload motivates it).

- **Rests on:** D17 (T1 tension — consumed-by-use exempt, closed-without-use
  fires; badge-closure mechanism; opt-in tracking), D11 (close path — where
  badge-closure checking lives), D51 (send-once boolean flag — no additional
  encoding needed), D13 (Field-based delivery — no separate cancellation
  primitive), D53 (arena-lock serialization — eliminates the concurrent-close
  race that would motivate an explicit flag), D16 (reply Field — pre-allocated,
  per-Observer, kernel-managed; structural purpose requires tracking), D64
  (badge-closure message format — all-zero data words sufficient, no reason
  code), D33 (destroy cascade — cascade-triggered close is D11 close, fires
  badge-closure as expected), A4 (purely reactive — reply Field without tracking
  allows permanent blocking with no signal), A1 (Rust — consume = move, close =
  drop; naturally distinct operations).
- **Status:** settled. Closes G10. Revisit if a concrete authorization-audit
  workload requires consumed-by-use notification (motivates per-Field exemption
  policy — additive), or if a workload demonstrates that D18 queue-full drop on
  the reply Field is a practical reliability concern (motivates direct
  cancellation — but D13-compliant form is structurally equivalent).
- **Journal:** `journal/073-send-once-exemption-encoding.md`.

### D74 — Register save/restore flow: direct-to-RegisterState on EL0, TrapFrame on EL1h

EL0 exceptions (SVC, fault, IRQ from userspace) save registers directly into the
current Observer's RegisterState in structural backing (D43). No intermediate
TrapFrame. EL1h exceptions (kernel-interrupting-kernel) save to a
stack-allocated TrapFrame (unchanged). This eliminates double-write overhead on
context switches and keeps RegisterState always correct.

**Save path (EL0):** Assembly obtains the current Observer's RegisterState
pointer via per-core state (TPIDR_EL1 → per-core struct → RegisterState
pointer). Saves all GPRs (x0–x30), SP_EL0, PC (ELR_EL1), PSTATE (SPSR_EL1),
TPIDR_EL0, and FP/SIMD (q0–q31, FPCR, FPSR) directly to RegisterState. ESR_EL1
and FAR_EL1 are read into scratch registers for dispatch — not stored in
RegisterState (transient, exception-specific).

**Restore path:** Load all registers from the incoming Observer's RegisterState.
Modify SPSR_EL1 if needed (D49 carry-flag error signaling). Write
CNTKCTL_EL1.EL0VCTEN from Observer's clock_access flag (D66). Switch TTBR0_EL1
if address space differs (D5). Eret.

**x0–x3 handling:** Saved unconditionally on the save side (RegisterState always
accurate — D39 read-registers returns correct values for suspended/blocked
Observers). On the IPC fast path (D50), x0–x3 are NOT loaded on the restore side
— they pass through in physical registers carrying data words from sender to
receiver (D47). Cost of unconditional save: ~4–8 cycles (~1–2% of ~400-cycle
fast-path budget). Save-side skip (Option C — deferred save with dirty bit)
rejected: complexity disproportionate to ~2–5% gain, breaks D39 read-registers
correctness, harms Verus readiness (journal 023).

**TPIDR_EL1 as per-core state pointer:** Set during boot-time per-core
initialization. Points to a per-core struct containing (at minimum) the current
Observer's RegisterState pointer. Updated on context switch. Standard ARM64
kernel convention (Linux, FreeBSD, seL4, Zircon).

Does NOT settle: lazy FP/SIMD save (orthogonal, future optimization), per-core
state struct layout beyond RegisterState pointer, fast-path assembly as separate
routine vs. branch within exception.S (implementation detail).

- **Rests on:** D43 (register save pointer in metadata, structural backing
  location), D47 (x0–x3 pass-through, kernel restricted to x4–x15), D49 (SPSR
  carry-flag modification), D50 (fast-path conditions — defines when x0–x3
  pass-through applies), D69 (DAIF.I masking — save/restore in non-preemptible
  window), D66 (CNTKCTL_EL1 on context switch), D6 (one RegisterState per
  Observer), D35 (write-registers establishes RegisterState as primary
  representation), D39 (read-registers must return accurate state — rejects
  save-side x0–x3 skip), A1 (unsafe in frame/), A2 (ARM64 register set,
  TPIDR_EL1), A4 (single exception invocation), D1 (per-core hot path),
  `design/landscape.md` §5.4 (minimal-save + ESR dispatch is microkernel
  consensus), `design/research/ipc-fastpath-conditions.md` (seL4/L4 save
  directly to thread save area).
- **Status:** settled. Revisit if D47 is revised (alters pass-through), if D43
  is revised (changes RegisterState location), if profiling justifies save-side
  x0–x3 skip (Option C upgrade), or if a second architecture requires a
  fundamentally different split.
- **Journal:** `journal/074-register-save-restore-flow.md`.

### D75 — Global arena organization: bundled KernelState global with data-owning Lock

The five per-type arenas (D53) and the SpaceManager (D3, D31) live in a single
global `KernelState` struct. Cold-path code accesses arenas through this global.
The hot path (D1, D50, D74) never touches it — it works exclusively with
per-core `NonNull<Observer>` pointers, RegisterState, and scheduler state.

`Lock<T>` is refactored from `PhantomData<T>` to `UnsafeCell<T>`: the lock owns
its data, and `LockGuard` provides `DerefMut<Target=T>`. This closes the gap
where "caller must hold this arena's lock (D53)" was enforced by convention
rather than the type system. A1 says ownership maps to resource lifecycle and
unsafe boundaries map to trust boundaries — the lock-before-access invariant is
a trust boundary that Rust can enforce.

The bundle collects all kernel-wide shared cold-path state in one namespace.
Arena access and SpaceManager access have identical patterns (shared, cold-path,
under locks) and should be organized consistently. The bundle is also a single
point of change if arena sharding (D53's flagged SMP optimization) is
implemented — "isolate uncertain decisions behind interfaces."

Parameter threading (passing `&Arenas` through every cold-path function) was
rejected: it pushes a leaf concern (storage location of shared state) into
inter-module interfaces. Every function signature grows to advertise an
implementation detail. The reference is a constant (all cores point to the same
struct) — threading it adds ceremony without information. This conflicts with
"push complexity to the leaves."

Five separate statics (no bundle) was rejected: scatters the organization across
import sites, losing the single-point-of-change property for future sharding.
The bundle is better aligned with "isolate uncertain decisions behind
interfaces."

Per-core arena copies (Barrelfish model) are not foreclosed but would require
reopening D53. D33 cascade crosses types and cores (every cap close becomes
cross-core), ObjectId encoding assumes a single per-type arena, and D53's lock
ordering assumes one arena per type. Per-core sharding (front-end magazines,
shared back-end) remains compatible within this organization.

Initialization: the `KernelState` global is initialized by the BSP during boot,
before secondary core activation (D46, PSCI CPU_ON). Arena slabs start empty;
first allocations (boot objects — root Observer, initial Fields) draw pages from
the root Space pool.

Does NOT settle: `KernelState` struct layout beyond arenas and SpaceManager
(other kernel-wide shared state — IRQ routing table, idle bitmap — added as
derived), Lock<T> internals beyond the UnsafeCell ownership change (interrupt
masking on acquire, WFE spinning — frame/ implementation detail), arena
initialization ordering details (boot sequence).

- **Rests on:** D53 (per-type arenas with lock ordering — this decision
  organizes what D53 defined), D70 (slab internals — arena internal structure is
  settled; this is the outer organization), D1 (hot/cold split — arenas are
  cold-path shared state; organization must not leak into hot path), D50
  (fast-path conditions — fast path never touches arenas; confirms organization
  is invisible to performance-critical code), D3 (one logical Space manager —
  same shared-state character as arenas; bundled together for consistency), D31
  (root Space pool — slab pages drawn from SpaceManager; co-location simplifies
  the allocation path), D46 (core lifecycle — BSP initializes global before
  secondary cores; no lazy init under A4), A1 (Rust ownership — Lock wrapping
  UnsafeCell enforces lock-before-access at the type level; PhantomData left a
  safety gap), philosophy: "push complexity to the leaves" (rejects parameter
  threading), "isolate uncertain decisions behind interfaces" (bundle provides
  single change point for future sharding).
- **Status:** settled. Revisit if per-core sharding is implemented (changes Lock
  semantics — per-core magazines behind the same Lock interface), or if a new
  kernel-wide shared resource doesn't fit the KernelState bundle pattern.
- **Journal:** `journal/075-global-arena-organization.md`.

### D76 — Dispatch entry contract: pull registers, push results, three-variant DispatchResult

The frame/ → safe-code boundary at exception entry follows a pull/push split.
Safe dispatch reads registers lazily from RegisterState via frame/ helpers
(pull) and writes syscall results back via frame/ helpers (push) before
returning DispatchResult. DispatchResult carries only the scheduling decision —
frame/ gets a uniform restore path.

**Register access model: Pull.** Registers are already saved to RegisterState by
EL0 exception entry assembly (D74). Safe dispatch reads them via
`read_ipc_registers` / `read_typed_registers` only when needed. D47: IPC
dispatches from ESR_EL1 alone before reading GPRs. D48: Yield reads zero GPRs.
D50: fast-path avoids reading x0–x3 from the sender.

**DispatchResult:** Three variants. `Resume(Observer)` loads all registers.
`ResumeFastPath(Observer)` skips x0–x3 (D50/D74 pass-through). `Idle` enters
WFI.

**Write helpers (frame/):** `write_ipc_error` (carry + x0), `clear_ipc_carry`,
`write_typed_result` (x0), `write_message_to_registers` (x0–x7, slow path),
`write_metadata_to_registers` (x4–x7, fast path).

**handle_timer parameter:** `current_ticks: u64` — pushed by frame/ as a single
consistent snapshot. The timer counter is volatile; unlike stable RegisterState,
it must be pushed.

Does NOT settle: ~~cap resolution protocol~~ (D77), ~~global state
organization~~ (D82), ~~error/fault delivery paths~~ (D80), fast-path assembly
as separate routine vs. branch within exception.S.

- **Rests on:** D1 (per-core hot path), D7 (IPC vs typed split), D47 (register
  layout, ESR-only dispatch), D49 (error signaling encoding), D50 (fast-path
  conditions), D74 (direct-to-RegisterState on EL0), A2 (ARM64 SPSR carry flag,
  ESR_EL1 syndrome), A4 (purely reactive — every exception ends with a
  scheduling decision).
- **Status:** settled. Revisit if D47 is revised (register layout changes alter
  helper interfaces), if D50 is revised (fast-path condition changes may remove
  the ResumeFastPath variant), or if D74 is revised (save model change would
  restructure the pull interface).
- **Journal:** `journal/076-dispatch-entry-contract.md`.

### D78 — IPC message ownership: explicit transfer through return types

Message ownership at each IPC stage is tracked through return types, not
convention. `send()` consumes the Message by value. On `WokeReceiver`, the
message is returned in the enum variant for dispatch to deliver to the
receiver's saved registers. On `Enqueued`, ownership transfers into the queue.

`call()` has three outcomes. `DirectSwitch` (D50 fast path) carries only the
observer pointer — no Message struct needed because x0–x3 pass through in
physical registers (D74) and the dispatch layer writes only x4–x7 metadata.
`WokeReceiverSlowPath` (waiter present, user cap in message) returns the
observer and message for slow-path delivery with cap transfer. `Enqueued` means
the message entered the queue.

`reply_recv()` returns `ReplyRecvOutcome` containing both the reply-side
delivery (if a client was waiting on the reply field: observer + message) and
the receive-side outcome (dequeued message or blocked). Previously the reply
side discarded the woken client pointer.

Behavioral change from pre-D78: `call()` with a user cap now pops the waiter
when present (returning `WokeReceiverSlowPath`) instead of leaving the waiter
stranded and always enqueueing. The receiver should be woken regardless of cap
presence — the cap only forces slow-path delivery.

Does NOT settle: dispatch_ipc implementation body (future layer), cap
installation during slow-path delivery (D8 downstream), reply cap creation
mechanics.

- **Rests on:** D13 (queued fields — message is what the queue holds; direct
  delivery when waiter present), D16 (reply via send-once — reply_recv
  reply-side delivery), D28 (fixed-size message format — Message is the
  ownership unit), D50 (fast-path conditions — DirectSwitch vs slow path; 0-cap
  gate determines whether Message struct is needed), D74 (register pass-through
  — x0–x3 skip on fast path eliminates need for Message struct on DirectSwitch),
  D76 (write helpers — write_message_to_registers for slow path,
  write_metadata_to_registers for fast path; dispatch writes before returning
  DispatchResult).
- **Status:** settled — revisit if D50 is revised (fast-path condition changes
  alter the DirectSwitch/WokeReceiverSlowPath split), if D74 is revised
  (register pass-through changes alter what data needs to be carried), or if D28
  is revised (message format changes alter the ownership unit).
- **Journal:** `journal/078-ipc-message-ownership.md`.

### D79 — Scheduling decision matrix: state transitions and dispatch results per IPC outcome

For each (IPC operation x outcome) pair, the kernel performs specific Observer
state transitions, scheduler method calls, register writes, and returns a
specific DispatchResult. The matrix has 10 rows (9 operation-outcome pairs plus
Yield):

| #   | Operation x Outcome         | Sender state            | Receiver state                    | Scheduler calls                                                                                 | DispatchResult                                              |
| --- | --------------------------- | ----------------------- | --------------------------------- | ----------------------------------------------------------------------------------------------- | ----------------------------------------------------------- |
| 1   | Send x Enqueued             | Stays Runnable          | —                                 | None                                                                                            | Resume(sender)                                              |
| 2   | Send x WokeReceiver         | Stays Runnable          | Blocked→Runnable                  | enqueue(receiver)                                                                               | Resume(sender)                                              |
| 3   | Receive x Received          | —                       | Stays Runnable                    | None                                                                                            | Resume(receiver)                                            |
| 4   | Receive x Blocked           | —                       | Runnable→Blocked                  | dequeue(receiver), pick_next                                                                    | schedule_next()                                             |
| 5   | Call x Enqueued             | Runnable→Blocked        | —                                 | dequeue(sender), pick_next                                                                      | schedule_next()                                             |
| 6   | Call x DirectSwitch         | Runnable→Blocked        | Blocked→Runnable                  | should_switch_to; if yes: dequeue(sender); if no: dequeue(sender), enqueue(receiver), pick_next | Approved: ResumeFastPath(receiver); Denied: schedule_next() |
| 7   | Call x WokeReceiverSlowPath | Runnable→Blocked        | Blocked→Runnable                  | dequeue(sender), enqueue(receiver), pick_next                                                   | schedule_next()                                             |
| 8   | ReplyRecv x Received        | Server stays Runnable   | Client (if any): Blocked→Runnable | enqueue(client) if woken                                                                        | Resume(server)                                              |
| 9   | ReplyRecv x Blocked         | Server Runnable→Blocked | Client (if any): Blocked→Runnable | dequeue(server), enqueue(client) if woken, pick_next                                            | schedule_next()                                             |
| 10  | Yield                       | Stays Runnable          | —                                 | enqueue(sender at tail), pick_next                                                              | schedule_next()                                             |

Key design decisions:

1. **Send never uses ResumeFastPath.** D50 condition 1: only Call and ReplyRecv
   are fast-path eligible. Send is fire-and-forget — the sender always
   continues.

2. **D50 should_switch_to consulted only for Call x DirectSwitch (Row 6).**
   Approval returns `ResumeFastPath(receiver)` with x0–x3 pass-through (D74).
   Denial falls back to enqueue + pick_next.

3. **Yield re-enqueues before pick_next.** The yielding Observer goes to the
   tail of the run queue, then the scheduler picks the next. This ensures
   round-robin fairness and prevents losing the Observer from the queue.

4. **ReplyRecv handles both reply and receive atomically.** The reply phase may
   wake a client (enqueue it). The receive phase either delivers a message
   (server continues) or blocks the server. Both phases execute in one dispatch
   call.

Does NOT settle: cap resolution protocol body (D77 defines the sequence; D79
defines what happens after), Observer.block()/unblock() calls during dispatch
(requires arena mutable access), cap installation during message delivery (D8
downstream).

- **Rests on:** D2 (per-core schedulers — scheduler methods shape the matrix),
  D13 (queued fields with direct-switch — Send/Receive semantics), D16 (reply
  via send-once — Call/ReplyRecv blocking semantics), D39 (Observer state
  machine — Runnable/Blocked transitions), D48 (5 IPC operations — the rows),
  D50 (fast-path conditions — DirectSwitch eligibility, should_switch_to), D59
  (Scheduler trait — enqueue/dequeue/pick_next/should_switch_to/on_preempt), D76
  (dispatch entry contract — DispatchResult variants, register write helpers),
  D78 (message ownership — outcome types that carry messages and observer
  pointers).
- **Status:** settled — revisit if D50 is revised (fast-path conditions alter
  which rows use ResumeFastPath), if D59 is revised (Scheduler trait changes
  alter which methods are called), if D39 is revised (state machine changes
  alter transition validity), or if D78 is revised (outcome type changes alter
  what data is available for register writes).
- **Journal:** `journal/079-scheduling-decision-matrix.md`.

### D80 — Error and fault delivery protocol

Two distinct paths. Syscall errors: dispatch writes error encoding (D49) to the
current Observer's registers and resumes it — no state transition, no IPC. Fault
delivery: kernel constructs a fault message (D28 layout, D61 per-type data
words) and delivers it as IPC to the handler Field. The fault cap is constructed
directly by the kernel with a 5-right subset (D39: resume, destroy, install-cap,
write-registers, read-registers) — not minted from the Observer's self-cap.
Three delivery outcomes: direct delivery to a waiting handler, deferred via D18
pending list, or handler unavailable (D68 chain terminus).

- **Rests on:** D12, D13, D18, D21, D39, D49, D61, D76.
- **Status:** settled.
- **Journal:** `journal/080-error-fault-delivery.md`.

### D81 — Hardware event protocol

IRQ delivery uses a kernel-wide routing table: each INTID maps to a Field with
badge and generation for stale-route detection. The table is global (D22: IRQs
must reach any Field regardless of which core takes the interrupt); per-core
routing rejected. Timer interrupts check a per-core deadline structure; expired
Pulsars fire and rearm (D62/D63). Queue-full triggers deferred fault delivery
(D80); overruns tracked per-Pulsar.

- **Rests on:** D2, D22, D44, D62, D63, D75, D76, D82, D83.
- **Status:** settled.
- **Journal:** `journal/081-hardware-event-protocol.md`.

### D82 — Global state organization

Shared kernel state (five per-type arenas + SpaceManager) lives in a single
bundled structure, accessed through a safe global accessor. Each field is
independently locked (D53 ordering). The structure is initialized at boot and
immovable thereafter. Alternatives rejected: per-core copies (inflates per-core
state, complicates test setup), free-standing globals (scatters organization,
loses single point of change per D75), lazy initialization (boot-time init
sufficient per A4 + D46).

- **Rests on:** D75, D53, D1, D46, D3, D31, D70, A1.
- **Status:** settled.
- **Journal:** `journal/082-global-state-organization.md`.

### D83 — Per-core data organization

Per-core state splits into two layers: a minimal fixed-layout structure
addressable from assembly (register save target pointer + scheduler state
pointer), and a richer scheduler state structure accessible only from Rust. The
fixed-layout structure lives at TPIDR_EL1. Assembly never touches the scheduler
state directly — the indirection decouples assembly layout from Rust generics.
Each core also carries a fixed-capacity deadline structure for Pulsar timer
checking (D44). Pointing TPIDR_EL1 directly at scheduler state rejected (generic
type makes assembly layout unknowable). Dynamic deadline allocation rejected
(timer handler must not allocate).

- **Rests on:** D1, D46, D56, D74.
- **Status:** settled.
- **Journal:** `journal/083-per-core-data-organization.md`.

### D88 — TTBR0/TTBR1 split contract

Kernel occupies the upper-half virtual address space (TTBR1, 2-level walk, 64
GiB). Per-Observer user mappings occupy the lower half (TTBR0, 3-level walk, 128
TiB). E0PD1 prevents EL0 speculative access to kernel space. TTBR0-only rejected
(every page table must include kernel mappings; weaker security). Symmetric
2-level for both halves rejected (only 2048 Space slots; breaks D26 shared page
table subtree model). Full KPTI unnecessary with E0PD (fallback on pre-ARMv8.5
hardware).

- **Rests on:** A2, D1, D5, D26, D43, D74.
- **Status:** settled.
- **Journal:** `journal/088-ttbr-split-contract.md`.

### D89 — Per-Observer page table structure

Three-level structure: per-Observer root, per-Observer per-region intermediate
tables, and per-Space leaf tables shared across Observers. Each Space occupies a
fixed VA-aligned region. One leaf table per Space is referenced from each
holding Observer's intermediate table, scaling as O(Observers + Spaces) rather
than O(Observers × Spaces). Per-Observer leaf tables (no sharing) rejected on
memory cost. Sharing with a 2-level structure rejected (limits total Space
count, wastes VA for small Spaces).

- **Rests on:** D5, D24, D26, D43, D88.
- **Status:** settled.
- **Journal:** `journal/089-per-observer-page-table.md`.

### D90 — PTE population policy: eager

Page table entries for a Space's physical pages are populated eagerly at Space
creation. No demand faults on first access. Demand faulting rejected: per-fault
overhead exceeds per-entry write cost by orders of magnitude, adds a
demand-fault check to the exception path, and creates non-deterministic
first-access latency (D42 tension). Hybrid (eager for small, demand for large)
rejected: threshold heuristic adds complexity without measurable benefit since
even large Spaces populate cheaply.

- **Rests on:** D1, D12, D26, D32, D42, D61, D89.
- **Status:** settled.
- **Journal:** `journal/090-pte-population-policy.md`.

### D91 — Cap-to-mapping protocol

Page table mutations on cap install/close operate at the intermediate table
level: one table descriptor write to map a Space's shared leaf table, one clear
to unmap. Leaf tables are populated eagerly (D90) and shared immutably. On cap
install: if the Space is already mapped (duplicate cap), no work; otherwise
write the descriptor and allocate the intermediate table from root pool if
needed. On close: check whether any remaining caps reference the same Space; if
this was the last, clear the descriptor and TLB invalidate. Per-Observer
per-Space reference count rejected: avoids the cap table scan but adds state to
maintain across install, close, cascade, and transfer.

- **Rests on:** D8, D11, D24, D26, D33, D41, D89, D90.
- **Status:** settled.
- **Journal:** `journal/091-cap-to-mapping-protocol.md`.

### D92 — Page table memory accounting

Per-Observer root tables are allocated from the consumed Space at Observer
creation (D35 structural backing). Per-Observer intermediate tables are
allocated on demand from the kernel root pool (D31) when the first Space cap in
a region is installed. Per-Space leaf tables are charged to the Space's type
conversion overhead (D32) at Space creation. On-demand intermediate allocation
chosen over pre-reservation (matches typical single-region case, avoids wasting
memory for unused regions).

- **Rests on:** D31, D32, D35, D43, D70, D89.
- **Status:** settled.
- **Journal:** `journal/092-page-table-memory-accounting.md`.

---

## Open questions

- ~~**Time migration across cores.**~~ Dissolved by D31: Time is abstract
  scheduling capacity (vocabulary revised). Core assignment is kernel-internal.
  Migration is the kernel's internal scheduling decision — the Observer's Time
  cap doesn't change when the kernel moves it to another core. (Previously
  dissolved by D29 as a cap operation; D31 supersedes — migration is no longer a
  user-visible event at all.)
- ~~**Minimum abstract scheduling properties on an Observer.**~~ Settled by D42:
  three-value budget — responsiveness, throughput, precision — sharing a fixed
  per-Observer point allocation. D2's parenthetical "(priority, CPU/IO
  classification, optional deadline)" replaced entirely. No priority integer
  (inflation problem dissolved by budget trade-offs — maximizing any dimension
  costs the other two). CPU/IO kernel-inferred from the profile. Deadline
  kernel-derived from timer period + Time + precision value. Hard RT via
  dedicated cores (D2) with EDF admission using Time + timer + precision.
  Scheduling inheritance during IPC settled by D43 as a userspace concern
  (modify-scheduling mechanism, not kernel policy). Remaining: ~~budget
  encoding~~ (D57: budget 128, store R/T, derive P), ~~default profile~~ (D57:
  43/43/42), timer syscall surface, RT admission control details.
- ~~**Observer-Space cardinality formalization.**~~ Settled by D27: flat. An
  Observer holds multiple independent Space caps directly in its D8 table. No
  kernel-tracked hierarchy between Spaces. Grouping is userspace convention (D6
  parallel). Hierarchy rejected on D8, D6, D4, D11, A3. Provenance tracking
  deferred as kernel-internal optimization.
- ~~**Revocation add-ons.**~~ Settled by D67: universal generation counters (all
  object types). CDT rejected (gaps addressable through existing primitives).
  Remaining: revocation syscall surface (new typed op vs. modifier), cap entry
  layout (generation field placement), cross-core prompt-effect policy (strong
  vs. weak — deferred from D11, not settled by D67), stale slot reclamation
  mechanism.
- ~~**Observer minimum schema.**~~ Settled by D43: eight field clusters in the
  metadata struct — register save pointer, TTBR0, cap table pointer, scheduling
  state (D39 five-state enum + suspended flag), cached compute-unit aggregate,
  scheduling profile (R, T, P — one set, no base/effective split), wait-state
  linkage (Rust enum: inline single-field, allocated multi-field), reference
  count. Observer physically splits into metadata struct (root Space, ~80–100
  bytes) and structural backing (consumed Space: registers, cap table, L0 page
  table). Fault handler and reply field are cap-table reserved slots (D21
  pattern). Core assignment is transient (no struct field). Scheduling
  inheritance is a userspace concern (modify-scheduling mechanism, not kernel
  policy). Remaining: wait-state allocation source for multi-field, cap table
  capacity tracking placement. ~~Register save area layout~~ (already
  implemented), ~~budget encoding~~ (D57: budget 128, store R/T, derive P),
  ~~default profile~~ (D57: 43/43/42), ~~self-reference capabilities~~ (D57:
  kernel-installed at reserved slot 2).
- ~~**Address space binding mutability.**~~ Dissolved by D26: no address space
  object, no binding. Observers access Spaces through capabilities; the page
  table is updated automatically as caps are acquired and lost.
- ~~**Observer creation API shape.**~~ Settled by D35: minimal create + separate
  start + composable operations.
  `create_observer(space_cap, handler_field_cap, badge) → observer_cap [inert]`.
  PC/SP via write-registers, initial caps via general-purpose install-cap (same
  primitive as fault resolution), start via resume. All-params-upfront rejected
  (forecloses decomposition; composable primitives + userspace library is
  A5-consistent). Remaining: Observer rights model (install-cap,
  write-registers, resume confirmed as rights; complete set one level down).
- ~~**Observer rights model.**~~ Settled by D39: nine rights — resume, destroy,
  install-cap, write-registers, clone, read-registers, suspend, change-handler,
  modify-scheduling. Extract-cap excluded (proactive cap sharing via D23 + D28
  serves the use cases). Duplicate-control deferred to D8 derivation (not
  Observer-specific). Remaining downstream: Observer minimum schema (D39
  constrains state machine), ~~D2 scheduling properties~~ (settled by D42:
  three-value profile — responsiveness, throughput, precision; modify-scheduling
  gates these values), self-reference capabilities.
- ~~**Observer handle clonability.**~~ Settled by D23: clonable. Observer
  handles follow uniform capability rules (clone, attenuate, transfer)
  identically to all other kernel object types. Non-clonable rejected on five
  convergent structural arguments. Archive's "handle = handler unification"
  dissolved by D20/D21. Duplicate-control right deferred to Observer rights
  model.
- ~~**Suspend as distinct from faulted.**~~ Settled by D39: yes. External
  suspension exists as a fifth Observer state (alongside inert, runnable,
  blocked, faulted). Suspend can co-occur with blocked or faulted. Resume clears
  the suspension; underlying conditions remain. Use cases: debugging,
  checkpointing, resource pressure.
- ~~**Time reclamation on Observer destroy.**~~ Dissolved by D29: Time is a
  capability-held kernel object. Observer destroy closes the Time cap (D11 close
  semantics). If this was the last reference, the Time object is destroyed and
  its scheduling allocation returns to the per-core pool. If other caps exist
  (e.g., the creator retained a reference), the object persists.
- ~~**Time cardinality.**~~ Settled by D30: one or more. An Observer holds
  multiple independent Time caps in regular cap-table slots. Additive on same
  core — kernel maintains cached aggregate. Vocabulary revised from "exactly one
  Time" to "one or more Times." D6's "Multi-Time Observers" rejection was about
  execution streams, not resource accumulation — superseded for this
  interpretation.
- ~~**Time parameters.**~~ Settled by D36: normalized compute units. Each Time
  cap carries an integer quantity of compute units calibrated to hardware core
  capacity factors. The kernel translates to per-core scheduling time
  internally. Space = bytes, Time = compute units — both hardware-independent
  quantities with kernel-internal placement. Budget/period foreclosed by D2 +
  D30; per-core fraction foreclosed by D31 + A2 (breaks Space parallel on
  heterogeneous hardware). Remaining: unit encoding, minimum quantum, Time split
  syscall surface.
- ~~**Time clonability.**~~ Settled by D38: non-clonable. Time caps are linear —
  at most one capability reference per Time object. D30's aggregate
  (`total += cap.amount`) double-counts clones, violating the conservation
  invariant. D37's move-only donation reinforces. D16 send-once provides
  precedent. D23's universality framing narrowed: clone is a per-type right, not
  a universal meta-operation. Authority delegation for Time uses split (new
  object with a portion of the original's quantity), not clone.
- ~~**Time creation authority.**~~ Settled by D31: the kernel holds unallocated
  scheduling capacity as per-core root Time objects internally. Observers
  acquire Time through the pager chain (resource request → fault handler → ... →
  kernel). The kernel allocates from its per-core pools. Initial Time
  distributed at boot as part of the root Observer's minimal resource grant.
- ~~**Time donation on IPC.**~~ Settled by D37: explicit cap transfer via the
  D28 user cap slot on Call(). Standard move semantics. The server holds the
  donated Time (D30 multi-Time), returns it in the reply. Opt-in, no kernel
  enforcement of return. Transfers scheduling capacity (D36 compute units), not
  scheduling priority (D2 hints). Kernel-internal donation rejected (D29
  cap-graph tension). Kernel-injected dedicated field rejected (unnecessary D28
  revision). Priority-level inheritance deferred to D2.
- ~~**Can Observers share capability tables?**~~ Settled: per-Observer tables
  with no sharing (D8 confirmed). D26 resolved the most common sharing use case
  (shared memory) at the page-table level without requiring cap-table sharing.
  The remaining ergonomic pressure (threads sharing authority) is accidental
  complexity addressable by a userspace threading library — not essential
  complexity that A5 requires the kernel to absorb. Per-Observer tables satisfy
  D1 (zero synchronization on hot-path cap lookup), D4 (thread-granularity
  confused-deputy protection), D33 (destroy cascade unmodified), and D8's own
  typed-memory backing model (each table backed by its Observer's Space). Prior
  art confirms viability: EROS/KeyKOS and seL4-strict operate this way;
  userspace libraries absorb the authority-propagation pattern.
- ~~**Interrupt model (device interrupts, not exceptions).**~~ Settled by D22:
  delegation to userspace driver Observers through fields. No separate IRQ
  object type — the interrupt namespace maps onto the field namespace. The
  kernel routes hardware interrupts to fields; authority = receive cap. Ack via
  D16 send-once cap in each interrupt message. Split/combine field operations
  for IRQ range delegation (split settled by D45; combine dissolved by D45).
  Preemption timer and IPIs excluded (kernel-internal).
- ~~**Field split semantics.**~~ Settled by D45: badge-range routing with
  fallback-on-destroy. Split installs routing rules on the source Field;
  destination is a separate Field object. Generalizes beyond IRQ to all
  badge-range traffic. Fallback-on-destroy provides automatic crash recovery
  without parent tracking. Routing table structure settled by D54 (nullable
  pointer to external sorted array, root Space). Remaining: badge condition
  form, split-to-new vs. split-to-existing syscall shape, Field rights mask,
  queued message handling at split time, badge-closure tracking partitioning.
- ~~**Field combine semantics.**~~ Dissolved by D45: combine decomposes into
  split-to-existing (route traffic from Field A to Field B) + destroy (the
  now-empty A). No separate combine primitive.
- ~~**Interrupt priority and routing.**~~ Settled by journal 066:
  kernel-automatic routing (GICD_IROUTER tracked on migration and receive-cap
  transfer, following receive-cap holder), flat priority (all SPIs at same
  IPRIORITYR). D22 confirmed — no new interface surface. Priority exposure
  explicitly not foreclosed; revisit if a defined hard-RT workload requires
  simultaneous-interrupt arbitration not addressable through scheduling.
- ~~**Userspace timers.**~~ Settled by D44: Pulsar, a capability-held timer
  object with kernel-managed delivery. Fifth kernel object type. Created from
  Space (D32) with delivery field, badge, duration, period. Kernel manages
  re-arm, drift compensation, overflow. Period is EDF admission input (D42).
  Clock access per-Observer via CNTKCTL_EL1 on context switch. Remaining:
  ~~Pulsar rights mask~~ (D52), ~~creation API shape~~ (D62: single-call,
  armed-at-creation), ~~message content layout~~ (D63: badge + fire_time +
  overrun_count), ~~duration vs. absolute deadline~~ (D72: relative duration in
  nanoseconds), ~~clock access mechanism~~ (D66), clock access authority
  mechanism + default policy (genuine choices, decoupled from timer API by D72).
- ~~**Page size exposure.**~~ Settled by D25: page size is exposed. Hiding
  rejected — creates unpredictable hardware-dependent failures and security
  violations under sub-page packing. Remaining: whether the interface is fully
  page-addressed (all operations require page-aligned inputs) or ~~implicitly
  rounded~~ (settled by D60: byte values accepted, kernel rounds, PAGE_SIZE
  queryable).
- ~~**Fault handler attachment.**~~ Settled by D20: per-Observer. Each Observer
  stores its own fault handler field reference and badge.
- ~~**Pager unavailability protocol.**~~ Settled by D68: three failure modes,
  three mechanisms. Case A (dead handler Field): supervision notification at D33
  hook. Case B (unresponsive handler): cooperative escalation chain + Pulsar
  watchdog. Case C (chain terminus): kernel-autonomous destroy. Remaining:
  supervision Field mandatory vs. optional, escalation protocol standardization,
  error-faulted sub-state encoding.
- ~~**Root/bootstrap fault handling.**~~ Settled by D31: the kernel is root
  pager for hand-picked root Observer(s). Initial Spaces are fully physically
  backed (D26 + D24 — page faults can't occur on initial memory). Resource
  requests handled by kernel allocating from its pools. The kernel's policy is
  trivially simple (allocate-or-deny); real policy in userspace pagers.
- ~~**Pager reply/resume mechanism.**~~ Settled by D40: per-fault-type
  resolution via typed kernel syscalls. Resource requests (D31): install_cap +
  resume. Cap-table-full (D8): install_cap to reserved growth slot + resume
  (kernel consumes Space for table growth). VM page faults (OOB): D41 settles
  Space merge, enabling transparent demand paging — handler merges a source
  Space into the faulting Space, then resumes. Error notification remains
  available when the handler chooses not to grow. Lazy PTE population:
  kernel-internal. No kernel validation of fault resolution. install_cap +
  resume is the general-purpose pattern; D35's structural reuse holds across
  creation, resource requests, and table growth.
- ~~**D7 classification of fault traffic.**~~ Settled by D61: faults ARE IPC
  (kernel-as-sender Send() to handler Field). Standard Field queue, standard cap
  transfer, standard direct-switch. D18 pending list for overflow. Four fault
  types with specific data word assignments.
- ~~**Field overflow policy.**~~ Settled by D18: error-to-sender, deferred fault
  delivery for kernel-as-sender. No per-field policy modes.
- ~~**Coalescing / notification mechanism.**~~ Dissolved by D18: no overwrite
  means no cross-source data loss. Coalescing lives in shared memory + signaling
  (D9 shared Space caps), not in the field mechanism.
- ~~**Multi-field wait.**~~ Resolved by D19: badge fan-in (D15+D17) covers the
  common multi-source patterns (clients, faults, timers, replies on one field).
  Residual cases (structurally distinct fields) use thread-per-source. A
  stateless multi-receive syscall is planned (promoted from "not foreclosed" by
  journal 071: field_split settled as split-to-new only, multi-receive covers
  the two-Field case). Observer wait-state internals must accommodate N-field
  blocking from the initial implementation.
- ~~**Badge downstream details.**~~ D17 settles badge semantics
  (minter-assigned, mint right, opt-in per-badge tracking). All sub-items
  resolved: ~~badge size~~ (D58: u64), ~~send-once exemption encoding~~ (D73),
  ~~badge on D16 kernel-created send-once caps~~ (D65: caller-supplied
  reply_badge), ~~max-badge-count / capacity semantics~~ (growable map with hard
  ceiling; new badges beyond ceiling silently dropped), ~~badge-closure message
  format~~ (D64). (Badge-closure × overflow: resolved by D18 — dropped on full
  queue. Per-badge tracking × coalescing: dissolved by D18 — coalescing is not a
  field mechanism; per-badge map serves tracking only.)
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
  Remaining: ~~fault message content per type~~ (settled by D61),
  ~~badge-closure content~~ (settled by D64), interrupt content, ~~inspect()
  shape~~ (settled by D48/D39), ~~fast-path conditions~~ (settled by D50).
- ~~**Send-once right encoding.**~~ Settled by D51 (boolean flag on cap entry,
  not a rights bit) and D73 (exemption encoding: structural code-path
  separation, reply Field always-tracked).
- ~~**IPC fast-path conditions.**~~ Settled by D50: six conditions — operation
  is Call or ReplyRecv, same core, receiver waiting on target field, no user cap
  in message (0-cap gate), per-core scheduler approves switch (callback
  interface), field routing resolved. Scheduler callback generalizes seL4's
  priority check to work with any D2 algorithm. 0-cap gate makes D37 Time
  donation slow-path (cap-graph tradeoff accepted). Slow path can still
  direct-switch through general code. Remaining: scheduler callback interface,
  ~~Send-to-waiting-receiver "fast enqueue" optimization~~ (journal 055:
  implementation-only), ~~interrupt masking during fast path~~ (settled by D69:
  DAIF.I masking).
- ~~**Specific syscall surface.**~~ Settled by D48: 5 IPC operations (Send,
  Receive, Call, ReplyRecv, Yield) + 20 typed kernel operations = 25 total.
  NBSend rejected (redundant — Send never blocks under D13/D18). Reply rejected
  (redundant — Send to D16 send-once cap). NBRecv deferred (not foreclosed; D19
  pattern). Typed operations collected from D14, D35, D39, D41, D44, D45, D11,
  D17, D23, D31, D32. Generic cap operations (destroy, clone, close, mint) apply
  across types. Pending additions from Space/Field/Pulsar rights masks (typed
  operations only; IPC set is complete). D47 encoding details settled by D49:
  SVC assignments (#1–#5), typed op codes (grouped sequential 0–19), error
  signaling (carry flag for IPC, negative-x0 for typed), cap-present (sentinel
  u64::MAX), large return values (userspace buffer pointer).
- ~~**Address space lifecycle.**~~ Dissolved by D26: no address space kernel
  object. The page table is kernel-internal; per-Observer L0 tables are
  destroyed with the Observer; per-Space subtrees are reference-counted and
  freed when the last holder's cap is closed.
- ~~**Boot / bring-up model.**~~ Settled by D46: core lifecycle is fully
  kernel-internal. All discovered cores activate at boot (PSCI CPU_ON). Idle
  cores sleep (WFI/CPU_SUSPEND). Deactivation via CPU_OFF when conservation
  permits (unallocated pool ≥ core capacity). No userspace syscall, no Core
  object type. Cores are to Time what physical pages are to Space.
- ~~**Explicit unmap() semantics.**~~ Dissolved by D26: no explicit map() or
  unmap(). The page table is managed by the kernel based on Space cap holdings.
  Holding a cap grants access; losing a cap removes access.
- ~~**Sub-page packing under D24.**~~ Dissolved by D25 + D60: Spaces are
  page-granular (minimum size = one page, all sizes rounded to page boundaries).
  Multiple Spaces cannot share a physical page, so the cleanup concern cannot
  arise.
- ~~**Space acquisition at runtime.**~~ Settled by D31: Observers acquire Space
  through the pager chain. A resource request syscall is routed to the
  Observer's fault handler (D12 mechanism). The handler grants (from own
  holdings), denies, or escalates. The chain terminates at the kernel, which
  allocates from its internal root Space.
- ~~**Kernel-internal memory on cap transfer.**~~ Settled by D32: page table
  subtree cost is baked into the Space at split time (parent shrinks by
  child_size + overhead). First holder populates from reserved capacity;
  subsequent holders increment reference count. Per-Observer intermediate page
  table pages (L1/L2) charged via D8 fault mechanism (handler provides Space in
  fault reply). Kernel per-object metadata from root Space.
- ~~**CoreState arena references for dispatch.**~~ Settled by D75: arenas live
  in a global `KernelState` struct (not in CoreState). Cold-path dispatch code
  accesses them through the global. Lock<T> refactored to own data (UnsafeCell)
  — type-system enforcement of lock-before-access.
- ~~**Observer cap table capacity.**~~ Settled by D83: capacity stored in
  Observer metadata (hot-path access). D43's remaining item closed.
- ~~**Per-core Pulsar deadline queue.**~~ Settled by D83: fixed-capacity
  per-core deadline structure in CoreState. Timer handler checks pending
  deadlines on each preemption tick.
- ~~**IRQ-to-Field routing table.**~~ Settled by D81: kernel-wide direct-indexed
  routing table in KernelState (D82). Each route maps an INTID to a Field with
  badge and generation for stale-route detection.

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
  fields with direct-switch fast path subsume both sync and async patterns;
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
- `015-field-shape.md` — reasoning for D15: three convergent paths (D8+D11
  structural consistency, D12+D13 many-to-one composition, A3+capability
  topology) settle unidirectional many-to-many with send/receive rights;
  bidirectional (Zircon) rejected for structural exceptions to D8+D11 and
  aggregation requirement weakening D13; QNX constrained model dominated; peer
  disconnection gap addressable via badge-closure notifications (deferred to
  badge semantics).
- `016-reply-cap-mechanism.md` — reasoning for D16: pre-allocated reply field
  (regular field) with send-once cap; D14 decouples fault resume from IPC reply,
  removing archive's unification argument; send-once is general-purpose
  use-limited attenuation (Mach precedent), not reply-specific; dedicated Reply
  type rejected (optimization achievable behind field interface); archive
  convergence on same object model, refined with send-once.
- `017-badge-semantics.md` — reasoning for D17: D15's many-to-one patterns
  require sender identification; minter-assigned because identification (key
  into receiver state) requires receiver-controlled values; mint right as third
  independent right in D8's rights mask (D4 consistency, resource alignment);
  opt-in per-badge lifecycle tracking resolves A3/A4 tension (not all workloads
  need it, but those that do should not fall back to polling); five tensions
  accepted for tracked fields; archive convergence on representation and
  assignment, mint right and lifecycle tracking are new.
- `018-field-overflow-policy.md` — reasoning for D18: workload decomposition
  shows only error-to-sender is irreducible; coalescing is reducible to shared
  memory + signaling (landscape §3.2 standard pattern); D13 coalescing tension
  dissolves (no overwrite = no cross-source data loss); kernel-as-sender (D12)
  fault delivery via deferred pending list (intrusive linked list through
  Observer objects, zero allocation); badge-closure dropped on full queue
  (receiver discovers staleness lazily); archive convergence on error-to-sender.
- `019-multi-field-wait.md` — resolves D13 trigger #3: badge fan-in (D15+D17)
  covers common multi-source patterns (clients, faults, timers, replies
  consolidated onto one field); four mechanisms evaluated (no primitive, port
  set, multi-receive, field binding); no kernel primitive needed now;
  multi-receive syscall explicitly not foreclosed; Observer wait-state should
  accommodate N-field blocking for future addition.
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
  parallel D12 exactly; no separate IRQ object type — interrupts are field
  traffic; D13 commits delivery, D16 provides ack via send-once, D17 provides
  badge identification; field split/combine for IRQ range delegation; derivation
  trail: IRQControl factory → IRQ objects → fields-only, each revision
  eliminating a proposed type by applying D4/D13/D16 more thoroughly; every
  identified downside traces to a parent decision; archive convergence on
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
- `031-time-cardinality.md` — reasoning for D30: multi-Time (one or more Time
  caps per Observer). Fungibility breaks the D27 mechanical parallel, but the
  server multi-client scenario decides: a server holding Time from clients A and
  B returns each correctly without merge protocol. Costs minimal (cached
  aggregate on Observer struct, cold-path O(1) bookkeeping). D6's "Multi-Time
  Observers" rejection addressed execution streams, not resource accumulation —
  superseded. Vocabulary revised. D29 reserved-slot revised to regular slots.
  Archive convergence: archive explicitly considered "multiple Time fragments."
  Novel position (no surveyed system provides multi-time per execution unit).
- `032-resource-acquisition-and-boot.md` — reasoning for D31: resource
  acquisition through pager chain, boot architecture, root fault handling, Time
  vocabulary revision. Kernel retains resource pools (root Space, per-core root
  Time), creates root Observer with minimal resources, acts as root pager.
  Resource requests routed through D12 fault mechanism. Structural objects
  (Fields, Observers) created from Space caps. Factory caps rejected (D4
  indirection). Split-model-with-omnipotent-root rejected (security — god object
  at EL0). Time revised to abstract scheduling capacity (core assignment
  kernel-internal, A5 parallel with D9/D26). Strong archive convergence: journal
  013 derived same model independently. Time abstraction is novel.
- `033-kernel-memory-accounting.md` — reasoning for D32: kernel-internal memory
  accounting via type conversion model. Object creation = Space consumed
  entirely → becomes object backing. Destruction = reverse (object → new Space
  cap returned to destroyer). Kernel per-object metadata from root Space
  (invisible, bounded). Page table subtree cost baked into Space at split. Cap
  table growth: handler provides Space in fault reply (2A). Time destruction
  returns to kernel pool (asymmetric). Observer held-caps on destroy deferred to
  destroy cascade. Strong archive convergence (journal 013 same model).
- `034-destroy-cascade-protocol.md` — reasoning for D33: preemptible destroy
  cascade with structural-backing-only return. Object dead before cleanup (D11).
  Only Observers cascade (only they hold caps). Preemptible in bounded steps
  with saved continuation (seL4 MCS precedent). Structural backing to destroyer;
  cascade-freed to root Space (shared resources break return model). Destroy
  right in rights mask (D4). Badge-closure best-effort (D18). Partial archive
  convergence (cascade through owned resources); divergence on return
  destination (archive returns to supervisor via ownership tree; this design
  uses flat caps + root Space).
- `035-observer-creation-api.md` — reasoning for D35: minimal create + separate
  start + composable operations. Five creation models foreclosed (fork+exec,
  constructor, manifest, VSpace binding, Time-at-creation). Creation Space does
  not provide executable memory (D32 type conversion); Observer needs code Space
  cap before PC is meaningful (D26). Fault handler is a creation parameter under
  any model (D20). Cap installation is a general-purpose Observer operation
  shared with fault resolution — not creation-specific. Create-then-configure
  chosen over all-params because it forecloses nothing (all-params is a
  userspace library), introduces no new kernel surface, and syscall overhead is
  negligible on this cold path. Archive divergence: archive used all-params;
  explained by D31 (removes Time), D26 (removes VSpace), and cap-install reuse
  argument.
- `036-time-parameters.md` — reasoning for D36: normalized compute units as
  Time's parameter model. D30 + D2 + D31 foreclose budget/period (algorithm-
  specific, non-composable), no-parameter (aggregate requires quantity), and
  algorithm-specific parameters. Per-core fraction breaks the Space parallel on
  heterogeneous hardware (A2 big.LITTLE) — leaks core identity through the
  provisioning chain. Normalized compute units restore the parallel: Space =
  bytes, Time = compute units, both hardware-independent quantities with
  kernel-internal placement. Calibrated to ARM `capacity-dmips-mhz` / ACPI CPPC.
  Capacity factor is a first-order approximation (~1.2x–3.5x for a stated 2x);
  hard-RT precision on dedicated cores (D2). Strong archive convergence on the
  resource/requirements split; divergence on per-core fraction vs. compute units
  explained by D31 and A2.
- `037-time-donation-on-ipc.md` — reasoning for D37: four options evaluated
  (explicit cap transfer, kernel-internal donation, no donation, kernel-injected
  dedicated field). Explicit cap transfer via D28 user cap slot chosen —
  standard move semantics, opt-in, no kernel tracking. Kernel-internal rejected
  on D29 cap-graph completeness. Crash safety is not a kernel concern (caller
  already stuck on reply field). Time transfer is necessarily a move (D30
  aggregate double-counts clones). Donation transfers capacity (D36) not
  priority (D2). Strong archive convergence ("events-carry-resources").
- `038-time-clonability.md` — reasoning for D38: D30 aggregate soundness
  requires each cap to reference a distinct Time object; clone double-counts,
  violating conservation. D37 reinforces (clone defeats move-only donation). D16
  send-once provides non-clonable precedent. D23's universality framing narrowed
  — clone is a per-type right, not a universal meta-operation. Authority
  delegation uses split (new object), not clone (second reference). A1 parallel:
  linear Time caps map to Rust's move-only ownership.
- `039-observer-rights-model.md` — reasoning for D39: nine Observer rights. Five
  confirmed by prior derivations (resume, destroy, install-cap, write-registers,
  clone). Read-registers derived mechanically from D28. Suspend included (A3,
  100% landscape convergence). Change-handler separated from install-cap (D21
  reserved slot is already structurally special; D12 handler criticality
  justifies independent right). Modify-scheduling included (100% landscape
  convergence; gates D2 scheduling hints). Extract-cap excluded (proactive cap
  sharing via D23 + D28 covers use cases; extract compensates for policy
  failures, not mechanism gaps). Duplicate-control deferred to D8 (not
  Observer-specific). Strong archive convergence on operations.
- `040-pager-fault-resolution-protocol.md` — reasoning for D40: per-fault-type
  resolution via typed kernel syscalls. D26 transforms the question —
  install_cap IS the mapping operation (cap-table mutation triggers page table
  update). D26 also structurally limits demand paging (kernel-assigned VA bases
  mean new Spaces can't cover faulting VAs). Resource requests: install_cap +
  resume. Cap-table-full: reserved growth slot (D21 pattern) + resume; kernel
  consumes Space (D32) and retries. VM page faults: error notification (handler
  destroys or does PC surgery via write-registers). Lazy PTE population:
  kernel-internal. No new kernel surface. Archive divergence: archive unified
  fault resume with IPC reply, lacked D26.
- `041-space-merge-and-split.md` — reasoning for D41: Space merge (two → one)
  and split (one → two) as topology-changing operations. Framing shift from
  "resize" to merge/split — conservation is structural (boundaries move,
  material doesn't appear or vanish). Merge motivated by D26's demand-paging gap
  (new Spaces can't cover faulting VAs; merge extends the faulting Space
  in-place). Split already established as pattern (D31/D32/D33). Both follow D32
  conservation, require Space rights (D4/D8), operate at page granularity (D25).
  Cooperative recovery (D40 alternative) rejected on A5/O4 grounds — essential
  complexity pushed to userspace. Partial archive convergence on split
  ("distinguishable children"); archive didn't derive merge (VA-addressed model
  allowed traditional demand paging without it; D26 divergence).
- `042-scheduling-properties.md` — reasoning for D42: minimum abstract
  scheduling properties are a three-value budget — responsiveness, throughput,
  precision — sharing a fixed per-Observer point allocation. Priority integers
  dissolved (inflation problem — maximizing any dimension costs the other two).
  Each dimension has a physical trade-off. Precision captures the archive's
  tolerance spectrum with a self-enforcing per-Observer cost. CPU/IO
  kernel-inferred from profile, deadline kernel-derived from timer period +
  Time + precision. Hard RT via dedicated cores (D2) with EDF admission using
  Time + timer + precision. Archive convergence: strong on resource/preference
  split and "every parameter must have a cost." Archive's six parameters
  collapse to three because the kernel already knows period and compute budget.
- `043-observer-minimum-schema.md` — reasoning for D43: eight forced field
  clusters mechanically derived from settled decisions. Observer splits into
  metadata struct (root Space, ~80–100 bytes) and structural backing (consumed
  Space). Key decisions: no kernel-side scheduling inheritance (one R/T/P set,
  userspace manages adjustment), transient core assignment (no struct field),
  wait-state as Rust enum (inline single-field, allocated multi-field), reply
  field follows D21 cap-table pattern.
- `044-userspace-timer-interface.md` — reasoning for D44: Pulsar as fifth kernel
  object type. Virtual-field model explored and rejected (non-blocking
  composability). Precision-maximizing design explored and refined (kernel
  manages drift; one-shot for manual control). Timer object chosen over
  stateless operation (cancel authority, resource accounting). Kernel-managed
  re-arm, drift compensation, overflow via stop-re-arm-on-full. Per-Observer
  clock access via CNTKCTL_EL1. No archive convergence (archive had no userspace
  timer concept).
- `045-field-split.md` — reasoning for D45: Field split as badge-range routing
  with fallback-on-destroy. Split installs routing rules, not object
  restructuring. Combine dissolves into split-to-existing + destroy. Novel
  position (no surveyed kernel provides split/combine on IPC endpoints). No
  archive convergence.
- `046-core-activation.md` — reasoning for D46: core lifecycle is fully
  kernel-internal. All discovered cores activate at boot (PSCI CPU_ON). Idle
  cores sleep (WFI/CPU_SUSPEND); deactivation (CPU_OFF) when conservation
  permits. No userspace syscall, no Core object type. D36 fungibility makes
  deactivation a bookkeeping check (unallocated pool ≥ core capacity), not a
  revocation problem. Space parallel: cores are to Time what physical pages are
  to Space.
- `047-syscall-abi.md` — reasoning for D47: syscall ABI framework. SVC #imm16
  for operation encoding (frees all 8 argument registers). IPC-optimized uniform
  register convention (x0–x3 = data words, x4–x7 = metadata). Two-level
  numbering (nonzero SVC immediate = IPC, SVC #0 = typed ops with operation in
  x4). D28's message format exactly fills 8 ARM64 registers, making the
  discriminator placement load-bearing. Fast-path optimization: x0–x3 pass
  through in physical registers on direct switch (~20–30% of IPC fast path).
- `048-syscall-enumeration.md` — reasoning for D48: complete syscall
  enumeration. 5 IPC operations (Send, Receive, Call, ReplyRecv, Yield) + 20
  typed kernel operations = 25 total. NBSend rejected (Send never blocks). Reply
  rejected (Send to send-once cap). NBRecv deferred (not foreclosed). close,
  mint derived as explicit typed operations. inspect() reconciled as
  observer_read_registers(). Completeness verified against research §8
  irreducible set.
- `049-syscall-encoding.md` — reasoning for D49: syscall ABI encoding details.
  Error signaling: carry flag for IPC (preserves all 8 message registers; XNU
  precedent), negative-x0 for typed ops (Zircon precedent). Cap-present:
  sentinel u64::MAX. SVC assignments: #1–#5. Typed op codes: grouped sequential
  0–19. Large return values: userspace buffer pointer. Corrects D47's premature
  foreclosure of condition-flag error signaling.
- `050-ipc-fast-path-conditions.md` — reasoning for D50: IPC fast-path
  conditions. Three independent axes (scheduling check, cap transfer
  eligibility, operation scope) derived from settled decisions. Scheduler
  callback chosen over no-check (D42 authority), max-R tracker (D42-specific),
  and run-queue-empty (overly conservative). 0-cap gate from D28's "cheaply
  distinguishable" design. Call + ReplyRecv scope from D13's sender-blocks
  semantics. D37 Time donation explicitly slow-path (cap-graph tradeoff).
  Philosophy: "isolate uncertain decisions behind interfaces" applied to
  scheduling check.
- `051-routing-table-structure.md` — reasoning for D54: routing table on a Field
  is a nullable pointer to an external sorted array, allocated from root Space.
  D32 metadata pattern extended from "bounded per object" to "bounded per
  operation." Inline array rejected (arena bloat for ~10-cycle savings on a
  minority of Fields). Queue-page repurposing rejected (liveness coupling, type
  safety violation). Back-pointer list on destinations forced by D33 cleanup
  requirements.
- `052-field-destroy-routing-cleanup.md` — reasoning for D55: Field destroy
  routing-cleanup protocol. Preemptible back-pointer walk (D33 argument
  transfers), generation check on routing entries (D11 ABA pattern extended to
  kernel-internal references), IPI-requested removal for cross-core sources (D1
  hot-path isolation + O2). Inline walk rejected (bounded destroy time), lock
  rejected (D1 violation), deferred-on-send noted as Verus stepping stone.
- `053-cross-core-logic.md` — reasoning for D56: cross-core kernel mechanisms.
  Scored placement (idle status, queue depth, profile compatibility, capacity,
  affinity with decay). Steal-then-idle on idle entry. Push+pull rebalancing
  (push on timer tick, pull on idle entry). Dynamic core-type classification
  (algorithm adapts to workload). Per-core run queues forced by D1. Cross-core
  wake via mailbox + SGI + remote handler (D53 lock ordering). Boot-sized
  per-core arrays (A3). Precise combination chosen over Minimal (no rebalancing)
  and Pragmatic (two-tier) — bet that scoring overhead is recovered by improved
  utilization; interfaces make fallback painless. Landscape: no surveyed system
  combines D2 heterogeneity + D43 transient assignment + A4 reactive-only
  rebalancing.
- `064-revocation-addons.md` — reasoning for D67: D11 deferral discharged (IPC
  model settled); gap analysis identifies non-IPC cap gap (Space/Observer/Time
  revocation without destruction) as essential under A3; generation counters
  close the gap at O(1) revoke, O(1) use-site check, 8 bytes per object;
  universal (all types) over scoped (API bifurcation, phantom optimization); CDT
  rejected (separate structure, O(N) revoke, unresolved lock ordering,
  cross-core cost, gaps addressable via field-per-client + bump-and-reissue);
  Coyotos primary precedent (EROS→Coyotos replaced CDT-style with generation).
- `065-shared-cap-tables.md` — D8 confirmed: per-Observer cap tables with no
  sharing. D8's sharing revisit condition discharged — D26 resolves memory
  sharing at page-table level; authority propagation is userspace-library
  complexity (EROS/KeyKOS discipline). Shared CSpace, hybrid, and
  copy-on-reference all rejected on D1 (synchronization on hot path), D4
  (thread-granularity confused deputy), D33 (cascade unmodified), D8
  typed-memory backing.
- `066-interrupt-priority-routing.md` — settles G03: D22's two deferred GIC
  configuration sub-questions. Routing: kernel-automatic GICD_IROUTER tracking
  (follow receive-cap holder on migration and cap transfer); the kernel already
  knows SPI→Field→Observer→core, no userspace API needed. Priority: flat
  absorption (all SPIs at same IPRIORITYR); forward-compatible with future
  exposure. Every surveyed capability microkernel absorbs both; automatic
  routing tracking is novel but derived from D13 fast-path + D22 field model.
  Kernel- derived priority from D42 precision rejected (semantic mismatch,
  coupled knobs, A3 policy concern).
- `068-interrupt-masking-fastpath.md` — settles G05: IPC fast path masks IRQ via
  DAIF.I for the full ~400-cycle window. Five convergences (D50 TOCTOU
  elimination, journal 023 Verus readiness, A4 non-nesting, D1 straight-line hot
  path, Blackham et al. quantitative grounding). Unanimous prior art (seL4,
  Pistachio, Fiasco.OC, EROS, Barrelfish). Three alternatives rejected (don't
  mask, ICC_PMR_EL1 priority masking, restartable). D42 tension accepted (<4% of
  worst-case latency).
- `069-sub-page-packing.md` — settles G06: Arena<T> internal structure is a
  per-type slab allocator with page return. Copy-on-compact foreclosed (A4, D33,
  D4, SMP). Buddy foreclosed for fixed-size types (D32). One-per-page rejected
  (A3 memory cost, D1 cache locality). Grows-never-shrinks rejected (A3
  long-lived server memory stranding). Slab wins on both performance and
  behavioral correctness. Prior art: Zircon, Linux SLUB, QNX.
- `070-badge-condition-form.md` — settles D71: badge condition form is range
  (`low <= badge <= high`). Three convergences (D54 binary search compatibility,
  expressive sufficiency for common allocation patterns, incumbent). Bitmask
  foreclosed (O(N) lookup breaks D54; disjointness verification harder).
  Predicate foreclosed (A5 + D1). Also closes D44's deferred "badge-filtered
  receive" — D45 routing serves the use case; filtering tensions D13 queue
  semantics, D18 overflow, D50 fast-path. No surveyed kernel has badge-range
  receive filtering for queue-based IPC.
- `072-pulsar-deadline-form.md` — settles D72 (closes G09): `create_pulsar`
  duration parameter is a relative offset in nanoseconds. Kernel absorbs
  relative-to-absolute conversion (D66 anti-pattern: don't make callers provide
  information the kernel already has). Common case one syscall for all Observers
  regardless of clock access. Precision one-shot loops pay one clock read.
  Absolute mode (flag bit) not foreclosed — additive, non-breaking.
- `073-send-once-exemption-encoding.md` — settles D73 (closes G10): send-once
  exemption is structural code-path separation. Consume-on-delivery (used path)
  is a separate operation from D11 close (unused path); badge-closure lives only
  in D11 close. No extra data, no branch on the hot path. Reply Field
  always-tracked (D17 specialization for reply routing correctness under A4).
  Per-Field exemption policy (Option C) not foreclosed — additive if a concrete
  authorization-audit workload motivates it.
- `074-register-save-restore-flow.md` — reasoning for D74: register save/restore
  flow between exception entry and persistent per-Observer state. Three options
  evaluated: TrapFrame intermediate (A, rejected as tech debt), direct-to-
  RegisterState on EL0 (B, chosen), fast-path specialization with deferred save
  (C, rejected — complexity disproportionate to ~2–5% cycle gain, breaks D39
  read-registers). TPIDR_EL1 as per-core state pointer (standard ARM64
  convention). x0–x3 saved unconditionally (RegisterState always correct),
  restored conditionally (fast-path pass-through per D47).
- `075-global-arena-organization.md` — reasoning for D75: five arenas + Space
  manager in a global KernelState struct. Lock<T> refactored to own data via
  UnsafeCell (A1 type-system enforcement). Parameter threading rejected (pushes
  leaf concern into interfaces). Five separate statics rejected (scatters change
  points). Per-core copies not foreclosed but would require reopening D53.
  Consistent with "push complexity to the leaves" and "isolate uncertain
  decisions behind interfaces."
- `076-dispatch-entry-contract.md` — reasoning for D76: dispatch entry contract.
  Pull registers from RegisterState, push results back, three-variant
  DispatchResult (Resume, ResumeFastPath, Idle). Frame/ boundary between
  register manipulation and dispatch logic.
- `077-cap-resolution-protocol.md` — reasoning for cap resolution: handle →
  entry lookup with bounds check, generation check, rights check. The protocol
  that every typed operation and IPC path uses to validate capabilities.
- `078-ipc-message-ownership.md` — reasoning for D78: IPC message ownership.
  Explicit transfer through return types — outcome structs carry messages and
  Observer pointers. Prevents use-after-move and double-delivery at the type
  level.
- `079-scheduling-decision-matrix.md` — reasoning for D79: state transitions and
  dispatch results per IPC outcome. Systematic matrix enumerating every
  send/receive/call/reply-receive combination against Observer state and
  scheduling decision.
- `080-error-fault-delivery.md` — reasoning for D80: error and fault delivery
  protocol. Two paths: error-to-registers for syscall failures, fault-as-IPC for
  exceptions. Fault cap constructed with 5-right subset.
- `081-hardware-event-protocol.md` — reasoning for D81: hardware event protocol.
  Kernel-wide IRQ routing table (D22), per-core deadline checking for Pulsars
  (D44). Per-core IRQ routing rejected.
- `082-global-state-organization.md` — reasoning for D82: global state
  organization. Bundled shared kernel state with independent locks (D53). Lazy
  init rejected (boot-time sufficient). Per-core copies rejected (inflation).
- `083-per-core-data-organization.md` — reasoning for D83: per-core data
  organization. Two-layer split: fixed-layout assembly-visible structure at
  TPIDR_EL1, richer scheduler state behind pointer indirection. Fixed-capacity
  deadline structure for Pulsar timer checking.
- `084-el0-exception-entry-mechanics.md` — implementation mechanics for EL0
  exception entry. Bootstrap sequence for register save via TPIDR_EL1.
  Realization of D74 + D83.
- `085-context-switch-restore-sequence.md` — implementation mechanics for
  context switch restore. TTBR0 switch sequence, clock access restore, GPR
  restore ordering. Realization of D74 + D66.
- `086-svc-decode-el0-dispatch.md` — implementation mechanics for SVC decode.
  ESR_EL1 classification, dispatch routing for syscalls and faults. Realization
  of D47 + D48 + D49.
- `087-ipc-fast-path-mechanics.md` — implementation mechanics for IPC fast-path
  register pass-through. Deferred: marginal savings relative to complexity.
  Realization of D50 + D74.
- `088-ttbr-split-contract.md` — reasoning for D88: virtual address space
  partitioning. Kernel upper-half (TTBR1, 2-level), user lower-half (TTBR0,
  3-level). E0PD1 for speculative access prevention.
- `089-per-observer-page-table.md` — reasoning for D89: per-Observer page table
  structure. Three-level with shared per-Space leaf tables. O(Observers +
  Spaces) memory scaling.
- `090-pte-population-policy.md` — reasoning for D90: eager PTE population.
  Demand faulting rejected on cost and determinism. Hybrid rejected on
  complexity.
- `091-cap-to-mapping-protocol.md` — reasoning for D91: page table mutations on
  cap install/close. Intermediate-level operations only; leaf tables shared
  immutably. Last-cap-close triggers unmap.
- `092-page-table-memory-accounting.md` — reasoning for D92: which Space backs
  each page table level. Root from consumed Space (D35), intermediate from root
  pool (D31), leaf from type conversion overhead (D32).
- `093-boot-memory-and-multicore-init.md` — reasoning for D93: boot memory
  partitioning and secondary core activation sequence.
- `094-root-observer-bootstrap-protocol.md` — reasoning for D94: root Observer
  creation from DTB-discovered binary with initial resource allocation.
- `095-object-creation-protocols.md` — reasoning for D95: Observer, Field, and
  Pulsar creation protocols. Reserved slot population, composable setup.
- `096-ipc-cap-transfer-mechanics.md` — reasoning for D96: reply cap and user
  cap transfer during IPC. Move semantics for user caps, kernel-created
  send-once for reply.
- `097-cap-table-self-mutation-and-mapping-bridge.md` — reasoning for D97:
  clone, close, mint, install-cap, write/read-registers, change-handler
  protocols. D24 mapping bridge on Space cap close.
- `098-destroy-cascade-and-return.md` — reasoning for D98: preemptible destroy
  cascade with structural backing return. Continuation state for cross-batch
  preemption.
- `099-hardware-event-wiring.md` — reasoning for D99: IRQ delegation via
  FieldSplit routing table updates. Pulsar deadline installation in creating
  core.
- `100-fault-delivery-mechanics.md` — reasoning for D100: fault message register
  layout, fault Observer cap rights, kernel-as-root-fault-handler terminus.
- `101-asid-assignment-and-tlb-invalidation-policy.md` — reasoning for D101:
  sequential ASID assignment, no recycling, wrap triggers full broadcast. Per-VA
  vs per-ASID TLB invalidation threshold.
- `102-test-infrastructure-and-bootstrap-patterns.md` — reasoning for D102: flat
  binary test format, multi-Observer bootstrap sequence, IPC setup pattern.
- `103-write-read-registers-inline-protocol.md` — reasoning for D103: inline
  register transfer via syscall arguments. Buffer-based transfer rejected (leaks
  internal layout, expensive for common operations).
- `104-resource-request-dispatch.md` — reasoning for D104: dual-path resource
  request dispatch. Non-root faults upward; root handled directly by kernel.
- `105-pager-chain-no-kernel-stack-recursion.md` — observation for D105: pager
  chain does not recurse on kernel stack. Liveness under perpetually-faulted
  handlers remains open.

### D93 — Boot memory and multi-core initialization

Arena pages are drawn from the SpaceManager's root pool (D70, D31). Boot
resolves the circular dependency by sequencing: BSP constructs SpaceManager with
root pool from DTB-discovered RAM, then empty arenas, bundles into KernelState,
calls `frame::init_kernel_state()`. Arenas start empty; first allocation
triggers slab page request. Physical memory partitioning: DTB memory nodes minus
kernel image, DTB blob, and initial binary = root pool. Secondary cores
activated via PSCI CPU_ON after KernelState is complete; each initializes
PerCoreData/CoreState with RoundRobin scheduler and enters idle via WFI.

- **Rests on:** D31, D46, D70, D75, D82, D83, D1, D2, D59, A2, A4.
- **Status:** settled.
- **Journal:** `journal/093-boot-memory-and-multicore-init.md`.

### D94 — Root Observer bootstrap protocol

The initial binary is discovered from a DTB module node (not embedded in the
kernel image — preserves A3 generic). Flat binary format: entry at offset 0, no
ELF parser in kernel. Root Observer receives modest initial resource allocation;
kernel retains majority as root pool for arena growth and pager-chain grants.
Kernel assigns VA bases (D26), creates TTBR0 page table, maps code and stack
Spaces directly during boot. Initial registers: PC = VA base, SP = stack top, x0
= initial cap count, x1–x7 = 0. UART device-memory Space cap provided for serial
debug. Test exit via deliberate fault: D68 chain terminus triggers PSCI
SYSTEM_OFF with exit code.

- **Rests on:** D31, D26, D32, D35, D46, D68, D88, A3, A4, A5.
- **Status:** settled.
- **Journal:** `journal/094-root-observer-bootstrap-protocol.md`.

### D95 — Object creation protocols

CreateObserver: `create_observer(space_cap, handler_field_cap, badge)` →
observer_cap (inert). Space consumed entirely for structural backing (cap table,
L1 page table root, RegisterState). Observer metadata from root pool (D32).
Reserved slots populated: 0 = handler, 1 = reply (empty), 2 = self-cap (D57).
Composable setup via D35. CreateField: `create_field(space_cap)` → field_cap.
Queue capacity derived from Space size. CreatePulsar:
`create_pulsar(space_cap, field_cap, badge, duration_ns, period_ns)` →
pulsar_cap (armed at creation, D62). Deadline installed in creating Observer's
current core (D83 array, max 32).

- **Rests on:** D32, D35, D43, D57, D13, D44, D62, D72, D83, D89, D92.
- **Status:** settled.
- **Journal:** `journal/095-object-creation-protocols.md`.

### D96 — IPC cap transfer mechanics

Reply cap: kernel creates send-once Entry pointing at caller's reply Field (slot
1), installs in receiver's table via allocate_slot. User cap: move semantics —
removed from sender's table, installed in receiver's (D30 over-allocation
invariant forces move). Cap slot allocation: kernel picks via freelist; table
full → fault receiver (D40), handler provides Space for growth. DirectSwitch
denial: no enum change — Message constructed from sender's saved registers at
the denial point.

- **Rests on:** D16, D28, D30, D37, D43, D47, D50, D51, D74, D78, D8, D40.
- **Status:** settled.
- **Journal:** `journal/096-ipc-cap-transfer-mechanics.md`.

### D97 — Cap table self-mutation and mapping bridge

Clone: duplicate Entry via allocate_slot + install_at; Time forbidden (D38).
Close: Table::close() + D24 mapping bridge (Space cap close triggers
unmap_space_from_observer + TLB invalidation). Mint: attenuated cap via rights
intersection + minter-assigned badge (D17). ObserverInstallCap: install source
cap into target Observer's table; Space caps additionally trigger
map_space_in_observer (D24 invariant). ObserverWriteRegisters/ReadRegisters:
batch operation on full 816-byte RegisterState; target must be stopped.
ObserverChangeHandler: overwrites SLOT_FAULT_HANDLER; old handler NOT
auto-closed (D4).

- **Rests on:** D4, D8, D11, D17, D21, D24, D26, D33, D35, D38, D39, D51, D91.
- **Status:** settled.
- **Journal:** `journal/097-cap-table-self-mutation-and-mapping-bridge.md`.

### D98 — Destroy cascade and return

Preemptible cascade (D33): kernel runs N cascade_step() calls per batch, checks
for pending timer interrupt between batches. Continuation state saved in
CoreState: (ObjectId, cursor). Destroying Observer is blocked; other Observers
on the same core run between batches. Destroy return: structural backing becomes
new Space cap in destroyer's table (D32 reverse type conversion). Cascade-freed
objects return backing to kernel root pool (D31).

- **Rests on:** D11, D31, D32, D33, D8, A3, A4.
- **Status:** settled.
- **Journal:** `journal/098-destroy-cascade-and-return.md`.

### D99 — Hardware event wiring

IRQ delegation: kernel populates IrqRoutingTable at boot with all device INTIDs
routing to root interrupt Field (badge = INTID). Delegation via FieldSplit
(D45): split updates routing table entries for affected badge range. IRQ
acknowledgment: kernel reads IAR, constructs message, enqueues, writes EOI. No
userspace ack needed (A5). Pulsar deadline installation: installed in creating
Observer's current core's deadline array (D83). Array full → CreatePulsar fails.

- **Rests on:** D22, D44, D45, D62, D67, D72, D81, D83, A5.
- **Status:** settled.
- **Journal:** `journal/099-hardware-event-wiring.md`.

### D100 — Fault delivery mechanics

Fault message register layout follows D28 + D47: x0–x3 = data words per D61
fault type, x4 = label, x5 = badge, x6 = fault Observer cap, x7 = CAP_ABSENT.
Fault Observer cap: kernel constructs TransferredCap with 5-right subset
(RESUME, DESTROY, INSTALL_CAP, WRITE_REGISTERS, READ_REGISTERS). Installed in
handler's table. Kernel-as-root-fault-handler (D68 chain terminus): log fault to
serial, PSCI SYSTEM_OFF.

- **Rests on:** D12, D21, D28, D39, D47, D49, D61, D68, D80, A5.
- **Status:** settled.
- **Journal:** `journal/100-fault-delivery-mechanics.md`.

### D101 — ASID assignment and TLB invalidation policy

Sequential ASID assignment from kernel counter; maximum hardware width (16-bit
where supported). No recycling — sequential avoids ABA on stale TLB entries.
Wrap triggers full TLB broadcast (TLBI VMALLE1IS) and counter reset. TLB
invalidation on Space unmap (D24): per-VA (TLBI VAE1IS) when page_count <=
threshold; per-ASID (TLBI ASIDE1IS) for bulk. Always IS variant for cross-core
broadcast. DSB ISH for completion.

- **Rests on:** D5, D24, D25, D26, D46, D56, D88, D89, D91.
- **Status:** settled.
- **Journal:** `journal/101-asid-assignment-and-tlb-invalidation-policy.md`.

### D102 — Test infrastructure and bootstrap patterns

Flat binary format (entry at offset 0, no ELF). Hypervisor loads test binary
into guest RAM, describes via DTB module node. Kernel binary unchanged across
tests. Multi-Observer bootstrap: 5-step D35 composable sequence (SpaceSplit →
CreateObserver → InstallCap → WriteRegisters → Resume). IPC setup: root creates
Field, installs send cap in child, keeps receive cap.

- **Rests on:** D24, D26, D31, D32, D35, A3, A5.
- **Status:** settled.
- **Journal:** `journal/102-test-infrastructure-and-bootstrap-patterns.md`.

### D103 — WriteRegisters/ReadRegisters: inline register protocol

Registers are transferred inline via syscall argument registers (PC, SP, x0,
PSTATE masked to NZCV only) rather than through a memory buffer. This decouples
the kernel's internal RegisterState layout from the ABI, avoiding leaks across
version changes. Buffer-based full RegisterState transfer rejected: leaks
internal struct layout, requires expensive Space cap resolution and page table
walk for common 2–4 register operations. Inline with more registers (x0–x7)
rejected: not needed for current use cases. Unmasked PSTATE write rejected:
privilege escalation risk (PSTATE.M bits allow userspace to set exception return
level to EL1).

- **Rests on:** D35, D39, D47, D97, A5.
- **Status:** settled.
- **Journal:** `journal/103-write-read-registers-inline-protocol.md`.

### D104 — ResourceRequest dispatch: dual-path implementation

Non-root Observers fault upward through the pager chain via standard D80 fault
delivery. Root Observer ResourceRequests are handled directly by the kernel via
SpaceManager pool allocation — the distinction is structural, not a policy
choice. Fault delivery to kernel-internal Field rejected: unnecessary
indirection since the kernel can allocate synchronously. Always fault upward
rejected: no handler exists above root (D31 designates kernel as root's resource
provider). Unified path with handler-presence check rejected: conflates fault
message construction with resource pool management.

- **Rests on:** D12, D21, D31, D61, D80, D100.
- **Status:** settled.
- **Journal:** `journal/104-resource-request-dispatch.md`.

### D105 — Pager chain: no kernel-stack recursion

The pager chain does not recurse on the kernel stack. `deliver_fault()` enqueues
and returns immediately; each supervision level unrolls through a separate
scheduling round. Liveness under perpetually-faulted (but not destroyed)
handlers remains an open question — such handlers consume arena capacity without
triggering D68's cap-invalidation detection. Possible future resolution via
Pulsar watchdog (D44, D68 pattern), but kernel-level timeout versus userspace
supervision policy is not yet settled.

- **Rests on:** D12, D31, D44, D68, D80, D100.
- **Status:** observation (partial) — liveness open.
- **Journal:** `journal/105-pager-chain-no-kernel-stack-recursion.md`.

---

## Research

See `design/research/` for descriptive prior-art studies and
`design/landscape.md` for the survey of how other kernels resolved each major
design decision.
