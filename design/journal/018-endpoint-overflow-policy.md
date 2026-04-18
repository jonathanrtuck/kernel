# 018 — Endpoint overflow policy: error-to-sender with deferred fault delivery

**Date:** 2026-04-18 **Starting point:** D13 is tentative pending the downstream
cluster (overflow, coalescing, multi-wait, message format). Three revisit
triggers name this cluster. D15, D16, and D17 are settled derivations resting on
D13. This entry settles overflow policy — the first trigger — and dissolves the
coalescing tension that motivated the second.

---

## The question

What happens when a send to a queued endpoint finds the queue at capacity? And
does the answer resolve the D13 coalescing tension (cross-source data loss on
shared endpoints with overwrite semantics) within the single-mechanism model?

---

## Decomposition: what workloads need, and what's irreducible

The derivation started by tracing what different workloads actually need when
the queue is full, then checking which of those needs can only be served by the
kernel.

| Workload                        | Need at overflow        | Irreducible?                        |
| ------------------------------- | ----------------------- | ----------------------------------- |
| RPC server (many:1)             | Sender learns "full"    | Yes — only kernel knows queue state |
| Event/interrupt (many:1)        | Latest value per source | No — shared memory + signaling      |
| Fault delivery (kernel→pager)   | Always delivered        | Yes — no alternative under D13 + A4 |
| Badge-closure (kernel→receiver) | Notification delivered  | No — lazy cleanup on next access    |
| Pipe (1:1)                      | Sender learns "full"    | Yes — same as RPC                   |
| Status broadcast                | Latest per source       | No — shared memory + signaling      |

Two needs are irreducible: error-to-sender (the kernel must tell the sender
something) and fault delivery guarantee (the kernel-as-sender must succeed).

### Coalescing is reducible to shared memory + signaling

The established microkernel pattern (landscape §3.2: "every production
microkernel converges on hybrid") is: writer puts the latest value in shared
memory, sends a signal through a capacity-1 endpoint. If the signal fails (error
— already pending), no loss: the receiver reads shared memory on next drain and
gets the latest value. Data coalesces in shared memory; the endpoint is a
wake-up.

D9 (memory objects) and D10 (address spaces) provide the building blocks. The
setup is per-channel (a few syscalls), not per-message. For kernel→userspace
(interrupts), the standard approach (landscape §5.1) is interrupt masking: mask
on delivery, unmask on driver acknowledgment. If the endpoint signal fails, the
interrupt stays masked — the interrupt controller holds the pending state.

This means: per-badge coalescing (D13's resolution #1) and overwrite-oldest are
kernel-level conveniences for something achievable with existing primitives. The
kernel does not need a coalescing overflow mode.

### The D13 coalescing tension dissolves

The tension was: queued endpoints + capacity-1 overwrite + shared endpoints =
cross-source data loss. With error-to-sender as the only overflow behavior,
there is no overwrite — so there is no cross-source data loss. The tension was
predicated on overwrite semantics. Without overwrite, it doesn't arise. D13's
revisit trigger #1 ("coalescing gap cannot be solved without a full second
primitive") does not fire: coalescing lives in shared memory, not in the
endpoint, and requires no second primitive.

---

## Error-to-sender: the overflow policy

When a send finds the queue at capacity, the kernel returns an error to the
sender. The sender decides the policy: retry, discard, buffer locally, escalate.
Kernel provides mechanism; userspace provides policy.

### Why error is the only mode

A3 (generic) originally pushed toward per-endpoint policy at creation (different
workloads, different needs). But the decomposition above showed that the only
workloads needing something other than error (latest-wins, coalescing) are
reducible to shared memory + signaling. Error-to-sender is the irreducible
kernel mechanism. No per-endpoint policy flag is needed — just creation-time
capacity.

Applying "push complexity to the leaves" fractally: the overflow mode is a leaf
inside the endpoint implementation. Making it configurable would add a branch to
the send path and a policy field to the endpoint object. Since the only
non-error use cases are achievable through composition of existing primitives,
the branch is accidental complexity — it doesn't serve a need that can't be
served another way.

### Blocking-on-full is not adopted

D13's motivation was to avoid the sender-always-blocks limitation that rejected
sync-only: "Fan-out patterns (send to A, send to B, continue) break."
Blocking-on-full under load re-introduces this limitation. Additionally, A4
forecloses blocking for the kernel-as-sender (D12 faults), creating an asymmetry
between kernel and userspace sends. Error is consistent for both.

### No overwrite, no cross-source data loss

Without overwrite semantics, the D13 coalescing tension is structurally
unreachable. Source A's message cannot overwrite source B's message. Both fail
equally when the queue is full (both get errors). No silent data loss.

---

## Deferred fault delivery for the kernel-as-sender

D12 + D13 + A4 create a hard constraint: when an Observer faults, the kernel
must enqueue a fault message to the pager endpoint. The kernel cannot block
(A4), cannot be faulted, and cannot meaningfully "handle" an error (A4 means no
retry — the exception handler returns and the fault is lost).

Error-to-sender works for userspace senders (they can retry, degrade, escalate).
It does not work for the kernel-as-sender on fault messages: the faulting
Observer is suspended, waiting for a resume that requires the fault to be
delivered.

### The mechanism

When the kernel tries to enqueue a fault message and the queue is full, the
kernel marks the faulting Observer as "fault pending delivery" and links it into
a per-endpoint pending list. When the receiver's next receive() frees a slot,
the kernel checks the pending list and delivers the oldest deferred fault into
the freed slot before returning to the receiver.

**No new memory allocation.** The pending list is an intrusive linked list
threaded through existing Observer objects. Each Observer is already allocated
(from someone's Space budget). The linkage field can share space with other
wait-state linkage (blocked-on-receive, blocked-on-reply) — only one is active
at a time based on Observer state. Per-endpoint cost: one list head pointer (8
bytes). Per-Observer cost: one linkage field (shared with other wait states, 0
net bytes).

**A4-compatible.** The pending list check occurs during receive(), which is a
syscall (exception entry). No background work, no polling, no kernel thread.

**D1-compatible.** Receive is cold-path. Checking a list head pointer is one
branch.

**D13-compatible.** The fault is delivered through the endpoint queue. The
pending list is kernel-internal state, not a second visible mechanism. From
userspace, fault messages appear in the queue like any other message — just
delayed.

**The Observer doesn't notice.** It was suspended on fault. Whether the fault
message reaches the pager immediately or after a brief delay (until the queue
drains a slot) is invisible to the faulting Observer. It's suspended either way,
waiting for the pager to process and resume.

### Badge-closure notifications are not deferred

D17 badge-closure notifications are informational, not structural. If the queue
is full when a closure notification would be enqueued, the kernel drops it. The
receiver discovers staleness lazily on next interaction (send to dead handle →
error). This is acceptable: the receiver opted into tracking for proactive
cleanup, but proactive cleanup failing gracefully is not a correctness issue.

### Endpoint destroy with pending faults

If a pager's endpoint is destroyed while Observers have deferred faults in the
pending list, those Observers need cleanup. This folds into the pager
unavailability protocol (already an open question). Endpoint destroy must walk
the pending list — the cleanup action (kill the pending Observers, fault-chain
to a parent handler, etc.) is determined by the unavailability protocol.

---

## Archive convergence

The archive arrived at error-to-sender through the same reasoning:

- journal/007: "When full, send returns an error. The sender decides the policy
  — retry, drop, back off. Kernel provides mechanism, userspace provides
  policy."
- journal/009: Same statement. Listed as settled.

The archive did not address:

- The D13 coalescing tension (discovered in the current chain's D13
  exploration).
- Kernel-as-sender delivery guarantee (deferred fault delivery is novel to the
  current chain).
- The shared-memory + signaling argument for dissolving the coalescing need (the
  archive didn't have the coalescing question).

Convergence on overflow policy. Divergence on scope: the current chain goes
further by dissolving the coalescing tension and adding the kernel-as-sender
delivery mechanism.

---

## The decision

**Overflow policy: error-to-sender.** When a send finds the queue at capacity,
the kernel returns an error. No per-endpoint policy modes. No overwrite. No
coalescing at the endpoint level. Coalescing workloads use shared memory +
signaling (D9/D10 + capacity-1 endpoints).

**Kernel-as-sender fault delivery: deferred via pending list.** When the kernel
cannot enqueue a fault message (queue full), it links the faulting Observer into
a per-endpoint pending list. The next receive() that frees a slot delivers the
deferred fault. Intrusive linked list through Observer objects — zero additional
memory allocation.

**Badge-closure notification on full queue: dropped.** Receiver discovers
staleness lazily. Not a correctness issue.

**D13 coalescing tension: dissolved.** No overwrite means no cross-source data
loss. The tension was predicated on overwrite semantics that this derivation
does not adopt. Coalescing lives in shared memory, not in the endpoint
mechanism.

**D13 revisit trigger #1 ("coalescing gap cannot be solved without a full second
primitive"): does not fire.** Coalescing is achieved through composition of
existing primitives (shared memory + endpoint signaling), not through a second
IPC primitive.

---

## Costs accepted

- **No kernel-level coalescing.** Workloads needing latest-wins must use the
  shared-memory + signaling pattern. This is the established microkernel
  architecture (landscape §3.2), not a novel burden, but it is more setup than a
  hypothetical per-badge coalescing mode would require.
- **Deferred fault delivery adds receive-path work.** One branch (check list
  head) on every receive. Cold-path, minimal cost.
- **Badge-closure can be silently lost.** Receivers using tracked endpoints for
  proactive client cleanup must tolerate occasional staleness. Correctness
  doesn't depend on it — only cleanup timeliness.
- **Interrupt delivery on full endpoint.** The interrupt model (open question)
  must account for error-on-full. Standard approach: mask on delivery, unmask on
  acknowledgment. If delivery fails, interrupt stays masked until driver catches
  up. The interrupt controller holds the pending state.

---

## Axioms not load-bearing here

**A1 (Rust):** not load-bearing. Error return types (Result) are natural in
Rust, but the overflow policy derives from A3/A4/D12, not from language
features.

**A2 (ARM64):** provides the interrupt masking mechanism (GIC) that enables
deferred interrupt delivery, but the overflow policy itself derives from A3/A4
and the shared-memory argument. A2 becomes load-bearing one level down when
implementing the interrupt delivery path.

---

## What remains open

- **Interrupt model.** Must account for error-on-full: mask-on-delivery,
  unmask-on-acknowledgment. The interrupt controller provides the deferred
  delivery mechanism for interrupts (analogous to the pending list for faults).
- **Pager unavailability protocol.** K5's pending list adds a new trigger:
  endpoint destroy with pending faults. The cleanup action depends on the
  unavailability protocol.
- **Multi-endpoint wait.** D13 revisit trigger #3 remains open. Not addressed by
  this derivation.
- **Observer minimum schema.** The pending-list linkage field is a new schema
  entry (shared with other wait-state linkage — no net size increase, but must
  be accounted for).
- **Badge-closure × overflow interaction.** Resolved: dropped on full queue. No
  further interaction.
- **Per-badge tracking × coalescing interaction.** Dissolved: coalescing is not
  an endpoint mechanism. D17's per-badge map serves tracking only.

---

## Rejected alternatives (summary)

| Alternative                   | Rejected because                                                                                                                                                                                           |
| ----------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Overwrite-oldest              | Creates the D13 coalescing tension (cross-source data loss on shared endpoints). The use cases served by overwrite are achievable through shared memory + signaling.                                       |
| Per-badge coalescing          | Reducible to shared memory + signaling. Would change the endpoint from FIFO to per-badge map, losing cross-source ordering. The structural opportunity (D17 per-badge map) exists but is not needed.       |
| Per-endpoint policy modes     | The only non-error use cases are reducible. A policy flag adds accidental complexity (configurable branch on send path, policy field on endpoint object) for no irreducible benefit.                       |
| Blocking-on-full              | Re-introduces the sender-blocks limitation D13 was designed to avoid. A4 forecloses it for kernel-as-sender, creating asymmetry.                                                                           |
| Faulting the sender on full   | Sender can't fix the problem (queue belongs to endpoint creator, not sender). Breaks the D8 fault pattern's assumption that the faulted entity can resolve the fault.                                      |
| Reserved kernel slots (K1)    | Workable but wastes userspace capacity. R must be sized at creation for unknown peak — over-provision wastes, under-provision risks fault loss. Deferred delivery (K5) avoids both.                        |
| Kernel-overwrite (K2)         | May overwrite a previous fault from another Observer, leaving that Observer stuck forever. Kernel overwriting its own prior deliveries is the structural failure mode.                                     |
| Kill on delivery failure (K3) | Harsh — a transient overload (pager slow to drain) permanently kills Observers that could recover. Pushes "size your queue right" as a hard contract. Inconsistent with "make the right way the easy way." |
