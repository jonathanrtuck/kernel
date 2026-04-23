# 045 — Field split: badge-range routing with fallback-on-destroy

**Date:** 2026-04-22

## Starting point

D22 introduced two field operations — split and combine — as consequences of the
interrupt delegation model, but deferred their semantics. Two open questions in
spec.md:

- **Field split semantics.** Does the parent field retain a reference for
  automatic return on destroy (crash recovery)? Does split generalize to
  badge-range partitioning for IPC sources?
- **Field combine semantics.** What happens to existing send caps on the
  originals? Transparent forwarding, dead handles (D11), or explicit migration?

## Exploration

### The tunnel model

The derivation arrived at a physical metaphor: a Field is a tunnel. Messages
(cars) enter through entrances (send caps), follow internal routing signs
(badge- range rules), and exit at the appropriate receiver's exit point.

- **Entrances** are send caps. Managing entrances uses existing capability
  operations: mint creates more (D17), clone copies (D23 pattern), close
  removes. No new mechanism needed on the send side.
- **Exits** are receive endpoints. Currently each Field has one exit (one queue,
  one waiters list). Split creates additional exits with routing rules.
- **Senders are oblivious to splits.** A sender holds a send cap to the Field
  (the tunnel). Their cap designates the Field object; nothing changes for them
  when the receive side is restructured. This follows from D4 (designation =
  authority) and D15 (topology emergent from capability distribution — senders
  don't know or care about the receive side).

### Split creates a separate Field object

Split produces a new, independent Field object (backed by the caller's Space,
D32). The kernel installs a routing rule on the source Field: messages with
badges matching a condition are deposited on the destination Field instead of
the source's queue.

The destination can be:

- **A new Field** (split-to-new): caller provides a source Field cap + Space cap
  - condition. Kernel creates a new Field, installs the routing rule, returns a
    cap to the new Field.
- **An existing Field** (split-to-existing): caller provides a source Field
  cap + destination Field cap + condition. Kernel installs a routing rule on the
  source pointing to the existing destination.

Both variants use the same kernel mechanism: a routing rule on the source Field.
The only difference is whether the destination is created or pre-existing.

Split-to-existing dissolves the "combine" question (see below) and enables a key
use case: a driver that wants interrupts and IPC on one Field. The root handler
splits IRQs from the root interrupt Field to the driver's existing IPC Field.
The driver receives both client requests and interrupts on one Field,
distinguished by badge. No multi-wait needed.

### Why a separate object (not internal sub-queues)

An alternative model was considered: split adds internal sub-queues to a single
Field, with each sub-queue independently receivable. This was rejected for one
primary reason:

**Independent lifecycle.** The canonical use case (D22 IRQ delegation) requires
the split-off portion to be independently destroyable. A driver crashes; its
portion is destroyed; the parent's traffic is unaffected. If split only added
internal sub-queues, destroying the driver's portion would require either
destroying the entire Field (unacceptable) or inventing sub-object lifecycle (no
precedent in the kernel's object model).

With separate Field objects: the destination Field has standard D11 lifecycle.
Destroying it removes the routing rule on the source. All existing kernel
concepts apply without modification.

Additional issues with internal sub-queues that reinforced the separate-object
choice:

- Caps would designate sub-components rather than objects (new kind of
  designation, D4/D8 tension).
- D13 direct-switch must match per-sub-queue (implementation complexity without
  architectural benefit over separate objects).

### Routing is composable

Routing rules compose naturally across Field boundaries. When the kernel
deposits a routed message on the destination Field, the message enters the
destination's routing evaluation — before the destination's own routing rules.
If the destination has its own splits, those apply.

Example: root handler splits [32–128] to bus driver. Bus driver splits [32–64]
to device driver. A message with badge 40 entering the root Field routes to the
bus driver's Field, which routes to the device driver's Field.

Each Field only knows its own routing rules. The chain resolves naturally.

**Performance optimization:** The kernel can flatten the chain into a direct
lookup on the source Field — a materialized view mapping badge ranges directly
to leaf Fields. One O(log N) binary search over non-overlapping ranges, no chain
traversal. When a child is further split, the ancestor's flattened table is
updated. This parallels D24 (page tables as materialized views of cap state).
The flattened table is a kernel-internal optimization, not an object-model
commitment.

### Fallback-on-destroy

When a destination Field is destroyed, the routing rule on the source is
removed. Messages that were routing to the destroyed Field now fall through to
the source's own queue. This is automatic — no explicit "return" operation
needed.

This resolves D22's crash recovery question cleanly: if a driver crashes and its
Field is destroyed, the routed traffic falls back to the parent's queue. The
parent's receiver sees those messages again.

The fallback is always to the immediate source — the Field that holds the
routing rule. In nested splits, destroying a grandchild causes fallback to the
child (not the root). Destroying the child causes fallback to the root. If the
child is destroyed first, the grandchild becomes orphaned — it exists but
receives no new traffic, because the routing rule that directed traffic to it
lived on the child. The grandchild is alive but silent, same as any Field with
no senders.

Recovery from orphaned Fields is a userspace concern. The supervisor detects the
crash (badge-closure notification on a tracked Field, D17) and either destroys
the orphaned Field or re-establishes routing from a higher-level Field.

No parent→child reference is needed in the kernel beyond the routing rule
itself. When the destination is destroyed, the source's routing rule is cleaned
up as part of the destination's destruction (the routing rule holds a
kernel-internal reference to the destination, contributing to its refcount; when
the destination is destroyed, the reference is removed).

### Combine dissolves

"Combine" as a separate primitive is not needed. Both use cases decompose into
existing operations:

- **Reversing a split:** destroy the destination Field. The routing rule is
  removed, traffic falls back to the source's queue.
- **Merging unrelated Fields:** use split-to-existing to route all traffic from
  Field A to Field B. Field A now receives nothing on its own queue. Destroy
  Field A if desired.

No new kernel mechanism for combine. This follows "find the abstraction that
absorbs the edge cases" — split-to-existing plus destroy covers every combine
scenario.

### Per-send routing cost

Every send to a split Field incurs a badge-range lookup to determine the routing
destination. Unsplit Fields skip the check entirely (null routing table →
existing fast path unchanged).

For split Fields, the routing table is a sorted array of non-overlapping badge
ranges. Binary search gives O(log N) comparisons where N is the number of splits
on that specific Field. Cost estimates on the D13 fast path (~400 cycles ARM64):

- 5 splits: ~3 comparisons, ~10 cycles (~2.5%)
- 20 splits: ~5 comparisons, ~15 cycles (~3.7%)
- 100 splits: ~7 comparisons, ~20 cycles (~5%)

The pathological workload — a single Field split 100+ times with high-frequency
IPC — is architecturally unusual. The common case (IRQ Fields split a handful of
times, IPC Fields rarely or never split) is negligible.

The direct-switch fast path (D13) must follow routing before attempting the
optimization: determine the destination Field, then check that Field's waiters
list. A receiver waiting on Field F cannot fast-switch for a message routed to
child Field F'.

### Authority

- **Source Field:** receive cap with a split right (modifying what happens to
  incoming messages on the receive side).
- **Destination Field (split-to-existing):** send cap (adding a message source —
  same pattern as D44 Pulsar, where creation takes a delivery Field cap).
- **Space (split-to-new):** Space cap consumed for the new Field's backing
  (D32).

### What was rejected

**Transparent forwarding (redirect tables on every send).** Hot-path cost on ALL
sends to a Field, not just split Fields. Redirect chains with unbounded depth.
Foreclosed by D1.

**Non-destructive combine (port sets / aggregator object).** A second IPC
mechanism. Foreclosed by D13 (one mechanism). D19 already rejected port sets.

**Internal sub-queues (one Field, multiple receive endpoints).** No independent
lifecycle for sub-queues. New sub-object designation concept (D4/D8 tension).
All the complexity of separate objects without the architectural benefits.

**No general split (IRQ-only).** Considered but unnecessary as a restriction.
The badge-range routing mechanism works identically for IRQs and IPC — the
kernel checks the badge and routes. The sender's identity (kernel for IRQs,
Observer for IPC) doesn't affect the routing logic. Restricting to IRQ-only
would add a type check on every split ("is this Field an IRQ Field?") without
reducing implementation complexity.

**Automatic parent→child reference for crash recovery.** New kernel-internal
relationship type with no precedent (D41 Spaces don't have parent references,
D34 cascade doesn't return to parent). The routing-rule-as-refcount model
provides fallback-on-destroy without introducing parent tracking.

## Archive convergence

The archive does not contain a concept of field split or combine. The archive's
interrupt model routes through a supervision tree (claims.toml: "Faults and
interrupts are both events routed by the supervision tree"). The current design
routes through field topology instead. No convergence or divergence to check —
the archive didn't reach this question.

## The decision

**Field split** is a typed kernel operation (D7) that installs a badge-range
routing rule on a source Field, directing matching messages to a destination
Field. The destination is either a newly created Field (backed by caller's
Space) or an existing Field.

- Split is a receive-side operation. Senders are oblivious.
- The destination is a separate Field object with standard lifecycle (D11).
- Routing is composable across Field boundaries. The kernel may flatten chains.
- Fallback-on-destroy: when the destination is destroyed, the routing rule is
  removed and traffic returns to the source's queue.
- Per-send cost: O(log N) on split Fields; zero on unsplit Fields.

**Combine does not exist as a separate primitive.** It decomposes into split
(split-to-existing) plus destroy.

**Entrance management** (send-side) uses existing capability operations. No new
mechanism.

## What remains open (one level down)

- Badge condition form: range, bitmask, or arbitrary predicate
- Whether split-to-new and split-to-existing are one syscall or two
- Field rights mask: split right details, complete Field rights set
- Queued messages at split time: leave in source queue or move to destination
- D17 badge-closure tracking: partitioning behavior on split (does tracking
  state for routed badges move to the destination?)
- Routing table structure: sorted array, bitmap, or other (kernel-internal)
- Flattened routing table update protocol on nested split/destroy

## Status

**Settled.** Field split is badge-range routing with fallback-on-destroy.
Combine dissolves into split + destroy.

Revisit if D15 is revised (changes the Field shape or many-to-many model), if
D13 is revised (changes the IPC mechanism that split routes through), if D1 is
revised (changes the hot-path constraint that shapes routing cost analysis), if
D22 is revised (changes the interrupt model that motivated split), or if a
downstream derivation reveals that per-send routing cost is unacceptable for a
structurally required workload pattern.
