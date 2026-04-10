# Communication and Flows Exploration — 2026-04-09

Second exploration at Level 1. Shifted methodology from components-first
to flows-first. Main progress: narrowing the "Communication" open
question.

## Methodological shift: flows, not components

The previous exploration (001) identified components bottom-up from
hardware interfaces: Space allocator, Time allocator, Space manager,
Scheduler. Communication was left as an open question — a nebulous
"other" bucket that didn't fit the component pattern.

Problem: we were identifying boxes first, then looking for interfaces
between them. The philosophy says the opposite — "the architecture is
the interfaces, not the components." Trying to find the missing
component was the wrong entry point.

New approach: enumerate every input->output transformation the kernel
performs. The kernel is purely reactive; it only runs in response to
hardware exceptions. So: what are all the flows?

## Flow enumeration

Every exception, what the kernel does, what comes out:

| Input             | Kernel state change                | Output                                          |
| ----------------- | ---------------------------------- | ----------------------------------------------- |
| SVC (syscall)     | Depends on call (allocate, create, | Resume caller with return value, OR resume      |
|                   | send, configure, yield, etc.)      | different Context                               |
| Data/instruction  | If valid: allocate page, update    | If resolved: resume same Context (retries).     |
| abort             | page table. If invalid: mark       | If unresolvable: deliver fault info somewhere   |
|                   | Context faulted.                   |                                                 |
| Other sync faults | Mark Context faulted               | Deliver fault info somewhere                    |
| IRQ (device)      | ACK in GIC, update device state    | Notify the Context that cares about this device |
| Timer             | Update time accounting, run        | Resume whoever the scheduler picks              |
|                   | scheduler                          |                                                 |

Three outputs are "???": unresolvable faults, device interrupts, and
faulted-Context notification. All three have the same shape: the kernel
has information that needs to reach a Context.

## Key observation: three output types

Every kernel invocation produces some combination of:

1. **Update kernel state** — resource tables, page tables, scheduler
   queues, Context records.
2. **Deliver a message to a Context** — return values, fault info,
   interrupt notifications, IPC payloads.
3. **Choose which Context to resume** — scheduling decision, possibly
   the same Context, possibly different.

IPC (Context-to-Context messaging) enters via syscall (flow 1). The
kernel mediates it. So IPC is a sub-case of "Context does SVC, kernel
delivers a message to another Context."

## Unification: all information delivery is one mechanism

Fault delivery, interrupt forwarding, IPC, and syscall return values
are all instances of the same thing: the kernel makes data available
to a Context. The source differs (kernel itself vs. another Context),
the content differs (fault code vs. payload), but the delivery
mechanism could be identical.

No fundamental reason to have separate mechanisms. A message has:

- Source (kernel or Context ID)
- Type / metadata (enough for the recipient to prioritize)
- Payload

Faults, interrupts, and IPC are just messages with different metadata.

## Correction: kernel is the leaf node

The kernel is NOT connective tissue. From Level 0: `hardware ->
[kernel] -> userspace`. The kernel is a leaf node behind the
kernel|userspace interface. The philosophy says push complexity to the
leaves. That means push complexity INTO the kernel, keeping the
interface to Contexts simple.

Utilitarian argument: there is one kernel, written by one person.
There could be billions of Contexts written by countless people.
Essential complexity is conserved. It should live in the kernel so
every Context author benefits automatically. The right way should be
the easy way for Context authors.

This inverts the typical microkernel instinct of "keep the kernel
simple, push to userspace." The kernel is simple at its INTERFACE,
not in its INTERNALS. A leaf node can be arbitrarily complex inside
as long as its interface is clean.

## Notification model exploration

How does a Context know it has a message? Hardware constraint: the
kernel can only run during exceptions. It cannot reach into a running
Context. All "push" mechanisms pass through hardware (timer preemption,
IPI, or the Context doing a syscall).

Explored options:

- **Blocking receive (pull):** Context calls receive(), blocks until
  message. Simple for kernel, forces Contexts into event-loop
  structure. (seL4, L4, QNX primary mechanism)
- **Signal-like redirect (push):** Kernel redirects Context's PC to a
  registered handler. Complex, reentrancy risks. (Unix signals —
  notoriously error-prone)
- **Notification + pull:** Kernel sets a lightweight flag/bitmask.
  Context pulls the full message when ready. Separates "bell" from
  "letter." (seL4 notifications, QNX pulses — considered successful)

The "callback" framing: a syscall is Context->kernel RPC. A callback
is kernel->Context RPC. Same mechanism (register setup + control
transfer) through different hardware gates (SVC vs. modified eret).
Context registers a handler address; kernel "calls" it by setting up
registers and doing eret to the handler address.

Queue vs. mask vs. nest for concurrent messages: not resolved. This
is Level 2 (mechanism internals), not Level 1 (interface shape).

## Prior art for notification models

- **seL4 notifications:** bitmask word, cheap, composable with IPC.
  Considered successful.
- **QNX pulses:** small fixed-size async messages alongside regular
  messages. Same receive channel. Work well in practice.
- **Windows alertable waits:** APC fires only when thread opts in.
  Avoids reentrancy.
- **Mach notification ports:** async to a port, received via select.
- **Unix signals:** push-based, no timing control. Notoriously broken.

Common thread in successful designs: notification and payload are
separated. Notification is tiny and pushed. Payload is pulled when
the Context is ready.

## Containment hierarchy (observation, unclear if useful)

Resources (RAM, CPU) contain kernel state (lives in RAM), which
contains Contexts (records within kernel state), which reference
resources (back up to the top). The kernel is self-referential: it
uses RAM to track RAM allocation.

Interesting structural observation but unclear whether it changes any
design decision vs. restating the allocator/manager split differently.
Noted but not adopted as a framing.

## Communication is NOT a peer of Space/Time

IPC is not a component like Space manager. Space manager and Scheduler
each manage a distinct aspect of a single Context's state. Communication
is about relationships between Contexts — a different kind of thing.
The allocator/manager pattern (conserved resource + configurer) doesn't
have a natural parallel for messages, which are transient, not conserved.

Communication may be a separate component (leaf node abstracting the
delivery mechanism from the rest of the kernel) or behavior woven into
the exception handling flow. Open question, but it doesn't need to
follow the allocator/manager template.

## State of Level 1 after this exploration

Boxes identified: Space allocator, Time allocator, Space manager,
Scheduler, Communication (shape TBD).

Interfaces partially clarified:

- Kernel has three output types (state update, message delivery,
  scheduling decision)
- All information delivery to Contexts is one mechanism
- Message shape: source + type/metadata + payload (details TBD)
- Communication touches Scheduler (recipient needs CPU) and possibly
  Space manager (payload mapping)

Still open:

- Message shape (the concrete interface)
- Space manager / Scheduler boundary (entanglement at context switch)
- Communication as component vs. woven into flow
- How Contexts name each other (or whether they can)
- Who receives fault messages (not assuming supervision tree)
