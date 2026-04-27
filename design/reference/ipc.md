# IPC Reference

Inter-process communication in this kernel uses **Fields** -- bounded message
queues mediated by capabilities. All information delivery between Observers
passes through Fields: peer-to-peer IPC, fault notifications, hardware
interrupts, timer fires, and badge-closure signals. One mechanism serves every
communication pattern (D13).

This document covers the IPC model from the userspace perspective: how to send
and receive messages, how the Call/Reply protocol works for RPC, how to write
fast IPC paths, and what errors to expect.

---

## Overview

An Observer communicates by holding capabilities to Fields. A **send
capability** authorizes depositing messages into a Field. A **receive
capability** authorizes picking messages up from one. Fields are unidirectional
and many-to-many (D15): any number of senders can hold send capabilities to the
same Field, and the Field's owner decides who can receive.

Messages do not block the sender. A Send deposits the message and returns
immediately. If the Field's queue is full, the sender receives an error -- the
kernel never silently drops userspace messages. When a receiver is already
waiting on the Field, the kernel can deliver the message directly without
touching the queue, achieving near-rendezvous speed on the common path.

The IPC surface has five operations, invoked as `SVC #1` through `SVC #5` (D48).
The kernel dispatches them from the SVC immediate in ESR_EL1 before reading any
general-purpose register (D47).

---

## The Five IPC Operations

### Send (SVC #1)

Non-blocking deposit of a message into a Field.

**When to use:** Fire-and-forget messages. Event notifications. Any case where
the sender does not need a reply or does not want to block.

**Behavior:** The kernel resolves the target Field from the handle in x5, checks
that the sender holds the SEND right, injects the sender's badge from the
capability entry (D17), and deposits the message. If a receiver is already
blocked on Receive on that Field, the message bypasses the queue and is
delivered directly to the receiver's register save area (D13). The sender always
continues running -- Send never blocks the caller.

**On success:** The carry flag is clear. The sender resumes at the instruction
after the SVC.

**On error:** The carry flag is set and x0 contains the error code.

### Receive (SVC #2)

Blocking wait for the next message on a Field.

**When to use:** Waiting for incoming requests, notifications, or events. The
fundamental "listen" operation.

**Behavior:** The kernel resolves the target Field from x5, checks the RECEIVE
right. If the queue has messages, the front message is dequeued (FIFO) and
delivered to the receiver's registers. If the queue is empty, the Observer
transitions to the Blocked state (D39) and is linked into the Field's waiters
list. The scheduler picks the next runnable Observer on this core.

When a message later arrives (via Send, Call, or kernel-as-sender), the blocked
receiver is unblocked and the message is delivered to its registers. From the
receiver's perspective, the SVC returns with the message data -- the blocking is
transparent.

**On success:** x0-x3 contain the four data words, x4 the label, x5 the badge,
x6 the user capability handle (or `u64::MAX` if no capability was transferred),
and x7 the reply capability handle (or `u64::MAX` if no reply capability).

### Call (SVC #3)

Compound send-then-block: sends a request and blocks waiting for a reply. This
is the client half of the RPC pattern.

**When to use:** Remote procedure calls. Any request-response interaction where
the caller needs the reply before continuing.

**Behavior:** The kernel sends the caller's message to the target Field (same
mechanics as Send), creates a **send-once reply capability** pointing to the
caller's pre-allocated reply Field (capability table slot 1, D43), includes it
in the message, and blocks the caller on its reply Field. The server receives
the request with the reply capability in x7.

The caller's x7 register supplies a **reply badge** (D65). The kernel embeds
this badge in the send-once capability entry. When the reply arrives at the
caller's reply Field, it carries this badge, allowing the caller to identify
which outstanding RPC the reply corresponds to.

Call is eligible for the direct-switch fast path (D50). When conditions are met,
the kernel switches directly from caller to server without queue insertion,
achieving approximately 400-cycle round trips.

**On success (after reply arrives):** x0-x3 contain the reply data words, x4 the
reply label, x5 the reply badge, x6/x7 any transferred capabilities.

**On error (before blocking):** Carry flag set, x0 contains the error code. The
caller is not blocked.

### ReplyRecv (SVC #4)

Atomic reply-then-receive: sends a reply to a client and waits for the next
request. This is the server half of the RPC pattern.

**When to use:** Server loops. After processing a request, the server replies
and immediately waits for the next one. The atomicity prevents a scheduling gap
between reply delivery and next-request pickup (D16).

**Behavior:** The kernel resolves two Fields:

- **Reply Field** from x5: the send-once reply capability the server received
  with the previous request. The kernel sends the reply message here. The
  send-once cap is consumed after use (D51).
- **Receive Field** from x7: the server's service Field where new requests
  arrive. The kernel performs a Receive on this Field.

The reply phase and receive phase execute atomically. If a client is waiting on
the reply Field, the reply is delivered directly to the client's registers and
the client is unblocked. If the receive Field has queued messages, the next
request is delivered to the server. If the receive Field is empty, the server
blocks.

ReplyRecv is eligible for the direct-switch fast path on the receive side. The
reply side always uses the slow path (the reply capability is consumed, which
involves capability table mutation).

**On success:** Same register layout as Receive: x0-x3 data words, x4 label, x5
badge, x6 user capability, x7 reply capability from the new request.

### Yield (SVC #5)

Voluntary CPU relinquishment. No message, no Field, no capability resolution.

**When to use:** Cooperative multitasking. Compute-bound workloads that want to
yield their time slice without performing IPC. Universally supported across
kernel designs (D48).

**Behavior:** The yielding Observer remains Runnable (D39). The scheduler
rotates it to the tail of the run queue and picks the next Observer for this
core. If no other Observer is runnable, the yielder resumes immediately.

---

## Message Format

Every IPC message has the same fixed-size format (D28). The sender provides some
fields; the kernel fills in others during transit.

### Register layout on send (x0-x7)

| Register | Field           | Description                                   |
| -------- | --------------- | --------------------------------------------- |
| x0       | data[0]         | First untyped data word                       |
| x1       | data[1]         | Second untyped data word                      |
| x2       | data[2]         | Third untyped data word                       |
| x3       | data[3]         | Fourth untyped data word                      |
| x4       | label           | Application-defined message label             |
| x5       | target handle   | Capability handle to the target Field         |
| x6       | user cap handle | Capability to transfer (u64::MAX = none)      |
| x7       | reply info      | Reply badge (Call) or recv handle (ReplyRecv) |

### Register layout on receive (x0-x7)

| Register | Field          | Description                              |
| -------- | -------------- | ---------------------------------------- |
| x0       | data[0]        | First untyped data word                  |
| x1       | data[1]        | Second untyped data word                 |
| x2       | data[2]        | Third untyped data word                  |
| x3       | data[3]        | Fourth untyped data word                 |
| x4       | label          | Application-defined message label        |
| x5       | badge          | Sender's badge, injected by kernel (D17) |
| x6       | user cap slot  | Handle to received cap (u64::MAX = none) |
| x7       | reply cap slot | Handle to reply cap (u64::MAX = none)    |

### Field descriptions

**Data words (x0-x3).** Four arbitrary 64-bit values. The kernel does not
interpret them. Protocols define their meaning through the label.

**Label (x4).** A 64-bit value the kernel passes through without inspection.
Protocols use labels to distinguish message types. The kernel reserves the range
`0xFFFF_FFFF_FFFF_0000` and above for kernel-generated messages (fault
notifications, timer fires, badge closures, interrupt signals).

**Badge (x5 on receive).** The sender's identity as assigned by the minter of
the sender's capability. The kernel reads the badge from the sending
capability's table entry and writes it into the message. The sender cannot
choose or forge the badge -- it is fixed at mint time and immutable (D17). The
receiver sees which "client identity" sent the message without the sender being
able to impersonate another client.

**User capability (x6).** An optional capability transferred with the message.
Set to `u64::MAX` (the `CAP_ABSENT` sentinel, D49) when no capability is being
transferred. When present, the capability is **moved** from the sender's table
to the receiver's table -- the sender loses the capability regardless of
delivery outcome (D96). Move semantics are required because copy semantics would
violate the Time over-allocation invariant (D30).

**Reply capability (x7 on receive).** Present in messages delivered through
Call. A send-once capability pointing to the caller's reply Field. The server
uses this to send its reply. Absent (`u64::MAX`) for plain Send messages and
kernel-generated messages.

---

## Badge Delivery

When an Observer sends a message through a capability with badge value B, the
kernel writes B into the message's badge field (x5 on the receiver side). The
receiver always sees the badge that the minter assigned to the sender's
capability -- not any value the sender chose.

Badges enable receivers to identify senders without the sender being able to
forge identity. A server that mints capabilities to clients assigns each client
a unique badge. When a request arrives, the server reads x5 to know which client
sent it.

The badge is a full 64-bit value (D58), forced by the ABI: it occupies x5, a
64-bit register. The minter provides the badge value when creating a capability
through the Mint typed operation (D17). The badge is immutable after creation --
the only way to change it is to mint a new capability with a different value.

### Badge lifecycle tracking

Fields can optionally track per-badge reference counts (D17). When badge
tracking is enabled and the last capability with badge B targeting that Field is
closed, the kernel delivers a badge-closure notification to the Field: a message
with label `LABEL_CLOSURE` (`0xFFFF_FFFF_FFFF_0002`) and the closed badge in x5.
This is the kernel's answer to "how does a server detect client disconnection."

Reply Fields always have badge tracking enabled (D73).

---

## Call/Reply Protocol

The Call/ReplyRecv pair implements RPC. The protocol has three participants: the
client Observer, the server Observer, and the server's service Field.

### The full cycle

1. **Client calls.** The client issues `SVC #3` (Call) targeting the server's
   service Field. The kernel:
   - Sends the client's message to the service Field.
   - Reads the client's reply Field capability from cap table slot 1
     (`SLOT_REPLY_FIELD`, D43).
   - Creates a send-once capability entry pointing to the client's reply Field,
     with the client's reply badge (from x7) and generation-checked against the
     reply Field's current generation (D67).
   - Includes this send-once reply cap in the message (delivered in x7 on the
     server side).
   - Blocks the client on its reply Field.

2. **Server receives.** The server, blocked on Receive (or ReplyRecv) on the
   service Field, wakes up. It reads x0-x3 for the request data, x4 for the
   label, x5 for the client's badge, and x7 for the reply capability handle.

3. **Server replies.** The server issues `SVC #4` (ReplyRecv) with x5 set to the
   reply capability handle. The kernel:
   - Sends the reply to the client's reply Field via the send-once cap.
   - Consumes the send-once cap (D51) -- the server can no longer send to this
     specific reply capability.
   - If the client is blocked on its reply Field, delivers the reply directly to
     the client's registers and unblocks the client.
   - Receives the next request from the service Field (the Recv half of
     ReplyRecv).

4. **Client unblocks.** The client wakes with the reply in x0-x7. From the
   client's perspective, the Call returned with the reply.

### Reply capability lifecycle

The reply capability is ephemeral. The kernel creates it during Call and the
server consumes it during Reply. The underlying reply Field persists -- it is
pre-allocated at Observer creation and reused across RPCs. Only the send-once
capability is transient.

The reply capability carries the client-supplied reply badge (D65). If the
client has multiple outstanding RPCs to different servers, each Call supplies a
different reply badge. When the reply arrives, the client reads x5 to identify
which RPC completed.

---

## Send-Once Semantics

A send-once capability is consumed after one successful Send operation (D51).
The `send_once` flag is a boolean on the capability entry, not a rights bit --
it cannot be cleared through rights attenuation.

Reply capabilities are the primary use of send-once semantics, but send-once is
a general-purpose mechanism. Other applications include one-shot event
notifications, single-use authorization tokens, and edge-triggered interrupt
acknowledgments.

### Consumption protocol

After a successful Send or Call through a send-once capability, the kernel frees
the sender's capability table slot. The slot returns to the freelist and may be
reused for future capability installations.

### The AlreadyConsumed error

If a send-once capability has already been consumed (the slot has been freed or
reused), a subsequent Send attempt through the same handle will fail. The handle
decode finds either an empty slot (`InvalidCap`) or a reused slot with a
different slot tag (`InvalidCap` via the D11 ABA defense). If the slot was
reused for a new capability and the slot tag happens to match but the generation
does not, the error is `StaleCap` (D67). In all cases, the message is not sent
and the sender receives an error.

The `AlreadyConsumed` error code (`SyscallError::AlreadyConsumed`) is returned
specifically when the capability entry's `send_once` flag is set and the
consumption has already occurred. In practice, the handle becomes invalid after
the first use -- the specific error variant distinguishes "you already used this
one-shot cap" from "this handle was never valid."

---

## Fast Path

The kernel has an optimized code path for the common RPC case: a Call or
ReplyRecv where the server is already waiting and no capability transfer is
involved (D50). This path achieves approximately 400 cycles on ARM64 by avoiding
queue insertion, Message struct construction for data words, and extra register
save/restore work.

### Conditions for the fast path

All six conditions must hold simultaneously:

1. **The operation is Call or ReplyRecv.** Send, Receive, and Yield are not
   eligible. The sender must voluntarily block for direct switch to make sense
   -- Send is fire-and-forget (the sender continues) (D50 condition 1).

2. **Same core.** The SVC handler runs on the issuing core (hardware
   constraint). The receiver must be on this core. Cross-core IPC always goes
   through the queue (D1, D50 condition 2).

3. **A receiver is waiting on the target Field.** An Observer must be blocked on
   Receive on the destination Field. If the queue has messages but no waiter, or
   no messages and no waiter, the fast path is not taken (D50 condition 3).

4. **No user capability in the message.** The user cap register (x6) must be
   `u64::MAX` (CAP_ABSENT). Capability transfer requires rights validation,
   destination table allocation, and move-semantics bookkeeping -- too expensive
   for the fast path's cycle budget. Time donation via Call always takes the
   slow path (D50 condition 4).

5. **The scheduler approves the switch.** The per-core scheduler's
   `should_switch_to` callback returns true. The scheduler is the authority on
   which Observer runs next (D50 condition 5, D2). This prevents the fast path
   from bypassing scheduling policy.

6. **Field routing resolved.** For split Fields, badge-range routing evaluation
   determines the actual destination Field. Unsplit Fields skip this check at
   near-zero cost (null routing table pointer, D54).

### What happens on the fast path

When all conditions hold, the kernel performs a direct context switch from
caller to receiver:

- Data words (x0-x3) conceptually pass through in registers -- the kernel writes
  them to the receiver's register save area from the sender's saved state (D74,
  D87).
- The kernel writes only metadata (label, badge, cap-absent sentinel, reply cap
  handle) to the receiver's x4-x7 registers.
- No Message struct is constructed for data words.
- No queue insertion or removal occurs.
- The caller is blocked on its reply Field; the receiver resumes immediately.

### Writing fast IPC

To stay on the fast path:

- **Use Call/ReplyRecv for RPC**, not Send + separate Receive.
- **Avoid transferring capabilities in hot-path messages.** Move capability
  transfers to setup or teardown phases. Data-only messages hit the fast path;
  capability-carrying messages take the slow path (approximately 600-800
  cycles).
- **Keep client and server on the same core** when possible. Cross-core IPC
  always enqueues. The scheduler's placement decisions (D56) determine core
  assignment -- the Observer's scheduling profile (D42) influences but does not
  guarantee same-core placement.
- **Avoid contention on the service Field.** The fast path pops the first
  waiting receiver. If the server is not waiting (it is busy processing a
  previous request), the message enqueues regardless.

### When the fast path is denied

If any condition fails, the kernel falls back to the slow path. The slow path
can still bypass the queue when a receiver is waiting (the receiver is woken and
the message is delivered to its register save area), but it goes through the
general code path that handles all cases uniformly. The slow path costs
approximately 600-800 cycles -- still fast by microkernel standards, just not
the optimal path.

If the scheduler denies the direct switch (condition 5 fails), the kernel
constructs a Message from the sender's saved registers, delivers it to the
receiver through the slow path, enqueues the receiver in the scheduler, and lets
the scheduler pick the next Observer to run (D96).

---

## Field Routing

A Field's receive side can be subdivided through **field split** (D45). Split
installs a badge-range routing rule on the source Field: messages with badges in
a specified range `[low, high]` are delivered to a destination Field instead of
the source's queue.

### How routing works

When a message arrives at a split Field, the kernel evaluates the routing table
before checking the queue or waiters list. Routing uses the message's badge
(injected from the sender's capability) as the key.

- The routing table is a sorted array of non-overlapping closed ranges (D71).
  Lookup is binary search, O(log N) where N is the number of splits on this
  Field.
- If a range matches, the message is deposited on the destination Field.
- If no range matches, the message stays on the source Field.
- Unsplit Fields have no routing table (null pointer). The check is a single
  branch -- near-zero cost on the hot path (D54).

### Senders are oblivious

Field split is a receive-side operation. Senders hold capabilities to the source
Field and are unaware of splits. Their capabilities, badges, and behavior do not
change. This follows from the capability model: the sender designates the Field
object; what happens inside the Field's receive topology is not the sender's
concern (D4).

### Multi-field wait patterns

An Observer waiting on a receive Field sees messages from all senders whose
badges do not route elsewhere. If the Observer wants to receive from multiple
sources on one Field, the sources' capabilities should target the same Field
with badges in the non-routed range.

Alternatively, split-to-existing (D45) can route traffic from multiple source
Fields into a single destination Field, allowing an Observer to multiplex
several sources through one receive point. The Observer distinguishes sources by
badge.

### Fallback on destroy

When a destination Field is destroyed, the routing rule on the source is
removed. Messages that were routing to the destroyed Field fall back to the
source's queue (D45). This is automatic crash recovery: if a driver crashes and
its Field is destroyed, the traffic returns to the parent's queue.

---

## Capability Transfer During IPC

A message can carry one capability in the user cap slot (x6). The transfer uses
**move semantics**: the capability is removed from the sender's table and
installed in the receiver's table (D96).

### Transfer protocol

1. The sender places a capability handle in x6. The kernel resolves the handle
   in the sender's table, validates it, and extracts the entry as a
   `TransferredCap` -- an intermediate representation for a capability between
   tables.

2. The sender's table slot is freed. The sender loses the capability regardless
   of what happens next.

3. When the message is delivered (either immediately to a waiting receiver or
   later from the queue), the kernel allocates a slot in the receiver's table
   and installs the capability. The receiver sees the new handle in x6.

4. If the receiver's table is full, the kernel cannot install the capability. A
   cap-table-full fault is delivered to the receiver's fault handler (D40), and
   the handler provides Space for table growth.

### Why move, not copy

Move semantics are forced by the Time over-allocation invariant (D30). If a Time
capability were copied during transfer, both sender and receiver would hold
references to the same Time object. The kernel's per-Observer compute aggregates
would double-count the Time's compute units, causing the scheduler to allocate
more CPU time than physically exists. Move is the only correct transfer mode for
Time, and the IPC path applies move uniformly to all object types (D96).

### Reply capability transfer

The reply capability created during Call is also a capability transfer, but it
is kernel-initiated rather than user-initiated. The kernel reads the caller's
reply Field capability from slot 1, constructs a send-once entry, and installs
it in the receiver's table. The receiver sees the reply cap handle in x7.

---

## Error Conditions

IPC errors are signaled through the ARM64 carry flag in SPSR_EL1 (D49). On
error, the carry flag is set and x0 contains the error code. On success, the
carry flag is clear.

### QueueFull

**Trigger:** Send or Call to a Field whose queue is at capacity and no receiver
is waiting.

**What it means:** The Field cannot accept the message. This is the kernel's
error-to-sender policy (D18) -- the sender must handle overflow, not the kernel.
Backpressure, retry, or dropping the message is the sender's decision.

**Note:** Even when the queue is full, if a receiver is waiting, the message
bypasses the queue and is delivered directly. QueueFull only occurs when both
the queue is full and no receiver is available.

### InvalidCap

**Trigger:** The handle in x5 (or x7 for ReplyRecv's receive Field) does not
resolve to a valid capability. Causes include:

- Handle index is out of bounds.
- The table slot is empty (no capability installed).
- The slot tag does not match (D11 ABA defense -- the slot was reused since the
  handle was obtained).
- The capability does not target a Field (wrong object type).

**What it means:** The caller passed a bad handle. Re-acquire the capability
through IPC or check that the handle has not been invalidated.

### StaleCap

**Trigger:** The capability's stored generation does not match the target
Field's current generation (D67). The Field still exists, but the caller's
capability was explicitly revoked.

**What it means:** The Field owner called revoke, invalidating all outstanding
capabilities that stored the previous generation. The caller must obtain a new
capability from the Field owner.

### NoRight

**Trigger:** The resolved capability does not have the required right for the
operation. Send and Call require the SEND right. Receive requires the RECEIVE
right. ReplyRecv requires SEND on the reply Field (x5) and RECEIVE on the
receive Field (x7).

**What it means:** The capability was attenuated to exclude the needed right.
The caller needs a capability with broader rights, obtained through Mint from a
holder with sufficient rights.

### AlreadyConsumed

**Trigger:** Attempting to Send through a send-once capability that has already
been used. In practice, the handle has been freed after the first use, so this
typically manifests as InvalidCap (empty or reused slot). The AlreadyConsumed
error code exists for the case where the consumption is detected before the slot
is recycled.

**What it means:** The one-shot reply was already sent. This is a protocol error
-- the server attempted to reply twice to the same client request.

---

## Common Patterns

### Client-server RPC via Call/ReplyRecv

The canonical microkernel communication pattern. The client issues Call; the
server runs a ReplyRecv loop.

**Client:**

```rust
loop {
    // Prepare request in x0-x4
    // x5 = server field handle
    // x6 = u64::MAX (no cap transfer)
    // x7 = reply badge for this RPC
    SVC #3  // Call
    // On return: x0-x4 = reply data, x5 = reply badge
}
```

**Server:**

```rust
// Initial receive to get the first request
SVC #2  // Receive on service field (x5 = service field handle)
loop {
    // Process request from x0-x4
    // x5 (badge) identifies the client
    // x7 has the reply cap handle
    //
    // Prepare reply in x0-x4
    // x5 = reply cap handle (from previous x7)
    // x7 = service field handle (for receiving next request)
    SVC #4  // ReplyRecv
    // On return: x0-x4 = next request, x5 = next client badge
}
```

This pattern hits the fast path when: the message carries no capability, the
server is waiting when the client calls, both are on the same core, and the
scheduler approves the switch.

### Notification via Send

Fire-and-forget events. The sender deposits a message and continues without
waiting for a response.

```rust
// x0-x3 = event data
// x4 = event label (protocol-defined)
// x5 = target field handle
// x6 = u64::MAX (no cap)
SVC #1  // Send
// Check carry flag for QueueFull
```

If the receiver is blocked on the target Field, it is woken immediately.
Otherwise, the message waits in the queue.

### Resource delegation via capability transfer

An Observer transfers a capability to another Observer by including it in a
message.

```rust
// x0-x3 = protocol data
// x4 = label indicating "here is a resource"
// x5 = target field handle
// x6 = handle to the capability being transferred
SVC #1  // Send (or SVC #3 for Call)
// The capability in x6 is now gone from our table
```

The receiver finds the transferred capability handle in x6 of the received
message. The capability has been moved -- the sender no longer has it.

### Interrupt delivery

Hardware interrupts arrive as kernel-generated messages on a driver's Field. The
kernel constructs the message with label `LABEL_DEVICE_IRQ`
(`0xFFFF_FFFF_FFFF_0007`), the IRQ's badge in x5 (identifying which interrupt),
and the raw INTID in data[0] (D22, D81). The driver receives these through
normal Receive or ReplyRecv on its Field -- no special IPC operation needed.

### Timer events

Pulsar timer fires arrive as messages with label `LABEL_TIMER_FIRE`
(`0xFFFF_FFFF_FFFF_0001`). data[0] carries the fire time in raw counter ticks,
data[1] the overrun count (D63). The Observer receives them through normal
Receive on the Pulsar's delivery Field.

### Detecting client disconnection

On a Field with badge tracking enabled (D17), the kernel delivers a
badge-closure message (label `LABEL_CLOSURE`, `0xFFFF_FFFF_FFFF_0002`) when the
last send capability with a specific badge is closed. The badge of the
disconnected client is in x5. The server reads the badge and cleans up any
per-client state.

---

## Derivation References

| Topic                      | Derivations      |
| -------------------------- | ---------------- |
| IPC model (queued fields)  | D13, journal 013 |
| Field shape                | D15              |
| Reply mechanism            | D16, journal 016 |
| Badge semantics            | D17, journal 017 |
| Overflow policy            | D18              |
| Message format             | D28              |
| Field split                | D45, journal 045 |
| Syscall ABI                | D47, D48, D49    |
| Fast-path conditions       | D50, journal 050 |
| Send-once flag             | D51              |
| Rights                     | D52              |
| Routing table              | D54              |
| Badge range condition      | D71              |
| Register pass-through      | D74              |
| Message ownership          | D78, journal 078 |
| Scheduling decision matrix | D79              |
| Fast-path mechanics        | D87, journal 087 |
| Capability transfer        | D96, journal 096 |
