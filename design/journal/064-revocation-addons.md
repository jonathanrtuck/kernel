# Journal 064 — Revocation Add-Ons: Generation Counters Only (Universal)

Settles D67. Discharges D11's deferral condition.

## Context

D11 settled the base revocation primitive (close-only + authoritative destroy)
and deferred add-on mechanisms — generation-as-revocation and CDT — pending the
IPC model. The IPC model is now settled (D13–D17). D11's revisit condition is
discharged.

The base primitive handles cooperative close and authoritative destroy. It
cannot revoke all caps to an object without destroying the object, revoke one
client's cap selectively, or track transitive re-delegation.

## Gap Analysis

Three workload gaps exist beyond the base primitive:

**Gap 1 — Mass invalidation without destruction.** Field rotation (destroy +
recreate) covers this for Fields. It does NOT cover Space, Observer, Time, or
Pulsar — destroying these objects destroys the underlying resource (memory
contents, execution state, scheduling capacity, timer configuration). A server
that grants temporary access to a shared Space has no way to revoke that access
short of destroying the Space. This is a standard microkernel workload pattern
(temporary read-only access to shared memory regions with a defined expiry).

**Gap 2 — Selective revocation (one client, not all).** Badge-based semantic
rejection (D17) lets a server refuse to act on a specific client's messages, but
the client's cap remains syntactically valid — the client can still enqueue,
consuming queue slots (denial-of-service vector). Field-per-client (each client
gets a dedicated Field, D19 multi-field wait) converts selective revocation into
"destroy the client's Field." This works for IPC. For non-IPC caps (Space,
Observer), there is no field-per-client analog — but generation counters also
cannot address this (they are all-or-nothing per object). CDT would address it,
but at high cost.

**Gap 3 — Transitive delegation chains.** If A delegates to B who delegates to
C, and A wants to revoke C's access without knowing C exists, neither generation
counters nor the base primitive can do this without revoking B as well (or
destroying B's Observer). CDT would track the derivation tree and enable subtree
revocation. However, userspace conventions can discourage deep re-delegation
(Zircon's component framework does this in practice), and field-per-client +
bump-and-reissue handles the reachable cases.

## Decision: Generation Counters Only, Universal

Every kernel object carries a `generation: AtomicU64` counter. Every capability
table entry stores the generation value at time of creation or clone. On
explicit revocation: atomically increment the object's counter — O(1). On
capability use: compare the entry's stored generation against the object's live
generation; mismatch → stale cap error. Stale slots are lazily rewritten to Null
on next access (Coyotos lazy-rewrite pattern), maintaining A4 compliance.

Universal: applies to all five kernel object types (Space, Observer, Field,
Time, Pulsar) uniformly.

## Why Universal (Not Scoped)

Scoping generation counters to non-IPC types only (Space, Observer, Time,
Pulsar) was considered. Rejected for three reasons:

1. **API bifurcation.** "Revoke all caps to this object" becomes conditional on
   object type: field rotation for Fields, generation bump for everything else.
   Two mechanisms for the same semantic operation. D4 (uniform capability
   semantics) pushes against this.

2. **Field rotation is not generation-equivalent.** Field rotation destroys the
   object and its state (queued messages, wait sets). Generation bump
   invalidates caps while the object survives. A Field with queued messages that
   a server wants to preserve while rotating access tokens — plausible scenario,
   foreclosed by scoping.

3. **The optimization is ungrounded.** The per-use generation check is one
   comparison against a field likely in the same cache line as the object
   pointer already loaded for the syscall. The branch predictor correctly
   predicts "match" in the common case (revocation is rare). The cost that
   motivates scoping may be zero. Optimizing before measuring, for a cost that
   depends on a cap entry layout not yet specified, adds complexity for no
   demonstrated benefit.

## Why Not CDT

CDT (Capability Derivation Tree) would address selective revocation (gap 2) and
transitive delegation (gap 3). Rejected:

1. **Separate kernel structure.** D8's flat table forecloses CDT living inside
   the table. CDT requires intrusive linkage in kernel object structs (16 bytes
   per derivation node) and a traversal protocol that crosses object types. This
   is a new kernel data structure — tension with A5 when the gaps it addresses
   have userspace alternatives.

2. **O(N) revocation time.** CDT walk visits all derived caps. For a server with
   10,000 clients, revoking the root visits all 10,000. Requires D33-style
   preemption points (continuation state) — feasibility not demonstrated.

3. **Cross-type lock ordering.** D53 establishes pairwise arena ordering. CDT
   traversal crosses all types in one walk. The lock ordering extension is not
   defined and would require its own derivation.

4. **Cross-core cost.** Barrelfish measured ~1ms for 2-core CDT revocation. This
   kernel's IPC fast path targets ~400 cycles. CDT cross-core revocation would
   be orders of magnitude slower. (Barrelfish's architecture differs, so the
   number is not directly transferable, but the order of magnitude is
   informative.)

5. **Coyotos precedent.** The EROS → Coyotos transition explicitly replaced
   CDT-style link chains with generation counters (O(N) → O(1)). The lineage
   treats them as alternatives. No deployed system uses both.

6. **Gaps are addressable.** Selective revocation of IPC caps: field-per-client
   (D19). Selective revocation of non-IPC caps: bump generation + reissue to
   remaining clients (O(N-1) userspace work, but the server already knows its
   client set). Transitive delegation: userspace conventions discouraging deep
   re-delegation, or bump-and-reissue through the chain.

## Interaction with ABA Slot Tag

D11's ABA slot tag (per-slot, bumped on slot free-and-reuse) and generation
counters (per-object, bumped on explicit revoke) are distinct mechanisms serving
different purposes:

- **ABA tag:** prevents stale handle → reused slot aliasing. Defense against
  use-after-free of table slots.
- **Generation counter:** invalidates live caps to a surviving object.
  Revocation primitive.

Both appear in the capability entry but are checked at different points: ABA tag
on handle resolution (before dereferencing the object pointer), generation on
object access (after dereferencing, before proceeding with the operation).

## Downstream Consequences

- Cap entry layout gains a `generation: u64` field alongside object pointer,
  rights mask, and ABA tag.
- Each kernel object struct gains a `generation: AtomicU64` field (8 bytes per
  object).
- A `revoke` typed syscall (or modifier) is needed in the syscall enumeration
  (D48). Not settled here.
- Stale slot reclamation: stale caps occupy table slots until next access. A
  sweep mechanism may be needed if slot pressure becomes an issue. Deferred.
- Cross-core prompt-effect policy (strong vs. weak) remains open. Generation
  counters are naturally lazy/weak — prompt revocation still requires IPI.

## Prior Art

| System     | Mechanism                                | Outcome                                               |
| ---------- | ---------------------------------------- | ----------------------------------------------------- |
| Coyotos    | Allocation count (generation)            | O(1) revoke, lazy rewrite. Primary precedent.         |
| EROS       | Link-chain traversal (CDT-like)          | O(N) revoke. Replaced by Coyotos.                     |
| seL4       | CDT only                                 | O(N) revoke, WCET problem → MCS preemption.           |
| Zircon     | Neither                                  | Base only. Works via conventions. Leaves non-IPC gap. |
| Genode     | Neither                                  | Hierarchical structural revocation.                   |
| Barrelfish | Distributed CDT                          | ~1ms cross-core. Acknowledged as expensive.           |
| L4.Sec     | Versioned thread IDs (scoped generation) | Generation applied to one type only.                  |
