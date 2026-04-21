# 015 — Field shape: unidirectional, many-to-many, send/receive rights

**Date:** 2026-04-18 **Starting point:** D13 settled queued fields with
direct-switch fast path (tentative) and listed field shape as the first
downstream question: "Unidirectional vs. bidirectional. Many-to-many vs.
constrained topology." The archive chose unidirectional, many-to-many, topology
via capabilities (archive/009), but the current chain had not derived this.

---

## The question

What is the shape of a field — directionality (unidirectional vs.
bidirectional), topology constraints (many-to-many vs. constrained), and
capability-rights model (what rights, how expressed)?

---

## Derived constraints (not choices)

Four things follow mechanically from settled decisions before the shape question
is reached:

**1. Send and receive are independent capability rights.** D4 (designation =
authority) requires that send authority and receive authority be independently
grantable through the rights mask (D8). A single undifferentiated "access" right
would create confused deputy problems — an Observer with full-access
capabilities could receive messages intended for another Observer. This holds
regardless of directionality.

**2. Kernel-enforced fixed topology is foreclosed.** A3 (generic) requires
support for diverse patterns: server inbox (many:1), worker pool (many:many),
dedicated pipe (1:1), fan-out (1:many). The kernel must not hardwire a single
topology.

**3. The kernel must be a non-blocking sender in many-to-one patterns.** D12
(fault delegation) + D13 (all delivery through fields) mean the kernel enqueues
fault and interrupt messages. Multiple fault sources to one pager is a
many-to-one pattern that must work within the field mechanism.

**4. Constrained many-to-one (QNX model) is dominated.** QNX separates channels
(receive side) from connections (send side), enforcing exactly one receiver.
This restricts worker-pool patterns (multiple receivers) without reducing kernel
complexity — the kernel must enforce the restriction (added complexity) while
foreclosing patterns that A3 requires. Many-to-one is a usage pattern within
many-to-many, enforced by capability distribution rather than kernel policy.
Applying "push complexity to the leaves" fractally: the many-to-many field IS
the leaf; topology-enforcement would be connective tissue above it.

---

## The design space

After constraints eliminate the QNX model, two candidates remain.

### Option A: Unidirectional, many-to-many, send/receive as object-rights

One kernel object per field: bounded queue + waiters list. Capabilities carry
rights (send, receive, or both). Topology is emergent from capability
distribution — the kernel provides the most general mechanism, Observers choose
the pattern. Closest precedents: Mach ports, seL4 endpoints.

- Server inbox (many:1): many clients hold send caps, server holds receive cap.
- Worker pool (many:many): clone receive caps to multiple worker Observers.
- Dedicated pipe (1:1): one send cap, one receive cap.
- Request-reply: client transfers a send cap (to its reply field) in the request
  message. Server sends reply to that cap.
- Kernel-as-sender: kernel enqueues fault/interrupt messages to one field.

### Option B: Bidirectional, 1:1 paired, per-end capabilities

Two linked kernel objects per channel. Creation returns two capabilities (one
per end). Each end reads what the other wrote. Closest precedent: Zircon
channels.

- RPC: write request to your end, read response from your end. No cap transfer.
- Server pattern: one channel per client. N clients → N channels + aggregation
  object (port/wait-set) for multi-source wait.
- Kernel-as-sender: kernel holds one channel end per faulting Observer. Pager
  holds N ends + port.

---

## Analysis

### Option A against settled decisions

- **D8 (flat cap table):** standard (object, rights) entry. No structural
  exception.
- **D11 (revocation):** destroy makes all capabilities dead handles. Symmetric —
  no peer to signal.
- **D7 (split model):** creation returns one capability, like every other kernel
  object type (Space, Time, Coordinate System, Observer).
- **D12 + D13 (fault delivery):** kernel enqueues to one pager field for many
  faulting Observers. No per-source channel needed. Many-to-one is natural.
- **D13 "one mechanism":** preserved. No aggregation object needed for common
  patterns.

Cost: request-reply requires explicit reply-cap transfer per RPC. This is a
well-understood cost — seL4 (one-shot reply caps) and Mach (port rights in
messages) both pay it. A one-shot reply cap optimization (kernel mints
automatically during call, bypasses general cap transfer) can make this
near-zero on the fast path; seL4 proves this works.

### Option B against settled decisions

- **D8:** either two linked kernel objects (unusual) or direction encoding in
  handles (non-uniform entry format). Structural exception.
- **D11:** destroying one end must signal the peer. Asymmetric destroy
  semantics, unlike every other kernel object type.
- **D7:** creation returns two capabilities — the only kernel object type with
  paired creation.
- **D12 + D13:** many-to-one requires per-source channels + aggregation object.
  Zircon solves this with ports — a second kernel object type.
- **D13 "one mechanism":** weakened. Zircon has channels + ports + sockets +
  FIFOs + signals. This kernel's D13 commits to one mechanism.

Benefit: simpler RPC for the two-party case (no reply cap management), and
peer-closure detection for free (destroy one end → other end gets signal).

### Why Zircon chose bidirectional (and why it doesn't transfer)

Zircon's reasons are real: FIDL (RPC protocol generator) assumes bidirectional
channels; Fuchsia's service model is connection-oriented; developer familiarity
with socket-like semantics. But these reasons flow from Zircon's design axioms:
Zircon has kernel threads (no A4 equivalent), deliberately includes more than an
L4-family kernel (no A5 equivalent), and already pays the multi-mechanism cost
(five IPC-adjacent types). The costs Option B incurs in this kernel don't bite
in Zircon's context.

### Peer disconnection detection

The one genuine advantage of Option B that doesn't trace to Zircon-specific
axioms: when a channel end is destroyed, the peer gets a signal. Event-driven,
no polling.

In Option A, a server can't proactively detect that a client's send cap was
destroyed. Heartbeat/timeout is polling — which sits badly with A4's reactive
philosophy.

A plausible Option-A-native answer exists: badge-closure notifications. When the
last send capability with badge B is closed, the kernel enqueues a closure
notification to the field's receive side. This would be event-driven, stay
within the one-mechanism model, and track per-badge refcounts inside the field
object. This belongs in the badge-semantics exploration (D11's deferred add-on),
not here. The mechanism is not committed — only the observation that the gap is
addressable within Option A's framework.

---

## The decision

**Unidirectional, many-to-many, send/receive as object-rights (Option A).**

A field is a single kernel object: bounded queue + waiters list. Capabilities to
the same field carry different rights in the D8 rights mask: send (enqueue),
receive (dequeue), or both. Topology is emergent from capability distribution —
the kernel does not enforce sender/receiver counts. The archive's principle
applies: "allow shape, don't enforce it."

Three convergent paths:

1. **D8 + D11 structural consistency.** Option A uses D8's standard (object,
   rights) entry format and D11's symmetric destroy semantics. Option B
   introduces structural exceptions to both.

2. **D12 + D13 many-to-one composition.** Fault delivery, interrupt delivery,
   and server patterns are many-to-one. Option A handles them with one field.
   Option B requires per-source channels + aggregation — a second mechanism that
   weakens D13's "one mechanism" commitment.

3. **A3 + capability-distributed topology.** A3 requires diverse patterns.
   Option A supports all of them through one mechanism with capability-mediated
   access. Option B supports two-party patterns natively but requires additional
   kernel infrastructure for multi-party patterns.

**Costs accepted:**

- Request-reply requires explicit reply-cap transfer per RPC. This is
  well-understood (seL4, Mach) and amortizable with a one-shot reply cap
  optimization on the fast path.
- Peer disconnection detection requires a badge-closure notification mechanism
  (not yet designed). Without it, detection falls back to polling.

**Archive convergence:** The archive (journal/009) arrived at the same shape
through independent reasoning — "allow shape, don't enforce it" from the
journal/004 principle. The archive did not examine bidirectional as a rejected
alternative; the current chain fills that gap.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Rust can implement either model. A1 becomes
relevant at implementation (queue data structure, capability types).

**A2 (ARM64)** is not load-bearing. Both models run on ARM64. A2 provides the
register file for the D13 fast path but does not distinguish the models.

**A4 (purely reactive)** is not directly load-bearing for the shape decision. A4
already shaped D13 (no kernel broker). It provides background motivation for
event-driven design (peer disconnection via badge-closure rather than polling),
but the shape derivation doesn't pass through A4.

---

## What remains open

Downstream of D15, within the D13 field cluster:

- **Badge semantics.** Per-capability identifier attached by kernel to messages.
  Enables receiver to identify sender source. Encoding, assignment, and
  badge-closure notifications (the peer disconnection answer). Connected to
  D11's deferred add-ons.
- **Reply-cap mechanism.** One-shot (kernel mints during call, auto-revoked
  after reply) vs. persistent (client creates a reply field, transfers send cap
  explicitly). Affects fast-path design and cap table pressure.
- **Overflow policy.** Error to sender (archive), overwrite-oldest, fault
  sender. The coalescing tension from D13 (shared field + capacity-1 overwrite +
  multiple sources = cross-source data loss) is directly downstream.
- **Multi-field wait.** How an Observer waits on multiple fields simultaneously.
  Less urgent than for bidirectional (one field serves many sources), but still
  needed (pager with fault field + user-request field).
- **Message format.** Size, slot count, capability transfer encoding, badge
  placement.
- **Field lifecycle and naming.** Working name "field" is the lowercase
  common-term equivalent; final naming deferred with other public API names.

---

## Rejected alternatives (summary)

| Alternative                    | Rejected because                                                                                                         |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------ |
| Bidirectional, 1:1 (Zircon)    | Structural exceptions to D8+D11; requires aggregation for many-to-one (weakens D13); Zircon's reasons are axiom-specific |
| Constrained many-to-one (QNX)  | Dominated by many-to-many — adds enforcement without reducing complexity; forecloses worker-pool pattern (A3)            |
| Undifferentiated access right  | D4 foreclosed — confused deputy; send and receive are distinct authorities                                               |
| Kernel-enforced fixed topology | A3 foreclosed — diverse workload patterns require topology flexibility                                                   |
