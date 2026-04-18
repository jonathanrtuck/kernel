# Reply-Cap Mechanism for RPC over Unidirectional Endpoints

## Question

Given an IPC model built on unidirectional endpoints — where send-rights and
receive-rights are distinct capabilities on a shared endpoint object — how does
a server send a response back to a caller? Specifically:

1. **What is the reply object?** Is it a kernel-minted ephemeral capability, a
   client-allocated persistent endpoint, a typed kernel token, or something
   else?
2. **Who allocates it?** Kernel (implicit on call), client (explicit creation
   before call), or a hybrid?
3. **What is its lifecycle?** One-shot (consumed on first use), persistent
   (reusable across calls), or scoped (valid for the duration of the blocked
   call)?
4. **How does it interact with the fast path?** Does it bypass the endpoint
   queue? Does it require cap-table operations at reply time?

---

## Survey

### seL4 — Classic Configuration

**Reply object type:** A _reply capability_ (reply cap) — an ephemeral,
single-use capability deposited by the kernel in the receiver's TCB reply slot.
The reply cap names the _calling thread_ directly, not an endpoint.

**Allocation — kernel-implicit on `seL4_Call`:** When a client invokes
`seL4_Call`, the kernel atomically (a) sends the message to the target endpoint
and (b) creates a reply cap in the server's TCB. The client thread transitions
to `BlockedOnReply` state. No userspace allocation or setup is required.

**Lifecycle — one-shot, TCB-resident:** The reply cap lives in a dedicated slot
in the receiving TCB. It is consumed the first time `seL4_Reply` or
`seL4_ReplyRecv` invokes it, after which the slot is empty. If a server needs to
defer a reply (e.g., while awaiting async I/O), it must first call
`seL4_CNode_SaveCaller` to move the reply cap into an explicit CSpace slot;
otherwise the next `seL4_Recv` on the same thread overwrites it.

**Fast-path interaction — endpoint queue bypassed entirely:** `seL4_ReplyRecv`
combines reply-send and next-receive into one syscall. The kernel's fastpath
(`fastpath_reply_recv` in `fastpath.c`) directly switches to the
`BlockedOnReply` thread without:

- touching the endpoint queue
- performing a badge lookup
- inserting the caller into any scheduler queue (the caller transitions directly
  to `Running`)

Fast-path conditions that must hold: message fits in `seL4_FastMessageRegisters`
(no buffered data), no capabilities transferred in the reply, and no
higher-priority thread is currently runnable.

**Sources:** seL4 Reference Manual 14.0.0 §4.2–4.3; seL4 MCS pre-release release
notes; `src/fastpath/fastpath.c` in the seL4 kernel source.

---

### seL4 — MCS (Mixed-Criticality Scheduling) Configuration

**Reply object type:** A `Reply` kernel object. Unlike the classic design, the
reply cap is not stored in the TCB — instead, the kernel deposits the reply cap
into an explicit `Reply` object that the server supplies at receive time.

**Allocation — explicit Reply object created by server, referenced per-call:**
The server allocates a `Reply` object (kernel-typed, user-allocated memory) and
passes a cap to it as a parameter to `seL4_Recv` / `seL4_ReplyRecv`. The kernel
populates it on each call arrival. Multiple `Reply` objects can coexist, one per
outstanding deferred call.

**Lifecycle — scoped to the call, not overridable:** Because the reply cap is in
a user-managed object rather than a TCB slot, it cannot be silently overwritten
by subsequent receives. This eliminates the `seL4_CNode_SaveCaller` workaround
required by classic seL4. The `Reply` object also carries scheduling context
donation metadata (the MCS motivation for the change).

**Fast-path interaction:** The reply path retains the same bypass properties —
`fastpath_reply_recv` checks the `Reply` object instead of the TCB slot but
still performs a direct context switch without endpoint queue involvement.

**Sources:** seL4 MCS pre-release 10.1.1 release notes (docs.sel4.systems); seL4
MCS tutorial (docs.sel4.systems/Tutorials/mcs.html); RFC-13 mailing list
discussion on GrantReply rights in MCS.

---

### Mach / XNU

**Reply object type:** A _reply port_ — a standard Mach port allocated by the
client before the call. The client holds the receive right. The server is given
a _send-once right_ to this port, embedded in the request message. The send-once
right is a distinct port right type: it permits exactly one message send, after
which the right is consumed and a `MACH_NOTIFY_SEND_ONCE` notification is
generated (or not, depending on configuration).

**Allocation — explicit, client-side:** The client must call
`mach_port_allocate` (or the optimized `mach_reply_port()` syscall) before each
RPC. `mach_reply_port()` is a true syscall rather than an RPC (which would
itself require a reply port), and implementations may optimize the underlying
port for reply-port use patterns. The client passes a send-once right to this
port in the message body as a typed descriptor. The server extracts the
send-once right and uses it to reply.

**Lifecycle — send-once right is one-shot; reply port is persistent:** The
_send-once right_ is consumed when the server sends its reply. The _reply port_
itself (the kernel object) is not automatically destroyed — the client may
destroy it after receiving the reply, or reuse it for subsequent calls (though
reuse requires minting a new send-once right for each call, which the client
does by holding the receive right and passing rights derived from it). In
practice, MiG-generated stubs typically destroy and re-allocate reply ports
per-call rather than reusing them.

**Fast-path interaction:** Mach has no equivalent of seL4's fastpath. The reply
goes through the port's message queue (albeit a queue that holds at most one
message due to the send-once right). The client is unblocked by the kernel when
the server sends to the send-once right, but this involves queue insertion and a
scheduler notification, not a direct context switch. XNU's `mach_msg_trap` with
combined `MACH_SEND_MSG | MACH_RCV_MSG` flags performs atomic send+receive in
one syscall entry but does not eliminate queue involvement.

**Sources:** Apple Kernel Programming Guide — Mach Messaging (§IPC); GNU Mach
Reference Manual §Inter-Process Communication; mach_port_allocate manual page
(web.mit.edu/darwin); Mach IPC Basic Concepts (hurdextras.nongnu.org).

---

### KeyKOS / EROS

**Reply object type:** A _resume key_ (KeyKOS) / _resume capability_
(EROS/Coyotos) — a kernel-minted single-use key that names the blocked calling
domain's return address. In the KeyKOS type taxonomy, resume keys are a subtype
of gate keys distinct from start keys.

**Allocation — kernel-implicit on `CALL`:** When a domain issues a `CALL`
invocation on any key, the kernel atomically creates a resume key and passes it
to the callee as an implicit parameter in a dedicated slot (the "key register"
used for return path, distinct from the four general-purpose key parameters).
The calling domain transitions to a blocked-on-reply state. No caller-side
allocation is required.

**Lifecycle — one-shot, transferable:** The resume key is valid indefinitely
until invoked. The callee can pass it to a third party ("tail-call" delegation)
or save it for deferred reply. When any domain invokes the resume key (via
`RETURN` or `CALL`), the original caller is unblocked and the key is consumed.
This property enables the composition of call chains without requiring the
intermediate domain to own a persistent endpoint — the resume key embodies the
pending return obligation.

**Fast-path interaction:** In EROS (described in the SOSP '99 paper), the
invocation path is the single kernel operation. When a callee invokes the resume
key via `RETURN`, the kernel performs a direct domain switch to the caller
without any separate endpoint queue. KeyKOS measured IPC round-trips at
approximately 35 µs on 1989-era hardware; the EROS paper reports on the order of
5–10 µs on 1999 SPARC hardware. Both designs attributed their speed partly to
the directness of the resume key path.

**Sources:** Bomberger et al., "The KeyKOS Nanokernel Architecture," USENIX
Workshop on Microkernels, 1992
(css.csail.mit.edu/6.5660/2017/readings/keykos.pdf); Shapiro et al., "EROS: A
Fast Capability System," SOSP 1999
(sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf); Differences
Between Coyotos and EROS (cap-lore.com).

---

### L4.re / Fiasco.OC

**Reply object type:** Implicit — not a named capability object. The kernel
tracks which client thread is `BlockedOnCall` on behalf of the server's current
call context. The server accesses this via the `L4_SYSF_REPLY` flag rather than
naming a reply object.

**Allocation — none:** The server does not allocate or name a reply object.
`l4_ipc_reply_and_wait()` is the idiomatic server loop operation: it atomically
sends a reply to the implicit blocked caller and then waits for the next
request. The "reply" destination is the thread state maintained by the kernel
from the preceding `l4_ipc_wait` / `l4_ipc_call` pair.

**Lifecycle — scoped to the call:** The implicit reply right exists only while
the caller is in the blocked-on-reply state. It cannot be transferred or saved.
A server that needs to defer a reply must instead save the caller's thread ID
(available in the message tag) and re-establish the reply path by other means —
L4.re does not have a direct equivalent of `seL4_CNode_SaveCaller`.

**Fast-path interaction:** L4.re/Fiasco.OC implements an IPC fastpath similar to
seL4's. The reply path (`l4_ipc_reply_and_wait` with `L4_SYSF_REPLY`) directly
switches to the blocked caller without endpoint queue traversal when the
fast-path conditions hold (small message, no capability transfer,
single-register payload). The Fiasco.OC IPC round-trip cost was measured at
approximately 2× the seL4 cost on x86-64 in the SJTU XPC (TOCS 2022) benchmarks.

**Sources:** L4Re IPC concepts (l4re.org/doc/l4re_concepts_ipc.html); L4Re
Object Invocation API (l4re.org/doc/group**l4**ipc\_\_api.html); Du et al.,
"Boosting Inter-Process Communication with Architectural Support," TOCS 2022.

---

### QNX Neutrino

**Reply object type:** A _receive ID_ (`rcvid`) — an opaque integer token
returned by `MsgReceive()`. The rcvid identifies the blocked sender within the
server's channel. It is not a capability; it cannot be transferred between
processes or manipulated as a kernel object.

**Allocation — kernel-generated at receive time:** When the server calls
`MsgReceive()` (or `MsgReceive_r()`), the kernel returns an rcvid that encodes
the identity of the blocked sender (process, thread, connection ID). The client
has been in `REPLY_BLOCKED` state since `MsgSend()` returned to the kernel.

**Lifecycle — scoped to the blocked state:** The rcvid is valid until
`MsgReply()` or `MsgError()` is called with it. After the server replies, the
client transitions from `REPLY_BLOCKED` to `READY`, and the rcvid is
invalidated. The rcvid cannot be used across process boundaries, so deferred
replies must occur within the server process that received the message.

**Fast-path interaction:** QNX has no formally separate fast path in the seL4
sense. `MsgReply()` is documented as non-blocking from the server's perspective
— since the client is already blocked, no synchronization is required from the
server side. The kernel delivers the reply data and schedules the client
directly. QNX implements priority inheritance: the server runs at the sender's
priority for the duration of the call, and reply restores both threads to their
base priorities.

**Sources:** QNX Neutrino IPC Architecture §Synchronous Message Passing
(qnx.com/developers/docs/8.0); MsgSend() reference manual (qnx.com); Synchronous
Message Passing chapter (qnxcar2 sys_arch).

---

### Zircon (Fuchsia)

Zircon channels are bidirectional paired objects — there is no reply object
because the server's end of the channel already has write access to the client's
read queue. The server reads a request from its end and writes a reply to the
same end; the client reads the reply from its end. No additional routing
mechanism is needed. This section is included for contrast; Zircon's reply
routing is an intrinsic property of the bidirectional channel model, not a
separately designed mechanism.

---

## Measured Data

### seL4 fast-path: reply path vs. send path latency

The seL4 fastpath applies to both `seL4_Call` (initial send) and
`seL4_ReplyRecv` (reply + next receive). Both must satisfy the same fast-path
conditions (message in registers, no cap transfer, no higher-priority runnable
thread). In benchmarks reported in the seL4 whitepaper and the SOSP 2013 paper:

| Configuration              | Hardware       | Approx. round-trip latency |
| -------------------------- | -------------- | -------------------------- |
| seL4 (classic fastpath)    | ARM Cortex-A57 | ~700–2000 cycles           |
| seL4 (classic fastpath)    | x86-64         | ~280–400 cycles            |
| Fiasco.OC (L4.re)          | x86-64         | ~2× seL4                   |
| Zircon (no fast path)      | x86-64         | ~9× seL4                   |
| Mach (ports, no fast path) | various        | 10–20× original L4         |

The difference between seL4 and Zircon/Mach is partly attributed to the reply
path: seL4's kernel-minted reply cap bypasses endpoint queue operations
entirely, while Zircon and Mach involve per-call object lookups and queue
insertions.

**Sources:** seL4 whitepaper (sel4.systems/About/seL4-whitepaper.pdf);
Elphinstone & Heiser, SOSP 2013; Du et al., TOCS 2022 (XPC benchmarks).

### seL4 fast-path: conditions that disqualify the reply path

If any of the following hold, the reply falls off the fastpath to the slow path:

- The reply message contains capability references (`Grant` or `GrantReply`)
- The message length exceeds the register-only threshold (in practice, more than
  `seL4_FastMessageRegisters` words)
- A higher-priority thread has become runnable (preemption check)
- The target thread is not in `BlockedOnReply` state (call was already replied,
  or caller was killed)
- In MCS: the Reply object cap is not valid or the scheduling context checks
  fail

The slow path processes the reply through the general IPC path, which is
significantly more expensive due to scheduler interactions.

**Source:** seL4 `src/fastpath/fastpath.c` (github.com/seL4/seL4).

---

## Tradeoffs

### Kernel-minted vs. client-allocated reply object

**Kernel-minted (seL4, KeyKOS/EROS, L4.re implicit):**

- No client-side setup: call sites are simpler; nothing to forget to allocate
- The reply object names the caller thread directly — it is unforgeable and
  cannot be confused with a stale object from a prior call
- The kernel can optimize the reply path because it has full knowledge of the
  object's structure
- Deferred reply requires explicit "save" mechanism (seL4 classic: `SaveCaller`;
  KeyKOS: callee keeps the key; seL4 MCS: explicit Reply object handles this)

**Client-allocated (Mach send-once right):**

- Client controls reply port lifetime; can implement timeouts by destroying the
  port
- Reply port can be shared with multiple parties (though only one can reply,
  since the right is send-once on the port created per-call)
- Allocation overhead on every call: `mach_port_allocate` + rights manipulation
- MiG stubs absorb this overhead but it is real
- No kernel-level fast path: the server writes to a queue, not directly to the
  caller

### One-shot vs. persistent vs. scoped lifecycle

**One-shot consumed on first use (seL4 classic reply cap, Mach send-once right,
KeyKOS resume key):**

- Prevents double-reply: the server cannot accidentally reply twice to the same
  call; the second attempt will fail with a capability lookup error
- In seL4/KeyKOS, the reply object simply ceases to exist after use — no
  explicit cleanup required
- In Mach, the send-once right is consumed but the underlying reply port must be
  explicitly destroyed by the client to reclaim kernel memory

**Scoped to blocked state (QNX rcvid):**

- No kernel object allocated — the rcvid is an index into the kernel's
  per-thread state; no heap allocation
- Zero cleanup: the rcvid invalidates automatically when the sender unblocks
- Non-transferable: the rcvid cannot be moved to another process for deferred
  handling by a third party
- Non-saveable across calls in the same server thread: if the server calls
  `MsgReceive` again before replying, the first rcvid remains valid but a new
  one is returned; the server must manage these explicitly

**Explicit object, user-managed (seL4 MCS Reply object):**

- Eliminates the need for `SaveCaller` pattern: reply cap is always accessible
  in the named Reply object, not silently overwritten by the next receive
- Required for scheduling-context-aware deferred reply (MCS motivation)
- One Reply object per outstanding deferred call; server pre-allocates a pool if
  it handles concurrent deferred calls

### Named capability vs. opaque token vs. implicit thread state

**Named capability (seL4 reply cap, KeyKOS resume key, Mach send-once right):**

- Fits naturally in a capability model: the reply right is itself a capability
  that can be inspected, saved, and potentially delegated
- In KeyKOS, resume keys can be passed to a third domain (tail-call forwarding),
  enabling transparent call-chain composition without the intermediary needing
  to own a persistent endpoint
- In seL4 with `GrantReply` right: the server can include a derived capability
  in the reply payload, and the `GrantReply` right on the reply cap authorizes
  this

**Opaque token (QNX rcvid):**

- Lower overhead: no kernel object allocated, no cap table entry
- Practical for non-capability systems; fits existing POSIX/Unix mental models
- Cannot be delegated, transferred, or used as a general capability
- Scoped strictly to the server process that received the message

**Implicit thread state (L4.re):**

- Lowest overhead: no object, no token, no allocation
- Simplest server loop: the reply destination is implicit in the call context
- Deferred reply requires a different mechanism (save the thread ID and handle
  it externally); not idiomatic in L4.re

### Fast-path coupling

Systems where the reply cap names the blocked caller thread **directly** (seL4,
L4.re, KeyKOS) can fully bypass endpoint queue operations at reply time. The
kernel dispatches directly from the reply object to the `BlockedOnReply`/blocked
thread state.

Systems where the reply goes through a kernel queue (Mach, Zircon) incur queue
insertion and dequeueing at reply time, regardless of whether the receiver is
already waiting. Mach's `MACH_SEND_MSG | MACH_RCV_MSG` combined call reduces
syscall entry overhead but does not eliminate the queue path.

The fast-path coupling is also sensitive to atomic reply+receive operations.
seL4's `seL4_ReplyRecv` and L4.re's `l4_ipc_reply_and_wait` combine the reply
send with the next receive in a single syscall, eliminating one kernel entry
compared to separate `Reply` and `Recv` calls. This is architecturally possible
only when the reply object is ephemeral (consumed in the atomic operation) — a
persistent reply port (Mach) cannot be atomically consumed and replaced in the
same operation.

---

## Unsettled in the Literature

- Whether the **MCS Reply object** design (explicit allocation, MCS-aware) or
  the **classic implicit reply slot** design is preferable for non-MCS kernels
  is not settled. The seL4 community introduced MCS Reply objects to enable
  scheduling context donation, not primarily for ergonomic reasons. Some
  developers consider the SaveCaller pattern in classic seL4 acceptable.
- The cost of **reply port pre-allocation** in Mach-style designs vs. the cost
  of kernel-side object creation in seL4-style designs has not been
  authoritatively benchmarked in isolation. The total IPC benchmark numbers
  reflect the full pipeline.
- **Tail-call composition** (passing a resume key to a third domain as in
  KeyKOS) has no equivalent in seL4 or Mach, and its practical value in modern
  microkernel architectures is not widely assessed in recent literature.

---

## References

1. seL4 Reference Manual, version 14.0.0.
   https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
2. seL4 IPC tutorial. https://docs.sel4.systems/Tutorials/ipc.html
3. seL4 MCS pre-release release notes (10.1.1-mcs).
   https://docs.sel4.systems/releases/sel4/10.1.1-mcs.html
4. seL4 MCS tutorial. https://docs.sel4.systems/Tutorials/mcs.html
5. RFC-13: MCS GrantReply discussion.
   https://lists.sel4.systems/hyperkitty/list/rfc@sel4.systems/thread/GLQEEHS4SDQEIMEGXSO5AYBHWXR5QCPH/
6. seL4 fastpath source.
   https://github.com/seL4/seL4/blob/master/src/fastpath/fastpath.c
7. seL4 whitepaper. https://sel4.systems/About/seL4-whitepaper.pdf
8. Elphinstone & Heiser, "From L3 to seL4: What Have We Learnt in 20 Years of L4
   Microkernels?" SOSP 2013.
   https://sigops.org/s/conferences/sosp/2013/papers/p133-elphinstone.pdf
9. Bomberger et al., "The KeyKOS Nanokernel Architecture," USENIX Workshop on
   Microkernels and Other Kernel Architectures, 1992.
   https://css.csail.mit.edu/6.5660/2017/readings/keykos.pdf
10. Shapiro et al., "EROS: A Fast Capability System," SOSP 1999.
    https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf
11. Differences Between Coyotos and EROS.
    http://www.cap-lore.com/CapTheory/KK/Shap/eros-comparison.html
12. Apple Kernel Programming Guide — Mach Messaging.
    https://docs.huihoo.com/darwin/kernel-programming-guide/boundaries/chapter_14_section_4.html
13. GNU Mach Reference Manual — IPC.
    http://gnu.ist.utl.pt/software/hurd/gnumach-doc/mach_4.html
14. mach_port_allocate manual page.
    https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/mach_port_allocate.html
15. L4Re IPC concepts. https://l4re.org/doc/l4re_concepts_ipc.html
16. L4Re Object Invocation API. https://l4re.org/doc/group__l4__ipc__api.html
17. QNX Neutrino IPC Architecture — Synchronous Message Passing.
    https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc.html
18. QNX MsgSend() reference.
    https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.lib_ref/topic/m/msgsend.html
19. Du et al., "Boosting Inter-Process Communication with Architectural
    Support," TOCS 2022.
    https://ipads.se.sjtu.edu.cn/_media/publications/2022_-_a_-_tocs_-_xpc.pdf
