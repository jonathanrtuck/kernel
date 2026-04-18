# 017 — Badge semantics: minter-assigned, mint-right-controlled, opt-in lifecycle tracking

**Date:** 2026-04-18 **Starting point:** D15 settled unidirectional,
many-to-many endpoints with send/receive rights, and listed badge semantics as
the first downstream open question: "Per-capability identifier attached by
kernel to messages. Enables receiver to identify sender source. Encoding,
assignment, and badge-closure notifications (the peer disconnection answer)."
D11 had deferred badge-related revocation add-ons jointly with IPC; D13 + D15
now enable this exploration.

---

## The question

What are badge semantics — representation, assignment, and lifecycle visibility?

Three facets treated as one design decision:

1. **Representation/encoding** — what is a badge, where does it live?
2. **Assignment** — who chooses the badge value?
3. **Lifecycle visibility** — does the receiver learn when a badged capability
   is closed? (The peer disconnection detection gap from D15.)

---

## Derived constraints (not choices)

Five things follow mechanically from settled decisions before badge-specific
choices are made:

**1. Badge is a per-cap field in D8's entry layout.** D15 creates many-to-one
patterns (server inbox, fault handler, worker pool). The receiver needs to
identify which sender sent a message. The only per-sender metadata the kernel
can attach is per-capability — the capability is the sender's credential. D8's
flat table entry is the natural location:
`(object pointer, rights mask, badge, slot tag)`. The badge is on the referrer
(the cap), not the referent (the endpoint). This follows from the requirement:
different senders to the same endpoint carry different badges. If badge were on
the endpoint, all senders would carry the same value — equivalent to the
endpoint's identity, which the receiver already knows.

**2. Badge delivery is hot-path; badge management is cold-path.** D1 (per-core
hot path): every send reads the badge from the sender's cap-table entry and
attaches it to the message. The badge field must share a cache line with the
object pointer and rights mask to avoid an extra cache miss. Badge management
(clone-with-badge, close-with-refcount-check) is cold-path — close is
infrequent, clone is creation-time.

**3. Badge is unforgeable and immutable.** D4 (designation = authority): if a
sender could forge a badge, it could impersonate another sender — confused
deputy at the IPC layer. Immutability follows from identification: if the sender
could change its badge, the receiver's key-into-state model breaks. The kernel
enforces both: the sender cannot read, choose, or modify the badge at send time.

**4. Fault handler badge is structurally required.** D12 (fault delegation) +
D13 (all delivery through endpoints): the kernel synthesizes fault messages and
enqueues them to the handler's endpoint. The IPC path reads badges from the
sender's cap — but the kernel has no cap-table entry for fault delivery. The
Observer must store a badge alongside its fault handler endpoint reference.
Whoever installs the handler supplies both. (Note: whether the fault handler
reference itself is a cap-table entry or a kernel-internal reference is an open
question — see "What remains open" below. If it is a cap-table entry, the badge
is simply part of that entry. If it is a kernel-internal reference, the badge is
a sibling field on the Observer struct.)

**5. Badge-closure notifications, if adopted, must use the endpoint queue.** D13
commits to one delivery mechanism. Badge-closure notifications are messages to
the endpoint's receive side. They go through the queue, consuming a slot,
subject to the same overflow policy as any other message.

---

## Badge assignment: who chooses the value?

### Design space

Three candidates:

**Minter-assigned.** The entity calling `clone(handle, rights, badge)` chooses
the value. The kernel enforces mechanism only: immutability after clone,
unforgeability at send time, attachment to messages. What badges mean is
userspace policy.

**Kernel-auto-assigned.** The kernel assigns a unique value at clone time. The
minter cannot choose.

**Hybrid.** Minter chooses or requests auto. Two modes coexist.

### Derivation

D15's many-to-one patterns create two use cases for badges:

- **Distinguishing** — "message A came from a different sender than message B."
  Any unique value suffices.
- **Identifying** — "this message came from client #42, the one with account
  state at row 42 of my table."

Both use cases exist under A3 (generic workloads). But identification is the
load-bearing case: a fault handler needs to map faults to specific children, a
server needs to key into per-client state. Without identification, every
receiver that needs per-source state must maintain a `badge → identity` side
table — the badge saves a lookup only if its value IS the lookup key.

Identification requires that the badge value correspond to state the receiver
already has. The receiver's internal state structure determines what values are
useful. Therefore the receiver (or its delegate) must control the badge value.

Kernel-auto-assigned guarantees uniqueness but produces opaque values. Every
receiver with per-source state needs a translation layer. The badge
distinguishes but does not identify — the mechanism serves the less-demanding
use case while imposing overhead on the more-demanding one.

Hybrid combines both but introduces two badge semantics on the same endpoint
(minter-chosen vs. kernel-chosen). Collision risk between the two namespaces
requires partitioning or a distinguishing bit. Complexity with no structural
benefit over minter-assigned alone — minters who want uniqueness can generate
their own unique values.

**Decision: minter-assigned.** The minter chooses the badge value. The kernel
enforces mechanism; the minter controls semantics.

### Mint right

A secondary question: who is authorized to mint badged copies?

D4 (designation = authority): the authority to perform an operation should be
capability-mediated. D8 (rights mask): send and receive are already independent
bits. Badge assignment during clone is a third independent authority — not every
holder of a send cap should be able to mint new badged copies.

**Decision: mint is a third independent right in D8's rights mask (send,
receive, mint).** The endpoint creator controls who gets the mint right. A
client receives (send) only. A trusted nameserver receives (send, mint). The
receiver typically holds (send, receive, mint) or at minimum (receive, mint).

This achieves two things:

1. **D4 consistency.** Who can assign badges is capability-mediated, not
   ambient.
2. **Budget alignment.** Badge tracking (if opt-in per-badge tracking is chosen
   below) adds per-badge state to the endpoint. The endpoint creator controls
   who can grow that state by controlling mint-right distribution. The creator
   accepts the budget consequences of delegation — the same model as queue
   capacity, where the creator funds the queue and senders fill it.

---

## Lifecycle visibility: badge-closure notifications

### Design space

Four candidates for whether the receiver learns when badged capabilities are
closed:

**L1: No lifecycle visibility.** Badges are identification-only. The receiver
never learns about sender-side close events. Peer disconnection detection falls
to userspace: timeouts, heartbeats, explicit disconnect messages.

**L2: Per-badge closure notifications (always on).** Kernel tracks a refcount
per (endpoint, badge) pair. When the last send cap with badge B is closed, the
kernel enqueues a closure notification.

**L3: Per-endpoint last-sender-closed.** Kernel tracks total send-cap refcount.
Notification fires when all senders are gone.

**L4: Opt-in per-badge tracking.** The endpoint creator specifies at creation
whether per-badge tracking is enabled.

### Derivation

**A3 (generic):** Not all workloads need disconnection detection. Stateless
services, 1:1 endpoints, bulk data transfer — badges serve identification but
lifecycle tracking adds no value. No single workload pattern justifies forcing
the cost on all endpoints.

**A4 (purely reactive):** For workloads that DO need disconnection detection
(session-oriented servers, fault handlers managing children, resource managers),
polling-based detection (timeouts, heartbeats) requires periodic work with no
triggering event — inconsistent with the reactive philosophy. Kernel-side
event-driven notification, delivered through the endpoint queue (D13), is
A4-consistent.

**A5 (applied fractally):** Disconnection detection is the same essential
pattern in every server that needs it — the receiver wants to know when a
client's capabilities are gone so it can clean up per-client state. Each server
reimplements the same logic. The kernel could absorb this once. But because it's
not universal (A3), absorbing it unconditionally would add complexity for
workloads that don't benefit — a cost without a corresponding simplification of
the userspace interface.

**L1 rejected:** For workloads that need it, L1 forces every server to
reimplement disconnection detection in userspace. The polling-based alternatives
violate A4's reactive spirit — they require periodic timer-driven work. L1
leaves D15's peer disconnection gap permanently open.

**L2 rejected:** L2 imposes per-badge tracking on every endpoint, including
those that don't need it. The per-badge map grows with distinct badge count
(unbounded under A3), adds per-close overhead, and creates structural tensions:

- D16 send-once caps are auto-consumed on use. If consumption triggers
  badge-closure, every RPC reply generates a spurious closure notification.
- D14 Observer destroy closes all held caps, generating a burst of closure
  notifications across multiple endpoints.
- D13 bounded queue: N simultaneous client disconnections = N notifications
  competing with real messages for queue space.

These tensions are manageable (send-once exemption, queue capacity planning) but
imposed on every endpoint regardless of need.

**L3 considered but insufficient:** L3's "all senders gone" signal is useful for
endpoint cleanup but does not identify which client disconnected. It does not
solve D15's per-client peer disconnection gap. Minimal cost, minimal benefit.

**L4 chosen:** The receiver who wants closure notifications opts in at endpoint
creation and pays for the per-badge map. Receivers who don't need it pay nothing
— their endpoints are fixed-size objects with trivial close paths. The mint
right (above) gives the creator control over who can grow the badge population.

**Decision: opt-in per-badge tracking (L4).** Endpoint creation takes a flag (or
capacity parameter) specifying whether per-badge tracking is enabled. With
tracking: per-badge refcount map, closure notifications on last-close. Without
tracking: no per-badge state, no notifications.

### Tensions accepted (for tracked endpoints)

- **T1 (D16 send-once):** Send-once caps consumed by use must NOT trigger
  badge-closure (redundant — the reply already arrived). Send-once caps closed
  WITHOUT use SHOULD trigger badge-closure (informative — the reply will never
  come). This requires the kernel to distinguish consumed-by-use from
  closed-without-use. Deferred to send-once encoding details.
- **T2 (D13 bounded queue):** Closure notifications compete for queue space.
  Interacts with the still-open overflow policy question.
- **T3 (per-badge map size):** Bounded by max-badge-count at creation (like
  queue capacity) and controlled by mint-right distribution.
- **T4 (reverse information flow):** Receiver observes sender's local close.
  Accepted: the receiver minted the badge and opted into tracking — the
  information channel is deliberately constructed, not leaked.
- **T5 (D14 destroy cascade):** Observer destroy may generate up to M closure
  checks. Interacts with D11's open destroy cleanup protocol.

---

## The decision (summary)

**Badges are minter-assigned, per-capability identifiers. A mint right controls
who can assign badges. Lifecycle visibility is opt-in per-badge tracking at
endpoint creation.**

1. **Badge is a per-cap field** in D8's entry:
   `(object pointer, rights mask, badge, slot tag)`. Immutable after clone.
   Unforgeable. Kernel-attached to messages on send.

2. **Minter-assigned.** The minter chooses the badge value via
   `clone(handle, rights, badge)`. The kernel enforces mechanism only.
   Identification (key into receiver state) is the load-bearing use case,
   requiring receiver-controlled values.

3. **Mint right.** Third independent right in D8's rights mask: send, receive,
   mint. Controls who can assign badges when cloning. Endpoint creator controls
   mint-right distribution.

4. **Opt-in per-badge tracking.** Endpoint creation flag enables per-badge
   refcount tracking. When enabled: last close of all send caps with badge B
   triggers a closure notification to the receive side (through the endpoint
   queue, D13). When disabled: no per-badge state, no notifications.

5. **Badge size** deferred to implementation (64-bit natural default on ARM64).

---

## Archive convergence

The archive (restart-1) explored badges in journal/010 (message shape) and
journal/012 (badge assignment). The archive settled:

- Minter-assigned, receiver-identifying, per-cap (archive/012)
- Badge as `(object_ref, rights, badge)` in cap entry (archive/010)
- Fault path stores `(endpoint_ref, badge)` (archive/012)

The current chain arrives at the same conclusions for representation and
assignment via D15 → identification need → receiver-controlled values →
minter-assigned. The mint right is new — the archive did not explore
capability-mediated badge assignment authority. Opt-in per-badge lifecycle
tracking is entirely new — the archive left lifecycle visibility unaddressed.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Badge representation (a u64 field) is
implementation; Rust can implement any assignment or tracking model.

**A2 (ARM64)** is not load-bearing. ARM64's 64-bit register width suggests a
natural badge size, but that's an implementation detail. Badge semantics don't
pass through A2.

---

## What remains open

- **Badge size.** 64-bit default; implementation detail. Resolve during
  implementation.
- **Send-once exemption.** Consumed-by-use vs. closed-without-use — the kernel
  must distinguish these for tracked endpoints. Encoding detail deferred with
  D16's send-once right encoding.
- **Badge on D16 kernel-created send-once caps.** Call() creates a send-once cap
  to the caller's reply endpoint. What badge does the kernel assign? Interacts
  with whether the caller uses a shared reply endpoint with badge
  disambiguation.
- **Max-badge-count / capacity semantics.** For tracked endpoints: pre-allocated
  map with creation-time capacity? Fault on overflow (D8 table-full pattern)?
- **Fault handler representation.** Whether the fault handler reference is a
  cap-table entry or a kernel-internal reference determines whether
  badge-closure covers child Observer destruction. If cap-table entry: child
  destruction closes the cap, triggering badge-closure on the handler's
  endpoint. If kernel-internal: badge-closure doesn't cover it. This is a
  connection between two open questions (fault handler attachment and badge
  lifecycle).
- **Badge-closure message format.** Contents of the closure notification
  message.
- **Badge-closure × overflow policy.** Notifications compete with regular
  messages for bounded queue space. Interacts with the still-open overflow
  policy.
- **Per-badge tracking × coalescing.** D13's coalescing tension (capacity-1
  overwrite + shared endpoint + multiple sources = cross-source data loss) may
  interact with per-badge tracking — per-badge slots could serve as both
  tracking infrastructure and coalescing mechanism. Connection noted; not
  explored here.

---

## Audit note (2026-04-18)

Flagged by independent audit: the distinguish/identify distinction (load-bearing
for minter-assigned) mirrors archive/012's cognitive structure. "A5 (applied
fractally)" conflates the axiom with the philosophy principle without citation.
Three uncited assumptions about workload patterns. Independent re-derivation
(archive removed from tree) confirmed all three conclusions: minter-assigned
(from D15 identification need + receiver-controlled values), mint right (from D4
authority model), opt-in tracking (from A3/A4 tension). The conclusions stand;
the original reasoning path for badge assignment follows the archive's cognitive
structure without full independence.
