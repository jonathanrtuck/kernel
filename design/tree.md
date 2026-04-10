# Design Tree

The system at increasing levels of resolution. Each section decomposes
a black box from the level above into interfaces between smaller boxes.
The interfaces are the design. The boxes are opaque until the next level.

Exploration notes and rejected alternatives live in `design/journal/`.

---

## Level 0 — The System

```text
hardware → [ kernel ] → userspace
```

The kernel is a single black box between hardware and userspace.

### Axiom: Rust

The kernel is written in Rust (nightly, no_std). This is not a
language preference — it is a design input. Rust's ownership model,
type system, trait abstractions, and unsafe boundaries are tools the
design can lean on. Resource lifecycle maps to drop semantics.
Architecture abstraction maps to traits. Trust boundaries map to
unsafe boundaries.

### hardware | kernel

The kernel requires hardware that provides:

- An **MMU** for memory isolation (page tables, virtual addressing)
- A **timer** for preemption (one-shot deadline programming)
- An **interrupt controller** for device and fault delivery
- **Exception levels** (or equivalent privilege separation)
- **Physical RAM** and **CPU cores**

The kernel programs these directly. It complements hardware isolation —
the MMU is the sole enforcement mechanism. The kernel never replicates
hardware protection at a different granularity in software.

The current target is ARM64 (generic timer, GIC, EL0/EL1). The
codebase is structured for portability (`src/arch/`); x86_64 is an
anticipated future target. Architecture-specific details live behind
trait interfaces — they do not shape the kernel's design.

Hardware assumptions: fixed RAM (no hot-add). Core topology and NUMA
awareness are open questions — not yet needed, and likely addressable
as leaf-node concerns behind the scheduling and memory interfaces if
they arise.

### kernel | userspace

The kernel is a leaf node behind this interface. Its internals
(scheduling algorithm, page table format, allocator design) do not
leak. The interface surface is: syscalls, ABI (register and calling
conventions), fault delivery, and a boot protocol.

The kernel is generic — it makes no assumptions about the OS or
workload built on top. Personal devices, servers, embedded systems
should all be viable. Workload-specific policy belongs above this
interface, not in the kernel.

Where the boundary sits — decisions about what is on which side. These
follow from the philosophy (push complexity to leaves, architecture is
the interfaces, mechanism vs. policy) but are still choices. We start
here and see if they hold.

- **Page size is hidden.** The memory interface operates on byte-
  addressed objects, not pages. The MMU's page granularity is an
  implementation detail on the kernel side.

- **Program loading is userspace.** ELF parsing, address layout, and
  dynamic linking are on the userspace side. The kernel provides
  generic primitives: create memory, map at address with permissions.

- **Scheduling is kernel-owned.** The kernel owns both scheduling
  mechanism (timer, context switch) and policy (who runs when). The
  scheduling algorithm is a leaf node inside the kernel — swappable
  for different workloads without changing the kernel's interfaces.
  Userspace builds M:N threading on top.

- **Mechanisms are irreducible; policy is a separate choice.**
  Hardware-required mechanisms (context switch, page table manipulation,
  exception dispatch) are always in the kernel. Whether the kernel also
  owns the _policy_ layered on each mechanism is evaluated per-mechanism.
  Scheduling: kernel-owned (above). Other policies: TBD as they arise.

### Open questions at this level

None currently. The kernel's external shape is well-constrained by
hardware requirements and the axioms.

Note on scope: the kernel is generic but not infinitely general. We
don't design for NUMA, core hot-plug, or exotic hardware — but we
avoid decisions that would make them artificially hard to add later.
If the interfaces are clean, these concerns should be addressable as
leaf nodes by anyone who needs them. We won't go out of our way to
guarantee that, but we'll notice if we're painting ourselves into a
corner.

---

## Level 1 — Inside the Kernel

Journal: `design/journal/001-level1-exploration.md`,
`design/journal/002-communication-flows.md`,
`design/journal/003-component-boundaries.md`,
`design/journal/004-context-relationships.md`

Research: `design/research/context-relationships.md`

### Three irreducible responsibilities

The kernel exists because hardware restricts certain operations to EL1:
programming the MMU and timer, receiving exceptions, and crossing the
privilege boundary (save/restore/eret). These explain _why_ the kernel
exists but don't dictate its internal structure.

### Foundational observations

- **The kernel is purely reactive.** It only runs in response to
  hardware exceptions. Exception delivery (entry/exit protocol) is a
  hardware-imposed interface, not a kernel component.

- **Contexts are the data, not a component.** A Context (execution
  context) is the central entity: register set, address space, CPU
  time allocation, pending messages. The kernel's components are
  defined by which aspect of Context state they manage.

- **Three output types.** Every kernel invocation produces some
  combination of: (1) update kernel state, (2) deliver a message to
  a Context, (3) choose which Context to resume.

- **All information delivery is one mechanism.** Faults, interrupts,
  IPC, and syscall return values are all instances of the same thing:
  the kernel making data available to a Context. A message has source,
  type/metadata, and payload. Messages are small (register-sized);
  bulk data transfer uses shared memory. See journal 002.

### Component map

```text
              hardware exceptions
                     |
                     v
                 [ Reactor ]
                /     |     \
               v      v      v
    [ Space manager ] | [ Scheduler ]
                      |
               Context model
          (shared data structure)
```

- **Reactor** — the spine. The exception handler that decodes events,
  resolves names (for IPC), updates the Context model, and delegates
  to the Space manager and Scheduler. Most exception types are short,
  straight-line code paths through the reactor.

- **Space manager** — manages per-Context address spaces, programs
  page tables. Leaf node behind the reactor. Interface: resolve faults,
  map/unmap regions, create/destroy address spaces, share memory
  between Contexts. Physical page allocation is internal (Level 2).

- **Scheduler** — `pick()` → which Context to resume. Programs the
  timer for preemption. Reads the Context model anonymously —
  property-based selection, not identity-aware. The naming scheme
  is entirely the reactor's concern; the scheduler is decoupled from
  it. Leaf node behind the reactor. Time allocation is internal
  (Level 2). Scheduling algorithm is a swappable leaf node inside it.

- **Context model** — the shared data structure through which
  components communicate. The reactor writes to it; the scheduler
  reads it; the Space manager writes TTBR values into it. The schema
  of the Context record defines the interfaces. This is closer to a
  blackboard architecture than a call graph.

### Resolved questions

- **Communication is not a separate component.** It is the reactor
  updating the Context model (pending message state, payload in
  registers) and calling `pick()`. Messaging and scheduling are
  structurally the same activity: pick a Context, update some state.
  See journal 003.

- **Space manager and Scheduler are separate.** Separable state
  (address space vs. CPU time), substantial independent complexity,
  interfaces meaningfully simpler than their implementations. They
  communicate through the Context model (e.g., Space manager writes
  TTBR, Scheduler reads it at context switch). See journal 003.

- **Allocators are Level 2.** Space allocator is internal to the
  Space manager (its only client). Time allocator is internal to the
  Scheduler (its only client). Both are real components with real
  logic, but invisible at Level 1. See journal 003.

- **Naming is capability-based.** Capabilities bundle designation
  with authority — holding a capability IS the name AND the
  permission. No global namespace in the kernel. The reactor
  resolves capabilities, not names. See journal 004.

- **Context relationships: allow shape, don't enforce it.** The
  kernel does not impose a relationship structure (no required tree,
  no hierarchy). The capability graph IS the relationship graph.
  The kernel provides mechanism for structure (fault handler
  capabilities, communication capabilities) and makes it natural to
  build structure (creating a Context requires providing a fault
  handler capability), but doesn't constrain the topology. A tree,
  a flat pool, a DAG — all valid wirings. See journal 004.

- **Fault routing via capability chains.** Each Context has a fault
  handler capability. The kernel follows the chain: if a handler
  faults, deliver to its handler. Terminal case: no handler means
  the Context dies. This is strictly more general than a tree —
  any escalation topology is expressible. Kernel provides mechanism;
  userspace provides wiring. See journal 004.

- **Resource accounting is contingent.** Space is finite and
  conserved. Time is a flow. Each Context has some Space (must —
  instructions live somewhere) and receives some Time (the scheduler
  directs it). Per-Context limits, budgets, and accounting are
  design choices, not axioms. They solve specific problems (denial
  of service, fairness, QoS) and must be justified before entering
  the model. See journal 004.

### Context model schema (derived minimum)

Only fields derived from the design — no contingent additions:

- **Register state** — saved/restored at context switch
- **TTBR** — address space root (written by Space manager)
- **Runnable / blocked** — minimum scheduling input
- **Fault handler capability** — who receives this Context's faults
- **Pending message state** — source, type, payload (in registers)

Additional fields (priority, time budget, memory limit) are
contingent and enter only when justified by specific design problems.

### Open questions (now Level 2)

- **Capability representation.** How are capabilities stored and
  resolved? Per-Context table, CNode graph, or simpler?
- **Message shape.** Concrete register layout for the message
  primitive (source, type, payload). How many registers?
- **Scheduling algorithm.** What properties beyond runnable/blocked?
  This determines whether additional fields enter the Context model.
- **Space manager internals.** Page table format, allocator design.
- **SMP.** Multiple concurrent reactors, Context model
  synchronization.
- **Whether limits/budgets/accounting are needed.** If so, at what
  granularity and who controls them.
