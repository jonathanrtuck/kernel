# 030 — Time is a capability-held kernel object type

**Question:** What is Time's object status? Is it a capability-held kernel
object (like Space, field, Observer), a kernel-internal resource binding, or
something else?

**Answer:** Time is a capability-held kernel object type. The Observer's Time
reference is a capability in the Observer's D8 flat table at a reserved slot
(D21 pattern). Time joins Space, field, and Observer as the fourth
capability-designated kernel object type (correcting D14's count — D14 listed
Time alongside Space and field but Time's capability status was never formally
derived until now).

---

## Prior work

Journal 004 (D4) listed "Whether Time is a first-class capability" as unsettled,
deferred as downstream of D4.

Journal 006 (D6) listed "Time binding" as a concrete Observer field without
resolving whether it was capability-mediated or a direct binding.

Journal 014 (D14) counted Time among kernel object types ("Observer joins Space,
Time, and field as the fourth type") and referenced Time's "non-clonable
property" — but neither was formally derived.

Journal 023 (research implications) proposed Time as a capability-held object
based on seL4 MCS and S3K precedent. Noted it would dissolve Time migration and
Time reclamation open questions. Stated: "Not settled here — but noted as a
frame that may dissolve several open questions simultaneously."

Journal 023 also established the capability graph completeness principle (from
TreeSLS): every kernel object reachable only through the capability graph. D21
(fault handler as cap-table entry) was cited as this discipline in action.

---

## Derivation

### Three convergent paths

**Path 1: D4 (designation = authority).** The vocabulary defines Time as "a
claim to a portion of a specific logical core's scheduling time." A claim to a
bounded resource — the rate of scheduling time per logical core is bounded at
100%. D4 requires capabilities as the authority mechanism for bounded resources.
Every other bounded resource (Space for memory, fields for communication,
Observer for execution) is capability-designated. If Observers consume
scheduling time without presenting a capability to designate that claim, it is
ambient privilege — D4 forecloses this.

**Path 2: D21 precedent (cap-table entry for per-Observer resource
references).** D21 settled that the fault handler reference belongs in the cap
table, not as a struct field. Three arguments drove that decision:

1. D11 destroy-invalidation: the cap-table walk automatically invalidates the
   reference when the underlying object is destroyed.
2. D17 badge-closure: cap close fires lifecycle notifications generically.
3. D8 ABA protection: prevents stale references after destroy + slot reuse.

These arguments apply identically to the Observer's Time reference. If a Time
object is destroyed, the Observer's reference must be invalidated. A cap-table
entry handles this automatically. A struct field requires parallel tracking —
the same problem D21 rejected.

**Path 3: Cap-graph completeness (journal 023).** The discipline that every
kernel object should be reachable only through the capability graph. If Time is
kernel-internal, it is the sole resource outside the cap graph — a hole in the
discipline that D21 established as a design principle. Making Time
capability-held maintains completeness: all four object types (Space, Time,
field, Observer) are in the graph. System state is capturable by walking
capabilities.

### Dissolved open questions

**Time reclamation on Observer destroy** (spec.md open question). Previously:
"On destroy: return to destroyer? To creator? Destroy the Time?" Now: Observer
destroy closes the Time cap (D11 close semantics). If this was the only
reference, the Time object is destroyed and its scheduling allocation returns to
the per-core pool. If other caps exist (e.g., the creator held a reference), the
object persists. Existing mechanism, no new mechanism needed.

**Time migration across cores** (spec.md open question). Time is "a fraction of
a specific logical core's scheduling allocation." Migration is: close the Time
cap for the source core, acquire a Time cap for the destination core. This is a
cold-path capability operation, consistent with D1's hot/cold split. The
Observer's abstract scheduling properties (D2) transfer; the Time object is
core-specific.

### Discovery: "exactly one Time" is vocabulary assumption, not derived

The vocabulary says "An Observer holds capabilities to one or more Spaces and
exactly one Time." D6 carried this forward. But "exactly one Time" was never
derived from axioms — it was a vocabulary commitment.

During Phase 5, the Space parallel surfaced: Space is "one or more" because an
Observer can hold caps to multiple Spaces. Each Space cap represents a portion
of the bounded memory resource. If Time follows the same pattern, an Observer
could hold caps to multiple Time portions — each a fraction of scheduling
allocation. The total allocation is the sum of all Time caps held.

This parallel reframes scheduling properties: the Time cap represents the
quantity (how much scheduling allocation), and the Observer's abstract
properties (priority, deadline — D2) are hints about how the Observer wants its
total allocation distributed. When Time is transferred, the receiver gains more
scheduling allocation. This is structurally identical to Space: transfer a Space
cap and the receiver has access to more memory.

This derivation does NOT settle Time cardinality. "Exactly one Time" may be
correct (structural invariant) or may be an unnecessary restriction (vocabulary
assumption that should be re-examined in light of D27's flat Space cardinality).
The question requires its own exploration.

---

## Archive convergence

The archive (restart-1) reached the same top-level conclusion through different
framing:

- claims.toml "external-object-model": "time objects (units of time)" — treated
  as first-class kernel object type alongside memory objects.
- claims.toml "dynamic-resource-bindings": "Resource bindings (memory objects
  and time objects to Objects) are dynamic — they can be added and removed at
  any time." — Time is dynamically bindable.
- claims.toml "events-carry-resources": "Events can carry resource handles
  (Time, Space, routing capabilities). A sender that donates Time cannot run
  (effectively blocked)." — Time donation via IPC.
- spec.md line 313: "Time handle. The Context's active Time capability."

The archive had Time as capability-held with dynamic bindings and Time donation.
The current derivation arrives at the same conclusion through a different path
(D4, D21, cap-graph completeness). The archive's additional claims (dynamic
bindings, Time donation) are downstream questions this derivation defers.

---

## Costs

- **D2 interaction.** D2 says "Observer model carries abstract scheduling
  properties." If Time is a separate object, scheduling properties may split
  between Observer and Time. Which properties live where depends on Time's
  parameter model (deferred). D2's language needs refinement once Time
  parameters are settled, not revision of its substance.

- **One more kernel object type.** Creation authority, lifecycle, and parameters
  need derivation. This is real complexity — but it is essential complexity that
  was always present in the design. Making it explicit (capability-held) rather
  than implicit (kernel-internal) does not create new complexity; it makes
  existing complexity visible and manageable through the capability system.

- **A5 tension (scheduling resource management in userspace).** Userspace
  manages Time distribution. But the D12/D9 parallel holds: kernel absorbs
  enforcement (preemption, budget tracking), userspace provides allocation
  policy (which Observers get how much time) through capability distribution.
  The same pattern that justified D9 (kernel-managed memory with
  capability-designated objects) and D12 (kernel fault dispatch with userspace
  paging policy).

---

## What this does NOT settle

- **Time cardinality.** One or many Time caps per Observer. The vocabulary's
  "exactly one Time" is flagged as an unexamined assumption. The Space parallel
  (D27 flat cardinality — multiple independent caps) suggests multi-Time may be
  the consistent position. Requires its own exploration.

- **Time parameters.** What a Time object carries: budget/period (seL4 MCS), a
  fraction (vocabulary-literal), or just a claim-to-participate with quantity
  determined by the scheduler algorithm. Interacts with cardinality and D2.

- **Time clonability.** D23 settled all other types as clonable. The uniformity
  argument suggests clonable (multiple references to the same scheduling
  allocation). Journal 014's assumption of "non-clonable" was never derived.

- **Time creation authority.** Who creates Time objects. Per-core Time manager
  (graph.d2 already has this box). How initial Time is distributed at boot.

- **Time donation mechanism.** seL4 MCS donates scheduling context during IPC
  for priority inversion prevention. If adopted, likely a kernel-internal
  optimization on Call() (D16 reply-cap injection pattern). Deferred.

- **D2 scheduling property split.** Which abstract scheduling properties live on
  Time vs. Observer. Depends on Time parameters.

---

## Rejected alternatives

| Alternative                     | Foreclosed by        | Reason                                                                                     |
| ------------------------------- | -------------------- | ------------------------------------------------------------------------------------------ |
| Time as kernel-internal binding | D4, D21, journal 023 | Ambient privilege for bounded resource; cap-table precedent violated; cap-graph hole       |
| Time as Observer struct field   | D21                  | D11 destroy-invalidation, D17 badge-closure, D8 ABA protection all require cap-table entry |

---

## Axioms not load-bearing here

A1 (Rust) is not load-bearing. Time's object status is language-independent.
Rust will shape the implementation (ownership for Time objects), but the
derivation does not pass through A1.

A2 (ARM64) is not load-bearing. The ARM64 timer is the enforcement mechanism,
but Time's object status in the capability system is architecture-independent.

A3 (generic) is not directly load-bearing for the top-level question (Time is
capability-held). A3 is load-bearing for downstream questions (Time parameters,
scheduling algorithm diversity — D2 interaction).

A4 (reactive) is not load-bearing. Time enforcement via timer interrupts is
A4-consistent regardless of Time's object status.

A5 is load-bearing as a tension (userspace scheduling resource management) but
the tension resolves favorably via the D12/D9 parallel. A5 does not push against
the conclusion.
