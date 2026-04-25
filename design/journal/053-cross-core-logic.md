# Cross-Core Logic: Placement, Wake, Idle — 2026-04-24

Records the reasoning behind `spec.md#D56`. Settles the cross-core kernel
mechanisms: Observer placement strategy, cross-core wake protocol, core idle
management, rebalancing triggers, and cache affinity.

## Starting point

D43 settled "transient core assignment: the kernel makes a fresh placement
decision each time an Observer transitions to runnable." D46 settled core
lifecycle (boot-activated, idle via WFI, wake via IPI). D1 established the
hybrid SMP model (per-core hot path, shared cold path). D50 established that
cross-core IPC is always slow-path (enqueue + IPI). D53 established arena lock
ordering for cross-core operations.

None of these specified the actual mechanisms: how the kernel makes the
placement decision, what the cross-core wake protocol looks like, how
rebalancing works without a background thread, or how cache affinity is tracked.
Journal 001 listed four open sub-questions about these mechanisms. They remained
open through 51 subsequent entries.

This is a genuine choice (G11 in the autonomous plan). The design space has
multiple valid paths — the constraints narrow but do not determine the answer.

## Derived consequences (not choices)

Before reaching the choices, systematic checking of A1–A5, O1–O4, and D1–D55
produced ten implications. The key ones that constrain the design space:

1. **Per-core run queues are forced.** D1 (hot path touches no cross-core shared
   state) + "scheduler pick" is hot-path → global run queue is shared mutable
   state on hot path → foreclosed. Per-core queues with cross-core migration
   (cold-path) is the only topology consistent with D1.

2. **Placement is a two-outcome function.** Every wake event calls the function;
   it returns "local" (hot path, no IPI) or "remote(core_id)" (cold path,
   mailbox + IPI). Local is ~0 cycles overhead; remote is IPI round-trip cost.

3. **Four rebalancing triggers under A4.** No background thread exists. The only
   rebalancing opportunities are: (a) timer tick, (b) IPC delivery, (c) idle
   entry, (d) IPI handler. All are exception handlers. These are sufficient —
   the "Wasted Cores" paper's failure modes arose from failing to act on
   triggers, not from lacking them.

4. **D36 fungibility makes migration costless at the cap level.** Time caps
   remain valid regardless of core. No bookkeeping on migration. Only cost is
   cache warming.

5. **Cross-core wake protocol is IPI + remote handler.** D53 says "the IPI
   handler on the receiver's core acquires Field then Observer in the same
   order." Core A doesn't touch core B's run queue directly.

6. **Cache affinity must be per-core, not per-Observer.** D43 forbids a core ID
   field on the Observer. The per-core scheduler tracks affinity internally.

## Design space

Three coherent combinations were identified:

**Minimal:** Local-first + idle check. Immediate idle. No rebalancing.
Homogeneous schedulers. No affinity tracking. Handles common cases but doesn't
address sustained imbalance between non-idle cores.

**Pragmatic:** Local-first + idle check + timer-tick rebalance. Steal-then-idle.
Push + pull. Homogeneous first. Affinity as tiebreaker. Linux-shaped — separates
hot-path simplicity from cold-path thoroughness.

**Precise:** Scored placement. Steal-then-idle. Push + pull. Dynamic core-type
classification. Affinity with decay. Globally-aware on every placement decision.

All three use the same interfaces (placement function behind a trait, per-core
scheduler behind a trait). Upgrading Minimal → Pragmatic → Precise is
incremental, and falling back is equally straightforward.

## Stress testing the Precise choice

Three risks were identified:

1. **Queue depth cache-line bouncing.** Per-core queue depth changes on every
   scheduling event — the cache line bounces between Modified and Invalid. The
   ~50–200 cycle scoring estimate assumed L1 hits; with frequent invalidation,
   reads could cost ~30–80 cycles each, pushing total scoring to ~200–800 cycles
   on an 8-core system.

2. **D50 fast-path locality.** Scored placement may move a receiver away from
   its frequent sender, converting fast-path IPC (~400 cycles) to slow-path
   (~600–800 cycles). The utilization improvement could be offset by IPC
   regression.

3. **Marginal benefit over Pragmatic.** Pragmatic captures most utilization
   gains (idle check + steal + timer rebalance). Scored placement's marginal
   benefit materializes primarily on heterogeneous hardware with mixed RT/batch
   workloads.

The designer chose Precise, reasoning: (a) the bet is that improved CPU
utilization outweighs the scoring overhead, (b) the interfaces make fallback to
Pragmatic painless if the bet is wrong — the placement function is a leaf node
behind a trait boundary.

## What is settled

### Placement: scored, globally-aware

On every runnable transition, the placement function scores candidate cores
using:

- **Idle status:** idle core = zero queueing delay, strong preference.
- **Queue depth:** lower = better. Approximate (atomic read of per-core
  counter).
- **Profile compatibility:** Observer's (R, T, P) matched against each core's
  current scheduler algorithm. High-P Observer → EDF core preferred.
- **Capacity factor:** D36 per-core capacity. Observer's Time aggregate mapped
  to per-core scheduling weight.
- **Cache affinity with decay:** preference for the core where the Observer last
  ran, decaying over ~1–5ms (typical L2 lifetime). Stale affinity = cold caches
  = zero weight.

The scoring function is behind a trait. Weights are tunable. The function
returns "local" or "remote(core_id)."

### Cross-core wake protocol

When the placement function returns "remote(core_id)":

1. Causing core writes Observer reference to a per-core mailbox for the target.
2. Causing core sends SGI (GICv3 `ICC_SGI1R_EL1`).
3. Target core takes IRQ exception.
4. Target core's IPI handler reads mailbox.
5. Handler acquires arena locks (D53 ordering: Field < Observer).
6. Handler enqueues Observer on local run queue.
7. Handler returns to scheduling decision.

The causing core does NOT touch the target's run queue directly. The IPI handler
on the target core does all local work under local locks.

### Idle entry: steal-then-idle

When a core's last runnable Observer blocks:

1. Core scans other cores' queue depths (atomic reads, boot-sized array).
2. If any core has queue depth > 1, steal its lowest-affinity runnable Observer.
   Stealing acquires D53 arena locks on the victim's queue.
3. If no work found, set idle bit in the idle bitmap and enter WFI (or
   CPU_SUSPEND for deeper idle per D46 platform policy).

### Rebalancing: push + pull on reactive triggers

**Push (timer tick):** Each core's preemption timer handler checks local queue
depth against a fair-share target (total Observers / active cores). If
overloaded, pick the Observer with weakest affinity and push to the least-loaded
core (write mailbox + IPI).

**Pull (idle entry):** As above — steal-then-idle. The about-to-idle core pulls
work from overloaded cores before entering WFI.

Both are A4-consistent: timer tick is a hardware exception, idle entry is a
scheduling decision within an exception handler.

### Core-type classification: dynamic

The kernel assigns scheduler algorithms to cores based on the (R, T, P) profiles
of Observers currently assigned to each core. When the population changes
(Observer enqueued, dequeued, or migrated), the core re-evaluates whether its
current algorithm is the best fit. Reclassification rebuilds algorithm-specific
state from abstract properties (D2: "on migration, abstract properties transfer;
algorithm-specific state is re-derived").

This eliminates wasted capacity from static classification (no idle RT core when
no RT work exists). The reclassification cost is cold-path (population changes
are infrequent relative to scheduling decisions).

### Per-core data structures: boot-sized, not compile-time-fixed

All per-core arrays (idle bitmap, queue depths, algorithm tags, capacity
factors, affinity trackers) are sized at boot based on discovered core count. A3
(generic kernel) forbids compile-time core count limits. The idle bitmap uses an
array of AtomicU64, not a single u64. The interface (`is_idle`, `set_idle`,
`find_any_idle`) hides the representation.

### Cache affinity: per-core tracker with decay

Each per-core scheduler maintains a small tracker (ring buffer of recent
Observer IDs with timestamps). The placement function queries candidate cores'
trackers to determine affinity weight. Affinity decays over ~1–5ms (L2 cache
lifetime). After decay, affinity weight is zero — the Observer's data is cold
regardless.

D43's "no core ID on Observer" is preserved: affinity state lives in per-core
structures, not on the Observer.

## What this does NOT settle

- **Scoring weights.** How much weight idle status, queue depth, profile match,
  capacity, and affinity each receive. Tuning parameter.
- **Mailbox structure.** Per-core mailbox size, layout, overflow policy.
  Implementation detail behind the IPI interface.
- **Reclassification thresholds.** When does a core switch algorithms? After how
  many population changes? What hysteresis? Tuning parameter.
- **Affinity decay curve.** Linear, exponential, or step function.
  Implementation detail.
- **Timer tick interval for rebalancing.** Coupled to the preemption timeslice
  (D42 scheduling parameters). Not independently settable.
- **Work stealing synchronization.** Lock-based (D53 arena locks) or lock-free
  dequeue. Implementation choice behind the run queue interface.
- **NUMA awareness.** Whether scoring prefers same-NUMA-domain cores. Depends on
  A2 hardware topology. Deferred until NUMA hardware is tested.
- **IPC locality tracking.** The kernel could track communication patterns and
  co-locate frequent communicators. This would address the D50 tension (scored
  placement breaking fast-path locality). Not introduced here — requires its own
  derivation if deemed necessary.
- **Admission control on heterogeneous migration.** D2 journal 002 (line 126):
  "Whether cross-core migration in the presence of algorithm heterogeneity needs
  admission control on the destination." Not settled here — depends on the
  specific scheduler algorithms implemented.

## Rejected alternatives

**Background rebalancing thread:** Foreclosed by A4 (purely reactive).

**Global run queue:** Foreclosed by D1 (per-core hot path, no shared state).

**Userspace placement hints / core affinity:** Foreclosed by D46 (core existence
kernel-internal) and D31 (core assignment kernel-internal).

**Static per-core Observer binding:** Foreclosed by D43 (transient assignment).

**Local-first without scoring (Minimal):** Not foreclosed, available as
fallback. Rejected because it doesn't address sustained imbalance between
non-idle cores and doesn't do profile-to-core matching (D2).

**Two-tier without per-wake scoring (Pragmatic):** Not foreclosed, available as
fallback. Rejected in favor of Precise because: (a) the interface absorbs the
difference — the trait boundary makes switching cheap, (b) the bet is that
utilization improvement outweighs scoring overhead, (c) on heterogeneous
hardware, per-wake profile matching is needed for correctness (high-P Observer
on wrong scheduler type), not just performance.

## Landscape divergence

No surveyed system combines:

- Per-core algorithm heterogeneity (D2)
- Scored placement with profile-to-algorithm matching
- Dynamic core-type reclassification
- Reactive-only rebalancing (no background thread)

Linux EAS is closest (scored placement with energy model) but assumes a single
scheduling algorithm. seL4 uses BKL + explicit migration (no autonomous
placement). Barrelfish uses no cross-core scheduling. QNX uses a single
algorithm with per-core queues.

The novel position is a natural consequence of D2 (per-core algorithm
heterogeneity) intersecting D43 (transient assignment) and A4 (reactive only).
Each of those decisions is well-justified individually; their combination
produces a placement problem no existing system solves because no existing
system makes all three commitments simultaneously.

## Axioms

**A4 (reactive):** Load-bearing. Forecloses background rebalancing. Constrains
rebalancing to four piggy-back triggers. The entire cross-core design is shaped
by "the kernel runs only in response to hardware exceptions."

**A3 (generic):** Load-bearing. Forbids compile-time core count limits. Forbids
workload-specific placement heuristics. The scoring function must work for any
workload type.

**A5 (leaf node):** Load-bearing. Placement complexity is kernel-internal.
Observers don't see cores, don't influence placement. The kernel absorbs the
matching problem.

**A2 (ARM64):** Load-bearing. Cache coherency makes cross-core reads cheap. SGI
provides the IPI mechanism. WFI/CPU_SUSPEND for idle. big.LITTLE motivates
heterogeneous placement.

**A1 (Rust):** Load-bearing for implementation. Trait boundaries for placement
function and scheduler. AtomicU64 for lock-free per-core state. Ownership model
for per-core vs shared data.

## Status

**Settled.** Revisit if:

- D1 is revised (changes the hot/cold split — affects whether scoring is
  acceptable on the hot path).
- D2 is revised (unified scheduler eliminates the matching problem and the need
  for dynamic reclassification).
- D43 is revised (per-Observer core binding would replace the placement function
  with static assignment).
- D50 is revised (changes the same-core fast-path requirement — affects the D50
  locality tension).
- Scoring overhead proves consistently >500 cycles (>60% of D50's slow-path
  budget) — would motivate fallback to Pragmatic (two-tier).
- Cache-line bouncing on per-core queue depth proves measurably worse than
  stale-read-based Pragmatic scoring — would motivate switching to less-frequent
  cross-core reads.
