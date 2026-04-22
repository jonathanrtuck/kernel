# 042 — Minimum abstract scheduling properties: single axis

2026-04-22. Starting from the explicit open question in spec.md: "Minimum
abstract scheduling properties on an Observer. D2 says Observers carry abstract
scheduling properties, but the minimum set (priority? deadline? IO-bound flag?
period?) is not fixed."

All parent decisions settled: D2 (per-core schedulers, abstract properties on
Observer), D36 (Time carries compute units, Observer carries scheduling hints),
D37 (Time donation transfers compute not priority, priority-level inheritance
deferred here), D39 (modify-scheduling right, three structural use cases), D1
(hot-path scheduling), A2 (big.LITTLE), A3 (generic), A5 (kernel absorbs
complexity), D31 (core assignment kernel-internal).

---

## The priority problem

D2's parenthetical lists "priority, CPU/IO classification, optional deadline" as
illustrative scheduling properties. The natural approach: put an integer
priority on the Observer, optionally add CPU/IO and deadline fields.

This approach has a structural flaw. Priority is a relative ordering with no
built-in cost. Every Observer is better off at max priority than at any lower
value, regardless of what others choose. The supervisor sets the child's
priority (D39 modify-scheduling), but nothing prevents the supervisor from
setting all children to max. If every Observer is max priority, the scheduler
falls back to round-robin and priority becomes meaningless.

Compare with Time and Space: both are conserved resources. You cannot give a
child more Time than you have (D31 bounded pool). You cannot give a child more
Space than you have (D41 split conservation). Inflation is structurally
impossible. Priority, as an unbounded hint, has no such conservation.

The landscape's solution is seL4's max-controlled-priority (MCP): each TCB
carries a priority AND a ceiling on the priority it can set on others. This
creates a delegation chain. It works, but it is a bolt-on enforcement mechanism
for a problem created by the model — patching the inflation flaw rather than
dissolving it.

### Why priority doesn't have a natural cost

Time's conservation is physical: compute cycles are finite. Space's conservation
is physical: bytes are finite. Priority has no physical backing — it is an
ordering, not a quantity. There is no "priority pool" to deplete. One Observer
having high priority does not take priority "away" from anyone else.

This asymmetry (Time and Space are conserved, priority is not) suggests that
priority is the wrong abstraction. The question is whether there is a scheduling
property with a natural, built-in cost that dissolves the inflation problem
structurally.

---

## Dissolving priority: the responsiveness ↔ throughput axis

An Observer's Time allocation (D36 compute units) determines HOW MUCH compute it
receives. The scheduling property determines HOW that compute is delivered. The
delivery has a fundamental physical trade-off:

- **Many short slices:** the Observer is scheduled immediately when runnable
  (low wake-to-run latency), but each scheduling slice is short. More context
  switches occur, each costing real cycles (~1000–5000 on ARM64: TLB flush,
  cache pollution, pipeline drain). Effective compute per unit of Time is
  reduced.

- **Few long slices:** the Observer waits for scheduling opportunities (higher
  wake-to-run latency), but each slice is long and uninterrupted. Fewer context
  switches, less overhead. Effective compute per unit of Time is maximized.

This is one axis with two ends:

| End            | Behavior                                               | Who wants it                         |
| -------------- | ------------------------------------------------------ | ------------------------------------ |
| Responsiveness | Scheduled fast, short slices, high overhead            | Interrupt handlers, UI, audio        |
| Throughput     | Scheduled opportunistically, long slices, low overhead | File I/O, batch compute, compression |

The trade-off is physical. Context-switch overhead is real, measurable in the
same cycles that Time denominates. An Observer at max responsiveness gets its
Time allocation delivered in many tiny slices with maximum overhead. An Observer
at max throughput gets its Time allocation delivered in few large slices with
minimum overhead. Neither end is strictly better — the right answer depends on
the workload.

### Why this dissolves inflation

Every Observer gets the same scale (0–100). Spending points on responsiveness
does not take responsiveness from others (Time handles cross-Observer
conservation). The constraint is internal: high responsiveness costs the
Observer throughput efficiency. An Observer that sets max responsiveness is not
"winning" at others' expense — it is choosing a delivery shape that costs it
effective compute. A supervisor setting all children to max responsiveness
degrades all children's throughput. There is no incentive to inflate because
inflation has a real, self-imposed cost.

No MCP-style delegation bound is needed. No priority delegation chain. The
trade-off is self-enforcing.

### The responsiveness value as scheduling input

The per-core scheduler (D2) reads the responsiveness value and interprets it in
algorithm-specific terms:

- **Fixed-priority scheduler:** maps responsiveness to priority level. Higher
  responsiveness = higher priority = scheduled sooner = shorter timeslice.
- **Fair-share scheduler (CFS/EEVDF):** maps responsiveness to timeslice length
  and preemption threshold. Higher responsiveness = shorter timeslice = more
  frequent scheduling.
- **Deadline scheduler (EDF/CBS):** uses responsiveness as a secondary signal
  for ordering when deadlines are equal. Primary scheduling uses (Time, period)
  — see hard-RT section below.

All algorithm families can interpret the value (D2 requirement). No algorithm is
disadvantaged. The value is qualitative (D36 — quantity is Time's job).

---

## Hard real-time without additional properties

D2 allows per-core scheduler heterogeneity. A dedicated RT core can run an EDF
scheduler. The question: does the single-axis model provide enough information
for hard-RT guarantees?

### The kernel already knows the period

Userspace timers are kernel-programmed: the Observer requests a timer via a
typed kernel syscall (D7). The kernel programs the ARM64 generic timer and
deposits a message to the Observer's field (D13) when it fires. Timer delivery
follows the D22 interrupt pattern — kernel-as-sender to a field with a badge
(D17).

Because the kernel programs the timer, it knows the period. The audio driver
requesting a 5.3ms timer gives the kernel T = 5.3ms without any scheduling
declaration.

### Time gives the compute budget

The Observer's Time allocation (D36) gives C — the compute budget per scheduling
window. The kernel translates compute units to per-core time using the capacity
factor (D36).

### EDF admission from (C, T)

On a dedicated RT core, the scheduler has:

- C_i = Time allocation for Observer i (converted to per-core time)
- T_i = timer period for Observer i (kernel-programmed, kernel-known)

The EDF admission test: Σ (C_i / T_i) ≤ 1.0 (Liu & Layland 1973). If this holds,
all deadlines are met. The kernel has both values without any additional
Observer scheduling properties.

For event-driven RT Observers (no periodic timer), the responsiveness value
provides the latency requirement. On a dedicated RT core, the scheduler can map
responsiveness to a virtual deadline (CBS algorithm), enabling EDF scheduling
for event-driven tasks alongside periodic ones.

### Core partitioning for hard-RT

Hard-RT guarantees require a dedicated RT core — all Observers on the core
participate in RT admission control. Mixing hard-RT and non-RT on the same core
is the mixed-criticality problem, which is genuinely hard. D2's per-core
scheduler heterogeneity was designed for this: RT cores run RT schedulers;
non-RT cores run fair-share or whatever fits their workload. Core assignment is
kernel-internal (D31) — the Observer does not know which core it runs on.

This does not foreclose mixed-criticality on a single core as a future
optimization. The single-axis value is sufficient input for any such scheme. But
mixed-criticality is not required for hard-RT support — core partitioning is the
first-class mechanism.

---

## Candidate axes that were examined and rejected

### CPU/IO classification

D2's parenthetical lists "CPU-bound vs. IO-bound classification." This is
structurally motivated (A2 big.LITTLE placement) but not forced by any settled
decision. It fails the trade-off test only partially — the trade-off would be
"declare yourself IO-bound and accept being placed on an efficiency core" — but
the cost is not self-imposed by the Observer. Core placement is kernel-internal
(D31/D36/A5). The kernel can infer workload character from the responsiveness
value: high responsiveness with low Time utilization per wake = IO-bound. High
throughput with high Time utilization = CPU-bound. No explicit property needed.

### Optional deadline

D2 lists "optional deadline (for schedulers that support deadline-based
selection)." This fails the trade-off test: there is no desirable "patience" end
— every Observer would set the tightest deadline, and deadline inflates like
priority. Furthermore, the kernel derives deadline information from
timer-programmed periods for periodic tasks, and from the responsiveness value
for event-driven tasks. An explicit deadline on the Observer is redundant with
information the kernel already holds.

### Consistency / jitter

Examined as "how predictable is my scheduling latency." Collapses into
responsiveness: at high responsiveness, the Observer is always scheduled
immediately, so variance is inherently near-zero. At moderate responsiveness,
the variance is a consequence of the responsiveness level and the per-core
scheduler's algorithm. Not an independent dimension.

### Energy preference

Examined as "prefer performance core vs. efficiency core." Core placement is
kernel-internal (D31/D36/A5). The kernel derives placement from the
responsiveness value + Time allocation + hardware capacity factors. The Observer
should not know or express core-type preferences.

---

## Representation

The responsiveness value is a single unsigned integer on a fixed scale. The
specific range (0–100, 0–255, or other) is an encoding detail, not
architectural. Properties:

- **0** = maximum throughput. The Observer accepts maximum wake-to-run latency
  in exchange for maximum timeslice length and minimum scheduling overhead.
- **max** = maximum responsiveness. The Observer demands minimum wake-to-run
  latency and accepts short timeslices with maximum scheduling overhead.
- **Lives in the Observer struct** (D1 hot-path access). One integer field.
- **Mutable via modify-scheduling** (D39). The supervisor sets or adjusts the
  value. The kernel may also adjust internally during IPC (priority inheritance
  — see below).
- **Set at creation or via modify-scheduling.** If not set, defaults to a
  kernel-defined middle value (best-effort).

### Priority inheritance

D37 defers priority-level inheritance to this exploration. With the
responsiveness axis, priority inheritance becomes responsiveness inheritance:
during a Call(), the kernel temporarily boosts the server's responsiveness to
the caller's level (if higher). This ensures the server runs at the caller's
latency expectations while handling the request. The server's base
responsiveness is restored on reply.

This requires the Observer struct to distinguish base responsiveness (set by
supervisor via modify-scheduling) from effective responsiveness (potentially
boosted by the kernel during IPC). Two fields: base and effective. The scheduler
reads the effective value (hot-path, D1). The kernel writes the effective value
on Call/Reply (cold-path). If no boost is active, effective = base.

Whether the kernel performs this inheritance automatically or the supervisor
does it explicitly (D39 use case #2) remains open. Automatic is simpler for
userspace. Explicit gives the supervisor more control. Both are compatible with
the single-axis model. This is one level down from the property set.

---

## Archive convergence

Archive journal 008 ("Time Shape") derived a structured timing declaration
model: two modes (periodic and responsive), each with duration, tolerance,
period/latency, and tolerance. Every parameter has a cost in Time fraction.
Admission control: Σ (d-dt)/(denom+tol) ≤ 1.0 per core. No priority integers —
EDF derives urgency from deadlines.

**Strong convergence on four points:**

1. Resource (Time) and scheduling preference are separate. Both derivations
   split "how much" from "how to deliver."
2. No priority integers. The archive explicitly rejects them; this derivation
   dissolves them.
3. Every scheduling parameter must have a cost. The archive enforces this
   through the admission formula (tighter = costs more Time fraction). This
   derivation enforces it through the responsiveness/throughput trade-off (more
   responsive = costs throughput efficiency).
4. Scheduling classes emerge from the math, not from separate fields. The
   archive's hard-RT/soft-RT/best-effort spectrum comes from tolerance values.
   This derivation's spectrum comes from the responsiveness value.

**Divergence on three points:**

1. **Structured declarations vs. single axis.** The archive has rich timing
   modes (d, dt, p/l, pt/lt) on every Context. This derivation has one integer
   on every Observer plus kernel-derived (C, T) from Time + timer. The archive
   is more expressive — it can encode exact timing requirements per Observer.
   This derivation relies on the kernel deriving that information from existing
   sources. Divergence explained by: the archive did not have the timer-as-
   kernel-service pattern (timers were discussed separately in the archive), so
   the archive needed explicit period/latency declarations because the kernel
   didn't already hold the information.

2. **Per-core admission vs. per-core-type admission.** The archive applies
   admission control to every Observer on every core. This derivation applies
   hard-RT admission only on dedicated RT cores (D2 per-core algorithm
   heterogeneity). The archive's approach is more rigorous for
   mixed-criticality. This derivation accepts core partitioning as the
   first-class mechanism. Divergence explained by: D2 (per-core algorithm
   heterogeneity) was not in the archive's derivation context — the archive
   committed to EDF everywhere.

3. **Tolerance as explicit cost vs. throughput as implicit cost.** The archive's
   cost is in Time fraction (tighter tolerances consume more of your Time
   budget). This derivation's cost is in throughput efficiency (more
   responsiveness costs effective compute). Both are self-enforcing. The
   archive's is enforced by admission control (hard: the kernel rejects if the
   sum exceeds 1.0). This derivation's is enforced by physics (soft: more
   context switches cost real cycles). Divergence: the archive can reject an
   infeasible declaration; this derivation cannot — an Observer with
   responsiveness 100 on an overloaded core simply gets degraded service.

The archive's model is a superset in expressiveness. The trade-off: this
derivation's model is simpler (one field vs. a mode + four parameters),
consistent with A5 (kernel absorbs complexity — timing parameter management is
pushed from userspace into the kernel's timer mechanism and per-core scheduler),
and relies on information the kernel already holds (timer periods, Time
allocation). The archive needed explicit declarations because it did not have
that information.

---

## Refinement: from single axis to three-value budget

The initial derivation settled a single responsiveness ↔ throughput axis. Two
problems emerged during continued exploration:

### Problem 1: tolerance has no self-enforcing cost

Adding precision/tolerance as an independent property (to capture the archive's
hard-RT expressiveness) fails the trade-off test. From the Observer's POV, there
is no advantage to accepting imprecise scheduling — tight tolerance is strictly
better. This is the same structural flaw as priority: one end is always
preferred, so every Observer demands the maximum, and external enforcement
(admission rejection) is needed to prevent inflation.

### Problem 2: responsiveness and throughput are not strict opposites

The single axis forced responsiveness and throughput to be opposite ends of one
spectrum. But real workloads (interactive UI, moderate servers) want moderate
amounts of both. The single axis can't express "decent wake-up latency AND
decent timeslice length" — only a middle value that ambiguously trades off both.

### Resolution: three-value budget

Responsiveness, throughput, and precision become three dimensions sharing a
fixed per-Observer budget (e.g., 100 points). The Observer distributes points
across all three. The constraint: R + T + P ≤ budget.

- **Responsiveness:** how quickly I'm scheduled when runnable.
- **Throughput:** how long I run when scheduled.
- **Precision:** how accurately the scheduler hits my timing targets.

All three ends have genuine value. The budget creates real per-Observer
trade-offs: spending on one dimension takes from the other two. Precision now
has a self-enforcing cost — an Observer that demands precision 100 gets zero
responsiveness and zero throughput (precise timing on a terrible schedule). No
external enforcement needed.

Representative profiles:

| Workload           | R   | T   | P   | Rationale                            |
| ------------------ | --- | --- | --- | ------------------------------------ |
| Interrupt handler  | 80  | 5   | 15  | Wake fast, tiny work, some precision |
| Audio driver       | 30  | 10  | 60  | Timer-driven, must hit deadlines     |
| File copier        | 5   | 85  | 10  | Don't care when, maximize throughput |
| Interactive UI     | 50  | 35  | 15  | Responsive + decent throughput       |
| Hard-RT control    | 15  | 15  | 70  | Timing is everything                 |
| Background indexer | 5   | 90  | 5   | Maximum throughput, zero urgency     |

### Physical grounding

Each dimension has a physical cost within the Observer's fixed Time allocation:

- High responsiveness costs context-switch overhead (TLB flush, cache pollution,
  pipeline drain per switch — ~1000–5000 cycles ARM64).
- High throughput costs scheduling latency (the Observer waits for opportunities
  to get long runs, accepting delayed wake-up).
- High precision costs scheduling flexibility (the scheduler must deliver at
  exact timing targets, constraining how it optimizes for responsiveness and
  throughput).

The budget captures the physical reality: on a shared core, scheduling quality
is finite. Each Observer declares how to allocate its share of that quality.

### Archive convergence (revised)

The three-value model narrows the divergence with the archive. The archive's
tolerance spectrum (tight = hard-RT, loose = best-effort) maps to the precision
dimension. The archive's six parameters (mode + d + dt + denom + tol) collapse
to three values because the kernel already knows period (timer-programmed) and
compute budget (Time allocation). What remains is: how responsive, how
throughput-oriented, how precise — exactly the three values.

The remaining divergence: the archive's admission formula provides hard
rejection guarantees. The three-value budget is self-enforcing through
per-Observer trade-offs, but the kernel can additionally perform admission
control on RT cores using Time + timer period + the precision value. The
precision value tells the kernel HOW tight the scheduling guarantee must be —
the information the archive's tolerance provided, but now with a self-enforcing
per-Observer cost.

---

## What this settles

The minimum abstract scheduling properties on an Observer are **three values
sharing a fixed per-Observer budget: responsiveness, throughput, and
precision**. D2's parenthetical "(priority, CPU/IO classification, optional
deadline)" is replaced entirely. There is no priority integer, no CPU/IO field,
and no deadline field.

The per-core scheduler (D2) interprets the three-value profile in
algorithm-specific terms. Hard-RT scheduling uses Time + kernel-programmed timer
period + the precision value for admission control.

The Observer struct gains three fields (responsiveness, throughput, precision),
plus effective variants if kernel-internal scheduling inheritance is adopted.

---

## What this does NOT settle

- **Budget size and encoding.** 100 points? 256? Implementation detail.
- **Scheduling inheritance mechanism.** Which values are inherited during IPC
  Call()? Responsiveness is the primary candidate (server should handle the
  request at the caller's latency expectations). Automatic kernel-internal vs.
  explicit supervisor-driven. One level down.
- **Default profile.** What values does a newly-created Observer get if the
  supervisor doesn't set them? Even distribution (33/33/34) or a kernel-defined
  default. One level down.
- **Timer syscall surface.** The mechanism for requesting kernel-programmed
  timers. One-shot vs. repeating. Downstream of this derivation but structurally
  connected (the timer is how the kernel learns the period for RT admission).
- **Observer minimum schema.** This derivation constrains it: the Observer
  struct needs three scheduling fields (and possibly effective variants for
  inheritance). The full schema requires its own derivation.
- **Admission control on RT cores.** How the kernel uses precision + Time +
  timer period for admission. Whether admission applies on all cores or only RT
  cores (D2). One level down.

---

## Axioms

**A2 (ARM64):** Load-bearing. big.LITTLE asymmetric cores motivate the need for
scheduling properties that provide core-placement information without exposing
core identity. The profile provides this: high responsiveness + low throughput
suggests latency-sensitive IO work (prefer efficiency core or performance core
depending on compute need). The kernel translates internally (D31/D36/A5).

**A3 (generic):** Load-bearing. The properties must accommodate all workload
types. The three-value budget spans the full spectrum: interrupt handlers
(responsiveness-heavy), batch compute (throughput-heavy), RT control loops
(precision-heavy), and everything in between. No workload is excluded.

**A5 (leaf node):** Load-bearing. The kernel absorbs scheduling complexity. The
Observer provides three values; the kernel derives timing parameters from timer
requests and Time allocation. Structured timing declarations (archive model)
would push parameter management to userspace. The three-value budget is a
simpler interface than the archive's mode + four parameters.

**A1 (Rust):** Not load-bearing. The property model is language-independent.

**A4 (reactive):** Not load-bearing. The property model is compatible with any
trigger model.
