# Time Object Content: What a Time Capability Carries

## The Question

Assuming time is a first-class kernel capability (established in
`time-as-kernel-object.md`) and that cardinality is N-hold, 1-active
(established in `time-capability-cardinality.md`), the remaining question is:
**what parameters does the Time object itself carry?**

Three conceptually distinct models appear in deployed systems:

1. **Budget/period (absolute, hard-RT capable):** The object carries an explicit
   execution budget (e.g., 500 µs) and a period (e.g., 10 ms). The scheduler
   enforces the sporadic server invariant: the thread gets at most `budget`
   microseconds of CPU every `period` microseconds. Guarantees are time-absolute
   — independent of other threads' behavior.

2. **Fraction of core capacity (relative, proportional):** The object carries a
   weight or quota value that represents a share of the CPU's capacity. A thread
   with quota 20% of a super period gets one fifth of the core, but the absolute
   time depends on competing claims and system load. Guarantees are
   proportional, not time-absolute.

3. **Claim-to-participate (permission token, scheduler-determined quantity):**
   The object conveys authority to run at some scheduling class or priority, but
   carries no quantity. How much time the thread receives is determined entirely
   by the scheduler policy at runtime. The Time capability is a permission, not
   a resource allocation.

These models have different interactions with multi-Time cardinality: if an
Observer holds N Time caps but only one is active (as all surveyed systems
enforce), the model determines what "switching active caps" means in terms of
execution behavior.

An adjacent dimension — whether scheduling hints (priority, QoS class, RT flag)
live on the execution unit or inside the Time object — is surveyed in the final
section.

---

## Survey by Model

### Model 1: Budget/Period (Absolute, Hard-RT Capable)

#### seL4 MCS: SchedContext

The seL4 MCS SchedContext is the most fully specified budget/period Time object
in deployed microkernels. Fields:

- **`budget`** (µs): maximum execution time granted per period
- **`period`** (µs): the replenishment window
- **Refill list:** an array of `(rAmount, rTime)` pairs implementing the
  sporadic server algorithm. A replenishment `(a, t)` means "amount `a` can be
  consumed starting at time `t`." While executing, the thread drains from the
  head replenishment. When the head is exhausted, remaining execution is blocked
  until `rTime` arrives for the next entry.
- **`extra_refills`:** configurable count of additional refill slots (default: 2
  slots, i.e., "minimal" sporadic server). More slots allow more interrupted
  periods without forfeiting budget.
- **`scTcb` back-pointer:** to the currently bound TCB (1:1 binding enforced).
- **`scCore`:** the CPU core this SchedContext provides time on (per-CPU
  SchedControl creates per-CPU SCs).

The sporadic server algorithm guarantees: a thread using strictly less than its
budget in a period does not lose that time — it accumulates partial
replenishments and can "burst" up to budget in a later period. The invariant is
that total execution over any window of `n` periods cannot exceed `n × budget`.

**Configured via:**
`seL4_SchedControl_Configure(sc_cap, budget, period, extra_refills, badge)`. The
SchedControl capability is per-CPU; the core that the SchedContext provides time
on is determined by which SchedControl was used.

**What "active" means for multi-Time:** Switching active caps (unbind + rebind)
replaces the entire `(budget, period, refill_list)` triple. The outgoing cap
retains its refill state; the incoming cap's refill state takes over. Mode
switching (e.g., normal budget → elevated budget for a deadline) is possible by
rebinding to a different SchedContext.

Sources: seL4 MCS Tutorial (`docs.sel4.systems/Tutorials/mcs.html`); Lyons et
al., EuroSys 2018.

---

#### Zircon: Deadline Profile

Zircon's Profile kernel object in deadline mode carries:

- **`period`** (ns): scheduling window duration
- **`capacity`** (ns): CPU time granted per period
- **`deadline`** (ns): time within the period by which capacity must be
  delivered — the service deadline, not just the allocation window

The deadline profile is a parameter template applied to a thread
(`zx_object_set_profile`), not a stateful object tracking replenishments. Refill
state lives per-thread inside the kernel thread struct; the Profile itself is
stateless. This means a Profile can be applied to multiple threads — it's a
configuration, not a budget instance.

**Distinction from seL4 MCS:** seL4's SchedContext is stateful (carries refill
history); Zircon's Profile is stateless (parameters only). Applying the same
Profile to two threads gives them the same parameters but independent runtime
state. seL4's SchedContext can only be bound to one TCB at a time because the
refill state is instance-specific.

Sources: Zircon Kernel Scheduling
(`fuchsia.dev/fuchsia-src/concepts/kernel/kernel_scheduling`).

---

#### POSIX SCHED_DEADLINE (Linux)

Linux's `SCHED_DEADLINE` policy (implemented via CBS — Constant Bandwidth
Server) gives each task:

- **`runtime`** (ns): maximum execution time per period
- **`period`** (ns): replenishment window
- **`deadline`** (ns): relative deadline within the period

These are per-thread attributes, not a separate kernel object. The CBS algorithm
tracks a per-thread `remaining_runtime` and `absolute_deadline`, replenishing at
each period boundary. This is the same model as seL4 MCS budget/period but
embedded in the thread struct rather than a separate capability.

Source: Linux kernel documentation (`sched-deadline.rst`); Lelli et al.,
"Deadline scheduling in the Linux kernel," EuroSys 2016.

---

### Model 2: Fraction of Core Capacity (Relative, Proportional)

#### Genode base-hw: CPU Quota

Genode's base-hw kernel introduces a **super period** (configurable, typically 1
second) representing 100% of one CPU's time. A scheduling context carries a
**quota** expressed as a fraction of that super period.

The scheduler operates in two modes:

- **Claim mode:** At the start of each super period, each scheduling context
  with non-zero quota enters claim mode. Threads are scheduled in priority order
  while their quota remains. When a context's quota is exhausted, it exits claim
  mode.
- **Fill mode:** When all claim-mode threads have exhausted their quota (or no
  threads have quota), remaining super-period time is distributed round-robin
  among all ready threads ("fill" threads), regardless of quota. Fill threads
  function as background work — they get whatever CPU is unclaimed.

The Time object (scheduling context) thus carries:

- A quota value (microseconds of the super period, or equivalently a percentage)
- A priority (used for tie-breaking within claim mode, and for fill ordering)

**Guarantee semantics:** A thread with quota Q% of the super period is
guaranteed Q% of the CPU over each super period window (if it has work). This is
a proportional guarantee, not a per-period absolute-time guarantee. It cannot
make guarantees as tight as budget/period (you cannot say "deliver 500 µs within
the next 10 ms window").

Source: Genode Foundations 20.05, base-hw execution chapter; GitHub issue #1464
(relative CPU quota discussion).

---

#### Linux CFS Bandwidth Control (Group Scheduler)

Linux's CFS (Completely Fair Scheduler) with bandwidth throttling uses a
budget/period model for _cgroups_, but the semantics differ from SCHED_DEADLINE:

- **`cpu.cfs_period_us`**: the accounting window (e.g., 100 ms)
- **`cpu.cfs_quota_us`**: microseconds of CPU time the cgroup can use per period

Within the period, the cgroup's tasks run normally under CFS fairness. When
quota is exhausted, all tasks in the cgroup are throttled until the next period.
The _per-task_ allocation within the cgroup is determined by CFS weights, not
the quota — the quota sets an upper bound on the group.

This is a hybrid: the group Time object uses budget/period semantics (absolute
limit), but individual task allocation within the group uses proportional
(weight-based) semantics.

Source: Linux kernel doc `sched-bwc.txt`; cgroup v2 CPU documentation.

---

#### QNX Adaptive Partitioning

QNX's optional Adaptive Partitioning extension assigns each partition a
guaranteed CPU budget expressed as a **percentage of total CPU time** over an
averaging window:

- **`budget`**: percentage guaranteed (e.g., 25% means the partition receives at
  least 25% even under overload)
- **Adaptive lending:** unused budget is lent to other partitions; reclaimed
  within one averaging window when the partition becomes runnable

Threads are assigned to partitions; the partition's budget governs the group. No
per-thread Time object; the partition is the scheduling unit for budget
accounting.

**Distinguishing feature:** Adaptive Partitioning explicitly separates the
fractional _guarantee_ from actual consumption. A partition may consume more
than its guaranteed fraction if other partitions have spare capacity — but it is
guaranteed its minimum under any load condition.

Sources: QNX Adaptive Partitioning User Guide.

---

#### KeyKOS: Meter Key

KeyKOS implements the purest fractional model through hierarchical subdivision:

- The kernel holds a **prime meter** representing all future CPU time
- Meter keys are created by subdividing: a holder gives a child a meter
  representing a specified fraction of the parent's time
- The child's meter is bounded by the parent's — no over-allocation is possible

The Meter key carries no `period` field in the seL4 MCS sense. It carries a
quantity of time (derived from the prime meter), and execution is charged
against the domain's active meter. When the meter is exhausted, the domain
stops.

This is closer to a "conserved resource" model (like memory capability
subdivision) than either budget/period or percentage-quota.

Source: Bomberger et al., USENIX 1992.

---

### Model 3: Claim-to-Participate (Permission Token, Quantity Scheduler-Determined)

#### Classical L4 / seL4 pre-MCS

In L4 (all classical variants: Pistachio, Hazelnut, OKL4) and seL4 before MCS,
there is no separate Time object. Scheduling parameters are embedded in the
thread:

- **Priority** (0–255)
- **Timeslice** (microseconds): the quantum allocated per scheduler activation

These are not a "budget" in the sporadic server sense — there is no period, no
replenishment accounting, no inter-period carry. The thread gets a timeslice
when scheduled; it may be preempted and rescheduled arbitrarily. The total CPU
fraction received depends on competing priority traffic.

If Time-as-capability existed in this model, it would be a token saying "run at
priority P with quantum Q" — no amount guarantee, just a priority claim.

Source: L4 Specification Version X.2; seL4 Reference Manual pre-MCS.

---

#### NOVA Microhypervisor (Hedron): Scheduling Context

NOVA (used as Genode's foundation on x86) distinguishes global ECs (execution
contexts that can be scheduled) from local ECs (which only respond to IDC calls
and have no CPU time of their own). A global EC receives CPU time by associating
with an **SC (Scheduling Context)**.

NOVA's SC carries:

- **Priority**: the scheduling priority
- **Quantum**: the timeslice

The SC is a separate kernel object — distinct from the EC — but its content is a
claim (priority + quantum), not a resource budget. No period, no replenishment
tracking, no conservation invariant. The SC is closer to seL4 pre-MCS's embedded
scheduling parameters extracted into a separate object for capability management
purposes.

**SC donation:** When an IDC (inter-domain call) is made, the SC "passes along
the call chain" — the callee runs at the caller's priority. This is a form of
priority inheritance via SC donation, analogous to seL4 MCS donation, but
without budget tracking.

Source: Genode Foundations 19.05, base-nova execution chapter.

---

#### Mach / XNU: Thread Policy

Mach and XNU embed scheduling parameters in the thread:

- For timeshare threads: `sched_priority`, `max_priority`, `policy`
- For realtime threads: `period`, `computation`, `constraint`, `preemptible`

XNU's realtime policy (`THREAD_TIME_CONSTRAINT_POLICY`) does carry a
budget/period pair — `computation` (budget) and `period` (window). However,
these are per-thread attributes, not separate kernel objects, and have no
capability transfer semantics.

The `base_priority` and `policy` fields function as claims — "I want to run at
this priority, under this policy" — with actual allocation depending on
competing threads.

Source: XNU source (`osfmk/kern/sched.h`, `thread_policy_set`).

---

#### Plan 9

Each process has `p->priority` and `p->basepri`, plus a quantum counter. The
scheduler selects from per-core run queues, with no Time object at all.
Scheduling allocation is purely policy-internal; no capability carries any
quantity.

Source: Plan 9 kernel source (`kern/sched.c`).

---

## The Observer vs. Time Object Split

One dimension the question flags as a sibling is: which scheduling properties
belong on the execution unit (Observer) and which belong inside the Time object?

### Taxonomy from surveyed systems

| Property                      | Observed location                                                              |
| ----------------------------- | ------------------------------------------------------------------------------ |
| Budget (absolute µs)          | Time object (seL4 MCS SchedContext)                                            |
| Period (replenishment window) | Time object (seL4 MCS, Zircon Profile, POSIX DEADLINE)                         |
| Replenishment history         | Time object (seL4 MCS only among surveyed)                                     |
| Quota fraction (%)            | Time object (Genode base-hw)                                                   |
| Priority                      | Thread/Observer (L4, Mach), OR Time object (NOVA SC)                           |
| CPU affinity                  | Thread/Observer (L4, Mach, Zircon), OR Time object (seL4 MCS via SchedControl) |
| Scheduling policy class       | Thread/Observer (Mach: TIMESHARE vs REALTIME)                                  |
| QoS class                     | Thread/Observer (XNU)                                                          |

Key patterns:

- Systems that make time a first-class capability (seL4 MCS, KeyKOS) tend to
  move _more_ scheduling state into the Time object, including CPU core binding.
- Systems where Time is a template/claim (NOVA, Genode base-hw, Zircon Profile)
  tend to put policy hints (priority, QoS) on the execution unit and resource
  quantity in the Time object.
- No surveyed system puts _everything_ in the Time object. Even seL4 MCS puts
  the thread's register state and CSpace reference in the TCB.

### Why the split matters for multi-Time

If priority lives on the Observer, then switching active Time caps changes only
the budget/quota — the priority claim is stable. If priority lives in the Time
cap, switching active caps also changes priority, enabling "mode switching" in
the ARINC-653 sense (a thread operates under different priorities for different
operational phases, each phase having its own Time cap).

seL4 MCS supports mode switching via rebinding — the incoming SchedContext can
have a different priority, effectively changing the thread's scheduling behavior
atomically. No surveyed system requires priority to live _only_ on the execution
unit; some systems allow it in either place.

---

## Multi-Time Interaction by Model

The `time-capability-cardinality.md` document establishes N-hold, 1-active as
universal. What does "1-active" semantically mean for each content model?

### Budget/period (seL4 MCS, Zircon deadline)

1-active = one `(budget, period, refills)` triple governs at a time. Rebinding
switches which triple is consumed. The inactive caps' refill state is frozen —
neither advancing nor being consumed. This enables:

- Mode switching: bind SC-A for normal operation, SC-B for elevated priority
- Passive delegation: hold your own SC (frozen while serving) + donate to
  passive servers

### Fractional capacity (Genode base-hw quota)

1-active = one quota fraction governs. The scheduler sees one claim value per
scheduling context, not a sum. An Observer rebinding from 20%-quota SC to
30%-quota SC changes its claim in the next super period. No accumulation of
fractions from multiple held caps — the inactive caps' fractions are simply not
registered with the scheduler.

### Claim-to-participate (NOVA SC, classical L4)

1-active = one `(priority, quantum)` pair governs. Rebinding replaces the active
claim. This is a simple parameter swap, not a resource transfer. No conservation
invariant exists to enforce.

### Interaction with per-core scheduling

seL4 MCS: A SchedContext is created under a specific CPU's SchedControl — core
binding is _in_ the Time object. The per-core scheduler on that core owns the
SC's refill list and enforces its budget. Migrating = rebinding to a different
core's SchedControl.

Genode base-hw: CPU quota is registered in the per-core scheduler for the core
the thread currently runs on. The time object carries the quota value; the
per-core scheduler maintains the running consumption state for the current super
period. On migration, consumption history for the old core is dropped; the new
core's scheduler starts fresh.

NOVA: The SC follows the IDC call chain. No inherent core binding.

---

## Measured Data

**seL4 MCS SchedContext size:** 256 bytes minimum; each additional
`extra_refills` slot adds ~16 bytes. `extra_refills=0` gives a "minimal"
sporadic server (budget is forfeit if blocked more than twice per period).

**Sporadic server replenishment overhead:** ~50 cycles per period boundary (ARM
Cortex-A9; Lyons et al., EuroSys 2018). IPC donation (passive server) adds zero
cycles to the fastpath.

**Zircon Profile application cost:** Parameter copy to the thread struct.
Published latency not separately benchmarked from thread_create overhead.

**Genode base-hw super period accounting:** Accounting granularity is per
scheduler-tick (typically 1 ms). A 20% quota on a 1-second super period = 200
scheduler ticks allocated per second. No published overhead numbers for
scheduling context switching.

**POSIX SCHED_DEADLINE on Linux:** CBS replenishment is O(1) per period
boundary. Lelli et al. (EuroSys 2016) measured SCHED_DEADLINE task overhead at <
1 µs for admission control on a 4-core x86.

---

## Tradeoffs

### Budget/period vs. fraction vs. claim

| Property                       | Budget/period             | Fraction                          | Claim                       |
| ------------------------------ | ------------------------- | --------------------------------- | --------------------------- |
| Hard-RT guarantee expressible? | Yes (sporadic server CBS) | No (proportional, not absolute)   | No (scheduler-determined)   |
| Interacts with IPC donation?   | Yes (seL4 MCS passive)    | Harder (what fraction to donate?) | Possible (priority inherit) |
| Conservation invariant?        | Yes (budget not exceeded) | Yes (quota not exceeded)          | No                          |
| Scheduler complexity           | High (replenishment list) | Medium (super period tracking)    | Low (priority queue)        |
| Object state                   | Stateful (refill history) | Stateful (current consumption)    | Stateless or minimal        |
| Allocation admitted at?        | SchedControl invocation   | Session creation / configure      | Thread creation             |

### What "full" vs. "partial" allocation means

**Budget/period (seL4 MCS):** A SchedContext with `budget == period` grants 100%
of the CPU to the bound thread. `budget < period` grants a fraction. This makes
full and partial allocations expressible uniformly in the same object.

**Fraction:** A scheduling context with quota 100% of the super period grants
100% of the CPU (if no other claim-mode threads are scheduled at higher
priority). Quota < 100% grants that fraction.

**Claim:** No notion of "full allocation" in the object. The scheduler
distributes round-robin among equal-priority threads; 100% of CPU goes to the
highest-priority runnable thread.

### Admission control location

**Budget/period:** Admission control at SchedControl invocation (seL4) or at
`task_set_policy` (XNU realtime). The kernel can reject a SchedContext
configuration that would overcommit the CPU.

**Fraction:** Admission control at session/quota configuration. Genode's base-hw
does not enforce strict admission (the sum of all quotas may exceed 100%; fill
mode absorbs the excess). QNX Adaptive Partitioning enforces guaranteed
minimums.

**Claim:** No kernel admission control; the scheduler simply runs whoever is
highest priority.

### Scheduler coupling

Budget/period objects couple the time object to a specific scheduler algorithm
(sporadic server, CBS). The object _embeds_ algorithm state (refill list).
Changing the scheduling algorithm requires changing the Time object structure.

Fraction and claim objects are more algorithm-agnostic. The object carries a
parameter; the algorithm is in the scheduler, not the object.

---

## References

- Lyons, A., McLeod, K., Almatary, H., Heiser, G. (2018). "Scheduling-context
  capabilities: a principled, light-weight operating-system mechanism for
  managing time." EuroSys 2018.
  https://trustworthy.systems/publications/abstracts/Lyons_MAH_18.abstract
- seL4 MCS Tutorial. https://docs.sel4.systems/Tutorials/mcs.html
- seL4 MCS Release Notes 10.1.1-mcs.
  https://docs.sel4.systems/releases/sel4/10.1.1-mcs.html
- Lelli, J. et al. (2016). "Deadline scheduling in the Linux kernel." SMPTE
  Journal / Software: Practice and Experience. Related: Linux kernel
  `Documentation/scheduler/sched-deadline.rst`.
- Linux CFS Bandwidth Control. `Documentation/scheduler/sched-bwc.txt`.
- Genode Foundations 20.05, "Execution on bare hardware (base-hw)."
  https://genode.org/documentation/genode-foundations/20.05/under_the_hood/Execution_on_bare_hardware_(base-hw).html
- Genode Foundations 19.05, "Execution on the NOVA microhypervisor."
  https://genode.org/documentation/genode-foundations/19.05/under_the_hood/Execution_on_the_NOVA_microhypervisor_(base-nova).html
- Genode GitHub issue #1464 (relative CPU quota).
  https://github.com/genodelabs/genode/issues/1464
- Bomberger, A. et al. (1992). "The KeyKOS Nanokernel Architecture."
  USENIX 1992. http://cap-lore.com/CapTheory/upenn/NanoKernel/NanoKernel.html
- Shapiro, J.S., Smith, J.M., Farber, D.J. (1999). "EROS: a fast capability
  system." SOSP '99.
  https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf
- Shapiro, J.S. Coyotos Microkernel Specification (Chapter 7 incomplete).
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- QNX Adaptive Partitioning User Guide.
  https://get.qnx.com/developers/docs/6.5.0SP1.update/com.qnx.doc.adaptive_partitioning_en_user_guide/ap_overview.html
- Zircon Scheduling.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/kernel_scheduling
- XNU source: `osfmk/kern/sched.h`, `thread_policy_set`.
  https://github.com/apple-oss-distributions/xnu
