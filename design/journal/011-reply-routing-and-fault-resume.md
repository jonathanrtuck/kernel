# Reply Routing and Fault Resume — 2026-04-12

Eleventh exploration. Settled how a Context responds to a message — whether that
message was an IPC request from another Context or a fault notification from the
kernel. Unified the sender's interface while accepting kernel-internal
divergence on the receiver side.

## Starting point

Journal 010 left four options open for reply routing and fault resume:

- **A:** Explicit reply Endpoint (client transfers send cap in request)
- **B:** One-shot reply capability (kernel auto-creates)
- **C:** Per-Context control Endpoint (kernel creates management Endpoint)
- **D:** Badge-based reply (`reply(endpoint, badge)`)

Both IPC reply and fault resume are the same structural question: "how do you
respond to a specific message?" (Journal 010.)

## Ruling out B and D

**B is out.** A one-shot auto-created capability is a new kernel primitive that
doesn't compose from existing mechanisms. If existing mechanisms can't express
reply, the right response is to understand why — not to add a special case.

**D deferred.** Badge-based reply introduces a new syscall pattern and couples
reply routing to badges. Worth revisiting if A+C proves insufficient, but not
explored further here.

## Are faults and IPC the same operation?

Initial instinct: yes, they should be. Both are "something happened involving
Context X, and now Context Y needs to send a response."

But the two cases diverge on the receiver side:

- **IPC reply.** The client called receive() and is voluntarily blocked, waiting
  for a message. Delivering the reply wakes it up. Normal Endpoint semantics.

- **Fault resume.** The faulted Context was executing normal code and hit an
  exception. It never called receive(). It's involuntarily suspended by the
  kernel. Only the kernel can change its state from suspended to runnable and
  restart it at the faulting instruction.

The receiver side is genuinely different. But from the **sender's** perspective,
both are identical: "I received a message with a reply capability in one of the
payload slots. I send my response there." The sender doesn't know or care
whether the other side is a Context blocked on receive() or the kernel managing
a suspended Context.

**Key insight:** the important consistency is on the userspace side. The
receivers' perspectives differ, but in one case the receiver is a Context and in
the other it's the kernel — their implementations will differ anyway. Hide the
complexity in the kernel (leaf node). The Endpoint interface absorbs the
variation.

Prior art confirms the split. Systems with synchronous IPC (seL4, L4) unify
faults and IPC because the kernel can make the faulting thread appear to have
called a synchronous send+receive. Systems without synchronous IPC (Zircon) use
a separate resume syscall. This kernel's async Endpoints prevent the seL4
approach, but the sender-side unification achieves the same external simplicity.

## The reply mechanism

**Option A for both cases.** The reply capability is included in the original
message as one of the payload cap slots.

- **IPC:** The client includes a send cap to its own reply Endpoint in the
  request. The server sends the response there.
- **Fault:** The kernel includes a send cap to the faulted Context's control
  Endpoint in the fault message. The handler sends "resume" (or "kill") there.

From the handler's perspective, both are: "slot N has a reply cap, I send my
response to it." The handler dispatches on the message type field (IPC vs.
fault) to know what response to construct, but the delivery mechanism is
identical.

## Control Endpoint

Each Context has a control Endpoint, created by the kernel at Context creation
time. It serves as the kernel-side interface for operations on that Context.

The control Endpoint is structurally an Endpoint — capabilities to it have the
same representation (object reference + rights + badge), and the sender uses the
same send() syscall. But the kernel is the consumer, not a Context:

- Messages are processed inline during the sender's send() syscall, not queued.
- The kernel interprets the message payload (slot 0 as opcode: resume, kill,
  update timing parameters, etc.).
- The kernel checks Context state before acting (e.g., "resume" on a non-faulted
  Context is a no-op or error, not queued for later).

**No queue.** A persistent per-Context Endpoint with a queue would allow stale
messages to accumulate. A "resume" sent while the Context is running could
auto-fire on a future fault the handler hasn't evaluated. Processing inline with
state checking eliminates this class of bug.

**From the sender's perspective, this is just send().** The sender doesn't know
the message is processed inline rather than queued. The Endpoint interface
contract is preserved: send() either succeeds or doesn't. What happens on the
other side is never the sender's business.

### Per-Context vs. per-fault Endpoint

Explored whether the kernel should create a fresh Endpoint per fault instead of
a persistent per-Context Endpoint.

Per-fault Endpoints are not unbounded — a Context can have at most one
outstanding fault (it's suspended and can't fault again until resumed). The
maximum count equals the number of Contexts, same as per-Context. Per-fault
Endpoints may even have tighter lifetimes: cleaned up when the fault is
resolved, the faulted Context dies, or the handler dies.

Per-Context is favored on **performance grounds**: page faults happen frequently
(demand paging, copy-on-write), and per-fault allocation adds churn to the
kernel's hot path. A per-Context Endpoint is allocated once and reused.

The interface is identical either way — the handler receives a reply cap in the
fault message. This is an implementation detail behind the Endpoint interface,
deferrable to implementation time.

## Resolved: Context management without Context-as-object-type

The control Endpoint resolves the journal 006 tension about Context lifecycle
management. The concern was: if Context is not an object type, how does a
manager kill, resume, or reconfigure a Context?

Answer: through its control Endpoint. The creator of a Context receives a send
capability to its control Endpoint. Operations on a Context are messages to that
Endpoint. No new object type — the capability points to an Endpoint (established
type), and the kernel provides the semantics.

This preserves the journal 007 decision: Context is emergent from Memory + Time

- Endpoint, not a fourth object type.

## Updated Context model

The Context model gains one field:

```text
Context:
  register_state      saved/restored at context switch
  ttbr                address space root (written by Space manager)
  state               runnable | blocked | suspended
  current_core        which core is running this Context (if any)
  fault_handler       direct Endpoint ref (kernel-internal, not a handle)
  control_endpoint    kernel-internal Endpoint (processed inline on send)
  time_handle         capability handle (Context-manipulable)
  timing_mode         periodic(d,dt,p,pt) | responsive(d,dt,l,lt) | bulk
  pending_message     message waiting for delivery
  capability_table    pointer to per-Context handle table
```

State now has three values: `runnable` (can be scheduled), `blocked` (waiting on
receive()), `suspended` (involuntarily stopped by the kernel due to a fault).
This was implicit before — the distinction between blocked and suspended is
exactly the IPC/fault divergence.

## Open questions

- **Control Endpoint opcodes.** "Resume" and "kill" are clear. What else
  belongs? Update timing parameters? Modify fault handler? The set of operations
  defines the Context lifecycle management surface. Deferrable — add opcodes as
  needed.

- **Badge assignment.** Still open from journal 010. The control Endpoint
  capability needs a badge for the handler to distinguish which Context faulted.
  Minter-assigned (the Context creator sets the badge when cloning the control
  Endpoint send cap) is the natural fit.

- **Error reporting on control Endpoint.** If the handler sends an invalid
  opcode or "resume" to a non-faulted Context, what happens? Silent drop, error
  message back to the sender, or fault the sender? Probably just a return value
  from send() — but send() on a normal Endpoint doesn't have rich return values.
  Minor design point, deferrable.

## Status

**Settled:**

- Reply routing uses Option A: reply capability included in the message payload
- IPC reply: client-provided reply Endpoint (send cap transferred in request)
- Fault resume: kernel-provided control Endpoint (send cap delivered in fault
  message)
- From the sender's perspective, both are identical: send() to a reply cap
- Control Endpoint: per-Context, created at Context creation, kernel-intercepted
  (processed inline, no queue)
- Control Endpoint also serves as Context lifecycle management interface
  (resume, kill, potentially more)
- Context state is three-valued: runnable, blocked, suspended
- Per-Context Endpoint preferred over per-fault on performance grounds (avoids
  hot-path allocation), but the interface is identical either way

**Open:** control Endpoint opcode set, badge assignment, error reporting
semantics.
