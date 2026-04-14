# Context Relationships — 2026-04-10

Fourth exploration at Level 1. Addressed the remaining open questions: Context
relationships, naming, fault routing, and resource accounting. Research
document: `design/research/context-relationships.md`.

## Starting point

From journal 003, four open questions — all interconnected via one root
question: what is the relationship structure between Contexts?

## Naming and authority are the same question

Explored whether knowing a Context's name and being able to communicate with it
are the same thing or different things.

In capability systems, they're the same: holding a capability IS the name AND
the authority. You can't name what you can't access.

In ACL systems, they're separate: you can know a name without having permission
(you know `/etc/shadow` exists, but can't read it). This is the confused deputy
problem — naming and authority are decoupled, so a deputy can be tricked into
using its own authority on an attacker's behalf.

Key nuance: capability systems support _communicating with a Context you don't
identify_. You hold a capability to an endpoint; you don't know who's listening.
This is the service abstraction — authority without identity knowledge. The
split isn't "name without authority" vs. "authority without name" — it's
"identity knowledge" vs. "communication authority."

**Decision direction: capability-based naming.** Capabilities bundle designation
with authority. The reactor resolves capabilities, not names. No global
namespace in the kernel.

## Relationship structure: allow shape, don't enforce it

Explored the spectrum from the research:

- **Enforce a shape (Genode/Zircon):** kernel requires a tree. Rigid —
  reparenting impossible, quota fragmentation, latency in cross-subtree
  communication, trust concentrated at top.
- **No shape (seL4):** kernel provides disconnected primitives. Userspace must
  build all structure. Ecosystem fragmentation — every deployer reinvents
  process management.
- **Allow shape without enforcing one:** kernel provides mechanism for structure
  (capabilities) and makes it natural to build structure, but doesn't require a
  topology.

The third position avoids both failure modes. The capability graph IS the
relationship graph, but the kernel doesn't inspect or constrain its shape. A
tree, a flat pool, a DAG — all valid wirings.

The "kernel is a leaf node" philosophy (journal 002) pushes toward this
position: push complexity into the kernel, keep the interface simple. Enforcing
a tree makes the interface complex (structural requirements). Providing
disconnected primitives pushes relationship complexity out to every Context
author. The middle path — the kernel absorbs naming, fault routing, and
relationship mechanics behind a simple interface, without constraining topology
— is where the leaf-node principle and the evidence converge.

### Distinction: structural vs. interface requirements

seL4 has minimal interface requirements (you can create a thread without a fault
handler). Genode has structural requirements (you must be someone's child). This
kernel has **strong interface requirements without structural requirements**:
creating a Context MUST provide a fault handler capability (interface
requirement), but the handler can be anyone (no structural requirement). The
interface forces completeness; the topology remains free.

## Fault routing via capability chains

A tree provides an escalation path for fault handling. But capabilities can
create an escalation path without a tree:

Each Context has a fault handler capability. The handler Context also has a
fault handler capability. This creates a chain:

```text
Context A faults
  → deliver to A's handler (Context B)
    → if B faults...
      → deliver to B's handler (Context C)
        → ...
```

The chain is wired through capability distribution, not tree position. The
kernel's mechanism: follow the fault handler capability. If the handler faults,
follow its handler. Terminal case: if the last handler has no handler (or is its
own handler), the Context dies.

This is strictly more general than a tree — a tree is one possible shape of the
capability graph. Any topology is expressible. The kernel provides mechanism
(follow the chain); userspace provides policy (the wiring).

Parallel to Erlang: supervision trees are built from language-level patterns
(supervisor behaviours), not runtime-imposed hierarchy. The runtime just
delivers exit signals. Structure is in the wiring, not the mechanism.

## Resource accounting from first principles

Examined what the design actually derives vs. what we were importing.

### What's derived

- **Space is finite and conserved.** Physical pages exist, can be free or
  mapped. When they're all committed, the next request fails. Space is a must
  for any Context — its instructions live somewhere.
- **Time is a flow.** Cores produce computation continuously. The scheduler
  directs it. Time isn't stored or handed out.
- **Each Context has some Space and receives some Time.** That's all.

### What's contingent (not derived)

- **Limits** (maximum allowable per Context) — a design choice to prevent
  exhaustion. Not inherent.
- **Accounting** (tracking consumption per Context) — a design choice for
  fairness or observability. Not inherent.
- **Budgets** (pre-allocated quantity of Time) — one possible scheduling input.
  The scheduler could use priorities, round-robin, or other properties. Not
  inherent.
- **Funding/subdivision** (creating a Context reduces creator's allocation) — a
  policy choice, not a mechanism. Not inherent.

A first-come-first-serve system with no limits is a valid design: Context asks
for a page, gets one from the free pool. Pool empty? Fail. Scheduler picks among
runnable Contexts with whatever algorithm. No budgets, no quotas, no accounting.

This doesn't mean we won't add limits, budgets, or accounting — but they must be
justified by specific problems, not assumed as given. Each contingent addition
shapes the Context model and the reactor's interface, so they shouldn't enter
the Level 1 model unless derived.

## Context model schema (first-principles minimum)

Only what's derived from the design so far:

- **Register state** — saved/restored at context switch
- **TTBR** — address space root (written by Space manager, read by Scheduler at
  context switch)
- **Runnable / blocked** — minimum scheduling property (the Scheduler needs to
  know if this Context can run)
- **Fault handler capability** — who receives this Context's faults
- **Pending message state** — for communication (source, type, payload in
  registers)

Everything else (priority, time budget, memory limit, resource accounting) is
contingent and must be justified before inclusion.

## State of Level 1

The component map is settled (journal 003). The remaining questions now have
direction:

- **Naming:** capability-based. No global namespace in the kernel.
- **Fault routing:** fault handler capability per Context, kernel follows the
  chain. No enforced hierarchy.
- **Context relationships:** the capability graph is the relationship graph. The
  kernel allows but doesn't enforce shape.
- **Resource accounting:** Space is conserved and finite. Time is a flow.
  Limits/budgets/accounting are contingent, not inherent. Whether and how to add
  them is a future design decision driven by specific problems.
- **Context model schema:** register state, TTBR, runnable/blocked, fault
  handler capability, pending message state.

## Open questions carried forward (now Level 2)

- **Capability representation.** How are capabilities stored and resolved?
  Per-Context capability table (Zircon)? CNode graph (seL4)? Something simpler?
- **Message shape.** Concrete register layout for messages (source, type,
  payload). How many registers?
- **Scheduling algorithm.** What properties does the Scheduler use beyond
  runnable/blocked? Priority, deadline, fair-share? This determines whether
  additional fields enter the Context model.
- **Space manager internals.** Page table format, allocator design.
- **SMP.** Multiple reactors, shared Context model synchronization.
- **Whether limits/budgets/accounting are needed.** And if so, at what
  granularity and who controls them.
