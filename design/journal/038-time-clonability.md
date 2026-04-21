# 038 — Time clonability

2026-04-21. Starting from the open question in spec.md: "Time clonability. D23
settled all other types as clonable. The uniformity argument suggests clonable
(multiple references to the same scheduling allocation, not a second
allocation). Journal 014's assumption of 'non-clonable' was never derived. D37
constrains: D30's aggregate (`total += cap.amount`) double-counts if two caps
reference the same Time object — violating 'the kernel cannot over-allocate.'
Move-only semantics for donation are consistent with non-clonable Time. The
tension is D23 uniformity vs. D30 aggregate correctness."

All parent decisions settled: D29 (Time is capability-held), D30 (multi-Time,
additive aggregate), D37 (Time donation is a move), D36 (normalized compute
units), D23 (Observer capabilities are clonable), D8 (flat table with rights
mask).

---

## The soundness argument

D30's cached scheduling aggregate maintains a conservation invariant: the sum of
all Time quantities held by an Observer equals its total compute allocation.

```text
on cap acquisition: total += cap.amount
on cap loss:        total -= cap.amount
```

This assumes each cap in the aggregate references a distinct Time object. If two
caps reference the same object (via clone), the aggregate counts that object's
compute units twice. The kernel believes the Observer holds more compute
capacity than physically exists. This violates the vocabulary constraint: "the
kernel cannot over-allocate."

This is not a corner case or implementation concern. It is a conservation
violation — the system model diverges from physical reality. The kernel would
schedule more compute time than has been allocated, stealing from other
Observers or exceeding the per-core pool.

Clone is structurally incompatible with D30's aggregate model for Time.

## D37 reinforces independently

D37 settled Time donation as an explicit cap transfer via move semantics. The
caller relinquishes the Time cap; the server receives it. If Time were clonable,
the caller could clone before donating, retaining the original — defeating the
capacity transfer. The caller's aggregate would still include the original, and
the server's aggregate would include the clone. The same compute units are
counted in both Observers' aggregates simultaneously.

D37's move-only donation is consistent with non-clonable Time. It would be a
leaky abstraction under clonable Time — the mechanism _says_ "transfer capacity"
but can't _enforce_ it.

## D16 send-once is precedent

Send-once caps (D16) are already non-clonable in this design. A send-once cap is
consumed on use — cloning it would allow two sends, breaking "once." The kernel
must structurally prevent duplication.

Time and send-once share the structural pattern: an invariant (conservation for
Time, single-use for send-once) that clone violates. Both are non-clonable for
soundness, not by convention.

## D23's scope

D23 stated: "Observer handles follow uniform capability rules — clone,
attenuate, transfer — identically to every other kernel object type."

This was correct for Observer. The five structural arguments (D4 attenuation, D8
uniformity, D12/D20 fault delivery, D11 orphan risk, type consistency) all held.
But the generalization — "identically to every other kernel object type" — was
too broad. It described the state at the time of derivation (four object types,
all clonable) rather than a universal law.

D30's aggregate model, which postdates D23, introduced a structural constraint
that makes clone unsound for Time specifically. D23's argument #5 ("Observer
would be the sole non-clonable type") no longer applies — Time is non-clonable
for independent reasons, and send-once was already non-clonable.

D23's core insight — that Observer handles should be clonable — stands. The
overly broad framing is narrowed.

## Rights are per-type

D23's uniformity argument assumed a split between "universal capability
meta-operations" (clone, attenuate, transfer) and "type-specific rights" (read,
write, execute for Space; send, receive, mint for Field). This split implies
clone is in a different category from type-specific rights.

From the Observer's perspective, this distinction does not surface. An Observer
holding a capability sees a set of operations it can perform: read this Space,
clone this Space, send to this Field, split this Time. Whether "clone" is
philosophically a "meta-operation on the capability" or "a type-specific right"
does not change what the Observer does or sees. The Observer checks: can I do X
with this cap? The answer is in the rights mask either way.

Collapsing the two layers: each object type defines its valid rights. Clone
appears in the rights sets of Space, Field, and Observer. Clone does not appear
in Time's rights set. This is the same structure as "execute appears in Space's
rights set but not Field's" — no special category needed.

This is an A5 move. The two-layer model (meta-operations vs. type-specific
rights) pushes complexity toward the interface: the Observer must understand
which layer a right belongs to. A flat per-type rights set absorbs that — the
kernel says "here are the operations for this type." The kernel's internal
dispatch organization (table operations vs. object operations) remains
kernel-internal.

## The attenuation question

D23 argued: "D4 attenuation requires cloning (foreclosed by non-clonable)." Does
this apply to Time?

Attenuation means creating a derived cap with fewer rights. For Time, the
relevant derivation is **split** — creating a child Time with a subset of the
parent's compute units. Split is not clone. Split creates a new Time object with
a portion of the original's quantity; the original shrinks. This is the Time
analog of Space's split operation. The parent can then transfer the child cap
with whatever rights are appropriate.

Clone (two references to the same Time object) and split (two Time objects whose
quantities sum to the original) are distinct operations. Time supports split for
authority delegation. Clone is what it cannot support.

## Status

**Settled.** Time capabilities are non-clonable. At most one capability
reference exists per Time object. Time caps are linear — they can be transferred
(moved) but not duplicated.

The D30 aggregate model (`total += cap.amount` on acquisition,
`total -= cap.amount` on loss) requires each cap to reference a distinct Time
object. Clone creates two references to the same object, double-counting its
compute units and violating the conservation invariant ("the kernel cannot
over-allocate"). D37's move-only donation reinforces: clone would defeat
capacity transfer. D16 send-once provides precedent for non-clonable caps.

D23's "identically to every other kernel object type" is narrowed: D23's core
finding (Observer handles are clonable) stands; the universality framing does
not. Clone is a per-type right, not a universal meta-operation. Each type
defines its valid rights; clone appears in most types' sets but not Time's, for
the same structural reason it does not appear in send-once's.

A1 parallel: linear Time caps map to Rust's ownership model — a move-only type
with no `Clone` impl.

Revisit if: D30 is revised (changes the aggregate model that makes clone
unsound), or if a downstream derivation reveals that non-clonable Time creates
essential complexity (orphan risk, authority delegation) that D23 found for
Observer — noting that Time's split operation provides the delegation path that
clone provided for Observer.
