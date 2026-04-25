# SMP Placement, Cross-Core Wake, and Core Idle Management

## The Question

When an execution unit becomes runnable on an SMP system, three tightly coupled
decisions must be made:

1. **Placement policy** — which core should the newly-runnable entity run on?
2. **Cross-core wake protocol** — if the chosen core is not the current core,
   how does the kernel get work onto it? (IPI timing, handoff sequence,
   run-queue update ordering)
3. **Idle/wake policy** — when does a core stop executing and enter a low-power
   state, and how does it resume?

These are inseparable because the placement decision determines whether a
cross-core wake IPI is needed, and the idle policy determines what that IPI
costs. Getting placement wrong (failing to wake an idle core with available
work) is typically far more expensive than the IPI itself.

---

## 1. Placement Policy

### Linux CFS — `select_task_rq_fair()`

Linux's Completely Fair Scheduler makes a placement decision every time a task
becomes runnable (on `try_to_wake_up()`, which calls `select_task_rq()`).

The decision proceeds in layers:

1. **Wake affine (`SD_WAKE_AFFINE`):** If the waker and wakee share a scheduling
   domain (typically L2/L3 cache level), the scheduler tests whether placing the
   wakee on the waker's CPU would yield better cache locality than keeping it on
   its previous CPU. If so, the waker's CPU is tentatively selected.

2. **`select_idle_sibling()`:** From the candidate CPU (either waker's or
   previous), search for an idle CPU in the same SMT core, then same cache
   domain. An idle CPU is strongly preferred to avoid stealing from a busy one.

3. **`find_idlest_cpu()` → `find_idlest_group()`:** If no idle CPU in the cache
   domain, walk scheduling domains outward to find the least-loaded group, then
   least-loaded CPU within that group.

4. **EAS (`find_energy_efficient_cpu()`):** On big.LITTLE / DynamIQ systems with
   Energy Aware Scheduling enabled, `select_task_rq_fair()` calls
   `find_energy_efficient_cpu()` instead of `find_idlest_cpu()`. This function
   models per-frequency energy cost and selects the CPU that minimizes energy
   expenditure while meeting utilization targets. It prefers placing tasks on
   LITTLE cores unless the utilization demand requires a big core.

Priority order (simplified): idle SMT sibling > idle same-L2 CPU > last-run CPU
(if idle) > least-loaded CPU in domain.

**Affinity:** Linux respects per-task `cpus_allowed` masks at every step; a CPU
outside the mask is never a candidate.

Reference: `kernel/sched/fair.c: select_task_rq_fair()`,
`select_idle_sibling()`, `find_idlest_cpu()`.

### Linux EAS measured results

- Up to **48% power reduction** on big.LITTLE with EAS vs. vanilla CFS.
- Up to **15% performance improvement** from ML-assisted placement (academic
  results; not mainline).
- Energy Aware Scheduling mainlined in Linux 5.0.

Reference: Linux EAS documentation; Lozi et al. "The Linux Scheduler: a Decade
of Wasted Cores" (EuroSys 2016) for consequences of placement bugs.

### Zircon (Fuchsia) Fair Scheduler

Each CPU runs an independent scheduler instance managing its own priority
queues. A thread is assigned to a single CPU's queue at any time; it may only
compete for that CPU's scheduler.

On every wake-up (when a thread transitions from blocked to runnable), the
scheduler re-evaluates its CPU choice:

Priority order for CPU selection:

1. **Idle CPUs in affinity mask** — an idle CPU is strongly preferred; no
   context-switch cost.
2. **Last-run CPU (if idle)** — cache warmth; thread's working set may still be
   hot.
3. **Any idle CPU in affinity mask** — among equals, the one with lowest index
   (deterministic).
4. **Last-run CPU (if active)** — if no idle option, accept cache cost of a busy
   CPU the thread knows.
5. **Any active CPU in affinity mask** — lowest-priority fallback.

Work-stealing: a CPU with no eligible work attempts to steal work from other
CPUs in the same cluster before looking at distant clusters.

Reference: Zircon kernel scheduling documentation
(fuchsia.dev/fuchsia-src/concepts/kernel/kernel_scheduling).

### QNX Neutrino

The QNX SMP microkernel runs the same global priority policy as on uniprocessor:
the highest-priority ready thread always runs. On SMP this extends to: the
highest-priority ready threads run, one per available processor.

**Placement affinity:** When more than one processor could host a thread, QNX
prefers the processor where the thread last ran ("CPU affinity"), to reduce
cache pollution from migration. This is a performance hint, not a hard rule.

**Preemption:** If a thread becomes ready at a priority higher than the
lowest-priority running thread on any processor, the lowest-priority currently
running thread is preempted. The displaced thread's processor is then assigned
to the higher-priority newcomer.

Reference: QNX Neutrino System Architecture 7.0 — "SMP: How the SMP Microkernel
Works" (qnx.com/developers/docs/7.0.0/…/smp_HOWTHESMP.html).

### seL4 MCS Multicore

seL4 SMP uses a Big Kernel Lock. Each scheduling context (SchedContext) is
associated with a specific core: it is configured using that core's
`seL4_SchedControl` capability, and the invoked `SehedControl` determines which
processing core the SC grants access to.

**Placement mechanism:** Core assignment is explicit, not heuristic. A thread's
core is determined by which SC is bound to it. Migrating a thread to a different
core means either:

- Rebinding the thread to an SC already associated with the target core, or
- Reconfiguring the SC using the target core's `seL4_SchedControl` capability.

There is no kernel-internal "idle core" search or affinity heuristic. The kernel
places a thread on the core its SC specifies. Policy is entirely in userspace.

Reference: seL4 MCS pre-release 10.1.1 release notes
(docs.sel4.systems/releases/sel4/10.1.1-mcs.html); seL4 devel mailing list
"Cross-core thread migration"
(mail-archive.com/devel@sel4.systems/msg02737.html).

### Barrelfish

Barrelfish has no kernel-level cross-core placement. Each core runs an
independent CPU driver with its own dispatcher queue. Dispatchers do not migrate
between cores at the kernel level.

Cross-core thread management is done in **user space** by monitors:

- A thread in component A wants to run on core B: A's monitor sends a UMP
  (User-level Message Passing) message to B's monitor.
- B's monitor creates or activates a dispatcher on core B to run the thread.
- Work-stealing (pulling work from busy cores when idle) is implemented by
  user-space thread schedulers exchanging UMP messages.

An idle dispatcher polls its UMP channels briefly before blocking; it sends a
notification to its local monitor to be re-activated when a new message arrives.

Reference: Baumann et al., "The Multikernel" (SOSP 2009); Barrelfish TN-000
Architecture Overview (barrelfish.org/publications/TN-000-Overview.pdf); "Thread
Dispatching in Barrelfish" (DiVA 2014,
diva-portal.org/smash/get/diva2:731492/FULLTEXT01.pdf).

---

## 2. Cross-Core Wake Protocol

### Linux — `try_to_wake_up()` → IPI

The Linux cross-core wake sequence (simplified):

1. `try_to_wake_up()` is called with the target task.
2. `select_task_rq()` picks the target CPU (using the placement logic above).
3. If the target CPU is the current CPU: enqueue directly, set
   `TIF_NEED_RESCHED`, no IPI needed.
4. If the target CPU is a remote CPU: a. If CPUs share the same LLC: add task to
   the target CPU's **wakelist** (`ttwu_queue_wakelist()`) and send a reschedule
   IPI. The target CPU dequeues the task from the wakelist and enqueues it
   locally when it receives the IPI. This avoids cache-bouncing the run-queue
   lock. b. Otherwise: acquire the remote run queue's lock, enqueue the task,
   release the lock, send reschedule IPI.
5. The reschedule IPI on ARM64 is an SGI (Software Generated Interrupt) via
   `ICC_SGI1R_EL1`. The target core takes an IRQ exception, runs the IPI handler
   (`scheduler_ipi()`), which sets `TIF_NEED_RESCHED`.
6. The target core's scheduler runs on the next exception return or explicit
   `schedule()` call.

**Idle-specific path:** If the target core is idle (running the idle loop):

- `wake_up_idle_cpu()` tests if the core is in the non-polling idle state; if
  so, it sets `TIF_NEED_RESCHED` and sends an IPI.
- If the core is in polling idle (spin-waiting rather than WFI), no IPI is
  needed — the core polls `TIF_NEED_RESCHED` and picks up the task.

Reference: `kernel/sched/core.c: try_to_wake_up()`, `ttwu_queue_wakelist()`,
`wake_up_idle_cpu()`; Linux Kernel Archive LKML thread on wakelist optimization
(lkml.iu.edu/hypermail/linux/kernel/2205.2/03705.html).

### Zircon

Each CPU's scheduler runs independently. When a thread is made runnable:

1. The scheduler evaluates the CPU placement (see §1).
2. If the chosen CPU is the current CPU: insert into its priority queue
   directly.
3. If the chosen CPU is a remote CPU:
   - Insert the thread into that CPU's priority queue (under the scheduler's
     internal lock).
   - Send a reschedule IPI to the remote CPU.
4. The remote CPU's IPI handler triggers a scheduling pass. The highest-priority
   thread in the queue runs.

Work-stealing uses the same IPI path: an idle CPU that decides to steal from a
busy CPU moves the stolen thread and signals nothing (the idle CPU is already
running its scheduler).

Reference: Zircon kernel scheduling documentation
(fuchsia.dev/fuchsia-src/concepts/kernel/kernel_scheduling).

### QNX Neutrino SMP

QNX processors communicate via IPIs. The protocol follows the global priority
rule:

1. A thread becomes ready at priority P.
2. The kernel finds which processor is running the lowest-priority thread
   (P_low).
3. If P > P_low: the kernel sends an IPI to that processor to preempt P_low and
   schedule the new thread.
4. The receiving processor takes the IPI, checks for a higher-priority thread on
   the global queue, and schedules it.

The last-run CPU preference means the IPI is often to the same processor that
previously ran the thread.

Reference: QNX System Architecture 7.0 — SMP chapter.

### seL4 SMP (BKL approach)

With the Big Kernel Lock:

1. The core wishing to wake a thread on a remote core holds the BKL.
2. It changes the target TCB's state to runnable and adds it to the target
   core's run queue (all run queues are accessible under the BKL).
3. It sends an SGI IPI to the target core.
4. It releases the BKL.
5. The target core takes the IPI exception, acquires the BKL, runs the
   scheduler, and dispatches the newly-runnable TCB.

The BKL serializes all of this: no two cores are in kernel mode simultaneously.
IPI delivery is a signal to re-enter the kernel and check the run queue; the
actual run-queue update has already happened.

Reference: Peters et al., "For a Microkernel, a Big Lock Is Fine" (APSys 2015);
seL4 SMP kernel source (github.com/seL4/seL4).

---

## 3. Core Idle Management

### ARM64 Hardware Idle Mechanisms

Three hardware mechanisms for idle cores, in increasing depth:

**WFE (Wait For Event):** The core stalls until a `SEV` broadcast from another
core or a write to a monitored cache line (exclusive monitor event). Wakes
without IRQ delivery. Primarily used for spinlock backoff. Exit latency: <1 µs.

**WFI (Wait For Interrupt):** The core halts until an IRQ, FIQ, or async abort
arrives. A GICv3 SGI (IPI) is an IRQ; it wakes WFI immediately. Entry: execute
`wfi` instruction. Exit: the IRQ exception vector fires. Exit latency: <1 µs for
the WFI instruction itself (implementation-dependent but typically a few cycles
to tens of nanoseconds).

**WFI does NOT prevent TLB broadcast:** ARM64 inner-shareable TLBI instructions
(`TLBI VAE1IS`, etc.) complete across all cores including those in WFI. The
hardware handles the broadcast; WFI cores process TLB invalidations
transparently without needing an IPI or software wakeup. (`DSB ISH` on the
initiating core waits for confirmation.)

**PSCI CPU_SUSPEND:** Firmware-level deep idle via the Power State Coordination
Interface. The OS calls into Trusted Firmware-A via SMC with a power-state
parameter encoding the desired idle depth:

- Retention state: core halted, caches retained, very fast wake (~10–50 µs)
- Power-down state: core powered off, caches flushed, long wake (~100–500 µs) —
  requires saving and restoring the entire core context

On wake from CPU_SUSPEND power-down, execution resumes at a pre-registered entry
point; the kernel must re-initialize per-core state (GIC redistributor, MMU
registers, etc.).

Reference: ARM PSCI Specification (DEN0022); Trusted Firmware-A PSCI
documentation (trustedfirmware-a.readthedocs.io); ARM Architecture Reference
Manual (WFI, WFE, exclusive monitors).

### Linux CPUIdle Framework

Linux uses a layered idle framework:

**Governor (menu/TEO):** Predicts how long the CPU will be idle based on recent
history (timer expiry, task wake frequency). Selects an idle state depth
accordingly.

**Driver (cpuidle-arm, psci-cpuidle):** Executes the state entry: WFI for
shallow states; PSCI `CPU_SUSPEND` SMC for deep states.

**Idle loop (`kernel/sched/idle.c`):**

1. `cpuidle_idle_call()` → governor selects state → driver enters state.
2. On wake (IPI or timer): exit idle state, run IPI/interrupt handlers, run
   `schedule()` if `TIF_NEED_RESCHED` is set.

**Polling idle:** Some configurations spin-wait in the idle loop (checking
`TIF_NEED_RESCHED` without WFI) to eliminate IPI round-trip latency entirely.
Cost: ~100% of one core's power is wasted.

**IPI-to-schedule path (ARM64):**

- Waker sends SGI via `ICC_SGI1R_EL1`.
- Target core exits WFI, takes IRQ exception.
- GIC delivers the SGI to the core's IRQ handler.
- `scheduler_ipi()` runs, sets `TIF_NEED_RESCHED`.
- Idle loop resumes, calls `schedule()`, dispatches new task.
- Total added latency: ~600–960 ns (GICv3 one-way IPI) + interrupt entry/exit +
  scheduler path.

Reference: Linux kernel `kernel/sched/idle.c`;
`Documentation/admin-guide/pm/cpuidle.rst`; ARM PSCI S2Idle integration
documentation.

### Zircon Idle Thread

Zircon's idle thread runs at effective priority -1 (below all real threads). One
idle thread exists per CPU. When no other thread is runnable, the scheduler
dispatches the idle thread. The idle thread's body can execute a platform WFI
instruction for low-power waiting.

A new runnable thread at any priority higher than -1 will preempt the idle
thread. The cross-core mechanism: when the scheduler places a new thread on a
remote CPU that is running its idle thread, it sends an IPI. The IPI causes a
scheduling interrupt; the scheduler notices the new high-priority thread and
preempts the idle thread.

Reference: Zircon scheduler documentation (fuchsia.dev); Zircon fair scheduler
documentation (fuchsia.dev/fuchsia-src/concepts/kernel/fair_scheduler).

### QNX Neutrino Idle Thread

The QNX idle thread runs at priority 0 and is always ready to run. On SMP, each
core has an idle thread at priority 0. A core executing the idle thread can
execute a low-power instruction (WFI or equivalent). An IPI from another core
arriving with a newly-ready thread causes the idle thread to be preempted.

Reference: QNX System Architecture 7.0 — "Idle thread" section.

### Barrelfish / CPU Driver Idle

Each Barrelfish CPU driver manages its own idle state. When a core has no
runnable dispatcher, the CPU driver can execute WFI or equivalent. A UMP message
arriving in a shared ring buffer does not wake a WFI core directly (UMP is
polling). The pattern for waking a sleeping core:

1. The sending monitor posts a work item to the target core's UMP channel.
2. If the target core is idle (known via shared state), the sending monitor
   sends an IPI to wake the target CPU driver.
3. The CPU driver exits WFI, processes the UMP channel, and runs the newly-ready
   dispatcher.

This means UMP is fast for polling threads but requires an explicit "wake" IPI
for idle cores, adding the same ~1–2 µs IPI latency.

Reference: Barrelfish TN-000; "Dynamic Inter-core Scheduling in Barrelfish"
(DiVA, diva-portal.org/smash/get/diva2:482762/FULLTEXT01.pdf).

---

## 4. Measured Data

### IPI Latency on ARM64 (from smp.md)

| Configuration    | One-way     | Round-trip                  |
| ---------------- | ----------- | --------------------------- |
| GICv2 (MMIO)     | ~150 ns     | ~1 µs                       |
| GICv3 (sysreg)   | ~600–960 ns | ~1–2 µs                     |
| Linux real-world | —           | 2–5 µs avg, 50–200 µs worst |

Source: ARM community measurements; GIC specification.

### Cost of Missed Placement / Idle Core

Lozi et al., "The Linux Scheduler: a Decade of Wasted Cores" (EuroSys 2016):

- Scheduling bugs (including failure to IPI idle cores) caused **13–23%
  throughput loss** in database workloads.
- One missing-scheduling-domains bug: **up to 138× degradation** with cores
  sitting idle while other cores had full run queues.
- Fix: **up to 27× speedup** by restoring correct domain construction.
- The bugs persisted undetected for years because the test suite did not cover
  multi-socket NUMA configurations.

The cost of an idle core (ms–s) dwarfs the cost of an IPI (~1–2 µs) by 3–6
orders of magnitude.

### PSCI Idle State Entry/Exit Latency

Approximate values from ARM and Linux documentation; vary widely by SoC:

| Idle State      | Entry latency | Exit latency | Notes                             |
| --------------- | ------------- | ------------ | --------------------------------- |
| WFI (shallow)   | <1 µs         | <1 µs        | Core halted, caches retained      |
| PSCI retention  | ~5–50 µs      | ~10–50 µs    | Core halted, voltage retained     |
| PSCI power-down | ~100–500 µs   | ~100–500 µs  | Core off, caches flushed; context |
|                 |               |              | save/restore required             |

Source: ARM PSCI Specification; Linux CPUIdle documentation; TI AM62x SDK
documentation (software-dl.ti.com).

### BKL overhead in seL4 SMP

Peters et al., "For a Microkernel, a Big Lock Is Fine" (APSys 2015), quad-core
Cortex-A9 at 1 GHz:

- BKL strategy: **23% overhead** vs uniprocessor.
- Fine-grained locking strategy: **>70% overhead**.
- The 23% overhead is from the lock primitive itself (memory barriers), not from
  contention. Microkernel syscalls are short; contention is rare.

This result was obtained by moving IPI sends and page table updates outside the
BKL critical section.

---

## 5. Tradeoffs

### Heuristic placement (Linux/Zircon/QNX) vs. explicit binding (seL4 MCS) vs. userspace (Barrelfish)

**Heuristic placement:**

- Kernel adaptively selects a core on each wake-up based on current load, cache
  topology, and affinity masks.
- No userspace policy code needed for common cases.
- Heuristic can be wrong (as documented in "Wasted Cores"). Correctness of the
  heuristic is difficult to verify.
- Each wake-up pays the cost of evaluating the placement policy.

**Explicit SC-based binding (seL4 MCS):**

- Core assignment is static until explicitly changed by a privileged userspace
  actor.
- Kernel placement code is trivially correct: no search, no heuristic.
- Policy is entirely in userspace (or absent, if userspace doesn't manage it).
- A thread bound to a core with an empty run queue does not "opportunistically"
  migrate to a free core; it waits.

**Userspace placement (Barrelfish):**

- Maximum flexibility: placement can encode arbitrarily complex policy (NUMA,
  heterogeneous, energy, workload-specific).
- Round-trip latency for cross-core placement includes a UMP message round-trip
  before work starts.
- Correctness of the policy rests on the monitor, not the kernel; placement bugs
  are userspace bugs.

### IPI always vs. polling idle

**IPI-based wake (WFI idle):**

- Reduces idle power consumption.
- Introduces ~1–2 µs latency for any cross-core wake that hits an idle core.
- IPI can be lost or delayed under heavy interrupt load (worst-case tails to
  50–200 µs in Linux measurements).

**Polling idle (spin-wait):**

- Zero additional latency for cross-core wake: the idle core polls
  `TIF_NEED_RESCHED` continuously.
- Wastes one core's full power budget while idle.
- Used in latency-critical RT configurations and in the kernel's own `idle=poll`
  boot parameter.

### Shallow vs. deep idle states

**WFI only (shallow):**

- Entry/exit: <1 µs.
- Core is still clocked (retains coherent caches).
- Suitable for fine-grained idle (sub-millisecond expected waits).
- Woken by any IRQ including a reschedule IPI.

**PSCI CPU_SUSPEND power-down (deep):**

- Entry/exit: 100–500 µs.
- Core caches flushed; core context must be saved/restored.
- Useful only for long expected idle durations (tens of ms+).
- Wake requires reinitializing per-core hardware state (GIC redistributor, MMU
  registers, timer comparators).
- If a reschedule IPI arrives during power-down entry, the PSCI firmware may
  cancel the suspend or the kernel must handle "spurious wake" correctly.

### Placement frequency (per-wake vs. at-block-time)

Both Zircon and Linux re-evaluate placement at every wake-up. An alternative
(not used in surveyed production systems) is to fix placement at thread creation
or at block time and not re-evaluate at wake. The surveyed systems chose
per-wake re-evaluation because load changes between block and wake (the core
that was idle when the thread blocked may be busy when it wakes, and vice
versa).

seL4 MCS's explicit SC-binding is equivalent to "never re-evaluate" — placement
is fixed by userspace decision, not by any kernel observation at wake time.

---

## 6. References

### Systems Documentation

- Zircon kernel scheduling concepts (Fuchsia.dev):
  https://fuchsia.dev/fuchsia-src/concepts/kernel/kernel_scheduling
- Zircon fair scheduler (Fuchsia.dev):
  https://fuchsia.dev/fuchsia-src/concepts/kernel/fair_scheduler
- QNX Neutrino System Architecture 7.0, SMP chapter:
  https://get.qnx.com/developers/docs/7.0.0/com.qnx.doc.neutrino.sys_arch/topic/smp_HOWTHESMP.html
- seL4 MCS pre-release notes (10.1.1-mcs):
  https://docs.sel4.systems/releases/sel4/10.1.1-mcs.html
- seL4 cross-core migration discussion (devel@sel4.systems):
  https://www.mail-archive.com/devel@sel4.systems/msg02737.html

### Academic Papers

- Baumann et al., "The Multikernel: A New OS Architecture for Scalable Multicore
  Systems." SOSP 2009.
  https://people.inf.ethz.ch/troscoe/pubs/sosp09-barrelfish.pdf
- Peters et al., "For a Microkernel, a Big Lock Is Fine." APSys 2015. (seL4 BKL
  overhead data; ARM vs. x86 lock cost comparison)
- Lozi et al., "The Linux Scheduler: a Decade of Wasted Cores." EuroSys 2016.
  (Placement bugs and their costs in database workloads)
- "Thread Dispatching in Barrelfish." DiVA Portal 2014.
  https://www.diva-portal.org/smash/get/diva2:731492/FULLTEXT01.pdf
- "Dynamic Inter-core Scheduling in Barrelfish." DiVA Portal 2012.
  https://www.diva-portal.org/smash/get/diva2:482762/FULLTEXT01.pdf

### Linux Kernel Sources

- `kernel/sched/fair.c` — `select_task_rq_fair()`, `select_idle_sibling()`,
  `find_idlest_cpu()`, `find_energy_efficient_cpu()`
- `kernel/sched/core.c` — `try_to_wake_up()`, `ttwu_queue_wakelist()`
- `kernel/sched/idle.c` — idle loop, `cpuidle_idle_call()`, `wake_up_idle_cpu()`
- Linux EAS documentation: https://docs.kernel.org/scheduler/sched-energy.html
- Linux CPUIdle documentation:
  https://docs.kernel.org/admin-guide/pm/cpuidle.html

### ARM Hardware

- ARM Architecture Reference Manual (WFI, WFE, exclusive monitors, PSCI)
- ARM PSCI Specification, DEN0022 (CPU_SUSPEND, power states)
- ARM GICv3 Architecture Specification (SGI via ICC_SGI1R_EL1, redistributor)
- Trusted Firmware-A PSCI documentation:
  https://trustedfirmware-a.readthedocs.io/en/latest/design_documents/psci_osi_mode.html
