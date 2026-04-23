# Core Activation — 2026-04-22

Records the reasoning behind `spec.md#D46`. Settles the core activation and
lifecycle model: how the kernel brings secondary CPU cores online, manages idle
cores, and deactivates them.

## Starting point

D31 settled the boot architecture (kernel creates root Observer with minimal
resources on a single core, acts as root pager) and explicitly listed "secondary
core bring-up" as an open question. The question: how do secondary cores come
online, and what is their lifecycle?

Phase 2 confirmed no journal entry had explored this. Journal 001 established
the per-core execution model (D1). Journal 032 flagged the question and
suggested "a typed kernel syscall is likely." The landscape (§7.3) covers ARM64
firmware mechanisms (PSCI, spin tables, ACPI parking) but no research document
surveys how reference kernels expose core activation to userspace.

## Exploration

### Cores are kernel-internal

The existing design already hides core identity from Observers. D31/D36
established that core assignment, migration, algorithm selection, and the
compute-unit-to-time translation are kernel-internal. Observers reason about
abstract compute units (D36), not cores.

The natural extension: core _existence_ is also kernel-internal. There is no
"activate core" syscall, no Core kernel object type, no capability that
represents a core. Observers do not know what cores exist, how many are active,
or when one activates or deactivates.

The Space parallel is exact. Space = bytes, kernel manages physical pages. Time
= compute units, kernel manages cores. Observers never request "allocate
physical page 0x8000" and they never request "activate core 3." Both are
implementation details of the kernel's resource management.

This dissolves journal 032's suggestion of "a typed kernel syscall." No syscall
is needed. Core management is a kernel-internal implementation concern, like
physical page allocation or TLB management.

### Activation trigger

A4 (purely reactive) means the kernel has no background thread to decide "I need
another core." Core activation must happen as a side effect of handling an
exception — or at boot.

Boot is the one place the kernel acts proactively. The simplest model: activate
all discovered cores during boot, before creating the root Observer. Every core
initializes its per-core kernel structures (D1: exception vectors, stack,
scheduler instance, hot-path data), configures its MMU (D5), enables its GIC
redistributor (D22), and enters the scheduling loop. Cores with no runnable
Observers enter an idle state immediately.

Lazy on-demand activation (activate cores only when scheduling pressure or Time
allocation demands it) was considered and rejected for two reasons:

1. **Latency.** PSCI CPU_ON takes ~1ms. If activation is triggered during a
   scheduling decision, that's a 1ms synchronous delay — unacceptable for
   hard-RT workloads (D42 precision dimension).
2. **Complexity.** The kernel must detect "need more capacity," handle
   asynchronous bring-up, and manage a partially-initialized core entering the
   scheduling pool. Boot-time activation eliminates all of this.

The A3 concern (generic — some workloads don't need all cores; wasted power on
embedded) is addressed by idle power management, not by deferring activation.

### Idle power management

Once all cores are active, cores with no runnable Observers enter an idle state.
The specific power state (WFI, PSCI CPU_SUSPEND, or deeper platform-specific
sleep) is an architecture-specific implementation detail behind `src/arch/`.

On ARM64:

- **WFI** (Wait For Interrupt): clock-gated, ~5–15% of active power, <1μs wake.
  Universally available.
- **CPU_SUSPEND**: core power domain may be cut, ~1–3% of active power, ~100μs
  wake. Requires PSCI. Firmware saves/restores context.

The kernel wakes idle cores via IPI (O2) when work arrives. The idle/wake cycle
is invisible to Observers — they see only their abstract compute allocation.

The choice of maximum idle depth (how deep the kernel allows idle cores to
sleep) is a per-platform configuration concern, not a design decision. It
depends on hardware capabilities (PSCI version, available power states) and
deployment context (battery-powered vs. always-on).

### Deactivation

Cores may be fully deactivated (PSCI CPU_OFF) when the kernel determines they
are no longer needed. The key constraint is D36 conservation: "total system
capacity = sum of all core capacities" and "the kernel cannot over-allocate."

Because D36 makes Time fungible (normalized compute units, not per-core
fractions), deactivation does not require per-core-origin tracking or Time cap
revocation. The kernel simply checks:

```
unallocated_time_pool ≥ core_capacity
```

If the kernel's unallocated Time pool is at least as large as the core's
capacity, it can shrink the pool by that amount and power off the core. No
Observer is affected — their Time caps still represent valid compute claims
against the remaining active cores. The conservation invariant holds: the pool
shrinks by exactly the deactivated core's capacity, and the check ensures it
can't go negative.

The deactivation sequence:

1. Migrate all Observers off the target core (normal scheduling operation — D43
   says core assignment is transient, re-decided per runnable transition).
2. Verify unallocated pool ≥ core capacity.
3. Shrink the kernel's Time pool.
4. Issue PSCI CPU_OFF.

Re-activation follows the same boot-time initialization path (PSCI CPU_ON to
kernel entry point, initialize per-core structures, enter scheduler).

### What the activated core does

After initialization, the newly activated core enters the scheduling loop and
picks from the global runnable set. It does not wait for explicit Observer
migration — if any Observer is runnable and the core is available, the scheduler
may place the Observer there. Cache affinity is a per-core scheduler hint (D43:
"transient core assignment"), not a hard constraint.

The per-core scheduler algorithm is a kernel-internal decision (D2 + A5). The
kernel chooses based on the scheduling profiles (D42: responsiveness,
throughput, precision) of Observers it expects to schedule on that core.

### GIC redistributor

Each core has a GIC redistributor (landscape §5.7) with per-core registers
accessible only from that core (GICv3 architecture). The newly activated core
configures its own redistributor as part of its initialization sequence. This
includes enabling the redistributor, configuring SGI/PPI handling, and marking
the core as available for SPI routing. The kernel may update GICD_IROUTER
entries to route specific SPIs to the new core, but interrupt routing policy is
a separate kernel-internal concern.

### Rejected alternatives

**Userspace core activation syscall:** D31/D36 already hide core identity from
Observers. Exposing core activation as a syscall would re-introduce core
identity into the Observer's view, contradicting the Time vocabulary revision.
It would also require a capability type for cores — an authority model for a
resource that the Observer has no other reason to know about.

**Lazy on-demand activation:** Rejected for latency (~1ms PSCI CPU_ON during
scheduling path) and complexity (partial-core-availability management). Boot-
time activation with idle power management achieves the same power savings
without the runtime complexity.

**No deactivation (activate-only):** Considered but not chosen. D36 fungibility
makes deactivation simple (no reclamation). Omitting it loses real power savings
for no design benefit. The deactivation check is a one-line invariant.

**Symmetric bring-up (all cores boot simultaneously):** Foreclosed by D31's
minimal-boot model. Boot creates one root Observer on one core. The kernel
activates secondary cores sequentially via PSCI CPU_ON during its boot sequence.

## Status

**Decided.** Core lifecycle is fully kernel-internal. Cores activate at boot,
idle when unused, deactivate when the conservation invariant permits.

Does NOT settle: specific idle power state policy per platform, interrupt
routing policy across cores (which core receives a given SPI), per-core
scheduler algorithm selection policy (which algorithm a core runs and when it
changes), boot ordering for secondary cores (parallel vs. sequential PSCI
CPU_ON), specific thresholds for deactivation decisions.
