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

Journal: `design/journal/001-level1-exploration.md`

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
  time allocation, communication routing. The kernel's components are
  defined by which aspect of Context state they manage.

### Components identified

- **Space allocator** — tracks physical pages (free/used). Leaf node.

- **Time allocator** — tracks CPU capacity and its subdivision among
  Contexts. The core abstraction (multi-core → total ns/s) is the
  hardware-facing side of this component. Leaf node.

- **Space manager** — programs page tables, manages per-Context address
  spaces and permissions. Uses the Space allocator.

- **Scheduler** — decides which Context runs, programs the timer,
  triggers context switches. Uses the Time allocator. Scheduling
  algorithm is a swappable leaf node inside it.

### Space manager | Scheduler

Separable state (address space vs. CPU time), substantial independent
work. But they converge at context switch (one operation touching both)
and interact on blocking faults. Whether they are one component or two
with a narrow interface is unresolved.

### Open questions

- **Communication — partially explored.** The kernel delivers messages
  to Contexts. Faults, interrupts, and IPC are all instances of the
  same mechanism: a message with source, type/metadata, and payload.
  The concrete message shape is not yet defined. The delivery mechanism
  (how a Context receives) is a Level 2 concern. See
  `design/journal/002-communication-flows.md`.

- **Communication as component or flow?** Is message delivery a
  separate component (leaf node), or behavior woven into the exception
  handling path? The allocator/manager pattern doesn't have a natural
  parallel here — messages are transient, not a conserved resource.

- **Who receives fault messages?** When a Context faults
  unrecoverably, the kernel has information to deliver. To whom? This
  is an open design question — not assuming any particular structure.

- **Space manager | Scheduler boundary.** Still unresolved.
  Entanglement at context switch and blocking faults. Message delivery
  adds another interaction point: delivering a message touches the
  Scheduler (recipient needs CPU) and possibly Space manager (payload
  mapping).

- **One-shot timer.** Inside the Scheduler. Constraint at this level,
  or one level deeper?
