# 013 — IPC model: queued endpoints with direct-switch fast path

**Date:** 2026-04-18 **Starting point:** D7 settled the split interaction model
but deferred the IPC mechanism (sync/async/hybrid). D12 added a constraint:
fault traffic (kernel-as-sender, reply-resume, potentially high-frequency) is a
first-class IPC workload. The IPC model is the single decision that most shapes
a microkernel's syscall surface (syscall-landscape §10).

---

## The question

What is the IPC mechanism — synchronous rendezvous, asynchronous queued, or
hybrid? This is the top-level fork. Message format, channel structure, blocking
behavior, multiplexing, and the specific syscall surface all follow.

---

## Sync-only is foreclosed

A3 (generic) requires supporting both synchronous patterns (RPC, client-server)
and asynchronous patterns (event-driven, interrupts, pipeline/dataflow). D12
requires the kernel to deposit fault messages without blocking — the kernel
cannot wait for a pager to accept. These are independent paths. Sync-only with
zero async capability is foreclosed.

Sync-primary with a minimal async mechanism (seL4's bitmap notifications) is NOT
foreclosed — it's one of the candidate models.

---

## The design space

Three candidate models survived the derivation:

### Option A: Sync rendezvous + bitmap notifications (seL4 model)

Two IPC primitives. Endpoints are stateless rendezvous — no queue, no buffer.
Separate notification objects: word-sized coalescing bitmaps.

Strengths: absolute minimum kernel complexity, no queue memory, formally proven
(416 cycles ARM64), coalescing notifications solve interrupt delivery.

Weaknesses: sender always blocks on endpoint send. Fan-out patterns (send to A,
send to B, continue) break — require NBSend (lossy, drops message if no
receiver) or multiple Observer threads. Every "fire and forget" communication
must use the notification primitive (bitmap, no payload).

### Option B: Queued endpoints with direct-switch fast path (archive model)

One IPC primitive. Bounded queue. Sender deposits and continues (non-blocking
unless queue full). When receiver is already waiting, direct process switch
bypasses the queue (rendezvous-speed fast path, ~400 cycles).

Strengths: one mechanism for both sync (send + block-on-reply) and async (send +
continue). Non-blocking send is first-class. Fan-out is natural. "All
information delivery is one mechanism" (archive/journal/002) — faults,
interrupts, IPC through the same path.

Weaknesses: kernel manages queue memory (charged to creator's Space budget per
D8 pattern). Novel design — no exact deployment precedent. Coalescing gap:
messages don't coalesce, which is a problem for interrupt delivery on shared
endpoints.

### Option C: Sync rendezvous + queued notifications (QNX-like)

Two IPC primitives. Sync rendezvous for primary IPC. Queued notifications (more
capable than bitmap — carry per-event data, like QNX pulses).

Strengths: sync performance for RPC, richer async than Option A.

Weaknesses: still blocks on normal send (same fan-out limitation). Two
mechanisms that must interact for multiplexing. "Middle ground" may end up
neither the simplicity of A nor the uniformity of B.

---

## The A4 + A3 structural pressure

The same pattern from D12 appears here: A4 + A3 together create structural
pressure.

A4 (purely reactive) means no kernel-side event loop or message broker. IPC
dispatch must happen within syscall handlers. Both sync and queued models
satisfy this.

A3 (generic) means workloads vary. RPC workloads want request-reply.
Event-driven workloads want fire-and-forget. A3 pushes toward a mechanism that
handles both patterns — either two specialized primitives (Option A) or one
general primitive (Option B).

D7 notes the split model "couples naturally with async" — async IPC introduces
behavioral divergence (queuing, blocking, multiplexing) that aligns with a
separate mechanism family. This is a structural alignment signal, not a
requirement.

---

## The coalescing tension (Phase 5 finding)

During evaluation, a three-way tension was identified between Option B,
capacity- 1 overwrite-oldest (for coalescing), and shared endpoints:

If an endpoint is shared among multiple sources (different badges) with
capacity-1 and overwrite-oldest semantics, source A's message can overwrite
source B's unprocessed message. This is cross-source data loss.

Possible resolutions explored:

- **Per-badge slots:** each badge gets its own slot. Fixes cross-source loss but
  changes the endpoint from a FIFO queue to a per-badge map — fundamentally
  different data structure. Per-source loss remains (which IS what coalescing
  means, by definition).
- **Embedded bitmap:** FIFO queue + coalescing word inside one object. Two
  mechanisms in one object's coat.
- **One endpoint per source:** avoids shared-endpoint coalescing entirely. Each
  source gets its own endpoint. More endpoints, more capabilities, more Space.
- **Accept it:** overwrite mode is opt-in per endpoint. Creator chooses it
  knowingly for latest-wins use cases. Cross-source clobbering is a consequence
  the creator accepts by sharing an overwrite endpoint.

None of these eliminate the tension cleanly — they move it around. The
resolution is deferred as a downstream question of the endpoint shape
exploration. The tension is documented so it is not rediscovered.

---

## The decision

**Queued endpoints with direct-switch fast path (Option B).** The primary IPC
mechanism is bounded queued endpoints. Messages accumulate. Sender deposits and
continues (non-blocking). When the receiver is already waiting, direct process
switch occurs at rendezvous speed. All information delivery — IPC, faults (D12),
interrupts, system signals — uses the same mechanism.

The archive's "strictly dominates" argument: queued endpoints with direct-switch
fast path achieve rendezvous speed for the same-core, receiver-waiting case AND
provide async fallback when the receiver is not waiting. The sync-only model
cannot handle the async case at all; the queued model handles both.

**Costs accepted:**

- Kernel manages queue memory. Charged to creator's Space budget (D8 pattern).
  Memory per queued message ~48 bytes (register-sized). Fixed capacity set at
  creation.
- Novel design — no exact deployed precedent. Closest comparisons: Zircon
  channels (async, queued, but bidirectional and no direct-switch as first-class
  feature), Mach ports (async, queued, but complex rights model).
- Coalescing gap for shared endpoints remains open (documented above).

---

## Why tentative

This is accepted as tentative — not settled — because the downstream cluster is
tightly coupled: overflow policy, coalescing, notification mechanism, endpoint
shape, multi-endpoint wait, and message format all interact. Settling the IPC
model in isolation risks discovering in a downstream exploration that queued
endpoints don't compose with one of these concerns.

The tentative acceptance enables exploring those downstream questions with a
concrete IPC model to reason against. If the downstream work reveals a
structural problem, Option B moves — the revisit triggers name the specific
conditions.

---

## Archive convergence

The archive arrived at the same model through independent reasoning:

- journal/002: "all information delivery is one mechanism" — faults, interrupts,
  IPC are messages with different metadata.
- journal/007: Two independent paths converge — Time transfer patterns require
  non-blocking send; message unification requires async capability. Queued
  "strictly dominates" rendezvous.
- journal/009: Bounded queue + waiters, many:many, FIFO, send/receive rights.
- journal/010: 4-slot (32-byte) messages, badge, type, cap_mask.
- journal/011: Reply cap in message for IPC; resume() syscall for faults.
- journal/013: 10-syscall surface, "Wormhole" naming.

The current chain arrives at the same position from D12 (fault traffic as IPC
workload) plus the same A3/A4 structural pressure the archive identified.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Rust's type system accommodates both sync and
async IPC implementations. A1 becomes relevant one level down when implementing
the queue data structure and message types.

**A2 (ARM64)** provides the register file (message in registers for fast path)
and SVC mechanism but does not choose between models. seL4 (sync) and Zircon
(async) both run on ARM64.

---

## What remains open

Downstream cluster — all to be explored with Option B as the assumed IPC model:

- **Overflow policy.** What happens when the queue is full? Error to sender
  (archive), overwrite-oldest (ring buffer), fault the sender? Per-endpoint
  policy at creation? This determines whether the coalescing gap is solvable
  within the queued model.
- **Coalescing / notification mechanism.** Can capacity-1 + overwrite-oldest
  serve as a coalescing notification? The three-way tension (Option B +
  capacity-1 overwrite + shared endpoints) is documented above. May require a
  separate lightweight primitive.
- **Multi-endpoint wait.** How does an Observer wait on multiple endpoints
  simultaneously? Port aggregator (Zircon)? Multi-receive syscall? Notification
  binding to Observer? This is the "select/epoll" problem for the queued model.
- **Endpoint shape.** Unidirectional vs. bidirectional. Many-to-many vs.
  constrained topology. The archive chose unidirectional, many-to-many, topology
  via capabilities.
- **Message format.** Size, slot count, capability transfer encoding, badge
  placement. The archive chose 4 slots (32 bytes), cap_mask bitmask, badge from
  capability.
- **Reply routing.** Reply cap in message (archive for IPC), resume() syscall
  (archive for faults). Does D7's split model require the distinction?
- **D11 add-ons now explorable.** Badges (per-capability, attached by kernel to
  messages) are a natural fit for queued endpoints. Endpoint rotation (destroy +
  recreate for mass invalidation) requires endpoint lifecycle to be defined.
- **D12 fault delivery specifics.** How the kernel enqueues fault messages. How
  the pager receives them. How resume works.
- **Queue capacity policy.** Fixed at creation? Growable? Maximum? Minimum?
- **IPC fast-path conditions.** When does direct process switch occur? Only when
  receiver is waiting? What about priority? (seL4 fastpath requires no
  higher-priority runnable thread.)

---

## Rejected alternatives (summary)

| Alternative                        | Rejected because                                                                                       |
| ---------------------------------- | ------------------------------------------------------------------------------------------------------ |
| Sync-only (no async at all)        | A3 + D12 foreclose (event-driven workloads, kernel-as-sender)                                          |
| Kernel message broker              | A4 forecloses (no kernel thread)                                                                       |
| IPC timeouts (intermediate values) | Proven useless — only blocking/non-blocking used (§9.1)                                                |
| Option A (sync + bitmap)           | Not foreclosed but not chosen — fan-out limitation, sender-always-blocks. Archive explicitly rejected. |
| Option C (sync + queued notif)     | Not foreclosed but not chosen — still has sender-blocks limitation, two mechanisms                     |
