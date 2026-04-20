# Time as a Kernel Object

## The Question

How do real kernels represent temporal resources? Specifically:

1. Is a thread's scheduling allocation a _property embedded in the execution
   unit_ (per-thread/TCB attribute), or a _separate kernel object_ that the
   execution unit holds a reference to?
2. Can time allocations be transferred between execution units (e.g., donated
   over IPC)?
3. Is the time allocation bound to a specific CPU, or core-agnostic?
4. Where does scheduling-algorithm-specific state live — in the time object, in
   the execution unit, or per-core in the scheduler?
5. Are time allocations organized hierarchically (tree of budgets) or flat?

These questions arise whenever a kernel must decide whether to make temporal
isolation a first-class, capability-controlled abstraction.

---

## Survey of Existing Systems

### seL4 Classic (pre-MCS)

Scheduling parameters are embedded directly in the TCB kernel object:

- **Priority** (0–255)
- **Max controlled priority** (MCP): the maximum priority of threads this TCB
  can configure (capability-based policy limit)
- **Timeslice**: integer count of scheduling ticks

No separate time object. The TCB is the scheduling unit and the time container.
Scheduling state (current remaining timeslice, priority) lives inside the
kernel-managed TCB struct, inaccessible to userspace.

**Transfer**: Not possible. Scheduling parameters can be changed via
`seL4_TCB_SetSchedParams`, but this requires a capability to the target TCB and
sets values by copy, not transfer.

**CPU binding**: No explicit binding in classic seL4. Threads are assigned to
cores by the scheduler; no per-core time account exists at the object level.

### seL4 MCS (Mixed Criticality Scheduling, 2018)

The MCS branch introduces **SchedContext** as a first-class kernel object type.
This is the most fully worked-out example of time-as-object in deployed
microkernels.

Reference: Lyons, McLeod, Almatary, Heiser. "Scheduling-context capabilities: a
principled, light-weight operating-system mechanism for managing time."
EuroSys 2018.

**Structure**: A SchedContext contains:

- `budget` (microseconds): maximum execution time allowed in each period
- `period` (microseconds): the replenishment window
- Refill structures (`extra_refills`): an array of (amount, timestamp) pairs
  implementing the sporadic server algorithm — tracking partial budget
  consumption and scheduling future replenishments
- Reference to its creating `SchedControl` (which is per-CPU)

A TCB that is not bound to a SchedContext cannot be scheduled. Binding:
`seL4_SchedContext_Bind(sc_cap, tcb_cap)`. One SchedContext per TCB at a time;
one TCB per SchedContext at a time.

**SchedControl**: One SchedControl capability exists per CPU. Invoking a
SchedControl (`seL4_SchedControl_Configure`) configures a SchedContext and
determines which CPU's time it represents. A SchedContext created under core 0's
SchedControl provides time on core 0.

**Full vs. partial**:

- Full: `budget == period` — grants 100% of the CPU's time (for dedicated tasks)
- Partial: `budget < period` — upper-bounded allocation; enforced by sporadic
  server algorithm

**IPC donation (passive server model)**: The key enabling feature. When a server
thread blocks on a receive endpoint with no bound SchedContext, it becomes
"passive." On a `seL4_Call`, the caller's SchedContext is donated to the server
for the duration of the call, then returned on `seL4_ReplyRecv`. Reply objects
track the donation chain. The server runs on the caller's time budget — no
additional scheduling context needed, and no budget is wasted.

From the MCS paper: IPC donation adds zero overhead to the fastpath compared to
non-MCS IPC. The overhead is in tracking the donation chain in Reply objects.

**Migration**: Moving a thread to another core requires rebinding its
SchedContext under a different core's SchedControl capability. Budget is not
automatically transferred — the rebinding operation is explicit.

**Object size**: Minimum 256 bytes; scales with `extra_refills` count.
Replenishment overhead: ~50 cycles per period boundary.

### Zircon / Fuchsia (Google)

Zircon uses a **Profile** kernel object as a scheduling-parameters container:

- Profile is a separate kernel object type
- A profile carries: scheduling policy (fair or deadline), priority (for fair),
  and for deadline: `period`, `capacity`, `deadline` fields
- Applied to a thread via `zx_object_set_profile(thread_handle, profile_handle)`
- The profile is a _configuration template_, not a resource container: it does
  not hold a running time budget or refill state; that state is maintained
  per-thread inside the kernel

**Transfer**: Profile application is a copy of parameters, not a donation.
Multiple threads can be configured with the same profile; the profile object is
not consumed or locked by any single thread. Profiles cannot be donated over
IPC.

**Deadline profile** (Zircon 2020+): Three fields:

- `period`: scheduling window duration
- `capacity`: CPU time granted per period
- `deadline`: time within the period by which capacity must be delivered

Guarantee: "each period, the thread is allocated up to `capacity` CPU within
`deadline` of the start of each period."

**CPU binding**: Profiles have no inherent CPU binding. Thread-to-CPU affinity
is a separate mechanism.

### QNX Neutrino: Sporadic Scheduling

In QNX, sporadic scheduling is a **per-thread attribute** on the thread struct.
It is not a separate kernel object.

Thread-level scheduling parameters for `SCHED_SPORADIC`:

- `sched_ss_init_budget` (C): initial execution budget at normal priority
- `sched_ss_low_priority` (L): demoted priority when budget exhausts
- `sched_ss_repl_period` (T): replenishment interval
- `sched_ss_max_repl`: maximum pending replenishments (caps overhead)

Mechanics: When a thread consumes its initial budget, its priority drops to L.
At replenishment time (T after first becoming ready), priority restores and
budget refills. QNX limits to at most one pending replenishment per sporadic
thread.

**Transfer**: Not possible. Scheduling attributes are set via
`pthread_setschedparam` or `SchedSet_r` and live inside the thread's kernel
struct.

### QNX Adaptive Partitioning

QNX's optional Adaptive Partitioning extension adds a **partition** (group)
object above the thread level:

- A partition has a guaranteed CPU percentage budget
- Threads are assigned to partitions; all threads in a partition share the
  partition's budget
- Accounting unit: ClockCycles(); budget tracked as time-per-averaging-window
- Unused budget is _lent_ to other partitions (adaptive behavior) but reclaimed
  when needed
- Guarantees: partition gets its minimum even under overload from other
  partitions

The partition is a kernel-managed object, but it is a **group container**, not a
per-thread time capability. It organizes threads, not execution units.

### KeyKOS: Meter Key

KeyKOS (and its successor EROS) is the earliest capability system with an
explicit time-as-capability model.

Reference: Bomberger et al. "The KeyKOS Nanokernel Architecture." USENIX 1992.

**Meter key**: A capability that represents a specific quantity of CPU time:

- The kernel maintains a **prime meter** representing time from present to end
  of time
- Meter keys can be **subdivided** into sub-meters (like space banks for
  memory): a holder can create a sub-meter representing a fraction of their
  meter's time
- A domain (process) requires a valid meter key to be eligible to execute
- When a domain's meter is exhausted, it stops executing
- Hierarchical: meter → sub-meter → sub-sub-meter; time is allocated top-down

This is the purest "time-as-resource" model: time is a conserved quantity,
minted by the kernel's prime meter, subdivided into sub-capabilities, and
consumed on execution. Like memory in a capability system, time allocation
cannot exceed the total budget of the parent meter.

**Transfer**: Meter keys can be given to other domains — the capability is the
resource. Creating a sub-meter hands a portion of one's time to another.

### EROS: Schedule Capability

EROS inherits and refines KeyKOS's model:

- "Schedule capabilities convey the authority for a running domain to execute
  instructions under a particular scheduling reserve"
- Scheduling reserves are kernel-managed; schedule capabilities designate them
- A domain without a valid schedule capability cannot run

The EROS SOSP '99 paper notes that scheduling and storage allocation policies
are _exported from the kernel to user space_ via these capabilities, allowing
multiple OS personalities to implement different scheduling policies above the
kernel's resource-reservation primitives.

### Composite OS

Composite OS (Gabriel Parmer, George Washington University) treats the
**scheduling context** (called SchedContext in some versions) as a separate
kernel object, decoupled from the thread:

- Thread carries execution state (registers, stack)
- SchedContext carries scheduling budget and parameters
- Thread migration (the Composite IPC mechanism): the calling thread migrates
  into the callee's protection domain; the _same scheduling context_ follows the
  thread, so the callee runs on the caller's budget

This is structurally similar to seL4 MCS donation but implemented differently:
in Composite, the budget follows the _thread_ (which migrates), not the thread
it's bound to.

Reference: Parmer and West. "Predictable and Configurable Component-Based
Scheduling in the Composite OS." ACM TECS, 2013.

### L4 / Pistachio (L4Ka)

Scheduling parameters are embedded in the thread state in all classical L4
variants (L4/Pistachio, L4/Hazelnut, OKL4):

- Priority (0–255)
- Timeslice (microseconds)
- Total quantum (for time-limited threads)
- `ExchangeRegisters` syscall can modify thread state including scheduling
  parameters

No separate time object. Thread is the atomic unit of scheduling state.

**Transfer**: Not possible as a capability. Parameters can be copied via
`ExchangeRegisters` with appropriate authority.

### Mach / XNU

Scheduling parameters are per-thread attributes:

- Base priority
- Scheduling policy (FIFO, round-robin, timeshare)
- Per-policy parameters (e.g., `sched_priority`, `sched_cur_abs_time` for
  realtime threads)
- `thread_policy_set` / `thread_policy_get` syscalls modify them

XNU (macOS/iOS) adds QoS (Quality of Service) classes — user-visible priority
tiers (User Interactive, User Initiated, Default, Utility, Background) that map
to kernel priority bands. QoS is a thread property, not a kernel object.

No separate time capability. Cannot donate time over IPC.

### Plan 9

Each process has a per-process scheduling band (`p->priority`, `p->basepri`) and
a quantum counter. The kernel's `sched()` function selects from per-core run
queues. There is no time object; scheduling state is opaque kernel-internal data
attached to the process struct.

### Barrelfish

Barrelfish schedules **dispatchers** (one per core per domain). The dispatcher's
scheduling parameters (budget, deadline) are managed by the userspace monitor,
not the kernel CPU driver. The CPU driver exposes a simple timeslice; the
monitor implements whatever scheduling policy is needed above that.

Time is not a kernel object. Temporal guarantees are userspace-implemented.

---

## Design Dimensions and Observed Tradeoffs

### 1. Embedded attribute vs. separate kernel object

| Model                                  | Systems                                                 | Core property                                                                     |
| -------------------------------------- | ------------------------------------------------------- | --------------------------------------------------------------------------------- |
| Per-thread attribute                   | L4, Mach, QNX sporadic, Plan 9, seL4 classic            | Simple; scheduling authority merged with execution identity                       |
| Separate object (applied by reference) | seL4 MCS, Zircon Profile                                | Can be independently managed, but not donated (Profile) or donated (SchedContext) |
| Separate capability                    | KeyKOS meter, EROS schedule cap, Composite SchedContext | Time is a first-class capability; transferable, authority-bearing                 |

The capability model enables the largest set of operations: subdivision,
delegation, transfer, revocation via normal capability mechanisms. It also
couples temporal isolation to the existing capability enforcement infrastructure
(no new access control mechanism needed).

The embedded model is simpler, avoids binding/unbinding protocol, and has zero
overhead on the fast path (no indirection through a capability table slot).

### 2. IPC donation (passive server model)

seL4 MCS and Composite OS enable a server to run on the caller's time budget.
This solves a structural problem: if a server has its own scheduling context, it
must be scheduled independently of clients. Under high load, the server may not
run when a client call arrives, causing priority inversion. With donation:

- Server has no SchedContext of its own
- Server runs exactly when called, on the caller's budget
- No separate scheduling context allocation for the server
- Overhead: zero on the fastpath (measured by Lyons et al. 2018)

Systems without donation (classical L4, Mach, Zircon, QNX) require the server to
have its own time allocation. This forces either: (a) a high-priority server
(priority boosting), or (b) explicit real-time server scheduling.

### 3. Per-CPU binding of the time object

seL4 MCS: A SchedContext is created under a specific CPU's SchedControl and
provides time only on that CPU. Migration = rebinding to a different CPU's
SchedControl. This makes CPU binding an explicit property of the time resource,
not an afterthought.

Zircon Profile, QNX, L4, Mach: CPU affinity and scheduling parameters are
orthogonal attributes on the thread. There is no inherent coupling between "how
much time" and "which CPU."

Barrelfish: Each core has its own dispatcher and its own time account; they are
inseparable because the CPU driver is per-core.

### 4. Where scheduling algorithm state lives

| System       | Algorithm state location                                  |
| ------------ | --------------------------------------------------------- |
| seL4 MCS     | In the SchedContext (refill array for sporadic algorithm) |
| seL4 classic | In the TCB (priority, timeslice counter)                  |
| Zircon       | Per-thread, inside kernel thread struct                   |
| QNX sporadic | Per-thread                                                |
| Barrelfish   | User space (monitor implements policy)                    |
| L4           | Per-thread                                                |

The seL4 MCS choice — storing algorithm state (replenishments) in the
SchedContext — means that migrating a SchedContext to a different thread carries
its replenishment history. This enables time accounting across IPC boundaries.

Storing algorithm state _per-core in the scheduler_ (the Barrelfish/D2 approach)
means the SchedContext (or equivalent) carries only the policy specification
(budget, period), not the current execution history. History is local to the
core's scheduler and is re-derived on migration.

### 5. Hierarchical vs. flat allocation

| System                    | Structure                                                          | Guarantee                                                     |
| ------------------------- | ------------------------------------------------------------------ | ------------------------------------------------------------- |
| KeyKOS meter              | Tree (prime → sub-meter → ...)                                     | Parent cannot over-allocate; conservation enforced by kernel  |
| QNX Adaptive Partitioning | Flat groups with budgets                                           | Guaranteed minimums; adaptive lending of unused budget        |
| seL4 MCS                  | Flat (SchedContexts created from untyped; no structural hierarchy) | Per-SchedContext enforcement only; no inter-context hierarchy |
| Zircon Profile            | Flat (templates applied to threads)                                | Per-thread guarantee only                                     |

### 6. Reclamation on execution unit destroy

| System         | What happens to time object on thread destroy                                                          |
| -------------- | ------------------------------------------------------------------------------------------------------ |
| seL4 MCS       | SchedContext is unbound but not destroyed; can be rebound or destroyed separately by capability holder |
| KeyKOS meter   | Meter key is revoked with normal capability revocation; time returns to parent meter logically         |
| Zircon Profile | Profile is a separate object; not destroyed with the thread                                            |
| QNX sporadic   | Parameters disappear with the thread (embedded)                                                        |
| L4             | Parameters disappear with the thread (embedded)                                                        |

---

## Measured Data

**seL4 MCS IPC donation overhead** (Lyons et al., EuroSys 2018, ARM Cortex-A9):

- Passive server round-trip latency: approximately equal to direct non-MCS IPC
- Donation tracking adds zero cycles to the fastpath
- Replenishment operation: ~50 cycles per period boundary

**seL4 SchedContext object size**:

- Minimum: 256 bytes (OBJECT_SIZE_SCHED_CONTEXT in seL4 source)
- Scales with `extra_refills`: each additional replenishment structure adds ~16
  bytes

**QNX Adaptive Partitioning**:

- Guaranteed minimum CPU % maintained even under 100% system load
- Accounting granularity: per-ClockCycles(), not per-tick; microsecond precision
- Unused partition budget is lent out; reclaimed within one averaging window

**Barrelfish per-core budget (user-space)**:

- No kernel overhead for scheduling policy — policy is userspace code in the
  monitor; kernel only delivers timeslice interrupts

---

## References

- Lyons, A., McLeod, K., Almatary, H., Heiser, G. (2018). "Scheduling-context
  capabilities: a principled, light-weight operating-system mechanism for
  managing time." EuroSys 2018.
  https://trustworthy.systems/publications/abstracts/Lyons_MAH_18.abstract
- seL4 MCS Tutorial. https://docs.sel4.systems/Tutorials/mcs.html
- seL4 MCS Reference Manual, Version 10.1.1-MCS.
  https://sel4.systems/Info/Docs/seL4-manual-10.1.1-mcs.pdf
- Bomberger, A. et al. (1992). "The KeyKOS Nanokernel Architecture." USENIX
  Annual Technical Conference 1992.
  http://cap-lore.com/CapTheory/upenn/NanoKernel/NanoKernel.html
- Shapiro, J.S., Smith, J.M., Farber, D.J. (1999). "EROS: a fast capability
  system." SOSP '99.
  https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf
- Parmer, G. and West, R. (2013). "Predictable and Configurable Component-Based
  Scheduling in the Composite OS." ACM TECS.
  https://www2.seas.gwu.edu/~gparmer/pubs.html
- Zircon Scheduling.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/kernel_scheduling
- Zircon Kernel Objects.
  https://fuchsia.dev/fuchsia-src/reference/kernel_objects/objects
- QNX Sporadic Scheduling.
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/kernel_Sporadic_scheduling.html
- QNX Adaptive Partitioning Overview.
  https://get.qnx.com/developers/docs/6.5.0SP1.update/com.qnx.doc.adaptive_partitioning_en_user_guide/ap_overview.html
- Baumann, A. et al. (2009). "The Multikernel: A new OS architecture for
  scalable multicore systems." SOSP '09.
  https://people.inf.ethz.ch/troscoe/pubs/sosp09-barrelfish.pdf
