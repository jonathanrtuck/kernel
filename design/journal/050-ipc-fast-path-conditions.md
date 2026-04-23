# 050 — IPC fast-path conditions

**Date:** 2026-04-23 **Starting point:** D13 listed "IPC fast-path conditions"
as an open downstream question at the time queued fields with direct-switch fast
path was settled. The question was subsequently referenced in D28 (message
format), D47 (syscall ABI), and D49 (syscall encoding) without being derived.
D47 settled the register pass-through optimization (x0–x3 stay in physical
registers on direct switch); D49 settled the cap-present sentinel (u64::MAX).
Both deferred the conditions under which direct switch actually occurs.

---

## The question

Under what conditions does the kernel take the direct-switch fast path on IPC —
what must be true about the receiver, the scheduler state, and the message for
the kernel to bypass the queue and switch directly?

---

## Constraints from settled decisions

Before exploring choices, the settled design mechanically determines several
conditions:

**Same core.** D1 (per-core hot path, no cross-core shared state) + O3
(exceptions are taken on the causing core) + O2 (cross-core coordination
requires IPIs). The SVC handler runs on the issuing core. Direct switch to a
receiver on a different core is structurally impossible — cross-core IPC goes
through enqueue + IPI. This is a hardware constraint, not a design choice.

**Receiver blocked on Receive.** D13's stated trigger: "when the receiver is
already waiting." The receiver Observer must be in the blocked state (D43's
five-state enum), specifically blocked on a Receive operation on the target
field (after D45 routing evaluation).

**All messages fit in registers.** D28 (fixed-size: 4 data words + 1 cap slot) +
D47 (exactly fills 8 ARM64 registers). Every message fits in registers by
construction. seL4 must check message length on every IPC; this kernel cannot
fail that check. One entire class of disqualifying conditions disappears.

**Field routing evaluated first.** D45 requires badge-range routing evaluation
before the kernel knows which field the message lands on. Unsplit fields: null
routing table → skip (~0 cost). Split fields: ~10–20 cycles (2.5–5% of ~400
budget). This is a fixed cost within the fast path, not a qualifying condition —
all messages go through routing regardless.

**Cap validation and badge injection are fixed costs.** D4/D8 (O(1)
array-indexed cap lookup + rights check + ABA tag) and D17 (badge injection from
sending capability). These always happen. Part of the ~400-cycle budget, not
eligibility gates.

**No scheduling inheritance.** D43 explicitly: "The Call/Reply fast path does no
profile manipulation." The kernel does not adjust the receiver's scheduling
profile during direct switch.

**Run queues stay consistent.** The Benno scheduling lesson
(research/syscall-landscape.md §9.4): lazy scheduling was tried in original L4
and abandoned — "always keep run queues consistent, accept the small fast-path
cost for predictable behavior." The fast path must atomically update scheduler
state.

---

## Three independent axes of choice

The remaining questions decompose into three independent axes. Independence
means any combination is internally consistent.

### Axis 1: Scheduling check

The sender is about to block (Call or ReplyRecv). The receiver just became
runnable. Should the kernel switch directly, or consult the scheduler first?

**Option A — No check (always switch):** ~0 cycles. The sender voluntarily
blocked; always switch to the receiver. Risk: a high-responsiveness Observer
(D42) that should preempt gets delayed. seL4 classic (pre-MCS) used this for the
common case but added a priority check as defense against pathological
scenarios.

**Option B — Run-queue-empty check:** ~5 cycles (one branch on a per-core
counter). Only switch if no other runnable Observer exists. Safe for any D2
algorithm but overly conservative — a server with 10 queued clients would
_never_ take the fast path because the queue is never empty, even though direct
switch would be correct.

**Option C — Scheduler callback:** ~20–50 cycles (function call +
algorithm-specific logic). The per-core scheduler answers "should the receiver
run next?" using whatever policy it implements. Correct for any D2 algorithm.
Cost is 5–12% of the ~400-cycle budget.

**Option D — Max-responsiveness tracker:** ~5 cycles (one comparison). Maintain
a per-core "max responsiveness among runnable Observers" counter. D42-specific
analog of seL4's priority check. Approximation — ignores throughput and
precision dimensions.

### Axis 2: Cap transfer eligibility

D28's message has a single user cap slot. Should the fast path handle it?

**Option A — 0-cap only:** One field check (~1–2 cycles) gates on "no cap
present" (x6 = u64::MAX per D49). Data words pass through in x0–x3 (D47). No
cap-table mutation on receiver side. D37 Time donation on Call() always falls to
slow path.

**Option B — 0-or-1 cap:** +~30–60 cycles for cap transfer (rights validation,
destination slot allocation, ABA tag, move semantics). For Time caps: +~10
cycles for cached aggregate updates. Larger I-cache footprint (tension with
landscape §3.4: IPC path should occupy ~2–3% of L1). Error handling for
cap-table-full adds conditional complexity.

### Axis 3: Operation scope

Which IPC operations get the fast path?

**Option A — Call + ReplyRecv only:** The sender always blocks — scheduling is
trivial. Clean semantics. Send enqueues (or delivers to receiver's save area)
and returns to sender.

**Option B — Call + ReplyRecv + Send:** Send also triggers direct switch when
receiver is waiting. L4 tradition. But Send is specified as non-blocking (D13:
"deposits and continues"); direct switch effectively preempts the sender.
Changes Send's observable return latency.

---

## The decision

**{Axis 1: C, Axis 2: A, Axis 3: A} — scheduler callback, 0-cap gate, Call +
ReplyRecv scope.**

Six conditions, all must hold for the fast path:

1. **Operation is Call (SVC #3) or ReplyRecv (SVC #4).** The sender voluntarily
   blocks. Send, Receive, and Yield do not qualify.
2. **Same core.** Structural — O3 guarantees the SVC handler runs on the issuing
   core; the receiver must be on this core.
3. **Target field has a waiting receiver.** An Observer is blocked on Receive on
   the target field (post-D45 routing resolution).
4. **No user cap in message.** x6 = u64::MAX (D49 sentinel). Zero-cap is the
   fast path; cap transfer goes through the general IPC path.
5. **Scheduler approves the switch.** The per-core scheduler's
   `should_switch_to(receiver)` callback returns true. The scheduler is the
   authority on "who runs next" regardless of code path.
6. **Field routing resolved.** D45 routing evaluation completes. Unsplit fields:
   null table skip. Split fields: ~10–20 cycles within budget.

If any condition fails → slow path. The slow path can still direct-switch
through the general IPC code — it just takes more cycles (~600–800 vs. ~400)
because it handles all cases uniformly.

### Why scheduler callback (not no-check or max-R tracker)

The philosophy is explicit: "Isolate uncertain decisions behind interfaces. Code
against the interface, never the implementation." The scheduling check is an
uncertain decision — D2 settles that per-core schedulers may run different
algorithms. What "better candidate" means is defined by the scheduler, which is
a leaf node.

Option A (no check) bypasses D42's authority. A high-responsiveness Observer
should preempt; always switching to the IPC receiver could delay it. This is not
a corner case — it is the purpose of D42's responsiveness dimension.

Option D (max-R tracker) is a D42-specific heuristic. If D42 is revised, the
heuristic breaks. The callback survives any scheduler revision.

The ~20–50 cycle cost is real but bounded. And this is not extra work: A4
(purely reactive) means the kernel must choose which Observer to resume after
every SVC. The scheduler already makes this decision. The callback unifies the
fast-path scheduling check with the slow-path scheduling decision — the same
interface, called at different points. The scheduler is always the authority.

### Why 0-cap only (not 0-or-1)

D28 line 1359: "cheaply distinguishable (one field check), gating the fast
path." The spec's own language already assumed this answer.

Cap transfer is structurally more expensive than data-word pass-through: rights
validation + destination table allocation + ABA tag management + (for Time)
cached aggregate updates. Adding this to the fast path grows the I-cache
footprint and adds conditional error handling (destination table full is a fault
per D40).

The D37 tension: Time donation on Call() always takes the slow path. D37 chose
cap-graph visibility over seL4 MCS's zero-fastpath-overhead kernel-internal
approach. This is where that tradeoff materializes. The cost is concrete: Time
donation Call() goes through the general path (~600–800 cycles instead of ~400).
D37's "direct-switch fast path eliminates transit for the common case" (spec
line 2001) refers to queue bypass — the slow path also bypasses the queue when
the receiver is waiting; it just uses the general code path.

### Why Call + ReplyRecv only (not + Send)

The philosophy: "understand the true shape of the problem." The fast path exists
for the RPC loop. Call + ReplyRecv is the RPC loop. Send is a different shape:
fire-and-forget, sender continues. Including Send on the fast path would mean
the sender goes to the run queue instead of continuing — effectively preempted
despite D13's "deposits and continues" specification.

If Send-to-waiting-receiver needs optimization, the right approach is a
different optimization: deliver the message directly to the receiver's save area
(avoid queue allocation) but return to sender. That is a "fast enqueue"
optimization, not a "direct switch" fast path. Different shape, different
mechanism.

---

## Axioms and observations

**A1 (Rust):** Not load-bearing for the conditions themselves. The fast path
will use unsafe + inline assembly for register manipulation (D47's x0–x3
invariant), but the language does not constrain which conditions qualify.

**A2 (ARM64):** Background — provides the register file, SVC mechanism, and
ESR_EL1 dispatch that make the fast path possible. The ~400-cycle budget is an
ARM64 measurement.

**A3 (generic):** The conditions must handle all workload patterns. The
scheduler callback (1C) satisfies this — any D2 algorithm can answer the "should
switch?" question for its workload type.

**A4 (purely reactive):** Load-bearing. The kernel runs only in exception
handlers. The scheduler decision is part of every exception return. The callback
is not extra work — it's the same decision, asked inline.

**A5 (kernel is leaf node):** Load-bearing. The scheduler is the authority on
scheduling decisions. The fast path must not bypass this authority — the
callback preserves it. A5 also supports the 0-cap gate: the fast path absorbs
the complexity of the common case (data-only RPC) and delegates the uncommon
case (cap transfer) to the general path.

**O3 (exceptions on causing core):** Mechanical. Establishes same-core as
structural.

---

## What this settles

The complete set of conditions under which the kernel takes the direct-switch
fast path on IPC. Specifically: operation scope (Call + ReplyRecv), message
condition (no user cap), receiver state (blocked on Receive on target field),
scheduling condition (per-core scheduler callback), and the structural givens
(same core, field routing resolved).

Also settles that the slow path can still direct-switch — the fast path is an
optimized code path, not the only path that bypasses the queue.

## What this does NOT settle

- **Scheduler callback interface.** The function signature, constant-time
  requirement, and interaction with specific D2 algorithm implementations. One
  level down — depends on scheduler internals not yet derived.
- **Send-to-waiting-receiver optimization.** Whether a "fast enqueue" (deliver
  to receiver's save area, return to sender, no queue allocation) exists as a
  separate optimization from the direct-switch fast path.
- **Interrupt masking during fast path.** Whether interrupts are masked for the
  ~400-cycle fast-path window. Trades worst-case interrupt latency for
  scheduling-check consistency. Implementation concern within the ~400-cycle
  budget.

---

## Status

**Settled.**

Revisit if D13 is revised (different IPC model changes the direct-switch
concept), if D42 is revised (different scheduling model changes the callback's
semantics), if D2 is revised (unified scheduler removes the need for an
algorithm-agnostic callback), if D37 is revised (different Time donation
mechanism changes the cap-transfer tradeoff), or if the scheduler callback
proves too expensive in practice (consistently >50 cycles, consuming >12% of the
fast-path budget).
