# Object Creation and Context Handles — 2026-04-13

Thirteenth exploration. Addressed how objects come into existence, explored
whether kernel operations should use send() or dedicated syscalls, and
discovered that Context must become a fourth object type. This reverses the
journal 006 decision that "Context is not an object type."

## Starting point

The interface audit identified a cluster of missing interfaces: no creation path
for any of the three object types (Space, Time, Wormhole), no creation path for
Contexts, and no mechanism for proactive lifecycle management (kill, resume)
outside of the control Endpoint.

Three approaches to object creation were evaluated:

- **A: Creation syscalls.** Dedicated syscalls per type.
- **B: Factory capabilities.** send() to kernel-intercepted factory caps.
- **C: Subdivision of existing resources.** Creation = carving from what you
  hold.

## Approach C: creation as subdivision

Approach C aligns with conservation laws already in the design: Space is finite
and conserved, Time is a flow. Subdivision is the mechanism of conservation —
the total never changes, it just gets partitioned.

### Resource escalation via the fault chain

The naive version of subdivision puts all resources in the root Context at boot
— a god object in userspace (EL0), which is a poor security posture.

Instead: the kernel retains the physical resource pools and creates the root
Context with only what it needs. When a Context needs more resources, it faults.
The fault handler — itself a Context — either carves from its own resources or
faults upward to its handler. The chain terminates at the root Context, whose
fault handler is the kernel itself.

This produces the same hierarchical distribution as the naive approach, but the
god object is the kernel (EL1, behind the hardware trust boundary), not a
userspace Context. The root Context in userspace is not privileged — it's just
the first Context, holding only what it was given.

**Consequence: the kernel is a fault handler.** For exactly one Context (the
root), the kernel handles faults directly — interpreting resource requests,
carving from the physical pool, granting capabilities. This is the one place
where the kernel makes resource policy decisions. For all other Contexts, fault
handling is pure routing.

### Wormhole creation consumes Space

Space subdivides into Space. Time subdivides into Time. Both are same-type-in,
same-type-out.

Wormholes (the Endpoint naming — matching the Space/Time theme) are different:
they consume physical memory (queue storage, metadata) but are not themselves
Space. Creation is a cross-type transformation: spend Space to get a Wormhole.

The Context's mental model is simple: "I have Space. I want a Wormhole. Kernel,
use this Space to make me one."

```text
open_wormhole(space_handle) → wormhole_handle
```

The Space capability is consumed (or shrunk). The Wormhole appears. Conservation
holds — the physical bytes changed purpose, they didn't multiply.

Destruction reverses it:

```text
close_wormhole(wormhole_handle) → space_handle
```

The Space becomes usable again. The Wormhole is gone.

### Context creation is composition

A Context requires Space (register save area, page tables), Time (scheduling),
and a Wormhole (for fault delivery, at minimum). Creation assembles from
multiple resource types:

```text
create_context(space, time, fault_handler, ...) → context_handle
```

The exact parameters are open. The shape is: multiple resource capabilities in,
one context handle out.

## Syscalls vs. send(): two kinds of interaction

Explored whether lifecycle operations (kill, resume) should go through send() to
kernel-intercepted Endpoints (the control Endpoint model from journal 011) or be
dedicated syscalls.

### The "all send()" problem

For kill and resume to go through send(), there must be a target Endpoint. The
control Endpoint served this role. But for object creation, nothing to send to
exists yet — the object being created doesn't have an Endpoint.

This forces kernel-owned Endpoints: capabilities to Endpoints that the kernel
services. Three shapes were explored (single kernel service Endpoint,
per-operation Endpoints, per-resource Endpoints). All three are syscalls in
disguise — the Endpoint has no queue, no Context is blocked on receive(), the
kernel processes messages inline. The send() entry point is shared but the
semantics diverge completely from peer IPC.

### The distinction

The design has two genuinely different kinds of interaction:

- **Context ↔ Context.** Peer communication through Wormholes. Asynchronous,
  queued, capability-mediated. The verb is send().
- **Context → Kernel.** Requests to the kernel for privileged operations.
  Synchronous, inline, always available. The verb is a syscall.

"All send()" merges these columns. The observation: "the parent isn't telling
the child to kill — it's telling the kernel to kill the child." The kernel is
the actor, not a peer. Dressing kernel operations as IPC obscures the trust
model.

**Direction: syscalls for kernel operations, send() for peer IPC.** The two
mechanisms serve different relationships and should not pretend to be each
other.

## Context becomes a fourth object type

Dedicated kill and resume syscalls need a noun — something that designates
"Context X" and authorizes the operation. That noun is a capability: a context
handle in the caller's capability table.

```text
kill(context_handle)
resume(context_handle)
```

The handle can be cloned (delegate kill authority), attenuated (resume-only, no
kill), and closed — same operations as any other capability. Context joins
Space, Time, and Wormhole as a kernel object type.

This reverses journal 006's decision that "Context is not an object type" and
dissolves the control Endpoint from journal 011. The control Endpoint's job
(lifecycle management) is now handled by the context handle + typed syscalls.

### Why journal 006 was wrong

Journal 006 concluded that every post-creation operation on a Context could be
mediated through resource control, IPC, or endpoint indirection — so a direct
handle to the Context was unnecessary.

The critical missing case: **resume.** The faulted Context is suspended — it
can't receive messages, it can't participate in IPC. The fault handler needs a
direct mechanism to tell the kernel to change the Context's state. This is
inherently a Context → kernel operation on a specific Context, which requires a
noun that designates the target. That noun is a context handle.

Kill has the same structure: it's the kernel acting on a specific Context at the
request of a capability holder. Resource destruction (journal 006's "kill via
destroying resources") is indirect and doesn't cleanly handle all cases.

## Fault message contents

With context handles, the fault message simplifies. The handler receives:

```text
fault message:
  badge:      identifies faulter (minter-assigned at handler setup)
  type:       fault
  payload[0]: fault code
  payload[1]: faulting address
  payload[2]: flags
  payload[3]: context handle (cap transfer, resume + kill rights)
  cap_mask:   0b0001
```

The context handle is always included — even if the handler already holds one
from creating the child. The handler may receive from Contexts it didn't create
(the design allows arbitrary fault handler wiring). Receiving a redundant handle
is harmless; close() the extra.

### Resume on non-faulted Context

`resume(context_handle)` when the target Context is not in the `suspended` state
returns an error. This can happen legitimately in a race: child faults, handler
begins processing, another handle-holder resumes the child before the handler
acts. Error return lets the handler distinguish "I acted" from "someone else
already handled this."

Same semantics for `kill()` on an already-dead Context — error return. Parallel
to send() on a destroyed Wormhole.

## Control Endpoint is dissolved

The control Endpoint from journal 011 served two purposes:

1. **Fault resume** — the handler sends "resume" to the control Endpoint.
2. **Lifecycle management** — kill, potentially timing updates, handler updates.

Both are now handled by the context handle + typed syscalls. The control
Endpoint's properties (kernel-intercepted, processed inline, no queue, state
checked before acting) were symptoms of it not being a real Endpoint — it was a
syscall interface wearing an IPC costume. Making it an actual syscall is more
honest.

The `control_endpoint` field is removed from the Context model. The
`fault_handler` field remains (the kernel still needs to know where to deliver
faults).

## Updated syscall surface

```text
send(wormhole_handle, payload, cap_mask)     — peer IPC
receive(wormhole_handle) → message           — peer IPC (blocking)
clone(handle, badge) → new_handle            — capability management
close(handle)                                — capability management
destroy(handle)                              — capability management (gated)
open_wormhole(space_handle, capacity) → wormhole_handle  — object creation
close_wormhole(wormhole_handle) → space_handle           — object destruction
create_context(space, time, fault_handler, ...) → context_handle — object creation
kill(context_handle)                         — lifecycle management
resume(context_handle)                       — lifecycle management
```

Ten syscalls, up from five. The increase reflects operations that previously had
no home (creation) or were disguised as IPC (lifecycle management).

## Updated Context model

```text
Context:
  register_state      saved/restored at context switch
  ttbr                address space root (written by Space manager)
  state               runnable | blocked | suspended
  fault_handler       (wormhole_ref, badge) — where to deliver faults
  pending_message     message waiting for delivery
  ...                 (remaining fields unchanged from journal 011, minus
                       control_endpoint)
```

The `control_endpoint` field is gone. Lifecycle management goes through context
handle syscalls, not through a kernel-intercepted Endpoint.

## Updated object type table

| Object type | Shape                   | Object-rights        | Key property             |
| ----------- | ----------------------- | -------------------- | ------------------------ |
| Space       | size in bytes           | read, write, execute | Page size hidden         |
| Time        | fraction (% of core)    | —                    | Fungible, non-clonable   |
| Wormhole    | bounded queue + waiters | send, receive        | Many:many, FIFO          |
| Context     | execution state         | resume, kill         | Non-clonable (see below) |

## Explored: handle = handler unification

If context handles are non-clonable (like Time — exactly one holder), then the
handle holder and the fault handler are necessarily the same entity. This would
eliminate the `fault_handler` field entirely: the kernel delivers faults to
whoever holds the context handle.

**Attractive:** one relationship, one capability, one holder. The Context model
simplifies.

**Uncertain:** the delivery mechanism question remains. The context handle is a
noun in a capability table — the kernel can't "send to the holder." It needs a
Wormhole for fault message delivery. Options explored:

- The context handle includes a Wormhole reference for delivery (two concepts
  bundled into one cap).
- The holder specifies a Wormhole when accepting the handle.
- The context handle IS a Wormhole with extra rights (resume, kill) — but this
  reintroduces "some Wormholes are special."

**Deferred.** The simplification is real but may just move complexity from the
Context model to the capability/delivery mechanism. Needs more thought before
accepting.

## Status

**Tentatively accepted:**

- Object creation via subdivision: Space → Space, Time → Time, Space → Wormhole
- Resource escalation via fault chain; kernel as root fault handler for root
  Context
- Wormhole naming for Endpoints (spacetime theme: Space, Time, Wormhole)
- Wormhole creation/destruction: open_wormhole(space) / close_wormhole(wormhole)
- Syscalls for kernel operations, send() for peer IPC — two genuinely different
  kinds of interaction
- Context as a fourth object type with capabilities (reverses journal 006)
- Control Endpoint dissolved — replaced by context handle + typed syscalls
  (reverses journal 011)
- Fault messages include a context handle (cap transfer)
- resume() on non-faulted Context returns error (not no-op, not fault)
- kill() on already-dead Context returns error
- Ten-syscall surface: send, receive, clone, close, destroy, open_wormhole,
  close_wormhole, create_context, kill, resume

**Open questions:**

- **Handle = handler unification.** Whether the context handle holder is
  necessarily the fault handler. Deferred — attractive but delivery mechanism
  unclear.
- **Context handle clonability.** If non-clonable, reinforces handle = handler.
  If clonable, multiple holders can kill/resume independently, but fault routing
  is ambiguous. Tied to the unification question.
- **Context rights model.** Resume and kill are clear. What other rights exist?
  Inspect state? Modify timing parameters? Change fault handler? Each new right
  is a potential new syscall.
- **create_context parameters.** Exact set of resources and configuration
  required at creation time.
- **Kernel fault handling policy.** What the kernel grants the root Context on
  resource faults. Minimal policy (grant what's available) or something more
  structured.
- **Space subdivision semantics.** When Space is consumed by open_wormhole or
  create_context, does the Space handle shrink or is it fully consumed? What
  about partially consumed Space?
- **close_wormhole reclamation.** Does Space return to the original Space
  handle, or does the caller receive a new one?
