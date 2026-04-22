# 044 — Userspace timer interface: Pulsar

**Date:** 2026-04-22

## Starting point

D42 settled the three-value scheduling profile and identified "timer syscall
surface" as an explicit downstream question. The spec's open question entry
reads: "Userspace timers. Preemption timer is kernel-internal (D2). Userspace
timer callbacks: kernel programs timer on behalf of Observer and deposits
message when it fires. Connects to D2 scheduling model and D13 delivery."

The question: what does the Observer ask for, how does it ask, and what does the
kernel deliver?

## Constraints from settled decisions

- Timer request is a typed kernel operation (D7 — Observer→Kernel, not IPC)
- Delivery through queued field, kernel-as-sender with badge (D13, D17)
- Observer designates delivery field via capability (D4)
- Message fits D28 fixed-size format (4 data + 1 cap + label + badge)
- Hardware timer is per-core, one-shot; kernel multiplexes (A2, A5)
- Timer period feeds EDF admission: kernel uses T + C + precision (D42)
- D42 precision value modulates delivery guarantee
- Userspace-managed timers foreclosed by A5 + D2 + D22
- Timer-as-IPC foreclosed by D7

## Exploration

### Timer as virtual field (explored, rejected)

An idea was explored where the timer is a "virtual field" backed by the hardware
counter. Receive on the field materializes a message with the current time as
the badge. Badge-filtered receive (blocking until badge ≥ X) would serve as the
timer. This would have required two new mechanisms:

1. Badge-filtered receive — a receive variant taking a predicate. Interacts with
   D13 queue ordering (filtered receive skips non-matching messages, breaking
   FIFO) and D18 (skipped messages consume queue capacity).

2. Virtual fields — fields with no sender that synthesize messages from hardware
   state. Breaks D15 uniformity (no queue, no sender, some operations don't
   apply).

**Rejected** for a structural reason: the model is inherently blocking. The
Observer can only wait for a timer by calling receive, which means it can't set
a timer and continue running. Non-blocking timer patterns — timeouts on IPC,
watchdogs, delayed events — require the timer to deliver to a field
independently of the Observer's execution state. The virtual-field model splits
timer into a separate field that can't fan-in with IPC on one receive (D19 badge
fan-in pattern). The majority of timer use cases require non-blocking delivery.

Badge-filtered receive on regular fields was noted as independently interesting
(priority dispatch, type-based event handling) but requires its own exploration
due to D13/D18 interactions. Not pursued here.

### Precision-maximizing design (explored, refined)

A design optimized for hard-RT precision was explored:

- Expose CNTVCT_EL0 for zero-cost clock reads (~1 cycle vs. ~100–200 cycle
  syscall)
- Absolute deadlines (Observer computes from clock read)
- Ack-to-re-arm: one-shot timer delivers message with send-once ack cap (D16),
  Observer sends ack to re-arm. Natural flow control, no overflow, dissolves A4
  persistent-state tension.
- Three timestamps in message (requested deadline, actual fire time, delivery
  time) for drift visibility
- Explicit period parameter for D42 EDF admission

**Stress-tested.** Several problems emerged:

1. _Common case suffers._ Most timers are not hard-RT (UI frames, health checks,
   cleanup, timeouts). Ack-to-re-arm requires two syscalls per period (receive +
   send) where automatic re-arm needs zero after initial set. A5 tension —
   complexity pushed to userspace for the 99% case.

2. _Three timestamps mostly redundant._ The Observer already knows the requested
   deadline. If CNTVCT is exposed, it can read delivery time itself. Only actual
   fire time is information the Observer can't reconstruct. One data word, not
   three.

3. _Kernel can manage drift._ `next_deadline = scheduled_deadline + period` —
   the kernel has both values. No Observer intervention needed for fixed-period
   cases. Drift compensation is trivial for the kernel.

4. _Ack cost at high frequency._ At 100μs periods (10,000/sec), the extra
   syscall costs ~3M cycles/sec — not negligible for the fastest timers.

5. _A4 tension was overstated._ The kernel holds lots of persistent state (cap
   tables, page tables, GIC routing). Timer re-arm on interrupt is
   exception-triggered processing using persistent state — the same pattern as
   everything else the kernel does.

**Refinement:** kernel-managed re-arm as the default path (absorbs drift,
overflow, scheduling). One-shot timers serve as the manual-control escape hatch
for adaptive/variable-period workloads. Precision insights retained: expose
CNTVCT per-Observer, actual fire time in message, explicit period for scheduler.

### Timer lifecycle: object vs. stateless operation

Two structural choices for how the Observer interacts with the timer mechanism.

**Stateless operation** (Plan 9 SLEEP/ALARM model): syscall programs a one-shot
timer, returns opaque ID or nothing. No persistent kernel object.

- Cancel authority outside capability system (opaque ID or field+badge pair)
- Timer resource unbounded — no Space accounting (D32)
- Timer delegation impossible (can't transfer timer control via capability)

**Timer as kernel object** (Zircon model, refined): capability-held type backed
by Space (D32). Created, armed, cancelled, destroyed via typed kernel
operations.

- Cancel = destroy the Pulsar cap (D11 — clean authority)
- Space accounting bounds timer creation (D32 — self-limiting)
- Timer delegation via capability transfer or clone
- Full D4 consistency

The stateless operation model's two structural gaps — cancel authority and
resource accounting — both have ad-hoc solutions, but the capability-held object
model solves them with existing mechanisms (D4, D11, D32). No new patterns
needed.

### Overflow handling

When a repeating Pulsar fires and the delivery field is full:

- The kernel stops re-arming. The next fire would produce another undeliverable
  message.
- When the Observer receives from the field (freeing a slot), the kernel re-arms
  with the drift-corrected next deadline.
- The message includes an overrun count — how many periods elapsed while the
  field was full.
- This parallels D22 interrupt masking: the interrupt stays masked until ack.
  The Pulsar stays unarmed until a slot opens.

### Clock access

Use cases for reading the current time: timestamps, duration measurement,
absolute deadline computation, cross-Observer event ordering, profiling.

ARM64 provides CNTVCT_EL0, readable at EL0 if the kernel enables
CNTKCTL_EL1.EL0VCTEN. All production ARM64 kernels expose this (Linux vDSO,
Zircon, QNX).

Side-channel concern (Spectre-era): high-resolution timers enable cache timing
attacks and covert channels. Under A3, some workloads (multi-tenant isolation)
care.

Resolution: per-Observer control. The kernel flips CNTKCTL_EL1.EL0VCTEN on
context switch based on a per-Observer flag. Cost: one MSR instruction per
context switch (~1 cycle). Observers with clock-access authority get direct
counter reads; others use a typed kernel operation (syscall). The authority
mechanism (which capability controls the flag) is one level down.

## Decision

**D44 — Pulsar: capability-held timer object with kernel-managed delivery**

Pulsar is the fifth kernel object type (Space, Time, Observer, Field, Pulsar). A
Pulsar is a timer that the kernel programs on behalf of an Observer and delivers
as a field message when it fires.

**Creation:** A Pulsar is created from Space (D32 type conversion) with a
delivery field cap, badge, deadline, and period. The Pulsar is armed on
creation. Period = 0 means one-shot; period > 0 means repeating.

**Delivery:** When the deadline arrives, the kernel enqueues a message to the
designated field with the designated badge (D13/D17 kernel-as-sender pattern).
The message includes the actual fire time in a data word.

**Repeating:** For period > 0, the kernel re-arms automatically with
drift-compensated deadlines (`next = scheduled + period`, not
`actual + period`). The Observer does not participate in re-arm — the kernel
manages drift. This is the default path for periodic workloads.

**Overflow:** When the delivery field is full, the kernel stops re-arming. On
the next receive that frees a slot, the kernel re-arms and includes an overrun
count in the delivered message. Parallels D22 mask-on-delivery.

**Cancel/lifecycle:** Cancel = destroy the Pulsar via D11. The Pulsar holds an
internal reference to the delivery field (contributes to field refcount — field
stays alive while Pulsars reference it). D33 cascade: Observer destroy closes
Pulsar caps; if last reference, Pulsar is destroyed and pending fires cancelled.

**Scheduler input:** The Pulsar's period is an explicit input to D42 EDF
admission (T in the C/T test). Setting or destroying a Pulsar has scheduling
side effects — the kernel re-evaluates admission on the core.

**Manual control:** Observers needing adaptive timing (variable period, drift
compensation, tick skipping) use one-shot Pulsars (period = 0) in a loop. Each
set controls one deadline. This is the precision path — the default
kernel-managed re-arm serves fixed-period workloads.

**Clock access:** Per-Observer controlled. The kernel manages
CNTKCTL_EL1.EL0VCTEN per context switch. Observers with clock-access authority
read CNTVCT_EL0 directly (~1 cycle). Others use a typed kernel operation. The
authority mechanism is one level down.

## What this does NOT settle

- **Pulsar rights mask.** Arm, cancel/destroy, clone, inspect — exact rights set
  is one level down (D39 pattern).
- **Creation API shape.** One-call (create-armed) vs. two-step (create inert,
  then arm). D35 pattern available but Pulsars are simpler than Observers.
- **Full message content layout.** Actual fire time and overrun count in data
  words — specific word assignment is one level down.
- **Duration vs. absolute deadline.** API form for specifying the deadline.
  Library can convert; kernel can accept either. One level down.
- **Clock access authority mechanism.** Which capability controls the
  per-Observer CNTVCT flag. One level down.
- **Default clock access policy.** Whether Observers get counter access by
  default or must be granted it.
- **Badge-filtered receive.** Noted as independently interesting but deferred to
  its own exploration (D13/D18 interactions).

## Axioms

**A2 (ARM64):** Load-bearing. ARM64 generic timer is the hardware mechanism.
Per-core, one-shot, counter at fixed frequency. CNTVCT_EL0 readable at EL0 under
kernel control. The timer hardware shapes what the kernel can offer.

**A3 (generic):** Load-bearing. Timer interface must serve all workloads:
periodic RT, one-shot delayed events, timeouts, watchdogs. No single timer
pattern hardcoded. Per-Observer clock access control serves both
precision-sensitive and security-sensitive workloads.

**A4 (purely reactive):** Load-bearing. Timer fire is a hardware exception
(timer interrupt). The kernel responds by delivering the message and re-arming —
exception-triggered processing using persistent state, the same pattern as all
kernel behavior. No background timer management thread.

**A5 (leaf node):** Load-bearing. The kernel absorbs timer multiplexing, drift
compensation, overflow handling, and scheduling integration. The Observer
provides a deadline and period; the kernel handles everything else.
Kernel-managed re-arm is the A5-consistent default; one-shot Pulsars provide the
escape hatch when Observer-managed timing is needed.

**A1 (Rust):** Not load-bearing for the design choice, but Pulsar as a
capability-held object maps naturally to Rust ownership (cap = owned reference,
destroy = drop, refcount = Rc-like lifecycle).

## Archive convergence

The archive has no userspace timer concept. Timers are purely kernel-internal
(preemption). The archive's scheduling model (journal 008) places the period
directly on the Context as a timing declaration (d, dt, p, pt). D42 already
documented this divergence: "the archive did not have the
timer-as-kernel-service pattern (timers were discussed separately in the
archive), so it needed explicit timing declarations."

The divergence is explained by the same factor that explains D42's divergence
from the archive: the current design derives scheduler information (T) from the
Pulsar's period rather than requiring the Observer to declare it as a scheduling
property. The archive's six scheduling parameters collapse in the current design
because the kernel already knows period (from Pulsar) and compute budget (from
Time).

No convergence or divergence on Pulsar as an object type — the archive did not
reach this question.

## Status

Settled. Revisit if:

- D42 is revised (changes the scheduling profile or the role of timer period in
  EDF admission)
- D13 is revised (changes the delivery mechanism)
- D32 is revised (changes the type conversion model for object creation)
- D22 is revised (changes the interrupt delivery pattern that Pulsar parallels)
- A downstream derivation reveals that Pulsar's kernel-managed re-arm cannot
  serve a structurally required timer pattern (would reopen ack-to-re-arm as
  first-class mechanism)
