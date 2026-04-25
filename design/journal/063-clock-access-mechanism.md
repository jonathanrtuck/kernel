# 063 — Clock access mechanism: per-Observer CNTKCTL_EL1.EL0VCTEN

Date: 2026-04-24

## Starting point

D44 mentions "CNTKCTL_EL1 per-Observer" for clock access authority. The
mechanism, storage, and constraints were not derived as a single entry.

## What the design graph forces (not choices)

1. **Per-Observer CNTKCTL_EL1.EL0VCTEN on context switch.** D44 settled this.
2. **D43 gains `clock_access: bool`.** Per-Observer hot-path state lives in the
   metadata struct. Ninth field.
3. **EL0VCTEN only.** EL0PCTEN statically denied (physical counter leaks host
   timing through cntvoff_el2 in hypervisor contexts; A3 includes those
   workloads). EVNTEN irrelevant.
4. **Per-Observer granularity required.** A3 forecloses always-grant and
   always-deny (RT workloads need ~1-cycle reads; isolation workloads need
   counter denial for side-channel defense).
5. **`clock_read()` remains as capless fallback.** D48 settled this. Observers
   with access denied still need to read time.
6. **Hot-path field.** Context switch reads it before MSR write. Must be in
   metadata struct alongside TTBR0.

## Context-switch code shape

```rs
// Context switch: clock access
// SAFETY: CNTKCTL_EL1.EL0VCTEN controls EL0 virtual counter access.
// Written before restoring EL0 state to ensure the register is accurate
// when EL0 resumes.
if observer.clock_access {
    write CNTKCTL_EL1 |= (1 << 1)  // EL0VCTEN
} else {
    write CNTKCTL_EL1 &= !(1 << 1)
}
```

~1 cycle, negligible on context-switch path. May need trailing ISB before EL0
resume — verify against ARM ARM at implementation time.

## Two genuine choices (not settled here)

### Choice 1: Authority mechanism

- **A1 (graft):** Extend `modify-scheduling` right's hints parameter. No new
  right, no D39/D52 revision. Cost: bundles clock authority with scheduling
  authority.
- **A2 (new right):** 10th Observer right + new syscall. Most D4-pure. Cost:
  D39/D52 revision for a 1-bit property.
- **A3 (creation param):** Set at Observer creation. No new rights. Cost:
  immutable (forecloses dynamic revocation without recreation).

### Choice 2: Default policy

- **B1 (grant by default):** Matches all production ARM64 kernels (Linux, QNX,
  Zircon). A5-friendly (common case zero-overhead). Fail-open.
- **B2 (deny by default):** Fail-closed. Novel position. More consistent with
  multi-tenant security requirements. RT setup requires explicit grant.

These interact with G09 (duration vs deadline): if B2 (deny by default),
absolute-deadline-only Pulsar API has poor ergonomics for Observers without
clock access.

## Implementation note

Apple HVF treatment of CNTKCTL_EL1 from EL1 is unverified. If HVF traps it, the
~1-cycle estimate is wrong and the hypervisor testing workflow needs adjustment.

## Status

**Mechanism settled.** Per-Observer `clock_access: bool` in metadata struct,
CNTKCTL_EL1.EL0VCTEN on context switch, EL0PCTEN statically denied, clock_read()
as fallback.

**Authority mechanism and default policy are genuine choices.** Flagged for
designer settlement alongside G09.
