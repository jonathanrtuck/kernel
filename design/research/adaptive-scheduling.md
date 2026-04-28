# Adaptive Scheduling: Prior Art and Research

Research document for the open question: **how should a kernel dynamically
select and adapt scheduling algorithms based on workload?**

---

## Table of Contents

1. [EEVDF (Earliest Eligible Virtual Deadline First)](#1-eevdf-earliest-eligible-virtual-deadline-first)
2. [Scheduling Algorithm Portfolio](#2-scheduling-algorithm-portfolio)
3. [Adaptive and Self-Tuning Schedulers](#3-adaptive-and-self-tuning-schedulers)
4. [Online Optimization Theory](#4-online-optimization-theory)
5. [Production Kernel Scheduling Architectures](#5-production-kernel-scheduling-architectures)
6. [Workload Characterization](#6-workload-characterization)
7. [References](#7-references)

---

## 1. EEVDF (Earliest Eligible Virtual Deadline First)

### 1.1 The Original Algorithm

EEVDF was introduced by Ion Stoica and Hussein Abdel-Wahab in their 1995 paper
"Earliest Eligible Virtual Deadline First: A Flexible and Accurate Mechanism for
Proportional Share Resource Allocation." The algorithm was designed for
proportional-share scheduling in soft real-time systems, providing both fairness
guarantees and bounded latency.

**Core concepts from the 1995 paper:**

- **Virtual time.** Each client (task) has a _weight_ representing its share of
  the resource. The system maintains a global virtual time that advances in
  proportion to real time, scaled by the sum of active weights. Each client
  tracks its own virtual eligible time (VET) and virtual deadline (VD).

- **Virtual requests.** When a client requests a time quantum of length `q`, the
  system computes a virtual deadline as `VD = VET + q / w_i`, where `w_i` is the
  client's weight. The virtual eligible time advances to the previous virtual
  deadline upon each request: `VET_new = VD_old`.

- **The eligible concept.** A client is _eligible_ to run if its virtual
  eligible time is at or before the current system virtual time. This is the key
  distinction from pure EDF: not all tasks with early deadlines get to run. A
  task that has consumed more than its fair share has its eligible time pushed
  forward, making it ineligible even if its deadline is imminent. Only among the
  set of _eligible_ tasks does the scheduler pick the one with the earliest
  virtual deadline.

- **Lag.** The _lag_ of a client at real time `t` is defined as the difference
  between the ideal amount of service it should have received (in a perfectly
  fair fluid-flow model) and the actual service received. A positive lag means
  the task is owed time; a negative lag means it has consumed ahead of its
  share. The paper proves that lag is bounded: for any client `i` at any time,
  `|lag_i(t)| <= q_max`, where `q_max` is the maximum quantum size. This is the
  central fairness guarantee.

- **Time complexity.** The scheduler maintains clients in a data structure
  ordered by virtual deadline. Selection requires finding the eligible task with
  the earliest deadline, which in the original formulation is O(n) in the worst
  case but O(log n) with an augmented tree. The original paper uses a sorted
  list, noting that for typical task counts the constant factors dominate.

### 1.2 Linux's Adaptation (6.6+)

In March 2023, Peter Zijlstra posted the initial EEVDF patch set for the Linux
kernel's `sched_fair` class, replacing the Completely Fair Scheduler (CFS) that
had served since kernel 2.6.23 (October 2007). EEVDF landed in Linux 6.6
(October 2023). The implementation lives in `kernel/sched/fair.c`.

**Key adaptations from the original paper:**

- **Lag tracking replaces vruntime heuristics.** CFS tracked a single `vruntime`
  per task and always selected the task with the lowest vruntime (leftmost node
  in the red-black tree). EEVDF tracks both `vruntime` and `deadline` per
  `sched_entity`, and computes lag as the difference between ideal and actual
  service. The red-black tree is augmented to efficiently find the eligible task
  with the earliest virtual deadline.

- **Latency-nice.** Linux's EEVDF introduces a `latency_nice` attribute
  (complementing the existing `nice` value for weight). A lower `latency_nice`
  requests shorter time slices, which produces earlier virtual deadlines. This
  gives latency-sensitive tasks (audio decoders, game render loops, GUI event
  handlers) natural priority in deadline selection without changing their CPU
  share. The time slice for a task is computed as a function of its
  `latency_nice`, where smaller slices mean more frequent scheduling at the cost
  of higher context-switch overhead.

- **Eligibility enforcement.** The Linux implementation marks tasks as eligible
  when their lag is >= 0 (they are owed service or exactly caught up). When
  selecting the next task, the scheduler walks the augmented red-black tree to
  find the leftmost eligible node by virtual deadline. This replaces CFS's
  simpler leftmost-by-vruntime selection.

- **Slice-based preemption.** Under CFS, preemption was controlled by comparing
  vruntimes with a granularity check. Under EEVDF, preemption occurs when a
  newly woken task has an earlier virtual deadline than the running task and is
  eligible. The `sysctl_sched_min_granularity` tunable still prevents
  pathological preemption rates.

**What CFS got wrong that EEVDF addresses:**

1. **Latency unpredictability.** CFS selected purely by vruntime. A
   latency-sensitive task that had recently consumed CPU had a high vruntime and
   could be delayed behind batch tasks, even when it needed only microseconds of
   CPU. The Wine `wineserver` process was a notorious example: as a central
   event router, it consumed enough CPU to raise its vruntime, causing cascading
   latency for all dependent tasks.

2. **Heuristic fragility.** CFS relied on tunables like `sched_latency`,
   `sched_min_granularity`, and `sched_wakeup_granularity` to approximate good
   behavior. These interacted in complex ways, and optimal values depended on
   workload characteristics. EEVDF replaces these with a single mechanism
   (deadline selection among eligible tasks) that inherently balances latency
   and fairness.

3. **Sleeper fairness debates.** CFS had long-running disputes about how to
   credit tasks that voluntarily slept. Should a task that sleeps for 100ms get
   a vruntime bonus when it wakes? CFS went through multiple revisions of its
   sleeper fairness policy. EEVDF handles this structurally: a sleeping task
   accumulates positive lag (it's owed service), which makes it eligible with an
   early deadline when it wakes. No special-case logic required.

4. **Nice scaling.** CFS's exponential nice-to-weight mapping produced correct
   proportional sharing, but latency control required separate (and fragile)
   tuning. EEVDF decouples the two: `nice` controls CPU share (weight),
   `latency_nice` controls scheduling granularity (slice size and thus deadline
   spacing).

### 1.3 Performance Characteristics

**Strengths:**

- Mixed interactive/batch workloads: EEVDF excels because interactive tasks
  naturally get short slices and early deadlines while batch tasks get long
  slices and are not starved (they remain eligible and eventually have the
  earliest deadline among eligible tasks).
- Fairness is provably bounded. Lag never exceeds one maximum quantum, a
  property inherited from the original Stoica & Abdel-Wahab analysis.
- Reduced tuning surface. Fewer administrator-visible knobs compared to CFS.

**Weaknesses:**

- The augmented tree traversal for finding the earliest-deadline eligible task
  is more complex than CFS's simple leftmost selection. On very large runqueues
  (thousands of runnable tasks), this adds measurable overhead, though the
  constant factors are small relative to context-switch cost.
- EEVDF does not inherently handle real-time deadlines (hard timing
  constraints). It is a proportional-share algorithm with soft latency bounds.
  Hard real-time requires `SCHED_DEADLINE` or `SCHED_FIFO`.
- Under pure CPU-bound homogeneous workloads with identical nice values, EEVDF
  and CFS produce nearly identical behavior. The benefits appear primarily under
  heterogeneous workloads with mixed latency requirements.

### 1.4 Comparison Summary

| Property            | CFS                               | EEVDF                                             |
| ------------------- | --------------------------------- | ------------------------------------------------- |
| Selection criterion | Lowest vruntime                   | Earliest deadline among eligible                  |
| Latency control     | Tunables (fragile)                | Latency-nice (structural)                         |
| Fairness bound      | Empirical, tunable-dependent      | Provable: lag <= q_max                            |
| Sleeper handling    | Special-case heuristics           | Natural via lag accumulation                      |
| Data structure      | Red-black tree (vruntime)         | Augmented red-black tree (deadline + eligibility) |
| Time complexity     | O(log n) select, O(log n) insert  | O(log n) select, O(log n) insert                  |
| Preemption trigger  | vruntime comparison + granularity | Deadline comparison + eligibility                 |

---

## 2. Scheduling Algorithm Portfolio

### 2.1 Round Robin (RR)

**Mechanism.** Tasks are placed in a FIFO queue and each receives a fixed time
quantum (time slice). When a task's quantum expires, it moves to the back of the
queue. No priority distinctions.

**Optimizes for:** Simplicity and baseline fairness. Every task gets equal CPU
time in the long run.

**Strengths:**

- O(1) scheduling decisions.
- Zero starvation: every task runs within `n * quantum` time.
- Trivial to implement and reason about.

**Weaknesses:**

- No priority differentiation. A real-time audio thread and a background
  compression job get identical treatment.
- Quantum size is a fundamental tradeoff with no good universal answer. Too
  short: excessive context-switch overhead (typically 1-10 microseconds per
  switch on modern hardware, but cache/TLB pollution adds 10-100x more). Too
  long: poor interactive response.
- Convoy effect: short CPU-burst tasks queue behind long-running tasks.
- Measured data: Arpaci-Dusseau & Arpaci-Dusseau (OSTEP) show that with 3 tasks
  of 5-second bursts and a 1-second quantum, average turnaround time is 14
  seconds vs. 10 seconds for shortest-job-first.

**Used in:** POSIX `SCHED_RR`, Linux (as one scheduling policy), VxWorks, most
RTOS implementations as a baseline policy within a priority level.

### 2.2 Fixed Priority Preemptive

**Mechanism.** Each task is assigned a static priority. The highest-priority
runnable task always runs. Preemption is immediate when a higher-priority task
becomes runnable.

**Optimizes for:** Deterministic real-time response for the highest-priority
tasks.

**Rate Monotonic Scheduling (RMS):** Liu and Layland (1973) proved that for
periodic tasks, assigning priority inversely proportional to period (shorter
period = higher priority) is optimal among fixed-priority algorithms. The
utilization bound for `n` tasks is `n(2^(1/n) - 1)`, which converges to
`ln(2) ~ 0.693` as `n -> infinity`. For 2 tasks: 0.828. For 3: 0.780. This means
a fully loaded system can guarantee deadlines only if total CPU utilization
stays below ~69.3%.

**Priority inversion.** The central risk. The Mars Pathfinder incident (1997) is
the canonical example: a low-priority meteorological task held a mutex needed by
the high-priority information bus task. A medium-priority communications task
preempted the low-priority task, indirectly blocking the high-priority task.
Watchdog timeouts triggered system resets. The fix was enabling priority
inheritance on the mutex in VxWorks (the flag had been disabled for
performance). The Priority Inheritance Protocol (Sha, Rajkumar, Lehoczky, 1990)
and the Priority Ceiling Protocol address this by temporarily boosting the
lock-holder's priority, but add complexity and can cause priority inversion
chains in deeply nested locking scenarios.

**Strengths:**

- Deterministic worst-case response time (analyzable with response-time
  analysis).
- O(1) scheduling when implemented with a bitmap of priority levels and
  per-level FIFO queues.
- Well-understood theory (RMS, deadline monotonic scheduling).

**Weaknesses:**

- Starvation of low-priority tasks is inherent, not a bug.
- Utilization bound of ~69% is wasteful for non-real-time workloads.
- Priority assignment requires global knowledge of task timing requirements.
- Priority inversion requires mitigation protocols that add complexity.

**Used in:** VxWorks, FreeRTOS, QNX (POSIX `SCHED_FIFO` class), RTEMS, Linux
(`SCHED_FIFO`), most hard real-time systems.

### 2.3 EDF (Earliest Deadline First)

**Mechanism.** Among all runnable tasks, select the one whose absolute deadline
is nearest. Fully preemptive: if a newly arrived task has an earlier deadline
than the running task, preempt immediately.

**Optimizes for:** Maximum utilization under real-time constraints on a
uniprocessor.

**Key result:** Liu and Layland (1973) proved that EDF can schedule any task set
with total utilization <= 1.0 on a uniprocessor, compared to RMS's ~0.693 bound.
EDF is _optimal_ for uniprocessor preemptive scheduling of periodic tasks: if
any algorithm can meet all deadlines, EDF can.

**The domino effect under overload.** When total utilization exceeds 1.0, EDF
degrades catastrophically. Unlike fixed-priority scheduling where low-priority
tasks miss deadlines first (predictable degradation), EDF can cause _any_ task
to miss its deadline. The task that misses depends on the specific arrival
pattern, making failure analysis difficult. Buttazzo (2005, "Rate Monotonic vs.
EDF: Judgment Day") analyzed this extensively: under transient overload, EDF
misses deadlines unpredictably across the task set.

**Multiprocessor complications.** EDF is not optimal on multiprocessors. The
Dhall effect (Dhall & Liu, 1978) shows that global EDF can fail to schedule task
sets with utilization as low as `1 + epsilon` on `m` processors, even though the
per-processor utilization is barely above 1/m. Partitioned EDF (each task bound
to a processor) and semi-partitioned approaches address this but lose the
uniprocessor optimality result.

**Strengths:**

- Optimal uniprocessor utilization (100% bound).
- Simple selection criterion.
- No priority inversion problem (priorities are dynamic and transient).

**Weaknesses:**

- Domino effect under overload (unpredictable deadline misses).
- Harder schedulability analysis than RMS for complex task sets.
- Multiprocessor scheduling requires additional mechanisms.
- Higher runtime overhead than fixed-priority if deadline comparison is
  per-task.

**Used in:** Linux `SCHED_DEADLINE` (augmented with CBS, see below), ERIKA
Enterprise RTOS, SHARK kernel, various academic systems.

### 2.4 Stride Scheduling

**Mechanism.** Each client has a _ticket count_ representing its share. The
client's _stride_ is inversely proportional to its tickets:
`stride_i = LARGE_CONSTANT / tickets_i`. Each client maintains a _pass_ counter.
At each scheduling decision, the client with the lowest pass value is selected,
and its pass is incremented by its stride.

Introduced by Carl Waldspurger and William Weihl (MIT, 1995) as the
deterministic counterpart to lottery scheduling.

**Optimizes for:** Deterministic proportional share with low variance in
allocation rates.

**How it compares to lottery scheduling:** Lottery scheduling achieves the same
proportional shares _in expectation_ but with randomness that produces variance
in short-term allocation. Stride scheduling is strictly deterministic: a client
with twice the tickets runs exactly twice as often over any sufficiently long
window. Waldspurger's PhD thesis (1995) shows stride scheduling achieves
O(1)-accurate allocation after each stride period completes, while lottery
scheduling requires O(sqrt(n)) time to converge to within epsilon of the target
share.

**Strengths:**

- Deterministic proportional sharing (no randomness).
- O(log n) selection with a priority queue.
- Flexible share reallocation: change ticket count, recompute stride.
- Cross-applies to network and disk scheduling, not just CPU.

**Weaknesses:**

- New arrivals or departures require pass value initialization, which can cause
  transient unfairness. Waldspurger addresses this with "global stride"
  initialization but the bookkeeping is nontrivial.
- Dynamic weight changes require care to avoid discontinuities.
- No latency guarantees: a client with 1% of tickets runs once every 100
  scheduling decisions, regardless of urgency.

**Used in:** Xen hypervisor (credit scheduler is stride-based), research
systems. Not widely deployed in production OS kernels.

### 2.5 Lottery Scheduling

**Mechanism.** Each client holds _lottery tickets_ proportional to its desired
share. At each scheduling decision, a random ticket is drawn uniformly, and the
holding client runs. Introduced by Waldspurger and Weihl (OSDI 1994).

**Optimizes for:** Flexible proportional share with minimal global coordination.

**Key mechanisms:**

- **Ticket transfer.** A client can temporarily transfer its tickets to another
  client. In a client-server model, a client making an RPC transfers its tickets
  to the server, ensuring the server runs with the client's priority while
  handling its request. This elegantly solves priority donation in IPC.

- **Ticket inflation.** A mutually trusting set of clients can adjust their own
  ticket counts without global coordination. A client needing more CPU
  temporarily inflates its tickets. This requires trust (a malicious client
  could inflate to dominate), but avoids centralized reallocation.

- **Ticket currencies.** Each group of clients can define its own currency, with
  an exchange rate to the base currency. This supports hierarchical resource
  management: a department's 1000 tickets can be subdivided among its processes
  in "department dollars."

**Strengths:**

- Probabilistically correct proportional sharing without deterministic
  bookkeeping.
- O(1) scheduling decision (draw random number, binary search or walk the ticket
  space).
- Ticket transfer naturally solves priority donation.
- Composable via currencies.

**Weaknesses:**

- Short-term variance. A client with 50% of tickets may get 30% or 70% in any
  given 10-quantum window. Waldspurger's measurements on a Mach 3.0 prototype
  showed convergence to within 1% of target share after ~100 quanta.
- Randomness makes worst-case analysis impossible (relevant for real-time).
- Ticket inflation requires trust, limiting its use to cooperative domains.

**Used in:** Original implementation on Mach 3.0. Influenced Xen's credit
scheduler. Conceptual influence on Linux CFS (shares-based scheduling).

### 2.6 Multilevel Feedback Queue (MLFQ)

**Mechanism.** Multiple queues at different priority levels. New tasks enter the
highest-priority queue. If a task exhausts its time quantum at level `k`, it is
demoted to level `k+1` (lower priority, longer quantum). If a task blocks for
I/O before exhausting its quantum, it stays at the same level (or is promoted).
Periodically, all tasks are boosted back to the top queue to prevent starvation
("priority boost").

Originated with Fernando Corbato's Compatible Time Sharing System (CTSS, 1962)
at MIT, one of the earliest timesharing systems.

**Optimizes for:** Automatic workload classification. Interactive tasks (short
CPU bursts, frequent I/O) naturally stay at high priority. Batch tasks (long CPU
bursts) sink to lower priority with longer quanta (better throughput, less
context-switch overhead).

**BSD 4.3 adaptation.** The 4.3BSD scheduler used a variant with 32 run queues
spanning priorities 0-127. CPU usage was tracked via a decaying average
(`p_cpu`), and priority was recalculated every 4 clock ticks as
`p_pri = PUSER + p_cpu/4 + 2 * p_nice`. The `p_cpu` value decayed exponentially
with a load-dependent factor, causing heavily CPU-bound tasks to drop in
priority while I/O-bound tasks maintained high priority. This was the
predominant Unix scheduler for two decades.

**Starvation risks and mitigations:**

- Without periodic boosting, a CPU-bound task can be permanently stuck at the
  lowest queue while new tasks keep arriving at the top.
- _Gaming_: a task can issue a trivial I/O operation (read 1 byte from
  /dev/null) just before its quantum expires, tricking the scheduler into
  keeping it at high priority. Solaris addressed this by tracking total CPU time
  across queue levels, not just per-quantum behavior.
- Periodic boost (all tasks to top queue every `S` seconds) prevents starvation
  but causes transient priority inversions.

**Strengths:**

- No a priori knowledge of task behavior required.
- Automatic adaptation to workload phase changes.
- Robust in practice: dominated Unix/BSD scheduling for decades.

**Weaknesses:**

- Many tunable parameters (number of queues, quantum sizes per level, boost
  interval, decay rate) with complex interactions.
- Gaming vulnerability without total-CPU-time tracking.
- Not formally analyzable: no provable fairness or latency bounds.

**Used in:** 4.3BSD, most traditional Unix systems, Solaris TS class, Windows
NT/2000/XP/7 (with modifications), FreeBSD 4BSD scheduler.

### 2.7 Work Stealing

**Mechanism.** Each processor maintains a local deque (double-ended queue) of
tasks. When a processor's deque is empty, it _steals_ a task from the top of
another processor's deque (the oldest/largest task). Processors push newly
spawned tasks onto the bottom of their own deque and pop work from the bottom
(LIFO for local work, FIFO for stolen work).

Formalized by Blumofe and Leiserson (1999) as the scheduling algorithm for Cilk,
their multithreaded programming system at MIT.

**Optimizes for:** Parallel task workloads with fine-grained fork-join
parallelism.

**Theoretical result:** The expected execution time on `P` processors is
`T_1/P + O(T_inf)`, where `T_1` is the total work (serial execution time) and
`T_inf` is the critical path length (span). The expected number of steal
attempts is `O(P * T_inf)`, which means communication overhead is proportional
to the parallelism deficit, not the total work. Space usage is bounded by
`P * S_1`, where `S_1` is the serial stack depth.

**Continuation stealing vs. child stealing.** Cilk uses _continuation stealing_:
when a function spawns a child task, the spawning processor executes the child
and makes the continuation available for stealing. This preserves the serial
execution order on each processor and keeps stack depth bounded. The
alternative, _child stealing_ (the spawned child is made available for
stealing), is used in Java's ForkJoinPool and Intel TBB.

**Strengths:**

- Provably efficient for fork-join parallelism.
- Minimal overhead when parallelism matches processor count (no steals needed).
- Locality-friendly: LIFO local execution keeps recently allocated data in
  cache.
- No central scheduler: fully decentralized, scales to hundreds of cores.

**Weaknesses:**

- Not designed for general-purpose scheduling (tasks must be independent or
  fork-join structured).
- Steal latency causes load imbalance spikes during transient phases.
- Lock-free deque implementations are complex (the ABP deque from Arora,
  Blumofe, Plaxton 1998 is the standard but subtle).
- No priority support: all tasks are equally eligible for stealing.

**Used in:** Cilk/Cilk Plus, Intel TBB (Threading Building Blocks), Java
ForkJoinPool, .NET Task Parallel Library, Go runtime (goroutine scheduler uses a
work-stealing variant), Rust Tokio (work-stealing task scheduler).

### 2.8 Batch/FIFO (Non-Preemptive)

**Mechanism.** Tasks run to completion (or until voluntary yield) in arrival
order. No preemption.

**Optimizes for:** Maximum throughput by eliminating all context-switch
overhead.

**When it makes sense:**

- Database query execution within a single worker thread.
- GPU compute shader dispatch (kernels are non-preemptive on most hardware).
- Batch processing pipelines where all tasks are equally important.
- Kernel interrupt bottom halves that must complete atomically.

**Strengths:**

- Zero scheduling overhead.
- Maximum cache utilization (no context pollution).
- Deterministic execution order.

**Weaknesses:**

- Starvation is structural, not a risk but a certainty: a long task blocks
  everything behind it.
- No responsiveness guarantee whatsoever.
- Convoy effect: one slow task raises the average completion time of all tasks.

**Used in:** Mainframe batch systems, Linux `SCHED_BATCH` (advisory, CFS still
preempts but with longer quanta), GPU compute pipelines.

### 2.9 Energy-Aware Scheduling (EAS)

**Mechanism.** When placing a waking task, the scheduler estimates the energy
cost of running it on each candidate CPU using an energy model, and selects the
placement that minimizes total system energy. The energy model captures the
relationship between CPU utilization, operating performance point (OPP/DVFS
level), and power consumption for each CPU type.

Developed primarily by ARM for big.LITTLE and DynamIQ heterogeneous CPU
topologies. Merged into Linux 5.0 (March 2019).

**Optimizes for:** Energy efficiency on heterogeneous CPU topologies, subject to
a performance floor.

**How it works in Linux:**

1. Each CPU type provides an energy model via `struct em_perf_domain`, mapping
   OPP levels to power consumption.
2. On task wakeup, the scheduler (in `select_task_rq_fair`) calls
   `find_energy_ efficient_cpu()`, which computes the marginal energy cost of
   placing the task on each CPU in the system.
3. A task may be placed on a big core even if a little core has capacity,
   because placing it on the little core could force an OPP increase that raises
   energy for _all_ tasks on that little cluster. The scheduler reasons about
   system-wide energy, not just the new task.
4. EAS is disabled when any CPU exceeds 80% utilization ("over-utilized"
   threshold). Under high load, traditional load balancing takes over because
   performance becomes the bottleneck, not energy.
5. EAS coordinates with the `schedutil` cpufreq governor: both use the same
   Per-Entity Load Tracking (PELT) metrics, ensuring frequency selection and
   task placement are coherent.

**Strengths:**

- Measurable energy savings on heterogeneous platforms (ARM reports 5-15% on
  typical mobile workloads).
- Transparent to applications: no API changes needed.
- Graceful degradation: falls back to load balancing under high utilization.

**Weaknesses:**

- Requires an accurate energy model, which varies by SoC and is typically
  provided by the hardware vendor. Inaccurate models produce suboptimal
  placement.
- Only effective on heterogeneous topologies. On symmetric systems, all CPUs
  have the same energy profile and EAS provides no benefit.
- The energy computation adds overhead to every task wakeup, though this is
  bounded (proportional to the number of performance domains, typically 2-3).
- The 80% over-utilized threshold is a heuristic; there is no formal proof that
  it optimally balances energy and performance.

**Used in:** Linux (mainline since 5.0), Android (primary scheduler for mobile),
ChromeOS. Intel has proposed adapting EAS for x86 hybrid CPUs (Alder Lake and
later) but this remains in development.

### 2.10 BVT (Borrowed Virtual Time)

**Mechanism.** Each thread has an _actual virtual time_ (AVT) that advances by
`quantum / weight` when it runs, providing proportional sharing via virtual time
(similar to CFS). The key innovation is the _warp_ mechanism: a thread can
temporarily subtract a _warp value_ from its AVT, yielding an _effective virtual
time_ (EVT = AVT - warp) that moves it ahead of other threads in the scheduling
queue. A _warp time limit_ caps how long a thread can remain warped, preventing
monopolization.

Introduced by Kenneth Duda and David Cheriton (Stanford, SOSP 1999).

**Optimizes for:** Low-latency dispatching within a proportional-share
framework.

**The warp mechanism in detail:**

- When a latency-sensitive thread wakes (e.g., an MPEG decoder receiving a
  frame), it is dispatched quickly because its EVT is lower than other threads'
  AVTs.
- If the thread enters an infinite loop (bug or malice), the warp time limit
  expires, the warp is disabled, and the thread reverts to its AVT, allowing
  fair sharing to reassert.
- Warp is per-thread and configurable, allowing different latency budgets for
  different threads.

**Strengths:**

- Supports both proportional sharing and low-latency dispatch in a single
  mechanism.
- Simple implementation: O(log n) with a heap ordered by EVT.
- Warp time limit prevents latency-sensitive threads from starving others.

**Weaknesses:**

- Warp values and limits require per-thread configuration (not self-tuning).
- No formal latency bound: warp provides probabilistic improvement, not a
  guarantee.
- Sensitivity to warp parameter tuning: too much warp defeats fairness, too
  little defeats latency.

**Used in:** Xen hypervisor adopted BVT as its original scheduler (circa 2003,
later replaced by credit scheduler). Influenced VMware ESX scheduler design.

### 2.11 Other Notable Algorithms

**Ghetto scheduling (Barrelfish).** Barrelfish's multikernel architecture runs a
separate kernel ("CPU driver") on each core, with user-level dispatchers
multiplexed by each core's scheduler. The term "ghetto scheduling" refers to the
deliberate simplicity: each core runs a minimal round-robin among dispatchers,
while all sophisticated scheduling policy lives in user-space "system services."
Cross-core coordination happens via explicit message passing, not shared state.
The System Knowledge Base (SKB) maintains a Prolog-like knowledge base of
hardware topology that schedulers can query to make placement decisions. The
architecture treats scheduling as a distributed systems problem.

**seL4 MCS (Mixed-Criticality Scheduling).** seL4's MCS extensions introduce
_scheduling contexts_ as first-class kernel capabilities. A scheduling context
encodes a `(budget, period)` pair, enforced by a sporadic server algorithm. Key
properties:

- A thread can only execute if it holds a scheduling context capability. Time is
  a capability-controlled resource, not an implicit property of threads.
- The sporadic server maintains a sliding window constraint: a thread cannot
  consume more than `budget` microseconds in any `period` window. Enforcement
  uses a set of _replenishments_ that track eligible budget chunks.
- Scheduling contexts can be _donated_ via IPC. When a client calls a server via
  `seL4_Call`, the client's scheduling context is temporarily transferred to the
  server, ensuring the server runs on the client's time budget. This provides
  temporal isolation: a misbehaving server only burns the calling client's time.
- _Passive servers_ have their scheduling context unbound when blocked on an
  endpoint, consuming zero CPU. They reactivate on a client's context when
  called.
- Timeout fault handlers can be registered, allowing threads to be notified when
  their budget expires.

**Linux SCHED_DEADLINE.** Linux's real-time scheduling class combines EDF with
the Constant Bandwidth Server (CBS) algorithm (Abeni & Buttazzo, 1998). Each
task specifies `(runtime, deadline, period)`: the task receives `runtime`
microseconds every `period`, and the runtime is available within `deadline`
microseconds of each period start. CBS provides bandwidth isolation: a task that
overruns its runtime has its deadline pushed forward, preventing it from
stealing bandwidth from other tasks. The combination is optimal for uniprocessor
real-time scheduling with isolation, and is used for multimedia, industrial
control, and telecommunications workloads. Source: `kernel/sched/deadline.c`.

**Linux sched_ext.** Merged in Linux 6.12 (2024), `sched_ext` is a new
scheduling class whose behavior is defined by BPF programs loaded from
userspace. It sits between `SCHED_IDLE` and `SCHED_NORMAL` in the priority
stack, allowing experimentation with scheduling algorithms without kernel
recompilation or reboot. If a BPF scheduler misbehaves (fails to schedule a task
for ~30 seconds), the kernel kills it and falls back to the default scheduler.
This framework enables runtime algorithm switching: different BPF schedulers can
be loaded for different workload phases, making it a form of adaptive
scheduling. The `sched_ext` project (`github.com/sched-ext/scx`) includes
example schedulers: `scx_rusty` (Rust-based), `scx_lavd` (latency- aware),
`scx_central` (single-CPU dispatch), and others.

---

## 3. Adaptive and Self-Tuning Schedulers

### 3.1 Bossa: A DSL Framework for Scheduler Composition

Bossa (Lawall, Muller, et al., INRIA, 2002) is a kernel-level framework for
implementing and composing scheduling policies using a domain-specific language
(DSL). The Bossa DSL provides high-level scheduling abstractions (queues,
timers, process states) while a verifier ensures that user-defined policies
cannot deadlock the scheduler or corrupt kernel state.

**Key contributions:**

- **Event-driven policy specification.** Scheduling policies are expressed as
  responses to events (task creation, task wake, timer expiry, task exit). Each
  event handler selects a task from the available queues and optionally modifies
  task state. This separates mechanism (context switch, timer programming) from
  policy (which task to run).

- **Hierarchical composition.** Bossa Nova (Lawall et al., 2005) extends the
  original with modularity: a tree of schedulers can be composed, where each
  node applies a different policy to its subtree. For example, a top-level
  scheduler partitions CPU time between a real-time group (fixed priority) and a
  timesharing group (MLFQ), with each group running its own policy internally.

- **Safety guarantees through the DSL.** Because the DSL is domain-restricted
  (no general loops, no pointer arithmetic), the Bossa verifier can statically
  ensure that policies terminate, do not access invalid memory, and always
  select a runnable task (no "return nothing" bug).

**Limitations:** Bossa never achieved mainstream adoption. The DSL added a
learning curve, and the Linux kernel community preferred runtime flexibility
(cgroups, sched_ext) over compile-time policy composition.

### 3.2 Scheduler Activations

Anderson, Bershad, Lazowska, and Levy (SOSP 1991) proposed _scheduler
activations_ as a mechanism for user-level thread schedulers to cooperate with
the kernel. The problem: pure user-level threading is fast but cannot handle
blocking kernel calls (the entire process blocks), while kernel threading is
correct but slow (every scheduling decision requires a system call).

**Mechanism:**

1. The kernel allocates _virtual processors_ to each application.
2. When a thread blocks in the kernel (e.g., page fault, I/O), the kernel sends
   an _upcall_ to the application's user-level scheduler, notifying it that a
   virtual processor is available and one of its threads is blocked.
3. The user-level scheduler can then run another thread on the now-free virtual
   processor.
4. When the blocked thread becomes runnable again, another upcall notifies the
   user-level scheduler.

This allows applications to implement their own scheduling policies (priority,
deadline, application-specific heuristics) while the kernel handles resource
allocation among applications. The N:M threading model maps N user threads onto
M kernel entities.

**Adoption and fate:** Implemented in NetBSD (by Nathan Williams), Solaris
(lightweight processes + user threads), and influenced Windows fibers and Go's
goroutine scheduler. However, most systems have since moved to 1:1 threading
(one kernel thread per user thread), trading the flexibility of user-level
scheduling for simplicity and predictability. Linux never adopted scheduler
activations; NPTL (Native POSIX Thread Library) uses 1:1 threading. FreeBSD
similarly uses 1:1 threading in its modern threading model.

**Why it matters for adaptive scheduling:** Scheduler activations demonstrate
the principle that different layers of the system can run different scheduling
policies simultaneously, with a well-defined interface between them. The kernel
schedules virtual processors among applications; each application schedules
threads among virtual processors.

### 3.3 Linux's Interactivity Heuristics (O(1) Scheduler)

The Linux O(1) scheduler (Ingo Molnar, 2002, Linux 2.5/2.6) attempted to
automatically classify tasks as interactive or batch using sleep-time analysis.

**Mechanism:**

- Each task accumulated a _sleep average_: time spent sleeping was added, time
  spent running was subtracted, with exponential decay.
- The sleep average was mapped to a _dynamic priority bonus_ ranging from -5 to
  +5, added to the task's static nice priority.
- Tasks with high sleep averages (lots of sleeping, little running) were
  classified as interactive and received priority bonuses. Tasks with low sleep
  averages were classified as CPU-bound and penalized.
- Interactive tasks were placed on the _active_ array; when they exhausted their
  time slice, they could be placed back on the active array (instead of the
  expired array) if their interactivity score was high enough.

**Why it was abandoned:**

1. **Heuristic fragility.** The sleep average interacted poorly with certain
   workloads. A task that alternated between short bursts of CPU and short
   sleeps could oscillate between interactive and batch classification,
   producing inconsistent behavior.
2. **Gaming vulnerability.** Tasks could exploit the heuristic by sleeping just
   long enough to maintain a high sleep average, then consuming CPU at elevated
   priority.
3. **Non-composability.** The bonus/penalty system interacted with nice values,
   real-time priorities, and the active/expired array swap in ways that were
   difficult to reason about or predict.
4. **Irreducible complexity.** Multiple attempts to fix edge cases (the
   "backboost" patch, the "sleep granularity" patch, various starvation fixes)
   added complexity without resolving the fundamental problem: heuristic
   classification of task behavior is unreliable.

Con Kolivas's alternative approaches (Staircase, RSDL/SD, and eventually BFS)
demonstrated that simpler algorithms with structural fairness properties could
outperform complex heuristics. Ingo Molnar's CFS (2.6.23, 2007) adopted this
insight, replacing the interactivity heuristics with virtual-runtime-based
scheduling.

### 3.4 Windows MMCSS (Multimedia Class Scheduler Service)

Windows Vista introduced MMCSS as a user-mode service (running as a system
service in `svchost.exe`) that temporarily boosts thread priorities for
multimedia workloads.

**Mechanism:**

1. A thread registers with MMCSS by calling `AvSetMmThreadCharacteristics()`,
   specifying a task category (e.g., "Audio", "Pro Audio", "Games", "Playback").
2. MMCSS reads registry-defined scheduling profiles for the category, which
   specify a _scheduling category_ (High, Medium, Low), _priority_ within that
   category, and a _background priority_.
3. While registered, the thread's priority is boosted to the profile level. "Pro
   Audio" threads are boosted to priority 26 (near real-time). "Playback"
   threads to priority 24.
4. MMCSS monitors CPU consumption. If a boosted thread consumes more than its
   allocated duty cycle (default: 80% of every 10ms scheduling period for the
   "high" category), MMCSS reduces its priority to the background level for the
   remainder of the period.

**Scheduling categories and priorities:**

| Category  | Base Priority | Duty Cycle    |
| --------- | ------------- | ------------- |
| Pro Audio | 26            | 80% of period |
| Audio     | 24            | 80% of period |
| Playback  | 24            | 80% of period |
| Games     | 8-15          | 80% of period |

**Quota management.** The duty cycle mechanism prevents boosted threads from
monopolizing the CPU. If all multimedia threads together consume more than their
combined duty cycle, MMCSS drops them to background priority for the remainder
of the scheduling period, ensuring that lower-priority system tasks (disk
indexing, updates) still make progress.

**Strengths:**

- Opt-in: only threads that register are affected.
- Category-based: different multimedia workloads get appropriate priority
  levels.
- Duty cycle prevents starvation of non-multimedia tasks.

**Weaknesses:**

- User-mode service adds latency compared to kernel-level solutions. The thread
  must cross the user/kernel boundary for priority changes.
- Fixed profiles: the categories and their priorities are defined at install
  time in the registry. No runtime learning.
- Limited to multimedia workloads. Does not help general interactive
  responsiveness.

### 3.5 QNX Adaptive Partitioning

QNX's Adaptive Partitioning (APS) scheduler provides guaranteed minimum CPU
budgets to groups of threads (partitions) while allowing unused budget to flow
to other partitions.

**Mechanism:**

1. Each partition is assigned a _budget percentage_ (e.g., safety-critical
   tasks: 40%, navigation: 30%, entertainment: 20%, system: 10%). Budgets across
   all partitions sum to 100%.
2. The scheduler measures CPU consumption per partition over a rolling
   _averaging window_ (default: 100ms, configurable from 20ms to 2000ms).
3. **Under load:** When total CPU demand exceeds 100%, each partition is
   guaranteed its budget. If partition A has a 40% budget and the system is
   fully loaded, partition A receives exactly 40% of CPU time. This is enforced
   per-tick by throttling partitions that exceed their budget.
4. **Under light load:** When some partitions are idle or underutilizing, their
   unused budget is redistributed to active partitions proportional to their
   weights. A partition with a 30% budget can use 90% of CPU if no other
   partition needs time.
5. **Within each partition**, standard QNX priority-based scheduling applies.
   The highest-priority runnable thread within a partition runs. APS only limits
   how much total CPU a partition can consume, not which thread within the
   partition runs.

**Critical budgets.** Partitions can be marked as _critical_, allowing them to
temporarily exceed their budget (borrowing from the system partition) for
time-critical operations. This is audited and rate-limited.

**Key design property:** APS provides isolation under overload and opportunistic
sharing under light load. The averaging window smooths out per-tick variations,
preventing thrashing when workloads have bursty CPU patterns.

**Strengths:**

- Hard guarantee: misbehaving partition cannot steal CPU from other partitions.
- Adaptive redistribution: unused CPU is never wasted.
- Composable with priority scheduling within partitions.
- Averaging window prevents oscillation.

**Weaknesses:**

- Per-tick enforcement adds overhead (partition accounting on every clock tick).
- Averaging window introduces lag: a budget violation may not be corrected for
  up to one full window.
- Budget assignment requires system integrator to understand partition
  requirements.

**Used in:** QNX Neutrino RTOS (automotive, industrial, medical). Widely
deployed in car infotainment and instrument cluster systems where
safety-critical and non-critical workloads share a processor.

### 3.6 macOS/iOS QoS Classes

Apple's Grand Central Dispatch (GCD) maps Quality of Service (QoS) classes to
scheduling behavior on both symmetric and asymmetric (Apple Silicon) hardware.

**QoS hierarchy:**

| QoS Class        | Purpose                                   | Typical Latency Target | Apple Silicon Core Affinity |
| ---------------- | ----------------------------------------- | ---------------------- | --------------------------- |
| User Interactive | UI updates, animations                    | < 16ms (60fps)         | P-cores preferred           |
| User Initiated   | Opening documents, user-triggered actions | < 1s                   | P-cores preferred           |
| Default          | General work (no explicit QoS)            | Seconds                | P or E-cores                |
| Utility          | Long-running tasks with progress UI       | Minutes                | E-cores preferred           |
| Background       | Backups, indexing, prefetching            | Unlimited              | E-cores preferred           |

**Scheduling integration on Apple Silicon:**

- The XNU kernel's scheduler is aware of core asymmetry. P-cores (Firestorm,
  Avalanche, etc.) have higher single-thread performance and power consumption;
  E-cores (Icestorm, Blizzard, etc.) have lower power.
- QoS class influences core placement: `userInteractive` and `userInitiated`
  work is steered toward P-cores. `utility` and `background` work toward
  E-cores.
- GCD integrates with XNU's scheduler: dispatch queue priorities map to QoS
  classes, which map to kernel thread priorities and core placement hints.
- The kernel monitors thermal pressure and can _downgrade_ core placement when
  the device is thermally constrained, moving work from P-cores to E-cores to
  reduce heat.

**Adaptation triggers:**

- Application foreground/background transitions change effective QoS. A
  backgrounded app's dispatch queues are transparently downgraded.
- Thermal pressure: sustained high P-core utilization triggers thermal
  throttling, which reduces clock speed or migrates work to E-cores.
- Battery level: low battery state may increase E-core preference.

**Strengths:**

- Integrates scheduling policy with application intent (QoS is declared by the
  developer, not inferred).
- Seamless asymmetric core support.
- Energy-efficient by design: most work defaults to low-power execution.

**Weaknesses:**

- Requires developer opt-in. Applications that do not declare QoS get "default"
  treatment, missing optimization opportunities.
- QoS is coarse (5 levels). Fine-grained scheduling within a QoS class is not
  controllable.
- Proprietary: the exact scheduling heuristics for core placement are not
  publicly documented.

### 3.7 Academic Work on Runtime Scheduler Switching

**ALPS (Automatic Load-balancing Policy Selector).** Pilla et al. (2014)
demonstrated runtime selection among load-balancing strategies in Charm++ based
on application phase detection. The system monitors load imbalance metrics and
switches between greedy, refinement, and hybrid strategies.

**Tesseract.** Ousterhout et al. (2019, UC Berkeley) proposed a system where
scheduling policies are expressed as constraint optimization problems, with the
runtime solver choosing the best policy for current conditions. This treats
scheduling as an online optimization problem rather than a fixed algorithm.

**Linux sched_ext as runtime switching.** While not originally designed for
automatic adaptation, sched_ext enables runtime scheduler replacement: a daemon
can monitor workload characteristics, unload the current BPF scheduler, and load
a different one. This is the closest production mechanism to true runtime
scheduler switching in a general-purpose OS.

### 3.8 Adaptation Triggers, Measurements, and Failure Modes

Systems that adapt scheduling behavior must answer three questions: _what_ to
measure, _when_ to adapt, and _how_ to avoid making things worse.

**What systems measure:**

| System        | Primary Metric                      | Measurement Method                             |
| ------------- | ----------------------------------- | ---------------------------------------------- |
| Linux O(1)    | Sleep average                       | Exponential decay of sleep/run times           |
| QNX APS       | Partition CPU usage                 | Per-tick accounting over averaging window      |
| macOS QoS     | Developer-declared intent + thermal | QoS API + thermal sensor polling               |
| Windows MMCSS | Thread CPU consumption              | Duty cycle monitoring within scheduling period |
| EEVDF         | Lag (ideal - actual service)        | Virtual time arithmetic                        |

**Common failure modes of adaptive schedulers:**

1. **Oscillation.** The scheduler alternates between two policies, never
   settling. Typically caused by adaptation thresholds too close together (the
   system crosses the threshold in one direction, adapts, and immediately
   crosses back). QNX's averaging window and Linux CFS's sched_min_granularity
   are examples of anti-oscillation measures.

2. **Starvation during transitions.** When switching policies, tasks scheduled
   under the old policy may have accumulated state (priority, virtual time) that
   is meaningless under the new policy. If not carefully translated, some tasks
   may be starved or over-served after the transition.

3. **Measurement overhead.** Per-task, per-tick measurements consume CPU that
   could be used for application work. Linux's PELT (Per-Entity Load Tracking)
   adds ~1% overhead on task wakeup. More sophisticated measurements (hardware
   performance counters, instruction mix analysis) add more.

4. **Hysteresis failures.** Too much hysteresis causes slow response to genuine
   workload changes. Too little causes oscillation. There is no universal
   correct value; it depends on workload variability.

5. **Gaming.** Adaptive schedulers that reward certain behaviors (sleeping, low
   CPU usage) can be exploited by tasks that mimic those behaviors to gain
   priority. The Linux O(1) scheduler's interactivity heuristic was the most
   prominent victim.

---

## 4. Online Optimization Theory

### 4.1 Framework

Adaptive scheduling can be formalized as an _online optimization problem_: the
scheduler must make irrevocable decisions (which task to run next, on which
core) without knowing future arrivals, departures, or behavior changes. The
theoretical question is: how close can an online algorithm get to the
performance of an offline optimal algorithm that knows the entire input sequence
in advance?

### 4.2 Online Bipartite Matching

Karp, Vazirani, and Vazirani (STOC 1990) introduced the online bipartite
matching problem: vertices on one side arrive online and must be immediately
matched to vertices on the other side. Their _Ranking_ algorithm achieves a
competitive ratio of `1 - 1/e ~ 0.632`, meaning it produces a matching that is
at least 63.2% as large as the offline optimal. This bound is tight for
deterministic online algorithms.

**Relevance to scheduling:** When tasks arrive online and must be assigned to
cores (each with different characteristics, cache state, or assigned scheduling
policy), the problem resembles online matching. The competitive ratio provides a
theoretical floor on how well any online scheduler can perform relative to an
omniscient oracle.

With random arrival order (rather than adversarial), the competitive ratio
improves to ~0.696 (Karp, Vazirani, Vazirani; improved by Mahdian & Yan 2011).

### 4.3 Online Facility Location

In the online facility location problem, clients arrive sequentially and must be
assigned to an open facility, or a new facility must be opened. Opening a
facility has a fixed cost; assigning a client to a distant facility has a
connection cost.

**Scheduling analogy:** "Opening a facility" corresponds to activating a new
scheduling policy on a core (startup cost: building data structures, migrating
tasks). "Connection cost" corresponds to the inefficiency of running a task
under a suboptimal policy. The goal is to minimize total cost (switching cost +
misfit cost).

Meyerson (FOCS 2001) gave an O(log n)-competitive randomized algorithm for
online facility location, later improved to O(log n / log log n). For the
scheduling analogy, this suggests that the cost of dynamically adapting policies
grows logarithmically with the number of tasks, not linearly.

### 4.4 Clustering with Capacity Constraints

When partitioning tasks into groups (each group assigned to a core or policy),
the problem maps to online clustering. Charikar et al. (1997) studied
incremental k-median clustering and showed O(1)-competitive algorithms exist for
the metric case. With capacity constraints (each core or policy can handle at
most `c` tasks), the problem becomes harder.

**Practical relevance:** If each core runs one scheduling algorithm and has a
capacity limit on runqueue length, task assignment is a capacitated clustering
problem. Heuristics (assign to nearest center, reassign when over capacity) work
well in practice despite poor theoretical worst cases.

### 4.5 Control-Theoretic Approaches

Hellerstein, Diao, Parekh, and Tilbury ("Feedback Control of Computing Systems,"
Wiley, 2004) formalized the application of control theory to computing resource
management.

**PID-style feedback for scheduling:**

A control loop for adaptive scheduling would measure the _error_ (difference
between desired and actual scheduling metric), apply proportional, integral, and
derivative corrections, and output a scheduling parameter adjustment.

Example control loop for a dynamic time quantum:

```text
error(t)   = target_latency - measured_p99_latency
integral   += error(t) * dt
derivative = (error(t) - error(t-1)) / dt
quantum    = quantum_base + Kp * error + Ki * integral + Kd * derivative
```

**Challenges specific to scheduling:**

1. **Non-linearity.** The relationship between scheduling parameters and
   performance metrics is non-linear and discontinuous. Doubling the time
   quantum does not halve latency; it may have no effect or cause a step change
   depending on workload.
2. **Multiple interacting control variables.** Time quantum, number of priority
   levels, load balancing frequency, and migration thresholds all interact.
   Multi-input multi-output (MIMO) controllers are required, which are
   significantly harder to tune than single-loop PID.
3. **Measurement delay.** Scheduling metrics (tail latency, throughput, fairness
   index) require observation windows of 10ms-1s to be statistically meaningful,
   introducing phase lag into the control loop.
4. **Plant model changes.** The system being controlled (the set of running
   tasks) changes continuously as tasks arrive and depart, requiring adaptive or
   robust control techniques.

Hellerstein et al. recommend integral control with anti-windup as a starting
point for computing systems, noting that proportional-only control leaves
steady-state error and derivative control amplifies measurement noise.

### 4.6 Stability and Convergence

An adaptive scheduler must converge to a good policy and remain there. Two
concerns:

**Lyapunov stability.** A scheduling system is stable if, after a perturbation
(e.g., a burst of new tasks), it returns to a bounded operating region.
Instability manifests as runqueue lengths growing without bound, latency
increasing monotonically, or throughput oscillating with increasing amplitude.

For proportional-share schedulers (CFS, EEVDF, stride, lottery), stability is
typically proven via the lag bound: the difference between ideal and actual
service is bounded, so the system cannot drift arbitrarily far from fair
allocation.

**Convergence rate.** How quickly does the scheduler reach good performance
after a workload change? Lottery scheduling converges to within epsilon of
target share after O(1/epsilon^2) quanta (law of large numbers). Stride
scheduling converges in O(1) scheduling cycles. EEVDF converges within one
maximum quantum. Adaptive schemes that use averaging windows (QNX APS) converge
in one window length (100ms default). Control-theoretic schemes converge at a
rate determined by the controller bandwidth, typically 2-10 control periods.

### 4.7 Hysteresis

Hysteresis is deliberate inertia in adaptation decisions: the threshold for
switching _to_ a policy is different (higher) than the threshold for switching
_away_ from it. This prevents thrashing when the system is near a decision
boundary.

**Formal model:** A hysteresis function `H(metric, state)` returns a decision
(switch or stay) based on both the current metric value and the current state.
If currently running policy A, the metric must exceed `threshold_high` to switch
to B. Once in B, the metric must drop below `threshold_low` (where
`threshold_low < threshold_high`) to switch back to A. The gap
`threshold_high - threshold_low` is the hysteresis band.

**Tuning the band:**

- Too narrow: oscillation when the metric fluctuates near the threshold.
- Too wide: slow response to genuine workload changes. The system is "stuck" in
  a suboptimal policy.
- Adaptive hysteresis: the band width itself adapts based on the frequency of
  recent switches. If switches are frequent, widen the band; if no switch has
  occurred in a long time, narrow it.

**Examples in production systems:**

- Linux EAS: the 80% over-utilized threshold has implicit hysteresis via PELT's
  exponential decay (load tracking lags actual utilization by several
  milliseconds).
- QNX APS: the averaging window provides temporal hysteresis (budget violations
  are smoothed over 100ms).
- Intel Thread Director: uses hardware counters with moving averages, providing
  measurement-level hysteresis.

### 4.8 Competitive Ratio Results

Key results relevant to scheduling:

| Problem                                          | Best Known Competitive Ratio | Reference                                  |
| ------------------------------------------------ | ---------------------------- | ------------------------------------------ |
| Online bipartite matching (adversarial)          | 1 - 1/e ~ 0.632              | Karp, Vazirani, Vazirani 1990              |
| Online bipartite matching (random order)         | ~0.696                       | Mahdian & Yan 2011                         |
| Online facility location (randomized)            | O(log n)                     | Meyerson 2001                              |
| Online scheduling (makespan, identical machines) | 4/3                          | Graham 1966 (LPT)                          |
| Online scheduling (makespan, unrelated machines) | O(log m)                     | Aspnes et al. 1997                         |
| Online weighted completion time                  | 2                            | Smith's rule (shortest weighted job first) |
| k-server problem                                 | 2k-1 (deterministic)         | Koutsoupias & Papadimitriou 1995           |

**Interpretation for adaptive scheduling:** These bounds tell us that no online
algorithm can perfectly match an offline oracle. A scheduler that achieves
within 2x of optimal makespan on identical machines (a common server workload)
is already performing near the theoretical limit. The practical gap is typically
much smaller: average-case performance of simple heuristics often approaches
offline optimal for non-adversarial workloads.

### 4.9 Practical Heuristics

Despite poor theoretical worst cases, several heuristics work well in practice:

- **Power of two choices.** When placing a task, sample two random cores and
  choose the less loaded one. This reduces maximum load from O(log n / log log
  n) to O(log log n) with trivial implementation overhead (Mitzenmacher 2001).
  Used in Nginx load balancing, applicable to core selection.

- **Exponentially weighted moving average (EWMA).** Track metrics with a
  decaying average: `estimate = alpha * measurement + (1 - alpha) * estimate`.
  Simple, constant space, automatically adapts to changing conditions. Used in
  TCP congestion control, Linux load tracking (PELT uses a sum of geometric
  series equivalent to EWMA), and QNX APS.

- **Multi-armed bandit.** Treat each scheduling policy as an "arm." Pull arms
  (try policies) to learn their payoffs. UCB1 (Auer et al., 2002) achieves
  O(sqrt(n \* log n)) regret, meaning the cumulative performance loss from not
  knowing the best policy in advance grows sublinearly. Epsilon-greedy is even
  simpler: with probability epsilon, try a random policy; otherwise, use the
  best known. Practical for systems with a small number of candidate policies.

- **Threshold-based switching.** Monitor a single metric (e.g., runqueue length,
  average latency) and switch policies when it crosses a threshold. Simple,
  deterministic, easy to reason about. Combine with hysteresis to prevent
  thrashing.

---

## 5. Production Kernel Scheduling Architectures

### 5.1 Linux

Linux's scheduler is structured as a hierarchy of _scheduling classes_, each
implementing a common interface (`struct sched_class`). The classes are ordered
by priority; the dispatcher always selects from the highest-priority class that
has a runnable task.

**Scheduling class hierarchy (highest to lowest priority):**

1. **stop_sched_class** -- Internal use only. Highest priority. Used for
   migration stop-machine callbacks. One task per CPU.
2. **dl_sched_class (SCHED_DEADLINE)** -- EDF + CBS. Tasks specify
   `(runtime, deadline, period)`. Admission control ensures total utilization
   per CPU does not exceed 1.0. Source: `kernel/sched/deadline.c`.
3. **rt_sched_class (SCHED_FIFO, SCHED_RR)** -- Fixed-priority real-time. 99
   priority levels (1-99). SCHED_FIFO: runs until it blocks or a higher-priority
   RT task arrives. SCHED_RR: round-robin within priority level, default quantum
   100ms. Source: `kernel/sched/rt.c`.
4. **fair_sched_class (SCHED_NORMAL, SCHED_BATCH)** -- EEVDF (since 6.6; CFS
   before). This is where the vast majority of tasks run. Source:
   `kernel/sched/fair.c`.
5. **idle_sched_class (SCHED_IDLE)** -- Lowest priority. Runs only when no other
   class has work. Source: `kernel/sched/idle.c`.
6. **ext_sched_class (sched_ext)** -- BPF-defined scheduling. Sits between idle
   and fair in priority. Source: `kernel/sched/ext.c` (since 6.12).

**Interaction between classes:** The dispatcher in `__schedule()` iterates
through classes from highest to lowest priority, calling `pick_next_task()` on
each. The first class that returns a task wins. This means a single SCHED_FIFO
task at priority 99 will preempt all SCHED_NORMAL tasks indefinitely. RT
throttling (`/proc/sys/kernel/sched_rt_runtime_us`, default 950ms per 1000ms
period) limits RT tasks to 95% of CPU to prevent total starvation of normal
tasks.

**cpusets.** Cgroups v1/v2 can restrict which CPUs a group of tasks may use
(`cpuset.cpus`). This provides spatial partitioning: real-time tasks on cores
0-3, general tasks on cores 4-7. Combined with scheduling classes, this enables
coarse-grained mixed-criticality scheduling.

**cgroups CPU controller.** Cgroups v2 provides `cpu.weight` (proportional share
within the fair class), `cpu.max` (bandwidth limit: `max_usec period_usec`), and
`cpu.pressure` (PSI-based stall monitoring). The bandwidth limit implements a
CBS-like mechanism for cgroups, throttling a group that exceeds its allocation.

**schedutil cpufreq governor.** The `schedutil` governor uses the scheduler's
load-tracking metrics (PELT) to set CPU frequency. When load increases,
frequency ramps up; when load decreases, frequency ramps down. This is tightly
integrated with EAS: both use the same load metrics, ensuring coordinated
decisions about task placement and frequency.

**Energy Aware Scheduling (EAS).** Described in section 2.9. Coordinates with
schedutil to minimize system energy on heterogeneous topologies. Disabled under
high load (>80% utilization on any CPU).

### 5.2 FreeBSD

FreeBSD's ULE scheduler (Jeff Roberson, first in FreeBSD 5.0, mature by 9.0)
replaced the traditional 4BSD scheduler.

**Historical evolution:**

- **4BSD scheduler** (inherited from 4.3BSD): multilevel feedback queue with 32
  run queues and priority recalculation every 4 ticks based on CPU usage decay.
  Worked well on uniprocessors but had no SMP awareness.
- **ULE** (sched_ule.c): developed over 10 years (FreeBSD 5.0 through 9.0) to
  address SMP shortcomings.

**ULE design:**

- Scheduling classes: idle, timeshare, real-time, interrupt (in ascending
  priority). These are subdivisions of the 0-255 priority space.
- Processor affinity: ULE tracks which CPU a thread last ran on and prefers to
  schedule it there (cache warmth). Migration occurs only when load imbalance
  exceeds a threshold.
- Load balancing: periodic (every `sched_rebalance_interval`, default ~125ms)
  and on task wakeup. Balancing considers CPU topology (package, core, SMT).
- Interactivity detection: ULE uses a simplified interactivity score based on
  voluntary sleep vs. CPU consumption. Interactive threads get smaller time
  slices and higher priority, similar in spirit to the BSD 4.3 approach but with
  less complex heuristics.
- Per-CPU run queues with per-queue spinlocks, eliminating the global runqueue
  lock that plagued the 4BSD scheduler on SMP.

**Comparison with Linux CFS/EEVDF:** Bouron et al. ("The Battle of the
Schedulers: FreeBSD ULE vs. Linux CFS," USENIX ATC 2018) measured both on
identical hardware. Key findings: CFS provided better fairness under mixed
workloads; ULE provided lower tail latency for interactive tasks. Both performed
comparably for throughput-oriented workloads.

### 5.3 Windows

Windows scheduling combines static base priority, dynamic priority boosts, and
specialized services for specific workload types.

**Base priority and dynamic boosts:**

- Each thread has a base priority (0-31). Levels 0-15 are "dynamic" (normal
  applications); 16-31 are "real-time" (requires
  SeIncreaseBasePriorityPrivilege).
- The kernel dynamically boosts thread priority on specific events:
  - I/O completion: +1 to +8 depending on device type (disk: +1, keyboard: +6,
    sound: +8).
  - Window input (foreground): +2.
  - Starvation detection: threads that have not run for ~4 seconds receive a
    temporary boost to priority 15.
- Boosts decay: after each quantum, the boosted priority drops by 1 until it
  returns to the base.

**MMCSS:** Described in section 3.4.

**Processor groups and heterogeneous scheduling (Windows 11):**

- Windows 11 integrates with Intel Thread Director (hardware-assisted workload
  classification). Thread Director is a microcontroller in Intel hybrid CPUs
  (Alder Lake and later) that monitors instruction mix and classifies thread
  behavior into categories (e.g., "integer-heavy," "FP/vector-heavy,"
  "memory-bound").
- Thread Director communicates these classifications to the Windows scheduler
  via a hardware interface. The scheduler uses this plus QoS class to decide
  P-core vs. E-core placement.
- On ARM64 Windows (Qualcomm Snapdragon X), similar asymmetric scheduling uses
  QoS hints from applications.

**Thread Director feedback loop:**

1. Hardware monitors instruction mix via performance counters.
2. Classification is communicated to OS via ACPI/CPPC or Intel-specific
   interface.
3. Windows scheduler uses classification + thread priority + QoS to select core
   type.
4. If the thread's behavior changes (e.g., switches from FP to integer work),
   Thread Director updates its classification, and the scheduler may migrate the
   thread.

### 5.4 Solaris/illumos

Solaris provides a rich set of scheduling classes that can coexist on the same
system, with processor sets providing spatial partitioning.

**Scheduling classes:**

| Class          | Abbr | Priority Range | Behavior                                            |
| -------------- | ---- | -------------- | --------------------------------------------------- |
| Time Sharing   | TS   | 0-59           | MLFQ with decay, default class                      |
| Interactive    | IA   | 0-59           | Like TS but boosts windowed (GUI-focused) processes |
| Fixed Priority | FX   | 0-59           | No dynamic priority adjustment                      |
| Fair Share     | FSS  | 0-59           | Share-based, not priority-based                     |
| Real-Time      | RT   | 100-159        | Fixed priority, fixed time quantum                  |
| System         | SYS  | 60-99          | Kernel threads only                                 |
| Interrupt      | -    | 160+           | Hardware interrupt threads                          |

**Processor sets.** Solaris allows binding scheduling classes to processor sets.
The key constraint: FSS and TS/IA/FX should not share a processor set because
they use the same priority range (0-59) and would interfere. RT can coexist with
FSS on the same processor set because they use disjoint ranges.

**Fair Share Scheduler (FSS).** Introduced in Solaris 9 as a resource management
tool for consolidation workloads. FSS assigns CPU shares to _projects_ (groups
of processes). The scheduler distributes CPU time proportional to shares,
measured over a decay window. Unlike TS, FSS makes no priority-based decisions
within the 0-59 range; all FSS tasks are scheduled purely by their project's
share allocation and current CPU usage.

### 5.5 QNX

QNX Neutrino provides POSIX scheduling classes (SCHED_FIFO, SCHED_RR,
SCHED_SPORADIC) plus the Adaptive Partitioning Scheduler (APS, described in
section 3.5).

**SCHED_SPORADIC.** Unique among production RTOS schedulers, this implements the
sporadic server algorithm: a task has a high-priority budget that it can consume
within a replenishment period. When the budget is exhausted, the task drops to a
low priority until the next replenishment. This provides bounded execution at
high priority without using fixed-priority scheduling (which risks starvation)
or round-robin (which dilutes responsiveness).

**Layered design:** APS provides inter-partition fairness (how much CPU each
partition gets), while POSIX classes provide intra-partition scheduling (which
thread within a partition runs next). This two-layer design cleanly separates
resource allocation from scheduling policy.

### 5.6 macOS/iOS/iPadOS

Described in section 3.6 (QoS classes). Additional architectural details:

**XNU scheduler internals.** XNU uses a hybrid scheduler:

- Mach scheduling: priority-based with 128 priority bands. Threads are assigned
  to bands based on their QoS class and Mach task policy.
- Decay scheduling: CPU-bound threads experience priority decay (similar to BSD
  4.3), causing them to sink in priority over time. I/O-bound threads maintain
  their assigned priority.
- Thread handoff: when a thread blocks on a lock, it can hand off its remaining
  quantum to the lock holder (similar to priority donation). This reduces
  priority inversion latency.
- Asymmetric scheduling on Apple Silicon: the scheduler maintains separate run
  queues for P-clusters and E-clusters. Thread migration between clusters
  considers QoS, thermal state, and core utilization. Cluster transitions have a
  cost (pipeline flush, different microarchitecture), so the scheduler applies
  hysteresis.

**Recommended cluster assignment by QoS:**

- `QOS_CLASS_USER_INTERACTIVE` -> P-cluster (latency-critical, benefit from high
  single-thread performance)
- `QOS_CLASS_BACKGROUND` -> E-cluster (energy-efficient, latency-insensitive)
- Middle tiers: dynamic based on system state.

### 5.7 seL4 MCS

Described in section 2.11 (scheduling contexts). Architectural significance:

**Scheduling as capability.** seL4 MCS is unique among production kernels in
treating CPU time as a capability-controlled resource. The kernel enforces that:

- A thread without a scheduling context capability cannot run.
- The scheduling context encodes the maximum CPU budget and period.
- Scheduling contexts can be transferred, delegated, and revoked, just like
  memory or IPC capabilities.

This enables user-level scheduling policies to be built on top of the kernel's
mechanism. The kernel provides temporal isolation (sporadic server enforcement);
user-level code provides scheduling _policy_ (which threads to prioritize, when
to donate scheduling contexts). This aligns with the capability-based
microkernel philosophy of minimizing kernel policy while providing strong
enforcement.

**SchedControl capability.** One `SchedControl` capability exists per CPU.
Configuring a scheduling context (setting budget, period, priority, core
binding) requires holding the `SchedControl` for the target CPU. This prevents
unprivileged tasks from modifying scheduling parameters.

---

## 6. Workload Characterization

### 6.1 Distinguishing Workload Types

Three primary workload categories are relevant to scheduling:

**Interactive.** Short CPU bursts (microseconds to low milliseconds) separated
by I/O waits (user input, network, disk). Key metric: tail latency of scheduling
delay (time from becoming runnable to actually running). Desktop studies (Li et
al., 2007) measured typical interactive CPU bursts at 0.1-5ms with inter-arrival
times of 10-100ms.

- Low CPU utilization per task (1-10%).
- High wakeup frequency (10-1000 wakeups/second).
- Scheduling delay sensitivity: user-perceptible degradation at ~16ms (60fps
  UI), noticeable at ~50ms (keyboard responsiveness), unacceptable at ~100ms.

**Batch/compute.** Long CPU bursts (seconds to hours) with rare I/O. Key metric:
throughput (tasks completed per unit time).

- High CPU utilization per task (90-100%).
- Low wakeup frequency (near zero: task runs until completion or time slice
  expiry).
- Scheduling delay insensitivity: additional milliseconds of latency are
  negligible relative to task duration.

**Real-time.** Periodic or sporadic activations with hard or soft deadlines. Key
metric: deadline miss ratio.

- Predictable CPU utilization per activation (known WCET).
- Activation frequency determined by external events (sensor sample rate,
  control loop frequency, audio frame rate).
- Scheduling delay is governed by formal analysis, not empirical observation.

### 6.2 Phase Detection

Programs exhibit _phases_: periods of relatively stable behavior (instruction
mix, cache miss rate, branch prediction rate) separated by transitions to
different behavior.

**Dhodapkar and Smith (MICRO 2003)** proposed detecting phase changes using
_working set signatures_: hash the addresses of accessed instructions or data
into a bit vector (Bloom filter). Compare consecutive signatures using relative
Hamming distance. A large distance indicates a phase change. This technique is
microarchitecture-independent and requires only ~1KB of storage per signature.

**Sherwood, Perelman, and Calder (MICRO 2003)** proposed _Basic Block Vectors_
(BBVs): a vector of basic block execution frequencies, compared across intervals
using Manhattan distance. Phases are identified by clustering similar BBVs. This
was later used for SimPoint (representative simulation points).

**Phase characteristics from measured data:**

| Workload Type          | Typical Phase Duration | Phase Stability                          | Detection Latency         |
| ---------------------- | ---------------------- | ---------------------------------------- | ------------------------- |
| Desktop (web browsing) | 100ms - 10s            | Low (frequent transitions)               | Detectable within 10-50ms |
| Media playback         | 10s - minutes          | High (stable decode loop)                | Detectable within 100ms   |
| Compilation            | 1-30s                  | Medium (parse, optimize, codegen phases) | Detectable within 100ms   |
| Database (OLTP)        | Milliseconds           | Very low (query-dependent)               | Difficult to detect       |
| Scientific computation | Minutes - hours        | Very high                                | Detectable within 1s      |

**Detection latency vs. phase duration.** For phase-aware scheduling to be
useful, the scheduler must detect a phase change and adapt faster than the phase
lasts. Desktop workloads with sub-second phases require detection in under
100ms. If the adaptation mechanism (e.g., switching scheduling algorithms) takes
100ms to stabilize, phases shorter than ~200ms cannot be meaningfully adapted
to.

### 6.3 Risks of Misclassification

When a scheduler classifies a workload incorrectly, the costs depend on the
direction of error:

| Misclassification                   | Cost                                                                                                                                                         |
| ----------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Interactive classified as batch     | User-perceptible latency spikes. User sees lag, stuttering.                                                                                                  |
| Batch classified as interactive     | Short time slices cause excessive context-switch overhead, reducing throughput by 5-20%. Mild starvation of actual interactive tasks if priority is boosted. |
| Interactive classified as real-time | Consumes RT scheduling bandwidth. May cause admission control to reject legitimate RT tasks.                                                                 |
| Real-time classified as batch       | Missed deadlines. System failure in safety-critical contexts.                                                                                                |

The asymmetry is important: misclassifying interactive as batch has immediately
visible user impact, while misclassifying batch as interactive has a performance
cost but no correctness issue. This suggests that classification should be
biased toward the _higher_ category when uncertain, accepting some throughput
loss to avoid latency spikes.

### 6.4 The Gaming Problem

_Gaming_ refers to tasks that deliberately exploit scheduler heuristics to gain
disproportionate CPU time or priority.

**Historical examples:**

1. **Linux O(1) sleep trick.** A task performs a brief `usleep(1)` just before
   its quantum expires, resetting its sleep average and maintaining high
   interactive priority while consuming nearly all CPU. This was widely known
   and exploited in early Linux 2.6 kernels.

2. **CFS vruntime manipulation.** A task forks a child, the child consumes CPU
   (raising its vruntime), then exits. The parent's vruntime remains low, giving
   it priority. Less practical than the O(1) exploit but theoretically possible.

3. **Priority inflation via ticket systems.** In lottery scheduling without
   trust boundaries, a task inflates its ticket count. Waldspurger explicitly
   noted this requires trust and proposed ticket currencies as a mitigation.

4. **cgroup CPU weight manipulation.** In container environments, a task creates
   many threads in a single cgroup, effectively multiplying its weight relative
   to single-threaded cgroups (before per-task weight normalization was added).

**Mitigations:**

- **Track total CPU time, not per-quantum behavior.** Solaris TS and Linux CFS
  both track cumulative CPU usage, making per-quantum gaming ineffective because
  the accumulated usage eventually catches up.
- **Bandwidth isolation.** CBS (used in SCHED_DEADLINE and cgroups v2 bandwidth
  control) enforces a hard budget regardless of behavior.
- **Structural fairness.** EEVDF's lag tracking is based on actual CPU consumed
  vs. fair share, which cannot be gamed by sleep/wake patterns: the lag
  computation reflects reality.
- **Capability-based time.** seL4 MCS makes CPU budget a capability. A task
  cannot grant itself more budget than it holds, and budget creation requires
  the `SchedControl` capability. Gaming is architecturally impossible.

### 6.5 Mobile Workload Studies

Mobile platforms present distinct scheduling challenges due to workload
heterogeneity and power constraints.

**ARM research (2019)** on Android devices with big.LITTLE processors measured:

- Average CPU utilization: 5-15% for typical usage (messaging, browsing).
- Peak utilization during app launch: 60-80% for 0.5-2 seconds.
- 90% of task wakeups had CPU bursts under 1ms.
- The majority of user-perceived latency came from task placement decisions (big
  vs. little core), not scheduling delay within a core.

These measurements motivated EAS: on heterogeneous mobile hardware, the _which
core_ decision dominates the _which order_ decision for user-perceived
performance.

---

## 7. References

### Papers

1. Stoica, I. and Abdel-Wahab, H. "Earliest Eligible Virtual Deadline First: A
   Flexible and Accurate Mechanism for Proportional Share Resource Allocation."
   Technical Report 95-22, Old Dominion University, 1995.

2. Liu, C.L. and Layland, J.W. "Scheduling Algorithms for Multiprogramming in a
   Hard-Real-Time Environment." Journal of the ACM, 20(1):46-61, 1973.

3. Waldspurger, C.A. and Weihl, W.E. "Lottery Scheduling: Flexible
   Proportional-Share Resource Management." OSDI 1994.

4. Waldspurger, C.A. and Weihl, W.E. "Stride Scheduling: Deterministic
   Proportional-Share Resource Management." MIT Technical Memo
   MIT/LCS/TM-528, 1995.

5. Waldspurger, C.A. "Lottery and Stride Scheduling: Flexible Proportional-Share
   Resource Management." PhD Thesis, MIT, 1995.

6. Duda, K.J. and Cheriton, D.R. "Borrowed-Virtual-Time (BVT) Scheduling:
   Supporting Latency-Sensitive Threads in a General-Purpose Scheduler."
   SOSP 1999.

7. Blumofe, R.D. and Leiserson, C.E. "Scheduling Multithreaded Computations by
   Work Stealing." Journal of the ACM, 46(5):720-748, 1999.

8. Anderson, T.E., Bershad, B.N., Lazowska, E.D., and Levy, H.M. "Scheduler
   Activations: Effective Kernel Support for the User-Level Management of
   Parallelism." ACM Transactions on Computer Systems, 10(1):53-79, 1992.
   (Originally SOSP 1991.)

9. Karp, R.M., Vazirani, U.V., and Vazirani, V.V. "An Optimal Algorithm for
   On-line Bipartite Matching." STOC 1990.

10. Hellerstein, J.L., Diao, Y., Parekh, S., and Tilbury, D.M. "Feedback Control
    of Computing Systems." Wiley/IEEE Press, 2004.

11. Sha, L., Rajkumar, R., and Lehoczky, J.P. "Priority Inheritance Protocols:
    An Approach to Real-Time Synchronization." IEEE Transactions on Computers,
    39(9):1175-1185, 1990.

12. Buttazzo, G.C. "Rate Monotonic vs. EDF: Judgment Day." Real-Time Systems,
    29(1):5-26, 2005.

13. Abeni, L. and Buttazzo, G. "Integrating Multimedia Applications in Hard
    Real-Time Systems." RTSS 1998. (Constant Bandwidth Server.)

14. Lawall, J.L., Muller, G., and Duchesne, H. "Bossa: A DSL Framework for
    Application-Specific Scheduling Policies." IEEE International Conference on
    Automated Software Engineering, 2002.

15. Lawall, J.L. et al. "Bossa Nova: Introducing Modularity into the Bossa
    Domain-Specific Language." GPCE 2005.

16. Dhodapkar, A.S. and Smith, J.E. "Comparing Program Phase Detection
    Techniques." MICRO 2003.

17. Sherwood, T., Perelman, E., and Calder, B. "Basic Block Distribution
    Analysis to Find Periodic Behavior and Simulation Points in Applications."
    PACT 2001.

18. Dhall, S.K. and Liu, C.L. "On a Real-Time Scheduling Problem." Operations
    Research, 26(1):127-140, 1978.

19. Meyerson, A. "Online Facility Location." FOCS 2001.

20. Mitzenmacher, M. "The Power of Two Choices in Randomized Load Balancing."
    IEEE Transactions on Parallel and Distributed Systems,
    12(10):1094-1104, 2001.

21. Auer, P., Cesa-Bianchi, N., and Fischer, P. "Finite-Time Analysis of the
    Multiarmed Bandit Problem." Machine Learning, 47(2):235-256, 2002.

22. Arora, N.S., Blumofe, R.D., and Plaxton, C.G. "Thread Scheduling for
    Multiprogrammed Multiprocessors." SPAA 1998.

23. Corbato, F.J., Merwin-Daggett, M., and Daley, R.C. "An Experimental
    Time-Sharing System." AFIPS 1962.

24. Roberson, J. "ULE: A Modern Scheduler for FreeBSD." BSDCon 2003.

25. Bouron, J. et al. "The Battle of the Schedulers: FreeBSD ULE vs. Linux CFS."
    USENIX ATC 2018.

26. Lyons, A. et al. "Scheduling-Context Capabilities: A Principled,
    Light-Weight Operating-System Mechanism for Managing Time." EuroSys 2018.

27. Baumann, A. et al. "The Multikernel: A New OS Architecture for Scalable
    Multicore Systems." SOSP 2009.

28. Graham, R.L. "Bounds for Certain Multiprocessing Anomalies." Bell System
    Technical Journal, 45(9):1563-1581, 1966.

29. Koutsoupias, E. and Papadimitriou, C.H. "On the k-Server Conjecture."
    Journal of the ACM, 42(5):971-983, 1995.

### Kernel Source References

30. Linux `kernel/sched/fair.c` -- EEVDF implementation (6.6+).
31. Linux `kernel/sched/deadline.c` -- SCHED_DEADLINE (EDF + CBS).
32. Linux `kernel/sched/rt.c` -- SCHED_FIFO and SCHED_RR.
33. Linux `kernel/sched/ext.c` -- sched_ext BPF scheduler (6.12+).
34. Linux `Documentation/scheduler/sched-eevdf.rst` -- EEVDF documentation.
35. Linux `Documentation/scheduler/sched-deadline.rst` -- SCHED_DEADLINE
    documentation.
36. Linux `Documentation/scheduler/sched-energy.rst` -- Energy Aware Scheduling
    documentation.
37. FreeBSD `/sys/kern/sched_ule.c` -- ULE scheduler.
38. seL4 MCS tutorial:
    `github.com/seL4/sel4-tutorials/blob/master/tutorials/ mcs/mcs.md`.

### Documentation and Specifications

39. QNX Adaptive Partitioning documentation:
    `qnx.com/developers/docs/6.3.2/neutrino/sys_arch/adaptive.html`.
40. Apple "Tuning Your Code's Performance for Apple Silicon":
    `developer.apple.com/documentation/apple-silicon/tuning-your-code-s- performance-for-apple-silicon`.
41. Microsoft MMCSS documentation:
    `learn.microsoft.com/en-us/windows/win32/procthread/multimedia-class- scheduler-service`.
42. Solaris FSS documentation:
    `docs.oracle.com/cd/E19120-01/open.solaris/819-2450/6n4o5mdan/`.
43. Intel Thread Director whitepaper:
    `cdrdv2-public.intel.com/685865/211112_Hybrid_WP_2_Developing_v1.2.pdf`.
44. Arpaci-Dusseau, R.H. and Arpaci-Dusseau, A.C. "Operating Systems: Three Easy
    Pieces." Chapter 9: Scheduling (Proportional Share).
