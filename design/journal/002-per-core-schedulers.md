# Per-Core Schedulers With Differing Algorithms — 2026-04-15

Records the reasoning behind `spec.md#D2`.

## Starting point

D1 establishes that each core has its own scheduler as a consequence of the
per-core hot path. That consequence alone would permit all per-core schedulers
to run the same algorithm, varying only their state.

The open question this entry answers: should per-core schedulers be required to
run the same algorithm, or should different algorithms per core be a first-class
possibility?

## Exploration

### What would force a single algorithm?

A single-algorithm commitment would be justified if:

- All cores were identical in hardware characteristics.
- All workloads had the same scheduling requirements.
- Or if sharing an algorithm bought something structural (e.g., enabling gang
  scheduling across cores).

None of these hold under the current axioms.

A2 (ARM64) covers asymmetric-core hardware. Apple silicon's big.LITTLE is
explicitly within the target: performance cores and efficiency cores have very
different power/throughput envelopes. A single algorithm tuned for a performance
core would be wrong for an efficiency core — fixed-priority round-robin on an
efficiency core consumes less scheduler overhead per decision than a
sophisticated fair-share algorithm, which matters when the core is
power-constrained.

A3 (generic kernel) forbids mandating one algorithm as "the right answer" for
all workloads. A kernel serving a real-time audio workload on one core and a
batch compute workload on another should not have to compromise between
scheduling algorithms. Each workload class has well-studied algorithms that fit
it; forcing them through a single algorithm is a policy decision the kernel
should not make.

A5 (kernel is a leaf node) is not load-bearing for D2. A5 answers the
kernel|userspace question — does scheduling policy belong inside the kernel or
pushed out to userspace? — and that is already settled for this project:
scheduling is kernel-side. A5 is silent on the internal question this entry
addresses: whether per-core schedulers must share one algorithm or may differ.
A2 and A3 do that work alone. `spec.md#D2`'s "Rests on" line correctly
reflects this by citing A2 + A3 + D1 without A5.

### Requirements on the Frame model

Allowing different algorithms per core imposes a constraint: the Frame model
cannot carry algorithm-specific state. A Frame that might migrate from a CFS
core to a fixed-priority core cannot carry CFS virtual runtime, because the
destination scheduler has no way to interpret it.

The Frame carries _abstract_ scheduling properties — properties any reasonable
scheduler can interpret in its own terms:

- Priority (relative, within a class).
- CPU-bound vs. IO-bound classification (hint for schedulers that distinguish).
- Optional deadline (for schedulers that support deadline-based selection).

The minimum set of abstract properties is open (noted in spec.md#Open
questions).

Algorithm-specific state — CFS vruntime, deadline tracking counters,
priority-aging state — lives in the per-core scheduler's own structures, not in
the Frame. On migration, abstract properties transfer; the destination scheduler
initializes its own algorithm-specific state from those abstracts and its own
current state.

Some information loss on migration is acceptable and expected. A CFS vruntime
does not transfer to a round-robin scheduler; the destination initializes with a
reasonable default based on the abstract priority. Migration is cold-path (D1),
so this reinitialization cost is paid infrequently.

### Landscape

No surveyed system cleanly exposes per-core scheduler algorithms as a
first-class feature. Linux has per-CPU runqueues but runs CFS globally;
algorithm variation across cores is not supported. Zircon, Fuchsia's kernel,
uses a single scheduling algorithm. QNX runs a single priority scheduler. seL4
delegates scheduling policy entirely and does not grapple with this question.

This is a novel position. The novelty is small (not an unprecedented
architectural move), but it does mean we cannot adopt a prior-art blueprint
wholesale — the Frame-model constraint above is a consequence we had to derive.

### Applying "isolate uncertain decisions behind interfaces"

The philosophy principle "isolate uncertain decisions behind interfaces"
(philosophy.md) does load-bearing work here. Scheduling algorithm choice is
genuinely uncertain: workloads, hardware envelopes, and user policies all argue
for different answers. The response is not to pick one algorithm and commit — it
is to make the scheduler algorithm a leaf node swappable per core, and to expose
a Frame-level interface that any reasonable algorithm can satisfy.

## Status

**Accepted as `spec.md#D2` — settled, with one revisit trigger.**

Settled because the reasoning rests on stable premises: A2's asymmetric-core
hardware scope, A3's prohibition on mandating one scheduling algorithm for all
workloads, D1's per-core structure, and the landscape check showing novelty
without structural blockers. The arguments are complete as stated.

**Revisit trigger:** the minimum abstract-property set on the Frame has not yet
been derived. If that downstream derivation finds no property set expressible
across candidate scheduling algorithms (CFS, round-robin, fixed-priority,
deadline-based, etc.), D2 must be re-examined — either walked back to commit to
a single algorithm, or qualified to permit heterogeneity only within compatible
algorithm groups.

Triggers considered and rejected as unrealistic under current axioms:
formal-verification pressure (not a project goal, and A5 should not bend to
accommodate downstream quality concerns if it ever became one); hardware
homogenization (ARM64's direction of travel is the opposite); emergence of a
universal scheduling algorithm (scheduling is NP-hard in general).

**Open sub-questions:**

- Minimum abstract scheduling-property set on the Frame.
- Whether cross-core migration in the presence of algorithm heterogeneity needs
  admission control on the destination (a Frame migrating to a deadline
  scheduler must fit into its deadline admission envelope).
- Whether userspace can influence per-core algorithm choice, or whether
  algorithm choice is strictly a kernel-configuration-time decision.
