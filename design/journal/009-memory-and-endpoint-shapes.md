# Memory and Endpoint Shapes — 2026-04-12

Ninth exploration. Settled the concrete shapes of the remaining two object
types: Memory objects and Endpoints. Time shape was resolved in journal 008.

## Memory objects

The key design decision is already settled (spec.md): page size is hidden. The
memory interface operates on byte-addressed objects, not pages. This is
genuinely novel — no surveyed system does this (landscape.md §2.7).

### Shape

```text
Memory object:
  size            bytes (not pages)
  backing         physical pages (kernel-internal, not exposed to Contexts)
```

The Memory object is just bytes. The kernel maps it to physical pages
internally. The Context never sees page granularity — the MMU's page size is an
implementation detail of the Space manager.

### Operations

- **Create:** `create_memory(size)` → handle. Kernel allocates physical pages.
- **Subdivide:** split into two smaller Memory objects. Original consumed.
  Parallels Time subdivision.
- **Map:** `map(memory_handle, va)` → Space manager programs page tables.
- **Clone:** supported (journal 006 table). Two Contexts can hold capabilities
  to the same Memory object and map it simultaneously. This is shared memory.

### Object-rights

read, write, execute. These are capability rights, not properties of the Memory
object itself. The same physical memory can be mapped read-only by one Context
and read-write by another — they hold different capabilities to the same object.

### What lives elsewhere

- Page table management → Space manager (leaf node, internal)
- Physical page allocation → Space allocator (inside Space manager)
- Address space layout → Context's decision (the kernel maps at requested VA)
- Permissions → capability rights, not Memory object fields

## Endpoints

From journal 007: queued (not rendezvous), fixed capacity, direct process switch
as fast path when receiver is waiting.

### Shape

```text
Endpoint:
  queue           bounded message queue
  waiters         Contexts blocked waiting to receive
  capacity        max queue depth (set at creation)
```

### Many-to-many, topology via capabilities

Endpoints support many senders and many receivers. Each message goes to one
receiver (first waiter in the queue, FIFO), not broadcast. The actual topology
is controlled by capability distribution, not kernel enforcement:

- **Server inbox (many:1 by usage):** many clients hold send capabilities, one
  server holds the receive capability. Don't clone the receive capability.
- **Worker pool (many:many by usage):** many clients send, many workers hold
  cloned receive capabilities. Messages distributed FIFO among waiting workers.
- **Dedicated pipe (1:1 by usage):** one sender, one receiver. Don't clone
  either capability.

This follows the journal 004 principle: allow shape, don't enforce it. The
kernel provides the most general mechanism. Contexts choose the topology.

### Object-rights

send, receive. These are the two distinct ways to use an Endpoint — enqueue a
message or dequeue one. A capability can be attenuated to send-only (for
clients) or receive-only (for workers). This parallels Memory's
read/write/execute.

### Direct process switch (fast path)

When a sender posts to an Endpoint with a waiting receiver:

```text
Receiver waiting?
  YES → direct switch, message in registers, ~400 cycles
  NO  → enqueue message, sender continues, ~1000-1500 cycles
```

The queue is the fallback. The fast path is equivalent to rendezvous speed.

### Queue overflow

When the queue is full, send returns an error. The sender decides the policy
(retry, drop, back off). Kernel provides mechanism, userspace provides policy.

Memory cost per queued message is small (register-sized payloads, ~48 bytes). A
64-deep queue is ~3KB. Overflow is a fairness/DoS concern, not a memory
exhaustion concern.

### Naming

The endpoint object needs a better name to match the Space/Time theme. Deferred
for now — the semantics are settled, the name isn't.

## Object-rights summary

Each object type has a small, natural set of rights corresponding to the
distinct ways the object can be used:

| Object type | Object-rights                                                  |
| ----------- | -------------------------------------------------------------- |
| Memory      | read, write, execute                                           |
| Time        | (none meaningful — fraction is the resource, not an operation) |
| Endpoint    | send, receive                                                  |

Time has trivial object-rights because there's only one way to "use" CPU time:
execute. The interesting rights for Time are handle-rights (transfer, subdivide,
attenuate). This was observed in journal 006.

## Updated object type table

| Object type | Shape                   | Object-rights        | Key property         |
| ----------- | ----------------------- | -------------------- | -------------------- |
| Memory      | size in bytes           | read, write, execute | Page size hidden     |
| Time        | fraction (% of core)    | —                    | Fungible, aggregates |
| Endpoint    | bounded queue + waiters | send, receive        | Many:many, FIFO      |

All three object types are now concretely defined. The remaining open questions
are about operations between them (message shape, capability transfer in
messages) rather than the objects themselves.

## Status

**Tentatively accepted:**

- Memory objects: byte-addressed, kernel-internal page backing, subdivide-able
- Memory object-rights: read, write, execute (on the capability, not the object)
- Endpoints: bounded queue, many-to-many, topology via capability distribution
- Endpoint object-rights: send, receive
- Direct process switch as Endpoint fast path
- Queue overflow returns error to sender

**Open questions carried forward:**

- **Message shape.** What goes in the Endpoint queue? Register layout,
  capability transfer encoding. This is the next major question.
- **Memory subdivision.** Exact semantics — does the original handle get
  consumed? What about mapped regions during subdivision?
- **Endpoint capacity.** Defaults, configurability, whether capacity is
  attenuatable.
- **Endpoint naming.** Something to match the Space/Time theme. Deferred.
