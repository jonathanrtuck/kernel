# Execution Units: What Runs Inside an Address Space

## The Question

What is the kernel's fundamental execution unit? Specifically:

1. What object carries register state, is scheduled, and executes instructions?
2. What object holds capabilities (authority)?
3. Are (1) and (2) the same object or different?
4. What is the cardinality between execution units and address spaces —
   one-to-one, or many-to-one?

This question recurs whenever a kernel must define what "a running thing" is.
The answers have deep downstream effects on IPC, capability namespacing, fault
handling, scheduling, and resource accounting.

---

## Survey of Existing Systems

### L4 (original, Liedtke 1993)

L4 defines three primitives: threads, address spaces, and IPC. There is no
first-class "process" object. A thread is the fundamental unit of execution and
the target of IPC operations. An address space is a separate object (owned by a
designated thread called the "pager"). Multiple threads can share an address
space by migrating or mapping.

Liedtke's minimality principle: "A concept is tolerated inside the microkernel
only if moving it outside the kernel, i.e., permitting competing
implementations, would prevent the implementation of the system's required
functionality." This principle led to dropping the process concept entirely — a
"process" is a composition of threads and address spaces, assembled by
user-level servers.

IPC targets are thread identifiers. Authority is not a first-class kernel
concept in original L4 — capabilities were added in later descendants (seL4).

**Cardinality**: Many threads per address space.

**Authority holder**: No kernel-enforced capability model; authority through IPC
thread-IDs only.

### seL4

The kernel execution unit is the **TCB** (Thread Control Block). Each TCB has:

- A CSpace (capability space): its own namespace of capabilities
- A VSpace: the address space it executes in (may be shared with other TCBs)
- An IPC buffer capability
- A fault handler endpoint capability
- Scheduling parameters (priority, max controlled priority, timeslice)

Multiple TCBs can share a VSpace — this is the standard model for threads within
a "process." CSpaces may also be shared by pointing multiple TCBs at the same
capability node tree, though this is a configuration choice, not a requirement.

The TCB itself is a capability-addressable kernel object; operations on it
(Configure, SetIPCBuffer, SetSpace, Resume, Suspend, etc.) are invoked via
capability.

**MCS extension (seL4 10.x+)**: The Mixed Criticality Scheduling branch splits
the scheduling parameters out of the TCB into a separate **SchedContext** kernel
object. A TCB cannot be scheduled without a bound SchedContext. SchedContexts
can be passed over IPC (enabling passive servers that run on the caller's
scheduling budget). This makes temporal isolation a first-class kernel
abstraction, separate from execution identity.

> "Scheduling contexts are separate from threads (although threads require one
> to run) and can be passed around over IPC, if the target of an IPC does not
> have its own scheduling context." — seL4 MCS documentation

**Cardinality**: Many TCBs per VSpace; one CSpace per TCB (but CSpaces can be
shared by configuration).

**Authority holder**: TCB (via its CSpace).

### L4.KeST / Fiasco.OC / seL4 lineage: "task" as address space

Later L4 descendants introduced a **task** object as a named address space, with
threads as the execution units within tasks. This mirrors Mach but without
Mach's heavyweight port namespace. Fiasco.OC has Task objects that own VSpaces
and can contain multiple Thread objects.

### Mach (Carnegie Mellon, 1985–1994)

Mach is the canonical split-authority design. Two distinct kernel objects:

- **Task**: resource container. Owns a virtual address space and a port-right
  namespace (the capability equivalent in Mach). Tasks have no execution of
  their own — "a task Y does X" means "a thread in task Y does X."
- **Thread**: scheduled execution unit. Belongs to exactly one task. Carries
  register state, stack pointer, execution context. Cheap to create. Has no
  independent port namespace; it uses its owning task's namespace.

> "Tasks are the units of resource ownership; each task consists of a virtual
> address space, a port right namespace, and one or more threads. A thread is
> the basic computational entity and belongs to one and only one task." — Apple
> Kernel Programming Guide (Mach overview)

Motivation: separating threads from tasks allows multiple threads to run
concurrently within a shared address space without duplicating the heavyweight
resource container. A task is expensive; a thread is cheap.

**Cardinality**: Many threads per task/address space.

**Authority holder**: Task (port namespace). Threads access the task's ports by
name, not by directly holding them.

### Zircon (Fuchsia, Google)

Zircon follows the Mach task/thread model closely with cleaner semantics:

- **Thread**: register state + stack + execution within exactly one process. The
  kernel scheduling unit. Threads hold no handles directly.
- **Process**: address space + handle table. The authority container.
- **Job**: grouping of processes and other jobs. Policy and resource limits.

> "Threads represent threads of execution (CPU registers, stack, etc) within an
> address space that is owned by the Process in which they exist." — Zircon
> Kernel Concepts

All kernel objects are accessed via handles in a process's handle table; threads
have no independent handle namespace. This means two threads in the same process
share exactly the same capability namespace — the process's handle table.

Threads can be created in another process (via `zx_thread_create` on a process
handle), which was noted as a design tension: remote thread creation can
undermine process isolation if the handle to a process is held by an untrusted
component.

**Cardinality**: Many threads per process/address space.

**Authority holder**: Process (handle table). Threads share the process's table.

### QNX Neutrino

> "A thread can be thought of as the minimum unit of execution, the unit of
> scheduling and execution in the microkernel. A process, on the other hand, can
> be thought of as a container for threads, defining the address space within
> which threads will execute. A process will always contain at least one
> thread." — QNX System Architecture documentation

The kernel scheduler operates on threads globally across all processes. Threads
carry priority and scheduling policy (FIFO, round-robin, sporadic). Processes
are containers; all authority (file descriptors, etc.) is attached to the
process.

QNX supports three scheduling policies per-thread: FIFO, round-robin, and
sporadic (budget-limited).

**Cardinality**: Many threads per process/address space.

**Authority holder**: Process.

### Barrelfish (ETH Zurich / Microsoft Research, SOSP 2009)

Barrelfish introduces the **dispatcher** as the kernel scheduling unit. A
dispatcher is not a thread; it is a scheduling domain that manages its own
threads via user-space scheduler activations.

> "A dispatcher is the unit of kernel scheduling, and on a single core roughly
> corresponds to the concept of a domain in Barrelfish. An application which
> spans multiple cores has a dispatcher on each core that it might potentially
> execute on." — Barrelfish Architecture Overview (TN-000)

When the kernel decides to schedule a dispatcher, it:

1. Brings in the VSpace pointed to by the dispatcher's vspace capability
2. Calls the dispatcher's `run()` upcall in user space
3. The dispatcher chooses which of its threads to run, restores that thread's
   register state, and executes

When a dispatcher is preempted, the kernel saves all register state to a save
area in the dispatcher's control block. The dispatcher can then re-schedule a
different thread.

This is scheduler activations (Anderson et al., 1992) taken to its logical
extreme: the kernel knows nothing about threads, only dispatchers. This enables
user-space scheduling policies without kernel involvement and avoids the
"descheduled while holding a lock" pathology (because the dispatcher is
re-entered and can yield).

**Cardinality**: One dispatcher per core per application domain; many threads
per dispatcher (managed in user space).

**Authority holder**: Capabilities live in a per-dispatcher (per-core)
capability space, replicated by user-mode monitors.

### Composite OS (Boston University)

Composite uses **thread migration** as its IPC mechanism. The kernel thread
object is the execution unit. When a thread in component A calls component B,
the same thread migrates — it continues executing but in B's protection domain
(a different address space and capability space). No new thread is created.

> "IPC mechanism: Synchronous invocation via thread migration. When component A
> calls component B, the calling thread migrates into B's protection domain." —
> Composite syscall landscape

A separate **SchedContext** (scheduling context) capability controls when a
thread can run. This is analogous to seL4 MCS. Threads do not inherently have
their own scheduling budget.

**Cardinality**: One thread migrates across address spaces (protection domains);
many threads can exist but each migrates on call.

**Authority holder**: Thread (carries its current protection domain's capability
space during migration).

### EROS / KeyKOS / Coyotos

EROS and KeyKOS take a unified approach: the **process** is both the execution
unit and the capability container.

> "In EROS, each process is a protection domain. Process state is recorded using
> nodes. Every EROS process includes a capability that names the root of its
> address space tree." — EROS SOSP 1999 paper

An EROS process has:

- Register state (PC, general-purpose registers)
- A set of **capability registers** (the kernel-protected slots holding
  capabilities)
- An address space descriptor (itself a capability)

Authority lives directly in the process's capability registers, not in a
separate task/process container. There is no thread/task split — the process is
the single kernel object for everything: execution, address space, and
authority.

EROS supports a single thread per process. Concurrency is achieved by creating
many processes (cheap due to persistent object model and checkpoint/restore).

In Coyotos (EROS successor), processes became first-class objects (in EROS they
were stored as nodes), but the unified model was retained.

**Cardinality**: One execution unit per address space (and per capability
space).

**Authority holder**: Process (unified).

### Plan 9 from Bell Labs

Plan 9 uses **processes** as the execution unit, but with a very different model
than Unix. The `rfork()` system call controls exactly which resources are shared
between parent and child:

- `RFMEM`: share memory (creating what other systems call a thread)
- `RFFDG`: share file descriptors
- `RFNAMEG`: share namespace
- Without RFMEM: independent memory copy (Unix-like fork)

There are no kernel threads in the traditional sense. The kernel scheduling unit
is always a process. "Threads" in Plan 9 are processes that share memory via
`rfork(RFMEM)`. The thread(2) library multiplexes user-level coroutines onto
these shared-memory processes.

> "Plan 9 eliminated the duality of threads and processes by implementing
> threading using lightweight processes that share the parent's data and bss
> segments." — Plan 9 documentation

**Cardinality**: One kernel-level execution unit per "process," but many
share-memory processes can simulate many-to-one.

**Authority holder**: Process (file namespace, capabilities through 9P).

### Genode OS Framework

Genode runs on multiple underlying kernels (seL4, NOVA, Fiasco.OC, base-hw). The
Genode abstraction is the **component**. Within a component, the **entrypoint**
is the primary thread:

> "The entrypoint is a thread that becomes active only when a call from a client
> enters the protection domain or when an asynchronous notification comes in." —
> Genode Foundations

Additional threads can exist within a component. All threads within a component
share the same address space and capability space. The entrypoint handles
incoming RPC calls and dispatches to RPC object implementations.

On base-hw (Genode's own kernel), the kernel maintains one kernel thread and one
scheduler per CPU core, with global spin-lock serialization for kernel object
access.

**Cardinality**: Many threads per component (address space).

**Authority holder**: Component-level (shared across threads within a
component).

---

## Design Dimensions and Observed Tradeoffs

### 1. Unified vs. split authority and execution

| Model                                        | Systems                   | Property                                                                   |
| -------------------------------------------- | ------------------------- | -------------------------------------------------------------------------- |
| Unified (process = execution + authority)    | EROS, Coyotos, Plan 9     | Clean capability model; each execution unit owns exactly its own authority |
| Split (task = authority, thread = execution) | Mach, Zircon, QNX, seL4   | Threads are cheap; authority is shared among threads in a task/process     |
| Split with per-thread CSpace                 | seL4 (TCB has own CSpace) | Maximum isolation possible; CSpaces can be shared by configuration         |
| Scheduling separated too                     | seL4 MCS, Composite       | Temporal isolation is first-class; scheduling budget ≠ execution identity  |

The unified model is simpler to reason about for capability security: authority
follows execution. The split model enables many lightweight execution units
sharing resources, but creates the question of "which thread's authority
applies" for kernel operations.

In seL4, the TCB has its own CSpace but the CSpace can be configured to point at
the same capability nodes as another TCB, giving a middle ground.

### 2. Cardinality: one-to-one vs. many-to-one

**One execution unit per address space** (EROS, some capability systems):

- Every execution unit has an independent address space and capability namespace
- Strong isolation between concurrent activities
- More address space switches (TLB/ASID pressure on context switch)
- Concurrency requires many processes (cheap if OS is designed for it)

**Many execution units per address space** (Mach, seL4, Zircon, QNX, Genode):

- Threads within a process share address space — IPC via shared memory is cheap
- Standard model for concurrent servers (one thread per client request)
- Capability/authority model must specify whether threads share or have
  independent namespaces
- Lock convoys and scheduler-in-critical-section problems arise

**Dispatcher/domain model** (Barrelfish):

- One kernel entity (dispatcher) per core per domain
- User-space manages thread-level multiplexing
- Kernel never sees threads; no scheduler-in-critical-section
- Complex to implement correctly (upcalls must be async-signal-safe)

### 3. What happens at the IPC boundary

The execution unit definition is tightly coupled to IPC semantics:

- **Thread-as-target** (L4, seL4): IPC is between TCBs. The calling thread
  blocks; the server thread is the IPC endpoint/thread.
- **Endpoint-as-target** (seL4 endpoints, Zircon channels): IPC is to an
  endpoint object; any thread blocked on it can receive. Decouples sender from
  specific receiver thread.
- **Thread migration** (Composite, L4e): The calling thread migrates into the
  callee's domain. No thread switch; same thread continues. Avoids per-call
  thread creation overhead.
- **Port/message queue** (Mach, QNX): Messages queue at ports/channels. Multiple
  receiver threads can pull from the queue.

### 4. Fault handling

The execution unit definition affects where faults (page faults, illegal
instructions) go:

- In seL4, a faulting TCB sends an IPC to its configured fault handler endpoint.
  A separate fault handler thread (in a different address space) handles it.
- In Zircon, a thread exception goes to an exception channel; a handler process
  (or the job hierarchy) receives it.
- In Mach, exceptions go to the task's exception port or the thread's exception
  port (thread overrides task).
- In EROS, a faulting process's keeper (a capability in the process) is invoked.
- In Plan 9, notes (like signals) are delivered to processes.

### 5. Register state ownership

All surveyed systems agree: register state (general-purpose registers, PC, SP,
PSTATE/CPSR) belongs to the execution unit, not the address space. The address
space descriptor is a pointer/capability held by the execution unit, not a
container.

The exception: Barrelfish dispatchers. The dispatcher's control block holds the
saved register state of the currently-running thread, but the kernel stores it
into the dispatcher's save area — so register state is in a user-accessible
region, not opaque kernel memory.

---

## Measured Data

**seL4 IPC latency** (fastpath, same-priority thread-to-thread, ARM):

- seL4 2023 on ARM Cortex-A57: ~200–250 ns one-way (Heiser et al. benchmarks)
- Fastpath invoked only when specific conditions hold (correct reply cap, same
  address space or compatible ASID, no scheduling change needed)

**Mach IPC overhead** (historical critique):

- Mach's task/thread model contributed to high IPC overhead vs. L4. Liedtke
  (1993) showed L4 achieving 5 μs on i486 vs. Mach's 100+ μs, largely due to
  Mach's indirection through tasks and ports for every operation.

**Barrelfish dispatcher switch** (vs. kernel thread switch):

- Barrelfish TN-000 reports that domain (dispatcher) switching has similar
  overhead to process context switching on the underlying hardware; the benefit
  is in eliminating in-kernel thread-scheduling overhead for many-threaded
  workloads.

**seL4 MCS scheduling context donation** (passive servers):

- The MCS design allows a passive server to run on the client's scheduling
  budget with no additional scheduling overhead vs. a non-MCS call.

---

## References

- Liedtke, J. (1993). "Improving IPC by kernel design." SOSP '93.
- Liedtke, J. (1995). "On µ-kernel construction." SOSP '95.
- Elphinstone, K. and Heiser, G. (2013). "From L3 to seL4: What have we learnt
  in 20 years of L4 microkernels?" SOSP '13.
  https://sigops.org/s/conferences/sosp/2013/papers/p133-elphinstone.pdf
- Heiser, G. et al. (2020). "L4 Microkernels: The Lessons from 20 Years of
  Research and Deployment." ACM TOCS 34(1).
  https://trustworthy.systems/publications/nicta_full_text/8988.pdf
- Shapiro, J.S., Smith, J.M., Farber, D.J. (1999). "EROS: a fast capability
  system." SOSP '99.
  https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf
- Shapiro, J.S. (2007). "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- Baumann, A. et al. (2009). "The Multikernel: A new OS architecture for
  scalable multicore systems." SOSP '09.
  https://people.inf.ethz.ch/troscoe/pubs/sosp09-barrelfish.pdf
- Barrelfish Architecture Overview (TN-000).
  https://barrelfish.org/publications/TN-000-Overview.pdf
- Apple Developer Documentation. "Mach Overview — Kernel Programming Guide."
  https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html
- Fuchsia documentation. "Zircon Kernel Concepts."
  https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
- seL4 Reference Manual, Version 14.0.0.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 MCS Reference Manual, Version 10.1.1-MCS.
  https://sel4.systems/Info/Docs/seL4-manual-10.1.1-mcs.pdf
- seL4 MCS tutorial. https://docs.sel4.systems/Tutorials/mcs.html
- QNX Neutrino System Architecture: Threads and Processes.
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/kernel_THREADSANDPRO.html
- Genode Foundations (20.05). "Execution on bare hardware (base-hw)."
  https://genode.org/documentation/genode-foundations/20.05/under_the_hood/Execution_on_bare_hardware_(base-hw).html
- Genode Foundations (20.05). "Inter-component communication."
  https://genode.org/documentation/genode-foundations/20.05/architecture/Inter-component_communication.html
- Plan 9 from Bell Labs. "rfork(2)." http://man.cat-v.org/plan_9/2/thread
- Anderson, T.E. et al. (1992). "Scheduler Activations: Effective Kernel Support
  for the User-Level Management of Parallelism." ACM TOCS 10(1).
