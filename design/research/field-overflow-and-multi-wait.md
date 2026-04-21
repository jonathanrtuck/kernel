# Field Overflow Policy and Multi-Source Wait

## Question

When a queued IPC field is full, what happens? And how do systems allow a single
thread to wait on multiple fields simultaneously? These two questions are
entangled: overflow policy determines what "full" means and what happens to the
sender; multi-wait policy determines whether a separate mechanism is required to
aggregate signals from multiple sources.

This document also covers the adjacent coalescing question: whether a
bounded-queue field with capacity-1 and overwrite-oldest semantics can serve as
a coalescing notification primitive without a second kernel object.

---

## Survey

### seL4 — No message queue; thread queue with unbounded depth

seL4 endpoints contain no message buffer. The kernel queues **threads**, not
messages. When a sender arrives and no receiver is waiting, the sending thread
is suspended and added to the endpoint's sender queue. Message payload lives in
the sender's register file (or IPC buffer for larger messages) and is
transferred only at the moment of rendezvous.

**Overflow policy:** "Queue full" does not occur in the buffer sense — the
endpoint can hold as many blocked senders as there are threads. There is no
configurable depth limit on the thread queue. The resource bound is the number
of threads that can exist, which is bounded by the system's Untyped memory.

**NBSend (non-blocking send):** `seL4_NBSend` is a polling send. If a receiver
is already blocked waiting, the message transfers immediately. If no receiver is
present, the send is silently discarded — **no error is returned**. This is
intentional: returning an error indicating "no receiver" would create a
back-channel, leaking information about the receiver's scheduling state. The
sender gets no indication that the message was dropped.

**Multi-source wait — badges + single endpoint:** seL4's answer to fan-in is
badging. The server holds one endpoint cap. It mints N copies with distinct
badge values — one per client. All clients send to the same endpoint. The kernel
attaches the badge from the _capability_ (not the message) to the received
message header. The server's `seL4_Recv` returns any waiting sender; the badge
identifies who. This is not multi-endpoint wait — it is many-to-one fan-in
through a single endpoint.

**Multi-source wait — bound notifications:** A notification object can be
_bound_ to a thread. When the thread calls `seL4_Recv` on an endpoint, the
kernel will also return if a notification arrives on the bound notification. The
thread receives either an IPC message or a notification word (the notification's
coalesced bitmap), but not both simultaneously. This is the only built-in
mechanism for waiting on two distinct kernel objects in one call.

**Coalescing — notification object:** seL4 provides a separate `Notification`
object (distinct from `Endpoint`) specifically for coalescing. Each
`seL4_Signal` on a notification ORs the sender's badge into a word-sized bitmap.
`seL4_Wait` returns the full bitmap and zeros it atomically. This is explicit
coalescing: multiple signals from source A collapse to a single bit in A's badge
position. The notification object has no queue — only one word. Per-source
coalescing is guaranteed by construction.

**Sources:** seL4 Reference Manual 14.0.0 §4–5; seL4 ipc.tex source; "How to
(and how not to) use seL4 IPC," Heiser blog.

---

### Mach / XNU — Bounded message queue; sender blocks on full

Mach ports contain an ordered message queue. Each port has a configurable
**queue limit** (qlimit), defaulting to 8 messages. The queue holds complete
messages, not thread references.

**Overflow policy — block sender:** If a message is sent to a port whose queue
is full, the sending thread blocks until space becomes available. This is the
default behavior with no flags. The kernel maintains a list of blocked senders;
when space opens, one is unblocked (with no starvation guarantee violated — FIFO
among blocked senders is maintained).

**MACH_SEND_TIMEOUT:** The sender can specify a timeout. If space does not
become available within the timeout, the call returns `MACH_SEND_TIMED_OUT` and
the message is not enqueued.

**MACH_SEND_NOTIFY:** The sender registers a send-possible notification port.
The send returns immediately (with an error), and when space becomes available
the kernel delivers a notification message to the registered port. This is
Mach's non-blocking-but-notified pattern.

**Send-once rights bypass queue limit:** A message addressed to a send-once
right ignores the queue limit and is delivered unconditionally. This is used for
reply messages: the reply must always deliver, regardless of queue state, or the
caller is deadlocked.

**Multi-source wait — port sets:** Mach's `mach_msg` receive can name a _port
set_ rather than a single port. A port set is a kernel object that aggregates
receive rights. A thread blocked on a port set dequeues from whichever member
port has a pending message. `msgh_local_port` in the received message header
identifies which port delivered the message. Ports can be added and removed from
a set while a receive is in progress. This is Mach's equivalent of `select()`.

**Coalescing:** No coalescing at the message queue level. Messages are FIFO. If
coalescing semantics are needed (e.g., "only care about whether event occurred,
not how many times"), it is implemented in userspace.

**Measured:** Default queue depth is 8. The qlimit can be changed with
`mach_port_set_attributes`. The blocked-sender approach means that queue
pressure propagates as scheduling pressure (the sender blocks) rather than data
loss.

**Sources:** GNU Mach Reference Manual; Apple Kernel Programming Guide — Mach
ports; XNU mach_msg(2) man page (MIT Darwin mirror).

---

### Zircon (Fuchsia) — Unbounded queue; policy exception on overflow

Zircon channels have no explicit, user-visible queue depth limit. Messages
accumulate in kernel memory until consumed or until a system-level limit is
exceeded.

**Overflow policy — policy exception:** When an IPC object's internal buffer
limits are exceeded, the kernel raises a _policy exception_ in the calling
thread. The specific numeric limits are intentionally not exposed as constants
to prevent code from targeting them. The design rationale (from the Zircon IPC
limits documentation) states: "sending IPC messages at a reasonable rate in a
healthy system always succeeds." When that assumption breaks, a fatal exception
is preferred over returning error codes that developers may mishandle. The
exception is expected to propagate to a crash-analysis service, not be caught
and retried.

**zx_channel_write return codes:** `ZX_ERR_PEER_CLOSED` if the other end has no
handles. `ZX_ERR_OUT_OF_RANGE` if a single message exceeds per-message limits
(65,536 bytes, 64 handles). No "try again" error for queue-full.

**Multi-source wait — port object:** Zircon's `zx_port_t` is a separate kernel
object designed for aggregating events from multiple kernel objects. Using
`zx_object_wait_async()`, any kernel object's signals can be registered with a
port. `zx_port_wait()` blocks until any registered object fires a signal,
returning a `zx_port_packet_t` that includes a caller-supplied key identifying
the source. This is Zircon's equivalent of `epoll`. For channels specifically,
registering `ZX_CHANNEL_READABLE` on a port allows a single thread to wait on N
channels simultaneously.

Packet types the port aggregates: user packets (explicitly enqueued by
applications), signal packets (from object signal changes), interrupt packets
(from hardware interrupts), and pager page-request packets. The port is a
general event-aggregation primitive, not channel-specific.

**Coalescing — signals, not messages:** Zircon object signals (e.g.,
`ZX_CHANNEL_READABLE`) are boolean per-object state. Multiple writes to a
channel assert `ZX_CHANNEL_READABLE` once; it stays asserted until the channel
is drained. There is no "N pending signals" count. The signal coalesces
naturally through boolean semantics. A port receives at most one packet per
signal edge (when the signal transitions to set).

**Sources:** Fuchsia `zx_channel_write` syscall reference; Fuchsia IPC limits
documentation; Fuchsia `zx_port_wait` syscall reference; Zircon Kernel Concepts.

---

### QNX Neutrino — Synchronous rendezvous + separate pulse queue

QNX uses synchronous rendezvous for primary IPC (`MsgSend` / `MsgReply`). No
message queue exists for regular messages — the sender blocks until the server
calls `MsgReply`. There is no "queue full" condition for the rendezvous path.

**Pulses (asynchronous, bounded queue):** QNX pulses are a separate mechanism: a
5-byte datum (1-byte code + 4-byte value) sent without blocking. Pulses have a
bounded queue per connection. The kernel maintains the pulse queue; if a pulse
arrives when the thread is not yet waiting, it is buffered. Pulses are FIFO and
are not coalesced by the kernel.

**Multi-source wait — single channel, multiple connections:** A QNX channel
aggregates messages from multiple connections. `MsgReceive()` on a channel
receives from any attached connection; the returned `rcvid` identifies which
connection. This is many-to-one fan-in at the channel level — structurally
similar to seL4 badges. No "port set" equivalent is needed because the channel
already aggregates at the server.

**Sources:** QNX Neutrino IPC architecture documentation; QNX Pulses
documentation.

---

### MINIX 3 — Rendezvous with coalescing notifications

MINIX 3 uses direct send/receive between processes. `sendrec()` is atomic send +
wait-for-reply. `notify()` is a non-blocking signal from one process to another.

**Overflow policy:** Direct send blocks the sender until the target calls
`receive()`. No message queue to overflow. `notify()` sets a per-sender bit in
the target's pending-notification bitmap; if the bit is already set
(notification pending), the new `notify()` is silently discarded. This is
coalescing by bitmap: each source gets one bit, and duplicate notifications
merge.

**Multi-source wait:** MINIX processes check the notification bitmap as part of
each `receive()` call. No separate primitive is needed — the bitmap is checked
atomically with the message receive.

**Sources:** MINIX 3 design documentation; capability-revocation.md (local)
§MINIX generation numbers.

---

### Composite OS — Separate send/receive capabilities; ring-buffer option

Composite provides `cos_asnd` / `cos_arcv` as separate kernel objects (send
capability vs. receive capability). The async send deposits into a ring buffer
associated with the receive capability. If the ring is full, the send returns an
error immediately (non-blocking drop). No blocking of the sender on a full
queue.

**Coalescing:** The ring buffer provides FIFO ordering, not coalescing. A
separate activation mechanism (tcap-based) is used for priority-aware delivery.

**Sources:** Composite OS syscall research (local
`design/research/syscall-landscape.md`).

---

## Measured Data

| System    | Queue model                                   | Queue depth                                          | Full behavior                                                      | Multi-wait primitive               |
| --------- | --------------------------------------------- | ---------------------------------------------------- | ------------------------------------------------------------------ | ---------------------------------- |
| seL4      | Thread queue (no msg buffer)                  | Unbounded (thread count)                             | NBSend: silent drop. Send: blocks until receiver                   | Badge fan-in + bound notification  |
| Mach/XNU  | Message queue                                 | 8 (default, configurable)                            | Block sender (FIFO, no starvation). MACH_SEND_TIMEOUT for deadline | Port set (receive from any member) |
| Zircon    | Message queue                                 | Not exposed (memory-bounded)                         | Policy exception (fatal, not retryable)                            | Port object (zx_port_wait)         |
| QNX       | Rendezvous (messages), FIFO (pulses)          | ∞ (messages block), bounded (pulses)                 | Block sender (messages), discard unclear (pulses)                  | Channel fan-in (connection ID)     |
| MINIX 3   | Rendezvous (messages), bitmap (notifications) | ∞ (messages block), 1-bit per source (notifications) | Sender blocks. Notification: coalesce (drop duplicate)             | Bitmap checked at receive time     |
| Composite | Ring buffer                                   | Fixed at creation                                    | Error return (drop)                                                | N/A (direct capability naming)     |

**seL4 IPC latency:** ~700–2000 cycles (ARM Cortex-A57) for rendezvous path.
**Mach port queue:** Default limit 8, max configurable to MACH_PORT_QLIMIT_MAX
(typically 16). **Zircon per-message limits:** 65,536 bytes, 64 handles per
message. Queue depth not exposed.

---

## Tradeoffs

### Overflow policy

**Block sender (Mach, QNX message, MINIX message, seL4 endpoint)**

- Backpressure propagates naturally: slow receiver stalls sender
- No data loss
- Risk: priority inversion. If a high-priority sender is blocked behind a
  low-priority receiver, the high-priority sender inherits receiver's priority
  floor. QNX avoids this by having the server inherit sender priority.
- Risk: deadlock. If A waits for B and B waits for A on full queues, neither can
  proceed.
- Kernel memory footprint is bounded by the sender queue (threads blocked), not
  by message count.

**Silent drop (seL4 NBSend, Composite)**

- Sender always continues; no stall
- Data loss is possible and silent
- No back-channel information (intentional in seL4 for security)
- Appropriate for idempotent notifications where "fired once" is sufficient
- Caller cannot distinguish "dropped" from "delivered" without additional
  protocol

**Error return on full (Composite ring buffer)**

- Caller can decide what to do (retry, escalate, discard)
- Requires caller discipline to handle errors; common failure mode is ignoring
  them
- Allows non-blocking deposit without implicit stall

**Policy exception / crash (Zircon)**

- No code path for "queue full" in normal operation
- Assumes architectural flow control (backpressure via higher-level protocols)
- Makes "queue builds up" a fatal event rather than a recoverable condition
- Appropriate for systems where queue buildup indicates a programming error
- Limits are not exposed, so applications cannot reliably probe or manage depth

---

### Multi-endpoint wait

**Badge fan-in (seL4, QNX connection ID)**

- All sources send to one endpoint; badge/connection-ID identifies who
- Single endpoint, single blocking call
- No separate multiplexing object
- Requires all senders to hold the same endpoint capability (or one minted from
  it), which constrains topology: server controls who holds which badge
- Suitable for N clients, 1 server where server creates all connections

**Port set / port aggregator (Mach port set, Zircon port, MINIX bitmap)**

- Separate kernel object aggregates events from multiple independent endpoints
- Endpoints remain independent; aggregation is configured separately
- Natural for event loops that span unrelated sources (network, timer, IPC)
- Zircon's port is general (any kernel object type, any signal); Mach's port set
  is specific (only receive rights on ports)
- Additional kernel object to create, manage, and account

**Thread-per-source**

- One thread per endpoint; threads synchronize via shared memory or a higher
  endpoint
- No kernel multi-wait primitive needed
- Cost: thread stack, scheduler overhead, synchronization complexity
- Scales poorly when N is large (e.g., many transient connections)

---

### Coalescing: can a capacity-1 overwrite endpoint serve as a notification?

The survey reveals a consistent pattern: **no deployed system uses a message
queue with overwrite semantics for coalescing**. Coalescing appears exclusively
in dedicated notification-class objects:

- seL4: `Notification` object (word-sized bitmap, OR-merge per send)
- Zircon: object signals (boolean per-object, not per-sender)
- MINIX 3: per-sender notification bits in receiver's bitmap
- Mach: no coalescing; signals sent as messages

The reason this pattern appears consistently: a message queue with overwrite
semantics requires a decision about what to overwrite _when multiple distinct
sources share one queue_. Per-source overwrite requires per-source slots — which
is no longer a queue but a map keyed by source. A single-slot overwrite on a
shared endpoint loses messages from other sources, creating cross-source data
loss.

Systems that want coalescing use **bitfields**, where each bit corresponds to a
source. OR-merge is defined for bitfields: N signals from source A merge to 1
bit. This is structurally incompatible with a message queue — the two data
structures (queue for ordered, distinct messages; bitfield for coalescing
events) serve different purposes and do not unify cleanly into one object.

The only system that comes close to the "single mechanism" goal for both
message-passing and coalescing is seL4's **bound notification**, which lets one
blocking call return either an endpoint message or a notification — but the two
objects remain distinct kernel types. The bound notification is glue between two
mechanisms, not a unification of them.

---

## References

1. seL4 Reference Manual, version 14.0.0.
   https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
2. seL4 IPC tutorial. https://docs.sel4.systems/Tutorials/ipc.html
3. seL4/seL4 ipc.tex source.
   https://github.com/seL4/seL4/blob/master/manual/parts/ipc.tex
4. Heiser, "How to (and how not to) use seL4 IPC." microkerneldude.org, 2019.
   https://microkerneldude.org/2019/03/07/how-to-and-how-not-to-use-sel4-ipc/
5. GNU Mach Reference Manual — Message Send.
   https://www.gnu.org/software/hurd/gnumach-doc/Message-Send.html
6. mach_msg man page (MIT Darwin mirror).
   https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/mach_msg.html
7. Apple Kernel Programming Guide — Mach messaging.
   https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html
8. Mach's IPC Basic Concepts (Hurd Extras).
   https://hurdextras.nongnu.org/ipc_guide/mach_ipc_basic_concepts.html
9. Fuchsia zx_channel_write syscall reference.
   https://fuchsia.dev/fuchsia-src/reference/syscalls/channel_write
10. Fuchsia IPC limits documentation.
    https://fuchsia.dev/fuchsia-src/concepts/kernel/ipc_limits
11. Fuchsia zx_port_wait syscall reference.
    https://fuchsia.dev/fuchsia-src/reference/syscalls/port_wait
12. Zircon Kernel Concepts.
    https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
13. QNX Neutrino IPC architecture.
    https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc.html
14. QNX Pulses.
    https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Pulses.html
15. Mitchell Hashimoto, "Don't use SEND_ONCE mach rights for async, use a queue
    limit of 1." libxev commit discussion. 2023.
    https://github.com/mitchellh/libxev/commit/a42b74ae8139738a14148f94543c659ec2d5b92b
