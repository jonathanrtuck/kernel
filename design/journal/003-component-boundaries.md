# Component Boundaries at Level 1 — 2026-04-10

Third exploration at Level 1. Focused on finding where the interfaces
actually are by stress-testing each proposed boundary.

## Starting point

From journal 002, the kernel has three output types:

1. Update kernel state
2. Deliver a message to a Context
3. Choose which Context to resume

And five candidate components: Space allocator, Time allocator, Space
manager, Scheduler, Communication (shape TBD).

## Method: boundary stress-testing

For each proposed component boundary, ask:

- Is the interface simpler than the implementation it hides?
- Does it have more than one client?
- Would inlining it lose anything?

If a boundary fails these tests, it's not earning its existence as a
Level 1 component. It might still be real code — just an internal
detail, visible at Level 2 rather than Level 1.

## Findings

### The reactor

The kernel is purely reactive (from journal 001). Exception delivery
is the entry point. The code that decodes an exception and decides what
to do is the spine of the kernel. Named it the **reactor**: the
exception handler that dispatches to leaf nodes.

The reactor does: decode exception, resolve names (for IPC), update
the Context model, delegate to Space manager and Scheduler.

### "Update kernel state" is not a component

The reactor updating fields on Context records can't be separated into
an independent component. The interface to such a component would be
at least as complex as the raw field updates, and it has exactly one
client (the reactor). No abstraction earned. Helper functions, sure.
Component, no.

### Messaging and scheduling converge

Key observation: messaging and scheduling are structurally the same
activity.

- Messaging = pick a receiver + update some state (payload)
- Scheduling = pick a resumer + update some state (registers)

Both operate on the Context model. The reactor updates Context state
(marking a Context as runnable, setting pending message data). The
scheduler reads that state and picks who runs next.

With register-sized messages (seL4 model), the message payload is just
fields in the Context record — the same data structure the scheduler
already reads. No separate mechanism needed.

Communication is NOT a separate component. It's what the reactor does
when it updates the model and calls pick().

### The scheduler is anonymous

The reactor resolves names — it maps "send to X" to a specific Context
in the model. The scheduler never receives or resolves identifiers. It
sees an array of Contexts with properties (runnable/blocked, priority,
time budget, pending messages) and picks based on those properties.

This means: the naming scheme (capabilities, integer IDs, whatever) is
entirely the reactor's concern. The scheduler is decoupled from it.
You could change how Contexts name each other without touching the
scheduler.

The scheduler is also highly testable in isolation: feed it Context
records with various properties, verify it picks correctly.

### Data-structure-as-interface

The interface between the reactor and the scheduler is not a function
call graph — it's the Context model itself. The reactor writes to the
model. The scheduler reads the model. This is closer to a blackboard
architecture than a call graph.

The scheduler does have one active interface: **pick()**, which the
reactor calls after updating the model. Sometimes the timer interrupt
triggers pick() directly (preemption). Sometimes the reactor calls it
after handling a syscall (e.g., IPC send to a higher-priority Context
that should run immediately). Either way, the scheduler reads the
current model state and returns who to resume.

### Allocators are Level 2

- Space allocator: only client is the Space manager. Physical page
  tracking is an internal detail of address space management.
- Time allocator: only client is the scheduler. CPU budget accounting
  is an internal detail of scheduling.

Both allocators are real components with real logic, but they're not
visible at Level 1. They become visible when we open the Space manager
and Scheduler boxes at Level 2.

### Space manager earns its boundary

Applied the same tests. The reactor's interface to the Space manager:

- resolve_fault(context, address, fault_code)
- map / unmap / create_space / destroy_space / share

Six operations hiding substantial internal complexity (multi-level
page table walks, TLB management, physical page allocation, permission
checking). The interface is meaningfully simpler than the
implementation. Unlike "update kernel state," this is a coherent body
of complexity with a natural boundary.

The scheduler never directly touches the Space manager. Context switch
needs TTBR (page table base register), but this value sits in the
Context record — written by the Space manager, read by the scheduler.
Communication through the model, same as everything else.

## Landscape check

Compared the resulting architecture against real systems:

- **Reactor-as-spine** matches seL4/L4 actual control flow (exception
  entry -> decode -> work -> schedule -> eret). We've made it explicit
  as a named component; most kernels leave it implicit.
- **Context model as interface** is the TCB in most kernels. Making it
  the explicit interface (not just "a struct everyone pokes at") is a
  framing improvement.
- **Message/scheduling unification** matches Heiser's dictum that IPC
  is a user-controlled context switch. Arrived at independently from
  the shared-model direction.
- **Register-sized messages** is the seL4 fastpath model. Well-
  validated.

Watchpoints for future levels:

- **Priority inheritance** requires identity-aware reasoning (A waits
  on B). Compatible with anonymous scheduler if the reactor does the
  identity reasoning and updates priority properties in the model.
  Scheduler still just sees properties.
- **SMP** means multiple reactors concurrently updating the model.
  Synchronization strategy shapes the model's design. Level 2+.
- **Reactor complexity** — if it starts feeling like a god component,
  something inside it wants to be a leaf node.

## Level 1 component map (settled)

```text
              hardware exceptions
                     |
                     v
                 [ Reactor ]
                /     |     \
               v      v      v
    [ Space manager ] | [ Scheduler ]
                      |
               Context model
          (shared data structure)
```

- **Reactor** — the spine. Decodes exceptions, resolves names, updates
  the Context model, delegates to Space manager and Scheduler.
- **Space manager** — manages address spaces, programs page tables.
  Leaf node behind the reactor. Physical page allocation is internal.
- **Scheduler** — pick() -> who runs next. Programs timer. Reads the
  Context model anonymously (property-based, not identity-aware).
  Leaf node behind the reactor. Time budget accounting is internal.
- **Context model** — the shared data structure. Schema defines the
  interfaces. Reactor writes, Scheduler reads, Space manager writes
  TTBR values.

Communication is not a separate component. It is the reactor updating
the model (pending message state) and calling pick().

## Open questions carried forward

- **Context model schema** — what fields does a Context record have?
- **Naming / addressing** — how does the reactor resolve "send to X"?
- **Who receives fault messages?** — depends on Context relationships
  (flat vs. tree vs. other graph structure).
- **Context relationships** — flat collection? hierarchy? graph?
  Naming, fault routing, and schema all depend on this.
