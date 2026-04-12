# Time Shape — 2026-04-12

Eighth exploration. Resolved the concrete shape of Time capabilities and
scheduling declarations. Extended from journal 007's open question on Time
object shape.

## Starting point

Journal 007 established: the scheduler picks a Time allocation, not a Context.
Scheduling properties were initially placed in the Time object. Open question:
what properties does a Time allocation carry?

## The split: resource vs. requirements

Extended exploration revealed that "scheduling properties" conflates two
distinct things:

- **How much CPU** — a resource, given to the Context by its creator.
  Transferable, subdividable, attenuatable. This is what the Time capability
  represents.
- **How to deliver it** — a declaration by the Context about its timing needs.
  Intrinsic to the Context, doesn't transfer. The scheduler validates it against
  the resource.

These were initially bundled in the Time object (journal 007). Separating them
resolves the transfer tension from journal 006: when Time is transferred during
IPC, only the resource moves. The server uses its own timing declaration, not
the client's. No reshaping needed.

The parallel to Space: a Memory capability is just bytes. How the Context uses
them (stack, heap) is the Context's business, constrained by the amount. A Time
capability is just a fraction of a core. How the Context needs it delivered is
the Context's business, constrained by the fraction.

### Where the Space analogy breaks

Space is partitioned — each Context's pages are independent (via virtual
memory). Time is multiplexed — all Contexts share one timeline per core. How one
Context's time is delivered affects others. The scheduler must solve a global
constraint satisfaction problem: fit all Contexts' timing requirements on the
shared timeline. This is why scheduling is harder than memory management.

The "extra dimension" for Time degradation is migration to other cores (via the
IPI mechanism from journal 005). If a core's timeline can't satisfy everyone,
move someone to a less-loaded core.

## Time object: fraction

The Time capability contains a single value: a guaranteed minimum fraction of a
core's time, expressed as a percentage (0–100%).

Properties:

- **Conservation:** sum of all fractions on a core ≤ 100%
- **Transferable:** move fraction between Contexts (the IPC Time-transfer
  pattern from journal 006)
- **Subdividable:** split 50% into 20% + 30%
- **Attenuatable:** reduce from 20% to 10% (irreversible)
- **Aggregates:** Time is fungible. Multiple Time handles to a Context represent
  a total allotment (unlike Space, where multiple Memory handles are distinct
  regions)

The fraction is a minimum guarantee, not a cap. When spare capacity exists on
the core (uncommitted fractions), the scheduler can give a Context more than its
guaranteed fraction.

## Timing declarations: two modes

Contexts declare timing requirements via syscall. The declaration is validated
by the scheduler against the Context's Time fraction. Two modes exist,
corresponding to two arrival patterns:

### Mode A — Periodic (self-timed)

The Context asks the kernel to wake it at regular intervals. Audio, video,
heartbeat tasks.

Declares: **duration** (d) with tolerance (dt), and **period** (p) with
tolerance (pt).

```text
Required fraction: f = (d - dt) / (p + pt)
Admission check:   (d - dt) / (p + pt) ≤ Context's Time fraction
```

Tolerances are one-directional toward the scheduler's benefit: dt is how much
LESS duration the Context can tolerate; pt is how much MORE period (longer gaps)
the Context can tolerate. The scheduler GUARANTEES (d - dt) every (p + pt) and
TRIES for d every p.

Latency is derived: bounded by the period. If the Context runs every p, worst-
case response time is ≤ p.

Example — audio (tight): d=1ms, dt=0.1ms, p=5.3ms, pt=0.1ms. Required fraction =
0.9 / 5.4 = 16.7%.

Example — audio (zero tolerance): d=1ms, dt=0, p=5.3ms, pt=0. Required fraction
= 1.0 / 5.3 = 18.9%. More expensive — pays for precision.

### Mode B — Responsive (externally-triggered)

Something external wakes the Context — a message, an interrupt. Device drivers,
input handlers, servers.

Declares: **duration** (d) with tolerance (dt), and **latency** (l) with
tolerance (lt).

```text
Required fraction: f = (d - dt) / (l + lt)
Admission check:   (d - dt) / (l + lt) ≤ Context's Time fraction
```

Same formula structure as periodic, with latency in place of period. Tolerances
work the same way: dt is how much less duration the Context can tolerate; lt is
how much more latency the Context can tolerate.

Period is external and unknown to the scheduler. The CBS (Constant Bandwidth
Server) algorithm assigns a virtual period equal to the effective latency (l +
lt), providing temporal isolation.

Example — device driver (with tolerance): d=10µs, dt=5µs, l=100µs, lt=50µs.
Required fraction = 5 / 150 = 3.3%.

Example — device driver (zero tolerance): d=10µs, dt=0, l=100µs, lt=0. Required
fraction = 10 / 100 = 10%. Three times more expensive.

### Mode C — Bulk (no timing requirements)

Background tasks, compilation, anything without timing needs. Falls out of the
math naturally: if dt = d (tolerate getting nothing) or pt/lt = ∞ (tolerate
waiting forever), the required fraction is zero. No separate mode needed — bulk
is just the extreme of loose tolerances.

### Constraint: denominator ≥ duration (after tolerances)

In both modes, the effective denominator (p + pt, or l + lt) must be ≥ the
effective duration (d - dt). This is just the physical constraint that fraction
≤ 100% — a computation that takes (d - dt) time cannot complete within a window
shorter than (d - dt).

### Why two modes, not three parameters

Initial exploration attempted three independent parameters: duration, period,
and latency. This created problems:

- Latency appeared to be a "free" parameter — no obvious cost to declaring
  latency = 0, which is wrong.
- Three independent values required constrained-deadline admission control
  (demand-bound function), which is significantly more complex than the simple
  utilization bound.
- Unclear when a Context would need all three simultaneously.

The resolution: period and latency serve the same role (bounding response time)
for different arrival patterns. A self-timed task's period IS its latency bound.
An event-driven task has no period, so latency is declared explicitly. No
Context needs both.

By forcing a choice between period and latency:

- Every parameter has an explicit cost (increasing d or decreasing p/l costs
  more fraction)
- Admission control stays simple: Σ (d/p or d/l) for all Contexts on a core ≤
  1.0
- The utilization check is O(n), proven sufficient for EDF (Liu & Layland, 1973)
- No constrained deadlines arise, because deadline always equals the denominator

### Tolerances have direct cost in the admission formula

Unlike an earlier approach where tolerances were "quality preferences" with no
mathematical cost, tolerances now appear directly in the admission formula.
Tighter tolerances (smaller dt, pt, lt) increase the required fraction. Looser
tolerances decrease it.

This means every parameter has a quantifiable cost:

| Action                                      | Cost            |
| ------------------------------------------- | --------------- |
| Increase d (more duration)                  | Higher fraction |
| Decrease p or l (tighter timing)            | Higher fraction |
| Decrease dt (less duration flexibility)     | Higher fraction |
| Decrease pt or lt (less timing flexibility) | Higher fraction |

No free parameters. Every declaration choice has a price in fraction.

The scheduling class spectrum emerges from the math, not from a separate field:

- **Hard RT:** dt ≈ 0, pt ≈ 0. Maximum admission cost. Scheduler guarantees
  near-exact timing.
- **Soft RT:** moderate tolerances. Reduced cost. Scheduler guarantees relaxed
  parameters, tries for ideal.
- **Best-effort:** dt → d, or pt/lt → ∞. Zero admission cost. Scheduler delivers
  fraction however it can.

Different per-core schedulers (journal 005) honor the guarantees but may differ
in how aggressively they pursue the ideal beyond the guaranteed minimum.

## Admission control

The admission check for a core is:

```text
Σ (d_i - dt_i) / (denom_i + tol_i)  for all Contexts on this core  ≤  1.0
```

Where denom is period (periodic) or latency (responsive), and tol is the
corresponding tolerance (pt or lt). Contexts with zero reservation (Mode C /
extreme tolerances) contribute 0 to the sum. This is the standard EDF
utilization bound with tolerances folded in — O(n), trivial to compute.

The scheduler runs this check when:

- A Context declares or updates timing requirements
- Time is transferred to or from a Context on this core
- A Context is migrated to this core

If the check fails: reject the declaration, or signal the Context that
requirements cannot be met, or migrate a Context to a less-loaded core.

EDF is optimal for single-core scheduling: if any algorithm can schedule a task
set, EDF can. CBS extends EDF to handle event-driven tasks alongside periodic
ones, with temporal isolation (a misbehaving task can only harm itself).

## How priorities work (and how RTOSes get away with them)

Traditional RTOSes use a single priority integer. This works for constrained,
static workloads where a human engineer hand-assigns every priority. It doesn't
scale to dynamic, general-purpose workloads: no bandwidth isolation, no temporal
guarantees, priority inversion. The industry is moving away from priority-only
(Linux SCHED_DEADLINE, seL4 MCS, Zircon deadline profiles).

In this design, there is no priority integer. The EDF algorithm derives urgency
from deadlines. A task with an earlier deadline runs first. No manual priority
assignment. No priority inversion (by construction — EDF doesn't have
priorities).

## Passive vs. active servers (revisited)

The split between resource and requirements resolves the transfer tension from
journal 006:

**Passive server:** has no Time of its own. Runs on client's fraction. Has its
own timing declaration (or none — Mode C). The accounting works: client's
fraction is consumed. The scheduling works: server declares its own delivery
needs.

**Active server:** has its own Time and its own timing declaration. Ignores
client's fraction. Runs on its own schedule. E.g., audio driver.

No reshaping, no attenuation of timing parameters during transfer. The fraction
moves. The declarations stay.

## Updated Context model sketch

```text
Context model entry:
  register_state          saved/restored at context switch
  ttbr                    address space root, written by Space manager
  state                   runnable | blocked(endpoint) | dead
  current_core            core ID | in_flight
  fault_handler           direct Endpoint reference (kernel-internal)
  time_handle             handle index into capability table
  timing_mode             periodic(d, dt, p, pt) | responsive(d, dt, l, lt) | bulk
  pending_message         source, type, payload (register-sized)
  capability_table        pointer to per-Context handle table
```

Changes from journal 007:

- `scheduling_hints` replaced with `timing_mode` — concrete, validated structure
- The scheduler reads `time_handle` (fraction) and `timing_mode` (requirements)

## Status

**Tentatively accepted:**

- Time object contains a single value: fraction (% of core)
- Conservation: sum of fractions per core ≤ 100%
- Time is fungible and aggregates across multiple handles
- Timing requirements are declared by the Context, not carried by Time
- Two modes: periodic (d, dt, p, pt) and responsive (d, dt, l, lt)
- Bulk (Mode C) falls out of extreme tolerances — not a separate mode
- No Context needs all three of (duration, period, latency) simultaneously
- Tolerances appear directly in the admission formula: f = (d - dt) / (denom +
  tol). Tight tolerances cost more fraction.
- Admission control: Σ (d_i - dt_i)/(denom_i + tol_i) ≤ 1.0 per core (O(n))
- Tolerances are one-directional toward scheduler's benefit: dt = less duration
  tolerated, pt/lt = more period/latency tolerated
- Every parameter has a quantifiable cost — no free parameters
- No priority integers — EDF derives urgency from deadlines
- Passive/active server patterns work without reshaping

**Open questions carried forward:**

- **Memory object shape.** Byte-addressed (spec.md), internal structure TBD.
- **Endpoint capacity.** Fixed at creation — defaults, configurability.
- **Message shape.** Register layout, capability transfer in messages.
- **pending_message vs. Endpoint queue.** When does a message move from queue to
  Context?
- **Blocked Context state.** Wait on one or multiple Endpoints?
- **Timing declaration syscall.** How does a Context declare/update its timing
  mode? What happens if requirements change while running?
- **EDF/CBS implementation details.** Virtual deadline management, budget
  replenishment. Leaf-node concerns but may warrant research for correctness
  confidence.
