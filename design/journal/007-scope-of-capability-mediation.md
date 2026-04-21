# Scope of Capability Mediation — 2026-04-16

Seventh exploration. Addressed whether every kernel operation goes through
capability invocation (unified model) or whether the kernel has separate
mechanisms for IPC and kernel operations (split model).

## Starting point

D4 settles capability-based authority. The open question listed in spec.md:
"everything through capability invocation (seL4/EROS — universal invoke,
capability type determines operation) vs. resources through capabilities with
operations as direct syscalls (Zircon-style)." This is the highest-leverage open
question — nearly every other open question's answer space narrows once this is
settled.

## The design space

Three positions on the spectrum:

**Option A — Pure unified (seL4/EROS).** One mechanism:
`invoke(cap, method, args)` for everything. The capability type determines the
operation. IPC and kernel operations use the same entry point. 3–12 syscalls.

**Option B — Split: dedicated IPC + typed kernel operations.** Peer IPC has its
own mechanism (send/receive). Kernel operations are typed syscalls taking
capability handles. Two mechanism families reflecting two different
relationships. 8–20 syscalls.

**Option C — Full fragmented (Zircon).** Every operation is a separate typed
syscall. Large surface. 50–170+ syscalls.

## Exploration

### A4 and the trust-model asymmetry

A4 (purely reactive) means the kernel is the exception handler, not a server. It
runs because hardware forced an exception, not because someone sent it a
message. The Observer→Kernel relationship is fundamentally asymmetric: the
kernel always handles, always synchronously, always in EL1, and cannot choose
not to respond. Observer→Observer IPC is a peer relationship — the receiver
chooses when to wait, and the interaction may be synchronous or asynchronous.

The unified model smooths over this asymmetry — both Observer→Kernel and
Observer→Observer look like "invoke a capability." The split model preserves it
— syscalls and IPC are different mechanisms reflecting different trust
relationships.

The archive (restart-1, journal 013) reached the same observation from a
different angle: "the parent isn't telling the child to kill — it's telling the
kernel to kill the child. The kernel is the actor, not a peer. Dressing kernel
operations as IPC obscures the trust model."

Applying "design boundaries that match the shape" (philosophy): the
Observer→Kernel and Observer→Observer relationships are genuinely different in
trust level, synchrony, and mechanism. The split model's two mechanism families
match this shape. The unified model's single mechanism hides it.

### D4's "designation = authority" is orthogonal

D4 says there is no separate naming step — the capability IS the name and
carries the permitted operations. This is about the confused deputy problem, not
about how operations are named. Both models satisfy D4: in both, you present a
capability to designate and authorize. The operation naming (embedded in the
capability type vs. encoded in the syscall number) is orthogonal to D4's
concern.

### D1 and hot-path dispatch

In the unified model, the hot path is: enter kernel → resolve capability handle
→ read capability type → dispatch on type × method. The kernel must look up the
capability before it knows what operation was requested.

In the split model, the hot path is: enter kernel → decode syscall number →
dispatch → validate capability. The kernel knows the operation from the syscall
number (in a register) before touching the capability table.

The split model has a structurally shorter hot path by one indirection. The
difference is small (one table lookup), but D1's philosophy is about keeping the
hot path minimal. seL4 mitigates with fast-path conditions, but the fast-path
check itself is an additional conditional that the split model doesn't need.

### IPC model coupling

The syscall landscape research (§10.2–10.3) shows that the IPC mechanism and the
interaction model are coupled:

- Synchronous register-based IPC → naturally small syscall surface → unified
  model's natural habitat (seL4, EROS, L4).
- Asynchronous buffered IPC → kernel-managed queues, buffer lifecycle,
  multiplexing mechanisms → split model's natural habitat (Zircon, Mach).

Async IPC introduces substantial kernel-managed state (message queues, channel
lifecycle, multiplexing) that is different in kind from resource operations like
"create Observer" or "map memory." The split model's two mechanism families
align naturally with this behavioral difference: IPC operations may block,
queue, or multiplex; kernel resource operations are always synchronous. A
unified model strains when these behavioral profiles diverge — the uniform
`invoke` syntax hides real differences in blocking behavior, memory allocation,
and multiplexing.

The designer confirmed a lean toward async IPC, which reinforces the split
model: the behavioral domains (messaging vs. resource ops) map naturally to two
mechanism families rather than one polymorphic entry point.

### A5 and interface surface

A5 says the kernel presents a simple interface. The unified model has fewer
entry points (3–12) but each is polymorphic. The split model has more entry
points (8–20) but each does exactly one thing. The full fragmented model
(50–170+) has the strongest A5 tension — largest interface, largest verification
surface, largest attack surface.

A5 is not load-bearing in distinguishing unified from split. Both can be
"simple" — they're different kinds of simple (interface-count vs.
per-entry-point clarity). A5 IS load-bearing in rejecting full fragmentation:
170+ syscalls is a large interface that bakes many assumptions into the kernel
boundary.

### Transparent interposition

The unified model provides transparent interposition: a child Observer calling
`invoke(cap, ...)` cannot distinguish whether the cap points to a real kernel
object or to a proxy field. In the split model, a child making a typed kernel
syscall goes directly to the kernel — a proxy cannot transparently intercept it.

This was examined against practical use cases:

- **Containers/jails:** Capability restriction (don't give the child caps to
  resources outside the container). Works identically in both models.
  Transparent interposition not needed.
- **Full VMs:** ARM64 EL2 hardware virtualization. Orthogonal to the syscall
  model entirely.
- **Paravirtualization:** Guest explicitly uses IPC to VMM. Both models.
- **Syscall tracing/debugging:** Kernel-level tracing mechanism. Both models.
- **Service interposition:** Most interposable operations in a microkernel are
  userspace services accessed via IPC. Transparent interposition at the IPC
  level works in both models.

The scenarios requiring transparent interposition specifically at the
kernel-operation level (not the IPC level) are either served by hardware (EL2),
by capability restriction (containers), or by kernel-level mechanisms (tracing).
The split model's limitation here is theoretical rather than practically
blocking.

### Formal verification

Initially considered as a cost of the split model (unified has fewer entry
points to verify). On examination, this does not hold: the unified model's
polymorphic dispatch requires reasoning about every type × method combination
regardless, plus the dispatch mechanism itself. The split model's per-syscall
proofs are more modular by construction. seL4 succeeded with unified, proving it
is possible — not that it is easier. Total verification work covers the same
operation space in both models.

### Option C rejection

Full fragmentation (Zircon, 170+ syscalls) was rejected on A5 grounds: the
interface surface is large, the verification surface is large, and the attack
surface is large. The syscall-landscape research notes: "the large syscall
surface creates a significant attack surface." The archive rejected this
position for the same reasons.

### Archive convergence

The archive (restart-1, journal 013) independently reached Option B from a
different starting point (the "all send()" problem with object creation, where
nothing exists to send to). The current chain arrives at the same position from
different reasoning: A4 trust-model alignment, D1 hot-path dispatch, and IPC
model coupling. Two independent paths converging — applying "when independent
paths converge, trust the convergence" (philosophy).

## Non-load-bearing axioms

- **A1 (Rust)** is not load-bearing here. Rust's type system accommodates both
  unified (trait-based polymorphic dispatch) and split (typed function calls)
  naturally. A1 constrains the implementation, not the interaction model.
- **A2 (ARM64)** is not load-bearing. The SVC exception mechanism works
  identically for both models. A2 constrains the hardware interface, not the
  syscall design.
- **A3 (generic)** is load-bearing only in rejecting Option C: a large fixed
  syscall table bakes workload assumptions. A3 does not distinguish A from B.

## What this derivation does NOT settle

- **IPC model.** Synchronous vs. asynchronous, message format, channel
  structure. The split model is compatible with both but couples naturally with
  async.
- **Specific syscall surface.** The exact set of syscalls, their names, and
  their signatures. The archive's 10-syscall design is a data point, not a
  commitment.
- **Notification mechanism.** Lightweight async signals — how they fit into the
  IPC family.
- **Capability transfer mechanism.** Whether cap transfer piggybacks on IPC
  messages or is a separate kernel operation.
- **Fast-path design.** The IPC fast path's specific shape under D1.

## Status

**Accepted as `spec.md#D7` — settled.**

Split interaction model: dedicated IPC mechanism for Observer↔Observer
communication, typed kernel operation syscalls for Observer→Kernel resource
operations. Full fragmentation (Zircon-style) rejected.

Revisit if: (1) a downstream derivation reveals that the IPC/kernel-op boundary
cannot be drawn principally — if too many operations are ambiguous, the split
degrades into two mechanisms plus special cases, which is worse than either pure
option; (2) a practical use case surfaces that requires transparent
interposition at the kernel-operation level and cannot be served by EL2
hardware, capability restriction, or kernel-level mechanisms.
