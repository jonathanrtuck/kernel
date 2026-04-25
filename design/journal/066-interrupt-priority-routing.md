# Journal 066 — Interrupt Priority and Routing: Kernel-Automatic

Settles the two deferred sub-questions from D22 (interrupt priority exposure,
SPI routing policy). Both are absorbed by the kernel — no new interface surface.
Closes G03.

## Context

D22 settled device interrupt delegation through fields but explicitly deferred
two GIC configuration questions: (1) GICv3 8-bit hardware priority — should it
be kernel-managed (flat) or exposed to userspace? (2) SPI affinity routing via
GICD_IROUTER — should the kernel decide which core receives each SPI, or should
userspace influence routing?

The G03 exploration evaluated five options spanning full absorption through full
exposure. This journal records the settlement and the reasoning that collapsed
the option space.

## The two sub-problems are structurally different

The exploration's key finding is that priority and routing have different
structural motivations, and can be decided independently.

**Routing** has a concrete performance-correctness argument. The D13 fast path
(~400-cycle direct-switch) requires that the interrupt land on the core where
the driver Observer is currently running. If GICD_IROUTER points to the wrong
core, the kernel must IPI the driver's core (~3000+ cycles) — an
order-of-magnitude latency penalty. This is not a soft performance preference;
it is a measurable gap that affects any workload needing deterministic interrupt
latency.

**Priority** has a workload-contingent argument. GICv3 hardware priority
(IPRIORITYR) controls which of _simultaneously-pending_ interrupts is delivered
first. If interrupts arrive non-simultaneously (the normal case), priority has
no effect on latency. The benefit of priority exposure is proportional to the
frequency and criticality of simultaneous-interrupt scenarios — a property of
the target workload, not of the design graph.

## Routing: kernel-automatic, no new API

The kernel already tracks every relationship needed to make optimal routing
decisions:

1. SPI → Field (the kernel's internal IRQ routing table, established at attach
   time)
2. Field → receive-cap holder (the Observer that holds the receive right)
3. Observer → core (transient assignment, known to the scheduler)

When the kernel migrates an Observer that holds the receive cap for an interrupt
Field, it updates GICD_IROUTER for all SPIs routed to that Field. Similarly,
when a receive cap is transferred to a different Observer, the kernel updates
GICD_IROUTER to point to the new holder's core.

This is entirely cold-path work (D1-consistent). GICD_IROUTER writes are MMIO +
DSB, estimated ~50–100 cycles per SPI. Migration is already expensive (TLB
maintenance, cache effects); the additional GICD_IROUTER update is negligible.
For the majority of Observers (non-drivers with no interrupt Fields), the check
is a no-op.

A race window exists between migration and the GICD_IROUTER update: during that
window, an SPI may fire to the old core. This window is bounded and minimized by
updating GICD_IROUTER as part of the migration operation itself (not lazily on
the next interrupt). For RT Observers on dedicated cores (D2), migration is
rare, making the race window negligible in practice.

### Why no userspace API

The exploration's Option 3 (routing exposure) proposed a new typed operation:
`field_route_irq_to(field_cap, irq_range, observer_cap)`. This would let
userspace name a routing target explicitly. But the only information the API
provides is "route to wherever this Observer is" — which is exactly what the
kernel derives from the receive-cap-holder relationship. The API would let
userspace say what the kernel already knows.

The one edge case where the routing target might differ from the receive-cap
holder — a supervisor routing interrupts to a sentinel Observer before the
actual driver starts — is handled by the supervisor holding the receive cap on
the sentinel Observer. The automatic policy is correct for this case too.

No new operation, no new right, no D48 extension.

## Priority: flat absorption, forward-compatible

GIC hardware priority stays kernel-internal. All SPIs are programmed at the same
IPRIORITYR value (currently 0xA0 in `gic.rs`). The kernel may introduce internal
tiering in the future (e.g., timer interrupts at higher priority than device
SPIs) without changing the userspace interface.

### Why flat is sufficient now

With routing solved (interrupts land on the driver's core), the kernel's
interrupt handler is short (~500 cycles). The window for simultaneous-interrupt
pending is narrow. The remaining benefit of hardware priority differentiation is
limited to the scenario where two SPIs are pending simultaneously on the same
core and their relative urgency matters for correctness. This scenario is:

- Proportional to the number of interrupt sources per core (partitioned systems
  with one driver per core are unaffected)
- Dependent on the target hard-RT workload profile (not yet defined for this
  kernel)
- Addressable through scheduling priority (D2/D42) rather than hardware
  interrupt priority — seL4 MCS demonstrates hard-RT guarantees through
  scheduling-context capabilities without GIC priority exposure

### Why not kernel-derived priority from scheduling profile

The exploration's Option 4 (Variant E) proposed deriving GIC priority from the
Observer's D42 precision value. This was rejected for three reasons:

1. **Semantic mismatch.** Precision means scheduling jitter tolerance; GIC
   priority means simultaneous-interrupt arbitration. These correlate in common
   cases but are independent dimensions. Coupling them loses expressiveness: a
   workload needing high GIC priority but relaxed scheduling precision cannot
   express this, and vice versa.

2. **One knob, two effects.** The precision value would control both scheduling
   behavior and hardware interrupt arbitration — producing unexpected
   side-effects in both directions.

3. **Policy baked into kernel.** The precision-to-GIC-priority threshold is a
   kernel policy choice that varies by deployment. A3 says the kernel should not
   assume workload characteristics.

### Forward-compatibility

Flat priority is forward-compatible with future priority exposure. A
`field_set_irq_priority` operation could be added later without breaking
anything built assuming flat priority. The reverse (exposing priority now,
removing it later) would be a breaking change. This satisfies the constraint of
not foreclosing hard-RT configurations.

## Prior art

Every surveyed capability microkernel keeps GIC hardware priority
kernel-internal with no userspace configuration API: seL4, Zircon, L4Re, NOVA,
Genode, Barrelfish, Minix 3, Redox. This is universal prior art for the
absorption direction.

For routing: no surveyed microkernel implements automatic GICD_IROUTER tracking
on Observer migration. Most either route to a fixed core or use "any available"
(GICD_IROUTER IRM bit). The automatic-tracking approach is novel but derived
from first principles (D13 fast path + D22 field model + the kernel's existing
knowledge of receive-cap-holder → core).

seL4 MCS achieves hard-RT guarantees through scheduling-context capabilities and
fixed-priority preemptive scheduling, not through GIC priority management
(Blackham et al., EuroSys 2012).

## Rejected alternatives

**Priority exposure via field cap (Option 2).** Novel — no capability
microkernel has implemented this. Leaks GIC-specific semantics (8-bit priority,
group configuration, ICC_PMR thresholds) into the userspace interface. Creates
conceptual asymmetry with D42 (which dissolved priority integers for
scheduling). The simultaneous-interrupt scenario it addresses is rare with
correct routing. Not foreclosed — can be added later if a defined workload
requires it.

**Routing exposure via Observer cap (Option 3).** Provides the same information
the kernel already derives from receive-cap-holder tracking. The one edge case
(routing to a non-receiver Observer) is handled by the receiver holding the
receive cap. Adds unnecessary API surface.

**Kernel-derived tiered priority (Option 4 / Variant E).** Couples scheduling
precision to GIC priority — semantic mismatch, loss of independent
expressiveness, kernel policy that A3 discourages. See "Why not kernel-derived
priority" above.

**Combined exposure (Option 3+2).** Maximum interface surface for the least
constrained problem. Both sub-problems are adequately handled by absorption.

## Summary

| Sub-problem | Resolution       | Mechanism                                        | API change |
| ----------- | ---------------- | ------------------------------------------------ | ---------- |
| Routing     | Kernel-automatic | GICD_IROUTER tracked on migration + cap transfer | None       |
| Priority    | Flat absorption  | All SPIs at same IPRIORITYR                      | None       |

- **Rests on:** D22 (interrupt delegation through fields — the two deferred
  sub-questions originate here), D13 (fast-path direct-switch — the structural
  argument for correct routing), D1 (GICD_IROUTER updates are cold-path — D1
  consistent), D42 (scheduling profile provides RT latency through scheduling,
  not hardware interrupt priority), D2 (per-core schedulers — RT Observers on
  dedicated cores rarely migrate, minimizing the routing race window), A5
  (absorbed complexity — simple interface, no new operations), A3 (generic — not
  foreclosed; priority exposure can be added if a defined hard-RT workload
  requires it).
- **Status:** settled. Revisit the priority sub-question if a defined hard-RT
  workload demonstrates that simultaneous-interrupt arbitration is a correctness
  requirement not addressable through scheduling.
- **Journal:** `journal/066-interrupt-priority-routing.md`.
