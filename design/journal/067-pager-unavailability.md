# Journal 067 — Pager Unavailability: Three Failure Modes, Three Mechanisms

Settles G04. The gap decomposes into three structurally distinct failure modes,
each with a different mechanism forced by the constraint graph.

## Context

G04 has been carried forward through D12, D18, D20, D21, D24, D40, and D58
without resolution. Every derivation that touches fault handling defers "pager
unavailability protocol" as a separate question. D31 committed to fault handler
chains and explicitly foreclosed double-fault-kill as a sole strategy. The
question: what does the kernel do when the chain cannot be traversed?

The `.brain/explorations/G04-pager-unavailability/` exploration evaluated five
options spanning let-it-hang through cooperative escalation. The key finding is
that G04 resists resolution as a single design choice because it is three
independent failure modes sharing a label.

## The decomposition

The three failure modes have different constraint structures:

**Case A — handler Field destroyed.** The handler Observer is gone. D11's
destroy-invalidation cleared the cap-table entry at slot 0. The kernel detects
this at fault dispatch time (dead entry, O(1) lookup). D33 already specifies a
hook point: Field destroy walks the pending list and wakes Observers with an
error. The mechanism exists; the policy at that hook point is the open question.

**Case B — handler Field alive, receiver unresponsive.** The handler Observer
exists but is itself faulted, blocked, or otherwise not draining its Field. The
fault message sits in the pending list (D18 deferred delivery) indefinitely. No
kernel hook point exists — the kernel cannot distinguish "handler is slow" from
"handler is dead" without active detection.

**Case C — chain terminus.** The escalation chain (D31) has been fully traversed
and the fault reached the kernel (root pager) without resolution. The kernel is
the final authority.

Each case has a different "most constrained" answer.

## Case A: kernel notification at D33 hook

When the kernel detects a dead handler entry during fault dispatch, or when
D33's Field-destroy walk encounters a faulting Observer in the pending list:

1. Kernel transitions the Observer to an **error-faulted** sub-state (distinct
   from normal faulted — the handler is known-dead, not merely unresponsive)
2. If the Observer has a supervision Field configured (see below), the kernel
   sends an error notification to that Field with the Observer handle and fault
   details
3. The Observer remains in error-faulted state until a supervisor with
   appropriate rights acts (destroy, re-assign handler via change_handler, or
   resume with resolution)

No new kernel mechanism is required. D33 provides the hook. D21 provides the
detection. D39's Observer handle with rights provides the resolution authority.

### Why not automatic destroy

D4 says the kernel should not autonomously destroy Observers without
capability-authorized instruction. The faulting Observer may hold Space, Time,
and Field resources that a supervisor needs to reclaim cleanly. Automatic
destroy by the kernel bypasses the supervisor's resource-tracking and recovery
logic. The kernel notifies; the supervisor decides.

### Half-open variant

If the handler received the fault message (D18 immediate delivery) but the
handler Observer is subsequently destroyed before calling resume, the faulting
Observer remains in faulted state with no live handler. This is structurally
identical to Case A: the handler's destruction triggers D33's cascade, which
reaches the faulting Observer's handler Field. The same notification mechanism
applies. No special case needed.

## Case B: cooperative escalation + Pulsar watchdog

When the handler Field is alive but the receiver is unresponsive, the primary
mechanism is **cooperative escalation** (D31's chain model):

1. Handler A receives the fault message for Observer X
2. If Handler A cannot resolve the fault, Handler A forwards the fault
   notification to its own handler Field (Handler B) via standard IPC Send()
3. The message includes Observer X's handle cap so Handler B can act on the
   original faulting Observer
4. Chain continues until resolution or the kernel root pager (Case C)

This is exactly the D31 model for resource requests, extended to fault handling.
Each handler is responsible for the next link. The kernel does not traverse the
chain — cooperative traversal avoids the back-pointer structure that automatic
traversal would require (no current mechanism to look up "who holds receive
rights to Field H" without O(N) scan over all cap tables).

### Timeout enforcement via Pulsar watchdog

Cooperative escalation has no built-in timeout — a handler that is alive but
permanently blocked leaves the chain stuck. A3 + A4 together eliminate
kernel-internal timeout: A3 says the timeout value is workload-dependent (cannot
embed in kernel), A4 says no background scanning (the O(N) timer-path scan from
Mach's approach is the "liability inversion" problem identified by Hand et al.,
HotOS 2005).

The timeout mechanism is a **Pulsar watchdog** — a pure userspace pattern using
existing primitives:

1. Supervision Observer arms a Pulsar with a watchdog deadline
2. If the Pulsar fires before the faulting Observer is resumed, the supervision
   Observer checks Observer state via read_registers (D39)
3. If still faulted: supervisor takes recovery action (destroy, re-assign
   handler, escalate)
4. If resumed: Pulsar was spurious, no action

D44 explicitly names watchdogs as a Pulsar use case. D39 provides the
state-checking capability. No new kernel mechanism.

### Silent failure concern

The main risk of cooperative escalation: a handler that silently drops an
unresolvable fault (neither handles nor escalates) leaves the faulting Observer
stuck with no signal. This is a userspace correctness concern, not a kernel
correctness concern. The kernel's job is to provide the mechanism; whether
handlers implement the escalation protocol correctly is a userspace
responsibility under A3.

The Pulsar watchdog provides the safety net: even if a handler silently drops
the escalation, the supervision Observer's watchdog fires on deadline and
detects the stuck Observer. The combination of cooperative escalation + Pulsar
watchdog covers both the happy path (escalation works) and the failure path
(escalation silently breaks).

### Cycle detection

If handler relationships form a cycle (Observer A's handler is B, B's handler is
A), cooperative escalation loops indefinitely. This is a userspace configuration
error, not a kernel detection responsibility. The Pulsar watchdog is the
detection mechanism: the watchdog fires on deadline regardless of whether the
chain is cycling or simply stuck. The supervisor cannot distinguish cycling from
unresponsiveness, but the recovery action is the same (destroy or re-assign). No
kernel cycle detection mechanism is needed.

## Case C: kernel-autonomous destroy at chain terminus

When the escalation chain terminates at the kernel (root pager) without
resolution:

1. The kernel is the final authority — it created the chain, and the chain has
   terminated
2. The kernel destroys the faulting Observer

This is the one case where kernel-autonomous destroy is justified. The kernel's
authority as root pager includes terminal cleanup authority — this is not a D4
violation because the kernel IS the authoritative entity at the chain terminus.
The root pager's implicit authority is analogous to the root Observer's initial
resource grant (D31): the kernel bootstraps the system and is the backstop.

### Why destroy and not park

At the chain terminus, the Observer has exhausted all possible handlers. No
supervisor can act because the chain terminating at the kernel means no
higher-level supervisor exists for this Observer. Parking in error-faulted state
(as in Case A) would leave the Observer permanently stuck with no entity capable
of intervention. Destroy is the only action that returns resources to the
system.

## Supervision Field

Cases A and B require a supervision Field for notification and watchdog
delivery. This is a **creation-time configuration parameter**:
`create_observer()` gains an optional supervision Field cap alongside the
handler Field cap (D35/D21).

Optional, not mandatory. The root Observer's supervision is the kernel itself
(Case C). Leaf Observers in simple configurations (single handler, no
supervision hierarchy) may omit it — their failure mode is that Case A produces
an error-faulted Observer with no notification target, and Case B relies solely
on the escalation chain reaching a supervisor somewhere up the chain.

The supervision Field is NOT a second fault handler. It receives only
error-faulted notifications (Case A) and can receive Pulsar watchdog messages
(Case B). The fault handler at slot 0 remains the primary mechanism for all
faults.

## Escalation message format

Cooperative escalation uses standard D28-format IPC messages. A handler
forwarding a fault sends to its own handler Field with:

- The faulting Observer's handle cap (passed through from the original fault
  message)
- The original fault type and data words (forwarded verbatim)
- The handler's own badge (identifying which handler is escalating)

This is standard IPC — no new message format. The escalation protocol is a
userspace convention, not a kernel-enforced format. A userspace supervisor
library can standardize the forwarding pattern.

## Prior art

| System          | Case A (dead handler)                 | Case B (unresponsive)               | Chain terminus      |
| --------------- | ------------------------------------- | ----------------------------------- | ------------------- |
| seL4            | Thread blocks forever                 | Thread blocks forever               | N/A (no chains)     |
| L4Ka::Pistachio | Exception forwarding                  | Cooperative chain                   | Kernel kills thread |
| EROS/Coyotos    | Keeper chain escalation               | Keeper chain escalation             | System keeper       |
| Mach            | Kernel timeout (liability inversion)  | Kernel timeout                      | Kernel kills task   |
| Zircon          | Thread terminated on pager VMO orphan | Pager process supervision           | Process exit        |
| This kernel     | Supervision notification              | Cooperative chain + Pulsar watchdog | Kernel destroy      |

The combination is closest to the L4/EROS lineage (cooperative chains with a
kernel backstop), with the Pulsar watchdog being novel to this kernel's design
(no surveyed system uses a capability-held timer object for pager timeout).

## Rejected alternatives

**Kernel-internal timeout (Mach model).** A3 violation (timeout value is
workload-dependent). A4 tension (O(N) scan on timer path). The "liability
inversion" problem (Hand et al., HotOS 2005): the kernel's correct operation
depends on userspace pager responsiveness. The Pulsar watchdog achieves the same
effect without embedding policy.

**Kernel-automatic chain traversal.** Requires back-pointers from Fields to
their receive-cap holders — no current mechanism, and maintaining it adds O(1)
work per cap transfer to support a rare failure path. Cooperative traversal
avoids this entirely.

**Let-it-hang as sole strategy.** Foreclosed by D31's commitment to chains.
Appears only as the intermediate state in Case A (error-faulted until supervisor
acts), not as the terminal design.

**Double-fault-kill as sole strategy.** Foreclosed by D31. Appears only as the
terminal action at the chain terminus (Case C), not as the first response.

## Summary

| Failure mode             | Mechanism                                | Constraint forcing it                                   |
| ------------------------ | ---------------------------------------- | ------------------------------------------------------- |
| Dead handler Field (A)   | Supervision notification at D33 hook     | D33 + D21 + D18 (hook exists)                           |
| Unresponsive handler (B) | Cooperative escalation + Pulsar watchdog | D31 (chains) + A3/A4 (no kernel timeout) + D44 (Pulsar) |
| Chain terminus (C)       | Kernel-autonomous destroy                | D31 (chains terminate at kernel) + root-pager authority |

Does NOT settle: supervision Field mandatory vs. optional at creation (deferred
to Observer creation API refinement), escalation protocol standardization
(userspace convention vs. kernel-defined format — a downstream question for
userspace framework design), error-faulted sub-state encoding in D39's state
machine (implementation detail).

- **Rests on:** D31 (fault handler chains — the structural commitment that
  forecloses standalone let-it-hang and standalone double-fault-kill), D33
  (Field destroy cascade — provides the Case A hook point), D21 (handler at
  reserved slot 0 — dead cap detection is O(1)), D18 (deferred delivery —
  pending list is the fault queue), D44 (Pulsar — provides the Case B watchdog
  without kernel-internal timeout), D39 (Observer rights — read_registers
  enables state checking, change_handler enables re-assignment), D40 (fault
  resolution — Observer handle in fault message enables chain forwarding), D11
  (destroy-invalidation — dead Field detection is automatic), A3 (generic — no
  embedded timeout policy), A4 (purely reactive — no background scanning), A5
  (kernel absorbs detection and notification; userspace provides policy), D4
  (designation = authority — kernel-autonomous destroy only at chain terminus
  where kernel IS the authority),
  `.brain/explorations/G04-pager-unavailability/`.
- **Status:** settled. Closes G04. Revisit if downstream userspace framework
  design reveals that cooperative escalation's silent-failure mode is
  structurally unacceptable (would re-open kernel-automatic traversal with
  back-pointers), or if the Pulsar watchdog pattern proves too complex for
  practical supervision hierarchies (would re-open kernel-internal timeout with
  A3 tension acknowledged).
- **Journal:** `journal/067-pager-unavailability.md`.
