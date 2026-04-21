# 036 — Time parameters: normalized compute units

**Question:** What does a Time object carry? Budget/period (seL4 MCS sporadic
server model), a fraction of scheduling capacity (vocabulary-literal), or just a
claim-to-participate with quantity determined by the scheduler?

**Answer:** A Time object carries a single numerical value: a quantity of
**normalized compute units**. The unit is calibrated to hardware-described core
capacity factors (ARM `capacity-dmips-mhz`, ACPI CPPC `highest_perf`, or
equivalent), so that a given number of compute units represents approximately
the same amount of work regardless of which core executes it. The kernel
translates compute units to per-core scheduling time internally using
precomputed capacity factors — one multiply, cold-path.

This is the Time parallel to Space's bytes: a hardware-independent quantity
where the kernel absorbs all hardware-specific translation.

---

## Prior work

No journal entry in the current chain explored Time parameters directly. The
question was deferred as a downstream question in three entries:

- Journal 030 (D29), line 169: listed as "Time parameters" with three
  candidates.
- Journal 031 (D30), line 161: listed as interacting with aggregate semantics.
- Journal 032 (D31), line 253: listed as unchanged from D29/D30.

Journal 030 (line 102) stated the intended Observer/Time split: "the Time cap
represents the quantity (how much scheduling allocation), and the Observer's
abstract properties (priority, deadline — D2) are hints about how the Observer
wants its total allocation distributed."

Research documents `time-as-kernel-object.md` and
`time-capability-cardinality.md` survey prior art extensively. Key finding: seL4
MCS carries budget/period/refill structures in the SchedContext; algorithm state
lives in the SchedContext. Landscape §4 ("Where scheduling algorithm state
lives") notes the D2 alternative: policy specification in the object, algorithm
state per-core.

---

## Derivation

### Three settled decisions narrow the field

**D30 (multi-Time, additive) requires composable parameters.** The aggregate
formula `total += cap.amount` / `total -= cap.amount` requires each Time cap to
carry a single numerical value where addition is well-defined. Fractions and
weights compose trivially (10% + 20% = 30%). Budget/period pairs with different
periods do not — there is no clean addition rule for (10ms/100ms) + (5ms/50ms).
seL4 MCS avoids this by enforcing 1:1 binding (one SchedContext per TCB). D30
broke that escape route.

**D2 (per-core scheduler heterogeneity) forbids algorithm-specific parameters on
Time.** D2 says "algorithm-specific state (e.g., CFS virtual runtime, deadline
parameters) lives per-core in the scheduler, not in the Observer." Time is part
of the Observer's resource holdings. Budget is quantity ("how much") but period
is algorithm-specific ("when to replenish" — this parameterizes the sporadic
server). Placing period on Time violates D2's state-placement rule.

**D31 (abstract scheduling capacity) implies quantity-only.** The vocabulary
says "a claim to a portion of the system's scheduling capacity." Journal 032's
table: "Observer sees: I have X% scheduling capacity." No mention of periods,
replenishment windows, or temporal patterns — only quantity.

These three together foreclose budget/period as sole parameter (D2 + D30),
no-parameter (D30 + D31 conservation), and any algorithm-specific parameters on
Time (D2).

### The remaining question: what kind of quantity?

With budget/period and no-parameter foreclosed, Time carries a numerical
quantity of scheduling capacity. The initial candidates:

- **Per-core fraction** (e.g., "20% of whatever core you're on")
- **Relative weight** (e.g., "weight 5, share depends on co-scheduled
  Observers")
- **Capacity bound** (fraction as an upper limit)

D31's conservation model (total per core = 100%, kernel cannot over-allocate)
forecloses relative weights — weights have no bounded total, so conservation
breaks. Capacity bound is per-core fraction with weaker semantics. Per-core
fraction initially appeared as the default answer.

### Discovery: per-core fraction breaks the Space parallel

The D9 parallel (Space = bytes, Time = fraction) appeared to hold: both are
hardware-independent quantities with kernel-internal placement. But it breaks
under asymmetric hardware (A2 — ARM big.LITTLE is in scope):

- **Space:** 4KB is 4KB regardless of physical placement. The functional
  guarantee (store 4KB) is hardware-independent. The pager never needs to know
  NUMA topology. The abstraction is lossless.

- **Time (per-core fraction):** 20% of scheduling capacity is NOT functionally
  equivalent across cores. An Observer with a deadline might complete its work
  in 20% on a big core and miss the deadline on a LITTLE core. The functional
  guarantee (complete work in time) is hardware-dependent.

D31 says "core assignment is kernel-internal" — the Observer doesn't see which
core it runs on. But if a pager must know core capabilities to provision Time
correctly for RT workloads, core identity leaks through the abstraction. This is
not a problem Space has: the pager never needs to know about NUMA nodes to
provision bytes.

The root cause: for Space, the unit of what the Observer needs (bytes) and the
unit of what the hardware provides (bytes) are the same. No translation needed.
For Time with per-core fractions, the unit of what the Observer needs (compute
to finish work) and the unit of what the hardware provides (scheduling time) are
different. Translation is unavoidable. The question is where it lives.

### Resolution: normalized compute units

The kernel already knows each core's relative capability. ARM DT provides
`capacity-dmips-mhz` per CPU node. ACPI provides CPPC `highest_perf`. x86
provides HWP `HIGHEST_PERF` from MSRs. RISC-V reuses ARM's DT binding. All
converge on a single scalar capacity value per core.

At boot, the kernel reads per-core capacity factors and establishes a normalized
unit. Each Time cap carries compute units against this scale. The kernel
translates internally:

- Observer with 2000 compute units on a big core (capacity 4000): uses 50% of
  that core's scheduling time.
- Same Observer migrated to LITTLE core (capacity 1000): uses 200% — can't fit.
  Kernel won't place it there.
- The Observer's Time cap didn't change. The kernel handled the translation.

The Space parallel now holds:

- Space = bytes. Hardware-independent quantity. Kernel manages physical
  placement.
- Time = compute units. Hardware-independent quantity. Kernel manages core
  placement.

Empirical measurement is also core-independent. The kernel charges consumed
compute as `elapsed_time × core_capacity_factor`. An Observer that measures "I
need 2000 compute units per frame" gets a measurement valid on any core of the
system. Measure once, on any core, result transfers.

### Properties of the model

- **D30 additive:** 2000 + 3000 = 5000 compute units. The aggregate is integer
  addition.
- **D31 conservation:** Total system capacity = sum of all core capacities. The
  kernel cannot over-allocate. Per-core admission: sum of compute units of all
  Observers on this core ≤ this core's capacity.
- **D2 clean split:** Time carries pure quantity (compute units). Observer
  carries scheduling hints (priority, CPU/IO classification, deadline). The
  per-core scheduler derives algorithm-specific parameters from both. No
  algorithm-specific state on Time.
- **D1 hot path:** Cached aggregate on Observer struct stores precomputed
  per-core fraction (converted on cold path when Time caps change). Scheduler
  reads one number — zero hot-path cost beyond existing model.
- **D9 parallel restored:** Space = bytes, Time = compute units. Both
  hardware-independent quantities with kernel-internal placement.
- **Migration transparent:** Observer's Time cap unchanged on migration. Kernel
  checks admission on target core internally.
- **Integer, no floats.** Resolution set by capacity scale. If largest core
  capacity is 4096 and minimum useful allocation is 1 unit, that's ~0.02%
  granularity — sufficient for scheduling.

### Hard-RT precision

The capacity factor is a first-order approximation (Linux kernel documentation
acknowledges this). For a DT-stated 2x ratio between big and LITTLE, actual
speedup ranges from ~1.2x (memory-bound) to ~3.5x (SIMD). This is imprecise for
cross-core-type RT guarantees.

Resolution: D2 allows dedicated RT cores running RT scheduling algorithms. On a
dedicated RT core, the capacity factor's cross-core-type imprecision is
irrelevant — the kernel knows exactly which core the Observer is on, so the
compute unit → time translation uses one known constant. Hard-RT Observers on
dedicated cores get precise guarantees. Best-effort Observers absorb the
approximation, which is acceptable because they have no hard deadlines.

---

## Archive convergence

Archive journal 008 ("Time Shape") reached the same top-level split through an
independent path:

- "How much CPU — a resource, given to the Context by its creator. Transferable,
  subdividable, attenuatable. This is what the Time capability represents."
- "How to deliver it — a declaration by the Context about its timing needs.
  Intrinsic to the Context, doesn't transfer."
- "Time object contains a single value: fraction (% of core)"
- "Conservation: sum of fractions per core ≤ 100%"
- "Time is fungible and aggregates across multiple handles"
- "The parallel to Space: a Memory capability is just bytes. How the Context
  uses them (stack, heap) is the Context's business, constrained by the amount.
  A Time capability is just a fraction of a core."

**Strong convergence** on the resource/requirements split, fraction-as-quantity,
conservation, fungibility, and the Space parallel.

**Divergence on three points:**

1. **Per-core fraction vs. compute units.** The archive uses raw per-core
   percentage; this derivation uses normalized compute units. The archive did
   not address big.LITTLE heterogeneity — it assumed per-core fractions are
   meaningful. D31's abstraction (core assignment kernel-internal) and the
   Space/Time parallel analysis reveal that per-core fractions leak core
   identity through the provisioning chain. Compute units resolve this.

2. **Timing declarations on the Context vs. D2 abstract properties on the
   Observer.** The archive placed periodic/responsive timing modes (duration,
   period, latency, tolerances) directly on the Context as a validated
   structure. The current design places abstract scheduling properties
   (priority, CPU/IO, deadline) on the Observer per D2. The archive's timing
   declarations are more specific (structured modes with admission control
   formulas); D2's properties are more abstract (hints any scheduler can
   interpret). This is a difference in the sibling open question ("minimum
   abstract scheduling properties on an Observer"), not in Time parameters.

3. **EDF commitment vs. D2 per-core algorithm heterogeneity.** The archive
   committed to EDF with CBS for all scheduling. D2 allows different algorithms
   per core. The archive's admission control formula (Σ (d-dt)/(denom+tol) ≤
   1.0) is EDF-specific. D2's model defers algorithm commitment to the per-core
   scheduler.

All three divergences are explained by settled decisions absent from the
archive's context: D31 (abstract, core-independent), D2 (algorithm
heterogeneity). The top-level conclusion (Time = quantity, scheduling policy
separate) converges independently.

---

## Costs

- **Capacity factor approximation.** The normalized compute unit depends on a
  single scalar per core. Actual speedup ratios vary by workload type (integer
  ~2x, memory-bound ~1.2x, SIMD ~3.5x for a stated 2x factor). This is the same
  approximation Linux EAS, Android cpu_capacity, and every heterogeneous
  scheduler uses. Scheduling is inherently approximate; the capacity factor
  makes it consistently approximate across cores.

- **Boot-time initialization.** The kernel reads capacity descriptors from
  DT/ACPI/firmware and establishes the compute-unit scale. One-time cost.

- **Cold-path conversion.** When Time caps are added/removed, the cached
  per-Observer aggregate (stored as per-core fraction for the current core) must
  be recomputed: one multiply per mutation. Marginal cost.

- **Novel position.** No surveyed system denominates time capabilities in
  normalized compute units. Per-core fractions (archive), budget/period (seL4
  MCS), and parameter templates (Zircon Profile) are the prior art. Novelty is
  justified by D31's core-independence requirement, which no surveyed system
  shares.

---

## What this does NOT settle

- **Unit encoding.** The specific integer representation (total per core, global
  scale, bit width). Implementation detail, not architectural.

- **Minimum Time quantum.** Whether there is a minimum useful allocation below
  which Time cannot be split. Likely kernel-internal policy (A5).

- **Time split semantics.** Split a Time cap with N compute units into two caps
  with N₁ + N₂ = N. Follows from the parameter model — the operation's shape is
  determined. The syscall surface is downstream.

- **Time clonability.** D23 uniformity suggests clonable. Not re-examined here.

- **Time donation on IPC.** seL4 MCS pattern. Multi-Time (D30) makes donation
  via explicit cap transfer natural. Kernel-internal donation on Call() remains
  an option. Deferred.

- **Minimum abstract scheduling properties on the Observer (D2 sibling
  question).** The Observer/Time split is now concrete: Time carries quantity
  (compute units), Observer carries scheduling hints. What minimum set of hints
  enables every per-core scheduler to derive its algorithm-specific parameters
  from (compute units + hints)? This is D2's revisit trigger.

- **Capacity factor source.** Which firmware/DT mechanism provides the capacity
  descriptor. ARM DT `capacity-dmips-mhz`, ACPI CPPC `highest_perf`, or a
  kernel-internal calibration benchmark. A2 implementation detail.

---

## Rejected alternatives

| Alternative                           | Foreclosed by | Reason                                                                                                  |
| ------------------------------------- | ------------- | ------------------------------------------------------------------------------------------------------- |
| Budget/period (seL4 MCS)              | D2 + D30      | Period is algorithm-specific (D2 placement); pairs with different periods don't compose (D30 aggregate) |
| No parameters (claim-to-participate)  | D30 + D31     | Aggregate requires numerical value; conservation requires tracking                                      |
| Algorithm-specific parameters on Time | D2            | D2 places algorithm-specific state per-core, not on Observer/Time                                       |
| Relative weight                       | D31           | Weights have no bounded total; conservation breaks                                                      |
| Per-core fraction                     | D31 + A2      | Breaks Space parallel on heterogeneous hardware; leaks core identity through provisioning chain         |
| Fraction + optional period            | D2            | Period is algorithm-specific regardless of optionality; two-source ambiguity with Observer hints        |

---

## Axioms

**A2 (ARM64):** Load-bearing. big.LITTLE asymmetric cores are within the A2
target. The capacity factor comes from ARM DT bindings (`capacity-dmips-mhz`), a
standard property on heterogeneous ARM SoCs. On homogeneous hardware, all
capacity factors are equal and compute units degenerate to per-core fractions —
no overhead.

**A3 (generic):** Load-bearing. The kernel must support hard-RT, soft-RT, batch,
and interactive workloads. Compute units are algorithm-independent — every
scheduler can interpret them. Hard-RT precision on dedicated cores (D2)
addresses the A3-RT interaction.

**A5 (leaf node):** Load-bearing. The capacity factor translation is
kernel-internal — the Observer never sees core capacities, capacity factors, or
per-core scheduling time. The kernel absorbs the hardware-dependent translation.
This extends D31's principle: Observers don't see core identity; now they also
don't see core capability.

**A1 (Rust):** Not load-bearing. The parameter model is language-independent.

**A4 (reactive):** Not load-bearing. Compute units are compatible with any
trigger model.
