# Endpoint Shape: Directionality, Topology, and Capability Rights

## Question

What is the shape of an IPC endpoint? Specifically:

1. **Directionality** — is the endpoint unidirectional (one designated sender
   side, one designated receiver side) or bidirectional (both ends can send and
   receive)?
2. **Topology** — what cardinality constraints exist between senders and
   receivers? (one-to-one, many-to-one, many-to-many)
3. **Capability-rights model** — what operations does a held capability permit,
   and how are these rights subdivided?

This question arises in the context of IPC object design immediately after
settling on queued endpoints with a direct-switch fast path.

---

## Survey

### seL4

**Object type:** The kernel provides an `Endpoint` object. It is a separate
kernel object distinct from threads.

**Topology — many-to-many:** Any number of threads holding a Write-capable
endpoint cap can send to the endpoint simultaneously. Any number of threads
holding a Read-capable endpoint cap can receive from it simultaneously. The
endpoint maintains a single FIFO queue that holds either waiting senders or
waiting receivers (never both at the same time). When a sender arrives and a
receiver is waiting, they rendezvous immediately (direct context switch on the
fast path). When none are waiting, the arriving thread joins the queue.

**Directionality — logically unidirectional with reply side-channel:** The
endpoint itself has no inherent notion of "the other end". The server holds Read
rights (Recv); clients hold Write rights (Send). For RPC semantics, `seL4_Call`
atomically sends and then blocks waiting for a reply. The kernel generates a
one-use _Reply capability_ that the server uses to respond directly to the
caller — bypassing the endpoint entirely. This reply cap is not itself an
endpoint and has no queue; it names the blocked caller thread directly.

**Capability rights model — four orthogonal bits:**

| Right        | Operation enabled                       | Notes                                           |
| ------------ | --------------------------------------- | ----------------------------------------------- |
| `Write`      | `seL4_Send`, `seL4_Call`                | Required to send any message                    |
| `Read`       | `seL4_Recv`                             | Required to receive; faults if absent           |
| `Grant`      | Include capabilities in message payload | Allows capability transfer through the endpoint |
| `GrantReply` | Transfer capabilities via reply cap     | Weaker form of Grant; governs cap-in-reply only |

`Grant` and `GrantReply` are independent of each other. Holding `Grant` implies
`GrantReply` is redundant; a cap with only `GrantReply` can pass caps through
the reply mechanism but not through the initial send.

**Badge mechanism:** An endpoint cap can be _minted_ with a badge (integer)
value. The kernel delivers the badge to the receiver as part of the message
metadata. The badge sits on the _capability_, not on the message content — so a
server can mint N distinct-badged copies of a single endpoint cap (one per
client) and use the badge value to identify which client sent each message.
Badging enables many-to-one fan-in without requiring separate per-client
endpoint objects.

**Sources:** seL4 Reference Manual 14.0.0 §4 (IPC);
[seL4 IPC tutorial](https://docs.sel4.systems/Tutorials/ipc.html);
[seL4 ipc.tex source](https://github.com/seL4/seL4/blob/master/manual/parts/ipc.tex);
[RFC-13 GrantReply discussion](https://lists.sel4.systems/hyperkitty/list/rfc@sel4.systems/thread/GLQEEHS4SDQEIMEGXSO5AYBHWXR5QCPH/)

---

### Mach / XNU

**Object type:** A _port_ is the kernel IPC object. Ports contain an ordered
message queue (kernel-buffered, asynchronous delivery possible up to a
configurable queue depth, default 8).

**Topology — many-to-one (MPSC):** Multiple tasks can hold Send rights to the
same port (fan-in of senders). Exactly **one** receive right exists per port in
the entire system — this is a global invariant enforced by the kernel. No task
can hold two receive rights to the same port. A receive right can be added to a
_port set_ to multiplex receiving across many ports on one thread.

**Directionality — explicitly unidirectional:** A single port is one directional
channel. Bidirectional RPC requires two ports: the server's main port (server
holds receive right, client holds send right) plus a per-call reply port (client
allocates a new port, holds receive right, and passes server a Send-once right
to it). The Send-once right is consumed after one use, enforcing that the server
can reply exactly once.

**Capability rights model — three right types:**

| Right           | Semantics                                                      |
| --------------- | -------------------------------------------------------------- |
| Send right      | Unlimited sends to the port; can be copied and transferred     |
| Receive right   | Can receive messages; exactly one per port; can be transferred |
| Send-once right | One message allowed, then right is consumed/destroyed          |

There is no separate "grant" right — capability transfer in messages is
permitted by default when you hold Send rights, as capabilities are carried in
the message body as typed descriptors. The kernel validates message descriptors
at send time.

**Port set:** A port set is an auxiliary kernel object that aggregates receive
rights. A thread waiting on a port set dequeues from whichever member port has a
pending message. This creates fan-in without exposing the structural many-to-one
topology to callers.

**Sources:**
[Apple Kernel Programming Guide — Mach ports](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html);
[GNU Mach Reference Manual](https://www.gnu.org/software/hurd/microkernel/mach/port.html);
[Darling Mach port internals](https://docs.darlinghq.org/internals/macos-specifics/mach-ports.html);
[dmcyk XNU IPC blog](https://dmcyk.xyz/post/xnu_ipc_i_mach_messages/)

---

### Zircon (Fuchsia)

**Object type:** A _channel_ is a kernel object created as a peer pair. Creation
always returns two handles simultaneously, one for each end.

**Topology — exactly one-to-one (1:1):** A channel has precisely two endpoints.
There is no many-sender or many-receiver fan-in at the kernel level.
Higher-level fan-in (many clients, one server) is composed by creating one
channel per client connection and using a _port_ object (a separate kernel type,
not the same as an IPC channel) to multiplex events across them.

**Directionality — bidirectional:** Each end of a channel can both write
messages to the other end and read messages from the other end. There is no
asymmetric write-only or read-only endpoint. Both ends are symmetrically
capable.

**Capability rights model — per-handle rights bits:**

| Right               | Semantics                                    |
| ------------------- | -------------------------------------------- |
| `ZX_RIGHT_READ`     | Can read (receive) messages from this end    |
| `ZX_RIGHT_WRITE`    | Can write (send) messages to the other end   |
| `ZX_RIGHT_TRANSFER` | Can transfer this handle to another process  |
| `ZX_RIGHT_WAIT`     | Can block waiting for signals on this handle |
| `ZX_RIGHT_SIGNAL`   | Can signal the handle's associated signals   |

Rights are per-handle, not per-object or per-process. A single process can hold
two handles to the same kernel object with different rights. Handles can be
duplicated with rights restricted (never expanded).

**Peer detection:** When one endpoint loses all handles, the other end receives
the `ZX_CHANNEL_PEER_CLOSED` signal. This lifecycle signal is an inherent
property of the paired model.

**Sources:**
[Fuchsia Zircon fundamentals](https://fuchsia.dev/fuchsia-src/get-started/learn/intro/zircon);
[Zircon kernel objects reference](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/objects);
[Fuchsia Zircon handles](https://fuchsia.dev/fuchsia-src/concepts/kernel/handles)

---

### Mach — bidirectional discussion

Because Mach ports are unidirectional and RPC requires two ports, XNU implements
a higher-level pattern where `mach_msg` with `MACH_SEND_MSG | MACH_RCV_MSG`
flags atomically sends on one port and receives on another — providing RPC
call-site convenience without changing the underlying port model.

**Source:**
[dmcyk XNU IPC bidirectional messages](https://dmcyk.xyz/post/xnu_ipc_ii_message_apis/xnu_ipc_ii_message_apis/)

---

### QNX Neutrino

**Object type:** QNX separates the server-side object (_channel_) from the
per-client object (_connection_). A server calls `ChannelCreate()` to publish a
receive target. A client calls `ConnectAttach()` to obtain a connection ID
(coid) pointing at the channel.

**Topology — many-to-one:** Multiple client connections can attach to one
channel. The channel collects incoming messages from all connections. The server
calls `MsgReceive()` on the channel; the returned message identifies which
connection ID (and thus which client) sent it.

**Directionality — send/reply model:** Messages flow client → server via
`MsgSend()`. The caller blocks until the server calls `MsgReply()`, which
unblocks the sender and returns the reply data. This is a synchronous
rendezvous: the send and reply are paired. _Pulses_ provide a nonblocking
one-way signal (4 bytes + 1-byte code) with no reply.

**Priority inheritance:** The server inherits the sender's priority for the
duration of processing the request, preserving relative scheduling fairness.

**Rights model:** QNX does not use a capability model. Channel access is
governed by Unix-style user/group permissions and channel-creation flags (e.g.,
`_NTO_CHF_PRIVATE` for same-process-only). There are no fine-grained
send/receive/grant rights bits.

**Sources:**
[QNX Neutrino IPC architecture](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc.html);
[QNX channels and connections](http://www.qnx.com/developers/docs/qnxcar2/topic/com.qnx.doc.neutrino.sys_arch/topic/ipc_Channels.html);
[QNX pulses](https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Pulses.html)

---

### EROS / Coyotos

**Object type:** EROS does not have a separate endpoint kernel object.
Communication is through _key invocation_ — the capability (key) directly names
the target object (a process or kernel object). The invocation mechanism is the
only major kernel operation (`InvokeCap` in Coyotos).

**Topology:** Any process holding a key to a process's _start key_ (or _gate
key_) can invoke it. Multiple callers can hold start keys to the same process.
The target process has a single entry point. In EROS, the kernel serializes
concurrent invocations at the process level.

**Directionality — unidirectional invocation with synchronous result:**
Invocation is caller → callee. The caller implicitly provides a _resume key_ (a
single-use return capability naming the calling thread) so the callee can return
data. The resume key is equivalent to a Mach send-once right or seL4 reply cap.
No persistent reply port or reply endpoint is allocated.

**Rights model (EROS):** Keys are typed; the type encodes the allowed
operations. There are no separate read/write/grant bits — the key type is the
access control. A _start key_ authorizes invoking the process's entry point. A
_data key_ carries inline data with no dispatch to a process. A _device key_
names hardware. Rights are not bits on a generic capability; they are encoded in
the key type identity.

**Coyotos extension:** Coyotos retains the key model but introduces typed
endpoints with protection-domain scoping and a richer invocation payload, though
the fundamental unidirectional invocation model is preserved.

**Sources:**
[EROS Wikipedia](<https://en.wikipedia.org/wiki/EROS_(microkernel)>);
[Coyotos Microkernel Specification](https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html);
[Differences Between Coyotos and EROS](http://www.cap-lore.com/CapTheory/KK/Shap/eros-comparison.html)

---

### Genode

**Object type:** An _entrypoint_ (or RPC object) is a server-side thread that
dispatches incoming RPC calls. The capability to an RPC object is what clients
hold.

**Topology — many-to-one:** Any client with a valid capability to the RPC object
can invoke it. The entrypoint serializes incoming calls.

**Directionality:** Clients invoke the server (call direction). Capabilities can
be passed as arguments or returned in replies, so the capability graph can be
extended in either direction during an RPC. The server does not "call back" to
clients via the same endpoint mechanism — callbacks require the client to expose
its own entrypoint.

**Rights model:** Genode wraps the underlying microkernel's capability model. On
seL4, seL4 endpoint capabilities and their rights are used directly. On NOVA,
portal capabilities with execution rights are used. Genode's C++ API presents
capabilities as opaque tokens with no exposed rights bits — rights enforcement
happens at the kernel level transparently.

**Sources:**
[Genode RPC specification](https://genode.org/documentation/genode-foundations/19.05/functional_specification/Remote_procedure_calls.html);
[Genode capability-based security](https://genode.org/documentation/genode-foundations/20.05/architecture/Capability-based_security.html)

---

### Composite OS (Composite)

**Object type:** Composite separates the _asynchronous send capability_
(`asndcap`) from the _receive capability_. These are distinct kernel objects,
not two rights on one object.

**Topology:** The send and receive capabilities are independently held; the
kernel does not enforce a pairing constraint. Any holder of an asndcap can
activate the receiving thread. This allows the topology to be composed flexibly
by userspace.

**Directionality:** The separation of send and receive capabilities makes
directionality explicit at the type level. Asynchronous sends activate the
receiver without synchronously waiting. Synchronous invocations (similar to EROS
key invocation) are also supported.

**Sources:** [Composite syscall landscape (design/research/syscall-landscape.md
local)]; Composite OS documentation and source (cos_asnd / cos_arcv primitives)

---

### Original L4

**Object type:** Original L4 (Liedtke's design) used _threads_ as the direct IPC
target — there was no separate endpoint kernel object. The IPC destination was a
thread ID.

**Topology — one-to-one (thread-addressed):** Each IPC operation named a
specific destination thread. No fan-in or fan-out at the primitive level. Thread
groups and higher-level routing were done in userspace.

**Rationale for thread-addressed IPC:** Avoiding the indirection of a separate
object eliminated cache and TLB pollution, achieving IPC latencies an order of
magnitude better than Mach at the time. Liedtke acknowledged that ports could be
added with approximately 12% overhead (primarily 2 extra TLB misses).

**Sources:**
[From L3 to seL4: 20 Years of L4 Microkernels (SOSP 2013)](https://sigops.org/s/conferences/sosp/2013/papers/p133-elphinstone.pdf)

---

### MINIX 3

**Object type:** No separate endpoint kernel object. The "endpoint" concept in
MINIX 3 is a `(process_id, generation_number)` pair — an integer tuple, not a
kernel-allocated object. The generation number invalidates stale references when
a process is restarted.

**Topology — one-to-one:** Messaging is direct process-to-process. No
multiplexing primitive at the kernel level.

**Directionality:** Bidirectional at the primitive level: `send`, `receive`, and
`sendrecv` (atomic send+wait-for-reply). No separate receive or send right
distinction.

**Rights model:** No capability model. Access control is based on allowed-sender
lists: each process configures which other processes may send it messages. This
is enforced at send time by the kernel.

**Sources:** MINIX 3 design documentation;
[capability revocation research (local)](design/research/capability-revocation.md)
§MINIX generation numbers

---

## Measured Data

### IPC latency comparison (synchronous round-trip, approximate)

| System    | Mechanism           | Hardware       | Approx. latency    | Source                              |
| --------- | ------------------- | -------------- | ------------------ | ----------------------------------- |
| seL4      | Endpoint Call/Reply | ARM Cortex-A57 | ~700–2000 cycles   | SOSP 2013 L4 paper; seL4 whitepaper |
| seL4      | Endpoint Call/Reply | x86-64         | ~280–400 cycles    | seL4 whitepaper                     |
| Zircon    | Channel call        | x86-64         | ~9× seL4 cost      | SJTU XPC paper (TOCS 2022)          |
| Fiasco.OC | IPC (L4Re)          | x86-64         | ~2× seL4 cost      | SJTU XPC paper                      |
| Mach      | Port send/receive   | various        | 10–20× original L4 | Liedtke 1995                        |

The Zircon cost is attributed to its two-endpoint model (each end is an
independent kernel object with independent queues) and per-message handle
validation.

**Sources:**
[SOSP 2013 — 20 Years of L4 Microkernels](https://sigops.org/s/conferences/sosp/2013/papers/p133-elphinstone.pdf);
[SJTU XPC TOCS 2022](https://ipads.se.sjtu.edu.cn/_media/publications/2022_-_a_-_tocs_-_xpc.pdf);
[seL4 whitepaper](https://sel4.systems/About/seL4-whitepaper.pdf)

---

## Tradeoffs

### Directionality

**Unidirectional (Mach, original L4, EROS)**

- Simpler invariants: a port/key has one use direction; the rights model is
  smaller
- RPC requires a second object (reply port, resume key, send-once right) — more
  allocation and bookkeeping per call
- Easy to audit: all traffic on a port flows one way; senders cannot receive on
  it

**Bidirectional (Zircon, MINIX 3)**

- Single object for a conversation; no separate reply object
- Both endpoints hold symmetric authority; harder to reason about who holds
  which role
- Natural for stream-style communication; less natural for strict client-server
  RPC

**Unidirectional + typed reply side-channel (seL4, Coyotos)**

- Endpoint itself is unidirectional (Write = send, Read = receive); reply is a
  separate ephemeral capability that names the blocked caller directly
- Fast path: reply cap avoids touching the endpoint queue entirely
- Reply cap is inherently one-use (like send-once) — no explicit destruction
  needed

---

### Topology

**One-to-one (Zircon channel, original L4)**

- Minimal kernel state per endpoint; no queue of waiters for multiple receivers
- Fan-in requires composition (e.g., Zircon _port_ objects for event
  multiplexing)
- Natural model for session-oriented protocols (one client, one server per
  channel)
- Peer-closed signaling is straightforward because the partner is known at
  creation

**Many-to-one (Mach, QNX)**

- Matches the natural server pattern: one receive right, many senders
- Receive right can be transferred to implement handoff or load balancing
- Port sets extend fan-in to the receiver: one thread, many ports
- Badge/connection-ID needed to identify senders; managed separately from the
  port

**Many-to-many (seL4)**

- Maximally general; server can also have multiple receiver threads on one
  endpoint
- Thread queue in the endpoint handles any cardinality dynamically
- Badging provides sender identity without requiring per-sender endpoint objects
- Queue is bounded by thread count (not message count — synchronous), avoiding
  kernel memory unboundedness

---

### Capability rights granularity

**Coarse (EROS key type, MINIX allowed-sender list)**

- Fewer bits; simpler kernel path; rights are implicit in the object type
- Hard to express "can send but not transfer capabilities" without a new key
  type
- Attestation is type-level: the type of key you hold is your authority

**Medium (Mach: send / receive / send-once)**

- Three distinct right types cover the real use cases without excess complexity
- Send-once enforces one-shot reply semantics at the rights level, not at
  runtime
- No sub-operation rights (e.g., can't say "send but don't transfer the port")

**Fine-grained (seL4: Write / Read / Grant / GrantReply; Zircon: per-handle
bitmask)**

- Maximum expressivity; can create a "send only, no cap transfer" endpoint cap
- Derived capabilities can be minted with strict subsets of rights
- More kernel checking per operation; rights validation adds overhead on hot
  path
- seL4 verification required formal model of all four rights and their
  interactions

---

## Unsettled in the literature

- Whether a **bidirectional paired channel** (Zircon-style) or an
  **unidirectional + ephemeral reply cap** (seL4-style) is preferable for
  capability-based kernels is actively debated. Both approaches appear in
  production systems.
- The cost of the **reply cap** approach (generating and consuming an ephemeral
  kernel object on every call) vs. the cost of a **second persistent port**
  (Mach) vs. **direct peer write** (Zircon) is workload-dependent.
- **Minting limits** (how many badged caps can be derived from one endpoint) and
  their interaction with revocation are not standardized; seL4 MCS introduced
  changes to Grant/GrantReply that affected minting semantics.

---

## References

1. seL4 Reference Manual, version 14.0.0.
   https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
2. seL4 IPC tutorial. https://docs.sel4.systems/Tutorials/ipc.html
3. seL4 ipc.tex source.
   https://github.com/seL4/seL4/blob/master/manual/parts/ipc.tex
4. RFC-13 MCS Grant via reply.
   https://lists.sel4.systems/hyperkitty/list/rfc@sel4.systems/thread/GLQEEHS4SDQEIMEGXSO5AYBHWXR5QCPH/
5. Elphinstone & Heiser, "From L3 to seL4: What Have We Learnt in 20 Years of L4
   Microkernels?" SOSP 2013.
   https://sigops.org/s/conferences/sosp/2013/papers/p133-elphinstone.pdf
6. seL4 whitepaper. https://sel4.systems/About/seL4-whitepaper.pdf
7. Apple Kernel Programming Guide — Mach ports.
   https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html
8. GNU Mach Reference Manual.
   https://www.gnu.org/software/hurd/microkernel/mach/port.html
9. Darling Mach port internals.
   https://docs.darlinghq.org/internals/macos-specifics/mach-ports.html
10. dmcyk, "XNU IPC — Mach messages".
    https://dmcyk.xyz/post/xnu_ipc_i_mach_messages/
11. dmcyk, "XNU IPC — bidirectional Mach messages".
    https://dmcyk.xyz/post/xnu_ipc_ii_message_apis/xnu_ipc_ii_message_apis/
12. Fuchsia Zircon fundamentals.
    https://fuchsia.dev/fuchsia-src/get-started/learn/intro/zircon
13. Zircon kernel objects reference.
    https://fuchsia.dev/fuchsia-src/reference/kernel_objects/objects
14. Fuchsia Zircon handles.
    https://fuchsia.dev/fuchsia-src/concepts/kernel/handles
15. QNX Neutrino IPC architecture.
    https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc.html
16. QNX channels and connections.
    http://www.qnx.com/developers/docs/qnxcar2/topic/com.qnx.doc.neutrino.sys_arch/topic/ipc_Channels.html
17. QNX pulses.
    https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Pulses.html
18. EROS Wikipedia. https://en.wikipedia.org/wiki/EROS_(microkernel)
19. Coyotos Microkernel Specification.
    https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
20. Differences Between Coyotos and EROS.
    http://www.cap-lore.com/CapTheory/KK/Shap/eros-comparison.html
21. Genode RPC specification.
    https://genode.org/documentation/genode-foundations/19.05/functional_specification/Remote_procedure_calls.html
22. Genode capability-based security.
    https://genode.org/documentation/genode-foundations/20.05/architecture/Capability-based_security.html
23. Du et al., "XPC: Architectural Support for Secure and Efficient Cross
    Process Call." ISCA 2019.
    https://ipads.se.sjtu.edu.cn/_media/publications/duisca19.pdf
24. Du et al., "Boosting Inter-Process Communication with Architectural
    Support." TOCS 2022.
    https://ipads.se.sjtu.edu.cn/_media/publications/2022_-_a_-_tocs_-_xpc.pdf
