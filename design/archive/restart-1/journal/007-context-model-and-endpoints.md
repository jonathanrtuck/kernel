# Context Model and Endpoint Shape — 2026-04-11

Seventh exploration. Began defining the concrete shapes of the kernel's internal
structures, starting with the Context model (the central blackboard) and the
Endpoint object type.

## Starting point

Journal 006 established the capability interface: three object types (Memory,
Time, Endpoint), per-Context handle tables, and the sync/async unification via
Time transfer. The next step is defining the concrete shapes of these
structures.

Four items to define, roughly in dependency order:

1. Context model — the shared data structure everything reads/writes
2. Object shapes — what Time, Memory, and Endpoint look like concretely
3. Message shape — register layout for all information delivery
4. Syscalls/ABI — emerges from the above

The kernel | userspace interface comes last. The kernel's internals must work
before anything sits on top. The syscall surface exposes operations on internal
structures — it falls out rather than being designed top-down.

## The scheduler picks a Time, not a Context

Working through how the scheduler interacts with Time capabilities produced an
inversion of the usual mental model.

Traditionally: the scheduler maintains a run queue of Contexts, picks one, loads
it. With Time capabilities: the scheduler maintains a set of Time allocations on
its core, picks the best one, then looks up the owning Context to load.

```text
per-core run queue:  [T1, T2, T3]     ← Time allocations on this core
scheduler picks:     T2                ← best according to algorithm
lookup:              T2 → Context B    ← one O(1) dereference
load:                Context B's register state, TTBR
```

The scheduler is a Time allocator, not a Context picker. This aligns with
journal 004's framing: "Time is a flow. Cores produce computation continuously.
The scheduler directs it."

Consequence: scheduling properties live IN the Time object, not in the Context
model. Whatever the scheduler needs to make decisions (bandwidth, priority,
deadline — all TBD) is a property of the Time allocation. When Time is
transferred during IPC, the scheduling properties travel with it — the passive
server pattern from journal 006 inherits the client's scheduling properties
automatically.

The `scheduling_hints` field from journal 005 disappears from the Context model.
The scheduler resolves the Context's time handle and reads the Time object
directly.

## Endpoint shape: queued, not rendezvous

Explored two models for Endpoints: rendezvous (stateless, pairs one sender with
one receiver) and queued (messages accumulate, sender doesn't block).

### Rendezvous (seL4, L4, EROS)

The endpoint is stateless — just a wait queue of blocked threads. Message goes
directly from sender's registers to receiver's registers. If no receiver is
waiting, sender blocks.

Strengths: zero allocation, direct process switch (~400 cycles on ARM64),
minimal kernel state. Weaknesses: both parties must be ready simultaneously,
can't fire-and-forget, every send is potentially blocking. Every production
rendezvous system (seL4, L4, QNX) ended up adding a second async mechanism
(notifications, pulses) because rendezvous alone can't handle async signals.

### Queued (Zircon channels, Mach ports)

The endpoint has a bounded queue. Sender posts and continues. Receiver reads
when ready.

Strengths: decoupled, natural async, multi-sender friendly, one mechanism for
both patterns. Weaknesses: kernel manages queue memory, needs capacity limits,
slightly more complex.

### The journal 006 async pattern requires non-blocking send

The sync/async unification from journal 006 depends on the ability to
send-and-continue. The async pattern: A subdivides Time, transfers a portion to
S, continues running. The fan-out pattern: A sends to S1, sends to S2,
continues.

Rendezvous breaks this — if send blocks until the receiver picks up the message,
A can't continue. The mechanism forces synchrony regardless of A's intent.

Queued endpoints support it — A posts messages and continues immediately.

Two independent reasoning chains converge on the same answer: Time transfer
patterns (journal 006) and the "all information delivery is one mechanism"
principle (journal 002) both push toward queued endpoints. Rendezvous-based
systems always end up with two mechanisms (sync + async). Queued endpoints can
be the single mechanism.

### The fast path still exists

A queued endpoint can still do direct process switch when the receiver is
already waiting:

```text
Receiver waiting?
  YES → direct switch, message in registers, ~400 cycles (≈ rendezvous speed)
  NO  → enqueue message, sender continues, ~1000-1500 cycles
```

The performance penalty only applies to the case rendezvous can't handle at all
(receiver not waiting). The queued model strictly dominates: same fast path when
the receiver is ready, plus a fallback for when it isn't.

### Queue overflow prevention

Endpoints have a fixed capacity set at creation time. When full, send returns an
error. The sender decides the policy — retry, drop, back off. Kernel provides
mechanism, userspace provides policy.

Memory cost is small. Messages are register-sized (~48 bytes). A 64-deep queue
is ~3KB per endpoint. Overflow is a fairness/DoS concern, not a memory
exhaustion concern.

### Impact on "all information delivery is one mechanism"

Journal 002 established that faults, interrupts, IPC, and syscall returns are
all messages. Queued endpoints make this concrete: all message delivery goes
through the same queueing mechanism. Faults and interrupts are messages the
kernel enqueues on an endpoint. IPC is messages a Context enqueues on an
endpoint. One mechanism, one code path.

## Context model sketch (updated)

Incorporating journal 005 (SMP), journal 006 (capabilities), and this session's
findings:

```text
Context model entry:
  register_state        saved/restored at context switch
  ttbr                  address space root, written by Space manager
  state                 runnable | blocked(endpoint) | dead
  current_core          core ID | in_flight
  fault_handler         direct Endpoint reference (kernel-internal, not a handle)
  time_handle           handle index into this Context's capability table
  pending_message       source, type, payload (register-sized) — OR empty
  capability_table      pointer to per-Context handle table
```

Key design choices in this sketch:

- **fault_handler is a direct reference**, not a handle. The kernel uses it (to
  deliver faults), not the Context. Set at creation, resolved from the creator's
  handle. The Context cannot attenuate or close its own fault handler — it can't
  escape supervision.

- **time_handle is a handle**, not a direct reference. The Context must be able
  to transfer, subdivide, and manipulate its own Time through normal capability
  operations. The scheduler resolves it (one O(1) lookup) to read the Time
  object.

- **state includes what the Context is blocked on** (which Endpoint). Needed for
  zombie detection: if that Endpoint's refcount hits zero, the kernel faults the
  Context.

- **scheduling_hints removed.** Scheduling properties live in the Time object,
  not the Context model. The scheduler reads the Time object directly.

- **pending_message is one slot.** Queued endpoints hold the queue, not the
  Context. This field is the message currently being delivered — written by the
  reactor just before the Context is resumed, read by the Context after resume.
  Not a queue.

## Status

**Tentatively accepted (this session):**

- The scheduler picks a Time allocation, not a Context. Scheduling properties
  live in the Time object.
- Endpoints are queued, not rendezvous. Fixed capacity, error on full.
- Direct process switch is the fast path when receiver is waiting (≈ rendezvous
  speed). Queue is the fallback.
- One mechanism for all message delivery (IPC, faults, interrupts).
- fault_handler is a direct Endpoint reference (kernel-internal).
- time_handle is a capability handle (Context-manipulable).

**Open questions:**

- **Time object shape.** Resolved in journal 008: Time = fraction (% of core).
  Timing declarations (duration, period/latency) live on the Context, not the
  Time object.
- **Memory object shape.** What does a Memory object look like? Byte-addressed
  (spec.md), but internal structure TBD.
- **Endpoint capacity.** Fixed at creation — but what's a reasonable default?
  Configurable? Attenuatable?
- **Message shape.** Register layout: source, type, payload. How many registers?
  How do capability transfers travel with messages?
- **pending_message vs. Endpoint queue interaction.** When does a message move
  from the Endpoint queue to the Context's pending_message slot? At pick() time?
  At resume time?
- **Blocked Context state.** Can a Context wait on multiple Endpoints
  simultaneously (select/poll), or only one at a time?
