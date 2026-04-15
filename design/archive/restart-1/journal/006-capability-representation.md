# Capability Representation — 2026-04-11

Sixth exploration, second Level 2 question. What are capabilities concretely,
and what operations does the kernel provide on them?

## Starting point

From journal 004, capabilities are settled as the naming mechanism: holding a
capability IS the name AND the authority. The reactor resolves capabilities, not
names. No global namespace in the kernel.

Open question from spec.md: "How are capabilities stored and resolved?
Per-Context table, CNode graph, or simpler?"

## Prerequisite: what IS a capability

Before choosing a representation, needed to establish the precise semantics.

### Not a shared secret

Initial mental model was: the kernel maps integers to permissions, a Context
"knows" an integer, and sharing means telling someone else the integer. This is
a **password/token** system, not a capability system. The critical distinction:

In a password system, the integer is a **global name** — anyone who learns it
has the power. Brute-forcing or guessing the integer grants authority. The
authority lives in the integer.

In a capability system, the integers are **per-Context**. Handle 3 in Context A
and handle 3 in Context B are unrelated — they refer to different things (or
nothing). The authority lives in the **kernel's bookkeeping**, not in the
integer itself. This is Dennis and Van Horn's 1966 insight and the structural
prevention of confused-deputy attacks.

### A capability is (object, rights)

A capability maps to an **object** plus a **set of permitted operations**, not
to a specific action. The Context uses the handle in a syscall to perform an
operation; the kernel checks the rights.

```text
handle 3 → (memory object Z, rights: {read, write, map})

map(handle_3, va=0x4000)   → kernel checks "map" in rights → proceeds
write(handle_3, ...)        → kernel checks "write" in rights → proceeds
destroy(handle_3)           → kernel checks "destroy" in rights → denied
```

The capability doesn't encode what you will do — it encodes what you're allowed
to operate on and how. The specific action comes from the syscall.

### Handles are opaque

A Context does not know what object a handle points to. It knows handle 3, and
it knows what handle 3 is _for_ — but only because of context (who gave it,
when, and accompanying convention). The kernel doesn't explain "this is a memory
object." Some systems provide introspection (Zircon's `zx_object_get_info`), but
the handle itself is purely a local name.

Contexts acquire handles in two ways:

- **At creation** — the creator provides initial capabilities. The Context is
  born holding handles 0, 1, 2... with their meaning established by convention.
- **Via IPC** — a message includes a capability transfer. The receiving Context
  gets a new handle and the message metadata tells it what to use it for.

## Decomposing the operations

Explored the standard "mint" operation (clone + attenuate rights, fused into one
syscall) and found it decomposes into two orthogonal primitives.

### Clone and attenuate as separate primitives

**Clone:** create a copy of a capability in the same or different Context's
table. Both the original and the copy exist independently.

**Attenuate:** reduce the rights on an existing handle. Irreversible — rights
can only narrow, never widen (monotonic attenuation). The kernel enforces this.

```text
Before:  handle 3 → (obj Z, {read, write})
attenuate(handle 3, remove: {write})
After:   handle 3 → (obj Z, {read})         — can't get write back
```

**Mint** is just their composition: clone, then attenuate the clone. But
decomposing reveals a useful operation: **self-attenuation**. A Context can
voluntarily drop rights it no longer needs (principle of least privilege). After
finishing writes to a memory object, attenuate to read-only. If the Context is
later compromised, it can't write.

With a fused mint, self-attenuation requires the wasteful pattern of cloning,
attenuating the clone, and closing the original.

### Transfer: move vs. copy

Two modes of giving a capability to another Context (via IPC):

- **Transfer (move):** handle removed from sender's table, added to receiver's.
  Sender loses it. Zircon's default.
- **Clone + transfer:** sender clones first to keep a copy, then transfers the
  clone. Sender retains access.

Both are kernel-mediated. The sender's handle number and the receiver's handle
number are unrelated — the kernel assigns a fresh local handle to the receiver.
Neither party learns the other's handle number.

### Value semantics

Each Context gets its own **independent** table entry. There is no shared
"capability object" that multiple Contexts reference. Closing Context A's handle
does not affect Context B's handle to the same object.

```text
A: handle 3 → (obj Z, {rw})     — A's entry
B: handle 0 → (obj Z, {r})      — B's entry, independent
```

This is like two variables holding pointers to the same heap object. The
pointers (capabilities) are independent values; the pointee (kernel object) is
shared.

### Rights introspection

No security concern in letting a Context see the rights on its own handles — it
already holds the capability, and rights are discoverable empirically (try each
operation, see what fails). Making it explicit is strictly better: less
error-prone, enables informed decisions ("do I have transfer rights?"), and
reveals no information the Context couldn't already obtain.

Decision direction: rights introspection should be available as a basic handle
operation.

## Object lifecycle

Two distinct operations that systems often conflate:

**close(handle)** — "I'm done with this." Removes the entry from the Context's
table. The underlying object continues to exist if any other Context holds a
capability to it. When the last handle across all Contexts is closed, the kernel
reclaims the object. This is **reference counting** — deterministic, not garbage
collection. Like `Arc<T>` in Rust: last drop triggers deallocation.

**destroy(handle)** — "Kill this object." The object is destroyed regardless of
who else holds capabilities to it. All other holders' handles become invalid.
Requires a `destroy` right — not every capability holder should be able to nuke
the object.

```text
close:    cooperative     — object lives as long as anyone needs it
destroy:  authoritative   — object dies, other holders get invalidated
```

### The supervision pattern

Exploring shared memory lifecycle (two Contexts sharing memory, one dies)
revealed how the existing fault-handler mechanism from journal 004 provides
recovery without additional kernel machinery:

1. Context B dies. Kernel closes B's handles (refcount decrements). Memory
   object stays alive because Context A still holds a handle.
2. Kernel delivers fault to B's fault handler (Context C).
3. C creates a new Context B'. B' is born with a fresh, empty capability table —
   it inherits nothing from the old B.
4. C re-establishes shared memory by transferring a handle to the memory object
   to B'. (This requires C to have held its own handle for this purpose.)

The pattern: supervisors should hold capabilities to resources they may need to
redistribute after a restart. The kernel provides mechanism (fault delivery,
capability transfer); userspace provides policy (whether to restart, what state
to re-establish).

This is the Erlang supervision tree reinvented from capabilities — not because
we copied Erlang, but because fault delivery + capability transfer naturally
produces it. The journal 004 observation about Erlang was noting exactly this
convergence.

## Time capabilities

Explored whether capabilities apply to CPU time the same way they apply to
memory.

### The parallel

Both Space and Time are finite pools that can be subdivided and mediated by
capabilities. At the interface level, a Context holds a handle to "some portion
of a pool." But the mechanics differ:

| Property    | Space                                    | Time                                                   |
| ----------- | ---------------------------------------- | ------------------------------------------------------ |
| Fungibility | No — specific pages are specific pages   | Yes — any nanosecond is interchangeable                |
| Persistence | Static — exists until freed              | Flows — unused time is lost                            |
| Unit        | N bytes (a quantity)                     | N time-units per time (a rate)                         |
| Sharing     | Real — simultaneous access to same pages | Illusory — interleaved, never simultaneous on one core |

The structural parallel holds (both are finite, subdivisible, capability-
mediatable). The mechanics diverge. Budgets, replenishment, and enforcement
algorithms are contingent — they are not part of this exploration.

### Attenuation vs. subdivision

These are different operations, easy to conflate:

- **Attenuate** changes **rights** (what operations are permitted). Same object,
  fewer rights.
- **Subdivide** changes **the resource** (how much there is). Creates new
  smaller objects.

Both apply to Space. Subdivision likely applies to Time. Attenuation applies to
Time only if there are meaningful object-level rights — and for Time, object-
level rights may be trivial (just "use").

### Time cannot be cloned

A structural consequence of fungibility: cloning a Time capability would
double-count the bandwidth. Two Contexts holding capabilities to the same Time
object would both claim the same allocation. This is overcommitment, not
sharing.

Space can be cloned because sharing is real — two Contexts can simultaneously
access the same physical pages. Time cannot — only one Context runs on a core at
a time. The valid operations diverge by type:

| Operation       | Memory        | Time                | Endpoint        |
| --------------- | ------------- | ------------------- | --------------- |
| Transfer (move) | Yes           | Yes                 | Yes             |
| Clone (copy)    | Yes — sharing | No — overcommitment | Yes             |
| Subdivide       | Yes — split   | Yes — split         | No              |
| Attenuate       | Yes — r/w/x   | Probably trivial    | Yes — send-only |

### Two layers of rights — observation, not decision

Examining Time capabilities revealed a possible decomposition:

- **Object-rights:** what can I do with the referent? Type-specific. For memory:
  read, write, execute. For time: use (possibly just that). For an endpoint:
  send.
- **Handle-rights:** what can I do with this capability itself? Potentially
  universal across types. Transfer, clone, attenuate, subdivide.

Time capabilities feel impoverished when looking for object-rights (there's only
"use"). But they have the same handle-rights as any other capability — transfer,
subdivide. The interesting dimension for Time is how the _allocation_ is
managed, not how the _time_ is used.

However, the clone restriction on Time shows that handle-rights are not fully
universal — clone is valid for Memory and Endpoints but not for Time. This means
handle-rights either depend on object type or the kernel refuses certain
operations based on the object's nature. The two-layer decomposition gets
messier.

**Status: observation only.** Whether this decomposition earns its place in the
design depends on the concrete rights model. One more reason to leave it as an
observation rather than a commitment.

## Sync/async unification via Time transfer

If Time capabilities exist and are transferable, synchronous and asynchronous
IPC are not separate kernel mechanisms — they are capability management patterns
on top of the same primitive.

### The mechanism

The kernel provides one operation: transfer a capability (including Time) as
part of a message. The Context's choice determines the calling pattern:

**"Sync" pattern.** Context A transfers all of its Time capability to Server S.
A has no Time, becomes unschedulable — effectively blocked. S works using A's
Time (charged to A). S transfers Time back to A in its reply. A becomes
schedulable again.

**"Async" pattern.** Context A subdivides its Time capability, transfers a
portion to Server S, keeps the remainder. A continues running. S works using the
portion. S may transfer Time back when done, or not — A is running either way.

**Fan-out pattern.** Context A subdivides its Time three ways. Transfers
portions to S1 and S2, keeps a portion. A, S1, and S2 all run concurrently. As
each server replies with Time, A's allocation grows. No scatter/gather mechanism
needed — just capability transfer.

The kernel doesn't know or care which pattern a Context uses. It transfers
capabilities and schedules Contexts that hold Time capabilities. Sync, async,
and fan-out parallelism all fall out of the same primitive.

### Lend is not necessary

Initial instinct was that Time transfer during IPC needs a "lend" mode with
kernel-enforced return. But the trust model is identical to traditional sync
IPC:

- Traditional: A calls S. If S never replies, A is blocked forever. A trusts S
  to reply.
- Time transfer: A transfers Time to S. If S never returns Time, A is
  unschedulable forever. A trusts S to return Time.

Same risk, same trust. "Lend" would be the kernel enforcing something the trust
model already handles. If you don't trust S, don't give it all your Time —
subdivide and keep some. That's the async pattern, and it's the natural defense
against untrustworthy servers.

### Implications

This eliminates "sync vs. async IPC" as a kernel interface decision. The kernel
provides capability transfer. Whether the call is synchronous or asynchronous is
the Context's choice, expressed through how it manages its Time capability.

The crosstalk accounting problem (research/context-relationships.md §5) is
solved as a side effect: Time spent in a server on behalf of a client is
naturally charged to the client, because the server is running on the client's
Time capability.

### Open: reply routing

When a server finishes and wants to return Time to the caller, it needs a way to
reach the caller. This is a message-shape question — reply capabilities, badges,
reply_to, or some other mechanism. The Time transfer model doesn't depend on the
specific reply mechanism, only that one exists. This is a downstream concern for
when message shape is explored.

## Context lifecycle and object types

### Context is not an object type

Explored whether Contexts need to be a kernel object type (i.e., something
capabilities can point to). Examined what post-creation operations a creator
needs to perform on a Context:

| Need                   | Already covered by                                             |
| ---------------------- | -------------------------------------------------------------- |
| Grant new capabilities | IPC — send message with capability transfer                    |
| Communicate            | IPC — send to endpoint                                         |
| Change supervisor      | Endpoint indirection — new listener, same endpoint             |
| Handle faults          | Listen on fault endpoint                                       |
| Limit resources        | Resource control — attenuate/subdivide before creation         |
| Kill                   | Destroy child's resources (forced fault) or don't handle fault |

No operation requires a direct handle to the Context itself. Everything is
mediated through resource control (Time, Memory), IPC (messages with capability
transfers), or endpoint indirection (fault handling, supervisor replacement).

Decision direction: **Context is not an object type.** A Context is the emergent
result of assembling Memory + Time + Endpoint capabilities. Lifecycle management
operates on the constituent resources, not on the Context directly.

### Creator control via subdivision

The creator chooses the level of control it retains:

- **Subdivide and transfer portion:** creator keeps the original resource
  handles with destroy rights. Can revoke by destroying the child's resource
  objects → forces a fault → fault chain terminates → kernel cleans up.
  Effectively "kill."
- **Transfer the whole thing:** creator gives up control. Context is fully
  independent. Creator can still decline to handle faults (non-action kill via
  fault chain termination), but cannot forcibly reclaim resources.

### Zombie prevention

A blocked Context whose endpoint loses all external references (refcount → 0)
will never be unblocked. The kernel detects this and delivers a fault: "endpoint
destroyed while waiting." Normal fault chain activates. If nobody handles →
Context dies → kernel cleans up (closes all handles, reclaims Context model
entry).

This is a mechanical consequence of reference counting, not a new mechanism. A
Context that IS reachable (endpoint refcount > 0) but hasn't received a message
yet is a legitimate waiter, not a zombie.

### Object types — tentative minimum set

Three kernel object types, each mapping to a Level 1 component:

| Object type     | Represents                       | Level 1 component |
| --------------- | -------------------------------- | ----------------- |
| Memory object   | A region of physical storage     | Space manager     |
| Time allocation | A portion of CPU bandwidth       | Scheduler         |
| Endpoint        | A communication rendezvous point | Reactor           |

Contexts are not objects — they are assemblies of these three, plus a Context
model entry managed by the kernel. Interrupts and faults require no new types:
they are messages delivered to endpoints (consistent with "all information
delivery is one mechanism" from journal 002).

## State of Level 2 (capability interface)

**Tentatively accepted:**

- Capabilities are per-Context table entries: (object reference, rights mask)
- Handles are opaque per-Context integers — not global, not guessable
- Transfer is kernel-mediated, via IPC
- Clone and attenuate as separate primitives; mint is their composition
- Clone is not valid for Time capabilities (would overcommit bandwidth)
- Attenuation is irreversible (monotonic rights narrowing)
- Rights are introspectable by the holding Context
- Object lifecycle: close (reference-counted reclamation) as the default,
  destroy (authoritative, gated by a right) as a separate operation
- Sync/async IPC is a Context-level pattern, not a kernel mechanism — the kernel
  provides capability transfer (including Time); Contexts choose whether to
  transfer all Time (sync) or a subdivision (async)
- Context is not a kernel object type — lifecycle managed through resource
  control, IPC, and endpoint indirection
- Three object types: Memory, Time, Endpoint
- Zombie prevention via endpoint refcount detection → forced fault → chain
  termination → cleanup

**Open questions (Level 2 and 3):**

- **Representation.** Flat array per Context? Hash map? Generation counters for
  ABA prevention on handle reuse? This is Level 3, behind the interface.
- **Rights model.** What rights exist per object type? Does the object-rights
  vs. handle-rights decomposition hold, given that clone validity is
  type-dependent?
- **Badges.** Do receivers need to distinguish which capability a sender used?
  seL4's badge mechanism (server identifies clients by an integer tag on their
  capability) is useful. Orthogonal to representation, but shapes the interface.
- **Reply routing.** How does a server reply to its caller? Reply capabilities,
  badges, explicit reply_to handle — mechanism TBD when message shape is
  explored.
- **Revocation scope.** Close-only (pure refcount) is simpler. Destroy is the
  escape hatch. Is selective revocation (revoke delegated copies without
  destroying the object) needed? Would require derivation tracking.
- **Whether destroy is needed at this level.** Close + refcount may be
  sufficient. Destroy solves the "leaked capability keeps object alive" problem
  but adds invalidation complexity. Destroy is currently used in the "kill via
  resource destruction" pattern — if removed, that pattern needs a replacement.
