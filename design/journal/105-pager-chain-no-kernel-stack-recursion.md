# D105 — Pager chain: no kernel-stack recursion; liveness is open

**Date:** 2026-04-26

**Observation:** The pager chain does not recurse on the kernel stack. Liveness
under perpetually-faulted handlers is an open question.

**Rests on:** D12 (fault delegation — faults route to handler), D31 (resource
acquisition through pager chain), D44 (Pulsar — supervision timer pattern from
D68), D68 (pager unavailability — dead handler detection via cap invalidation),
D80 (fault delivery protocol — deliver_fault enqueues and returns), D100 (fault
delivery mechanics — handler unavailable path).

**Status:** observation (partial).

---

## Confirms

### No kernel-stack recursion

`deliver_fault()` enqueues a fault message into the handler's Field and returns.
It does not call into the handler, does not wait for the handler to process the
message, and does not recurse if the handler itself faults.

The chain unrolls through scheduling rounds:

1. Observer A faults. Kernel calls `deliver_fault(A)`, which enqueues into
   handler B's Field. `deliver_fault` returns. A is now in Faulted state.
2. Scheduler picks B (B has a pending message). B runs, processes A's fault, and
   itself faults while doing so.
3. Kernel calls `deliver_fault(B)`, which enqueues into handler C's Field.
   `deliver_fault` returns. B is now in Faulted state.
4. Scheduler picks C. C runs and resolves B's fault. B resumes, resolves A's
   fault. A resumes.

Each `deliver_fault` call is a single stack frame — enqueue message, update
Observer state, return. The chain depth is bounded by the supervision hierarchy
depth, but each level is a separate scheduling round, not a nested kernel call.

**Stack overflow is not a risk.** Even with deeply nested handler chains, the
kernel stack usage per fault is constant (one `deliver_fault` frame per
scheduling round). This is a structural property of the message-passing design,
not a runtime check.

## Opens

### Liveness under live-but-stalled handlers

D68 handles the dead handler case: when a handler Observer is destroyed, cap
invalidation detects the destroyed Field, and `deliver_fault` returns
`HandlerUnavailable`. The chain terminates — the faulting Observer is destroyed
(or SYSTEM_OFF for root, per D100).

The unresolved case: a handler that is **live but perpetually faulted**.
Consider the scenario:

1. Observer A faults. Fault delivered to handler B.
2. Handler B faults while processing A's fault. Fault delivered to handler C.
3. Handler C faults while processing B's fault. Fault delivered to handler D.
4. The chain continues — each handler faults before resolving the previous
   fault.

No handler is destroyed. All Fields are valid. D68's cap-invalidation detection
does not trigger. The fault messages accumulate in Fields, consuming arena
capacity. Eventually, arena exhaustion prevents further enqueue — but this is a
resource exhaustion failure mode, not a liveness guarantee.

**"Bounded by arena capacity" is technically true but unsatisfying.** It
prevents unbounded resource consumption (the arena is finite), but it does not
guarantee timely progress. The faulting Observers are stuck indefinitely, and
the system makes no forward progress on their behalf.

### Possible future resolution: Pulsar watchdog

D44 defines Pulsars as kernel-managed timers. D68 establishes the supervision
pattern: a supervisor Observer monitors its children and takes corrective action
on failure.

A watchdog pattern: the supervisor creates a Pulsar that fires periodically. On
each fire, the supervisor checks whether its children have made progress (e.g.,
by inspecting a shared memory counter or checking Observer state via
ReadRegisters). If a child's handler chain is stalled, the supervisor can
destroy the stalled handler and install a replacement, or destroy the faulting
Observer directly.

This is a userspace policy, not a kernel mechanism — the kernel provides the
primitives (Pulsar, Destroy, ReadRegisters), and the supervisor implements the
timeout policy. Whether this is sufficient, or whether the kernel needs a
built-in watchdog for handler liveness, is the open question.

---

## Does NOT settle

- Liveness guarantee for perpetually-faulted handler chains (the kernel provides
  no built-in timeout or watchdog for stalled handlers)
- Whether a kernel-level watchdog is needed or whether userspace Pulsar-based
  supervision (D44, D68 pattern) is sufficient
- Arena exhaustion behavior when fault messages accumulate from stalled chains
  (whether the kernel should proactively detect accumulation or treat it as
  normal resource pressure)
- Maximum supervision hierarchy depth (no limit is imposed — depth is bounded
  only by the number of Observers in the system)
