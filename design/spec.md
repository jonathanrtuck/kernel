# Kernel Design Specification

The current state of the kernel's design. Settled decisions with brief
rationale. See `design/graph.d2` for the structural map and `design/journal/`
for full exploration history.

---

## Axioms

These are design inputs, not decisions. They constrain everything that follows.

- **Rust (nightly, no_std).** Not a language preference — a design input.
  Ownership maps to resource lifecycle. Traits map to architecture abstraction.
  Unsafe boundaries map to trust boundaries.

- **ARM64 target.** Generic timer, GIC, EL0/EL1. The codebase is structured for
  portability (`src/arch/`); architecture-specific details live behind trait
  interfaces and do not shape the design.

- **The kernel is generic.** No assumptions about the OS or workload. Personal
  devices, servers, embedded — all viable. Workload-specific policy belongs in
  userspace.

---

## Foundational observations

Principles derived from the hardware constraints and design philosophy. These
are not decisions — they are consequences.

- **The kernel is purely reactive.** It only runs in response to hardware
  exceptions. There is no kernel thread, no event loop. The exception vector is
  the entry point.

- **Contexts are data, not a component.** A Context (execution context) is the
  central entity. The kernel's components are defined by which aspect of Context
  state they manage.

- **Three output types.** Every kernel invocation produces some combination of:
  (1) update kernel state, (2) deliver a message to a Context, (3) choose which
  Context to resume.

- **All information delivery is one mechanism.** Faults, interrupts, IPC, and
  syscall return values are all instances of the same thing: the kernel making
  data available to a Context. A message has source, type/metadata, and payload.
  Messages are small (register- sized); bulk data transfer uses shared memory.
  (Journal 002.)

- **The kernel is a leaf node.** From the philosophy: push complexity to the
  leaves. The kernel IS the leaf behind the kernel|userspace interface. Simple
  interface, arbitrarily complex internals. (Journal 002.)

---

## External interfaces

### hardware | kernel

The kernel requires:

- An **MMU** for memory isolation (page tables, virtual addressing)
- A **timer** for preemption (one-shot deadline programming)
- An **interrupt controller** for device and fault delivery
- **Exception levels** (or equivalent privilege separation)
- **Physical RAM** and **CPU cores**

The kernel programs these directly. The MMU is the sole enforcement mechanism —
the kernel never replicates hardware protection at a different granularity in
software.

Hardware assumptions: fixed RAM (no hot-add). Core topology and NUMA are open
questions — likely addressable as leaf-node concerns if they arise.

### kernel | userspace

The interface surface: syscalls, ABI (register and calling conventions), fault
delivery, and a boot protocol. The kernel's internals do not leak.

Boundary decisions:

- **Page size is hidden.** The memory interface operates on byte- addressed
  objects, not pages. The MMU's page granularity is an implementation detail. No
  surveyed system does this — it is genuinely novel. (Landscape.md §2.7.)

- **Program loading is userspace.** ELF parsing, address layout, dynamic linking
  are above the interface. The kernel provides generic primitives: create
  memory, map with permissions.

- **Scheduling is kernel-owned.** Both mechanism (timer, context switch) and
  policy (who runs when). The algorithm is a swappable leaf node inside the
  kernel. Userspace builds M:N threading on top.

- **Mechanisms are irreducible; policy is a separate choice.** Hardware-required
  mechanisms are always in the kernel. Whether the kernel also owns the policy
  layered on each mechanism is evaluated per-mechanism.

---

## Internal structure

See `design/graph.d2` for the visual map.

### Components

- **Reactor** — the spine. Decodes exceptions, resolves capabilities (for IPC),
  updates the Context model, delegates to the Space manager and Scheduler. Most
  exception types are short, straight- line code paths. (Journal 003.)

- **Space manager** — manages per-Context address spaces, programs page tables.
  Interface: resolve_fault, map, unmap, create_space, destroy_space, share.
  Contains the Space allocator (tracks physical pages, interfaces with hardware
  memory). (Journal 003.)

- **Scheduler** — `pick()` → which Context to resume. Programs the timer. Reads
  the Context model anonymously — property-based selection, not identity-aware.
  Contains the Time allocator (tracks CPU capacity, interfaces with hardware
  cores). Scheduling algorithm is a swappable leaf node inside. (Journal 003.)

- **Context model** — the shared data structure through which components
  communicate. The reactor writes to it, the scheduler reads it, the Space
  manager writes TTBR values. Closer to a blackboard architecture than a call
  graph. (Journal 003.)

### Why these boundaries

Each boundary was stress-tested: is the interface simpler than the
implementation? Does it have more than one client? Would inlining lose anything?
(Journal 003.)

- "Update kernel state" failed the test — single client, interface as complex as
  the code. Not a component.
- Space manager and Scheduler passed — substantial independent complexity,
  interfaces meaningfully simpler than implementations.
- Allocators failed at this level — single client each. They're real components
  inside their managers, visible when those boxes are opened.
- Communication failed — messaging and scheduling are structurally the same
  activity (pick a Context, update state). Not a separate component.

### Context model schema

Derived minimum — only fields that follow from the design:

- **Register state** — saved/restored at context switch
- **TTBR** — address space root (written by Space manager)
- **Runnable / blocked** — minimum scheduling input
- **Fault handler capability** — who receives this Context's faults
- **Pending message state** — source, type, payload (in registers)

Additional fields (priority, time budget, memory limit) are contingent. They
enter only when justified by a specific problem. (Journal 004.)

---

## Cross-cutting decisions

### Naming

**Capability-based.** Capabilities bundle designation with authority — holding a
capability IS the name AND the permission. No global namespace in the kernel.
The reactor resolves capabilities, not names. (Journal 004,
research/context-relationships.md.)

### Context relationships

**Allow shape, don't enforce it.** The kernel imposes no relationship structure
— no required tree, no hierarchy. The capability graph IS the relationship
graph. The kernel makes it natural to build structure (creating a Context
requires providing a fault handler capability) but doesn't constrain the
topology. A tree, a flat pool, a DAG — all valid. (Journal 004.)

### Fault routing

**Capability chains.** Each Context has a fault handler capability. The kernel
follows the chain: if a handler faults, deliver to its handler. Terminal case:
no handler means the Context dies. Strictly more general than a tree — any
escalation topology is expressible. Kernel provides mechanism; userspace
provides wiring. (Journal 004.)

### Messages

**Small (register-sized).** All information delivery is one mechanism. Bulk data
uses shared memory mapped by the Space manager. The message primitive is
source + type/metadata + payload. Concrete register layout is an open question.
(Journal 002.)

### Resource accounting

**Contingent.** Space is finite and conserved. Time is a flow. Each Context has
some Space (must — instructions live somewhere) and receives some Time (the
scheduler directs it). Per-Context limits, budgets, and accounting are design
choices that solve specific problems (denial of service, fairness, QoS). They
are not inherent and must be justified before entering the model. (Journal 004.)

---

## Open questions

- **Capability representation.** How are capabilities stored and resolved?
  Per-Context table, CNode graph, or simpler?
- **Message shape.** Concrete register layout for the message primitive. How
  many registers?
- **Scheduling algorithm.** What properties beyond runnable/blocked? Determines
  whether additional fields enter the Context model.
- **Space manager internals.** Page table format, allocator design.
- **SMP.** Multiple concurrent reactors, Context model synchronization.
- **Whether limits/budgets/accounting are needed.** If so, at what granularity
  and who controls them.

---

## Journal index

- `001-level1-exploration.md` — initial component identification from hardware
  interfaces
- `002-communication-flows.md` — flows-first methodology, message unification,
  kernel as leaf node
- `003-component-boundaries.md` — boundary stress-testing, reactor
  identification, messaging/scheduling convergence
- `004-context-relationships.md` — naming, relationships, fault routing,
  first-principles resource accounting
- `005-smp.md` — multicore exploration, hybrid model, per-core schedulers,
  IPI coordination

## Research

- `research/context-relationships.md` — prior art on process models, naming,
  fault routing, resource accounting
- `research/smp.md` — multicore design space, Barrelfish analysis, IPI
  benchmarks, per-core scheduling
