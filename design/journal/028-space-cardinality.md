# 028 — Flat Space cardinality

**Date:** 2026-04-20. **Starting point:** The Vocabulary section describes
Observers as holding capabilities to "one or more Spaces." Under D26
(capability-addressed memory), each Space cap grants access to one contiguous
memory region with a kernel-assigned VA base. The open question: does an
Observer hold multiple Space caps directly as independent entries in its
capability table, or does the kernel maintain parent/child relationships between
Spaces?

---

## Prior work

Journal 010 (D10, now superseded by D26) discussed cardinality directly. It
confirmed "one or more Spaces" is consistent with Space-as-budget — an Observer
can hold multiple memory resource claims. But the analysis was in the context of
a separate first-class address space object, which D26 dissolved.

Journal 009 (D9) treated Spaces as the budget unit and described "subdividing
Space is budget delegation." Listed the Space-to-memory-object relationship as
unsettled.

Journal 027 (D26) describes the Observer's address layout as "the union of its
Space cap VA regions — a segment table derived from its cap holdings." It
assumes multiple independent Space caps per Observer but did not formalize
whether Spaces themselves form parent/child relationships.

No journal entry in the current chain explored flat vs. hierarchical Space
cardinality under D26.

## The fork

**Flat:** An Observer holds N Space caps as independent entries in the D8 flat
table. No kernel-tracked relationship between Spaces.

**Hierarchical:** The kernel maintains parent/child relationships between
Spaces. A parent Space can be subdivided into children. The kernel tracks the
tree.

## Derivation

### D8 favors flat

D8 settled per-Observer flat capability tables with no inter-entry
relationships. Entries are independent slots — the kernel manages them without
structural connections between them. Under the flat model, Space caps follow
this existing pattern identically.

The hierarchical model would introduce the first inter-entry structural
relationship in the cap table: parent Space caps have kernel-tracked connections
to child Space caps. This requires new kernel state outside the table (a tree
structure linking Spaces) and gives certain cap entries a privileged structural
role that D8's flat model does not anticipate.

### D6 parallel: grouping is policy

D6 settled that the kernel provides no Observer-grouping mechanism — "process"
is a userspace convention built from individual Observer caps. The same
reasoning applies to Space grouping. "A set of related Spaces" (code + data +
heap + stack for one program) is a userspace convention built from individual
Space caps. The grouping is policy (which Spaces belong together) rather than
mechanism (how Spaces work). D6 established that grouping policy lives in
userspace.

A5's "kernel absorbs complexity" applies to mechanism, not policy. The essential
complexity here (which Spaces form a program) is workload-specific policy —
exactly the category A3 + D6 push to userspace.

### D4: designation = authority

Under the flat model, each Space cap designates one contiguous memory region
with specific rights. Clean and consistent with D4.

Under the hierarchical model, a parent Space cap would designate both its own
region AND carry implicit structural authority over children. This conflates
resource designation with structural relationship — introducing a new kind of
authority (parent-over-child) that D4's model does not describe and that no
other kernel object type exhibits.

### D11: close/destroy simplicity

D11 settled close-only + destroy as the base revocation primitive. Close removes
one cap from one table. Destroy invalidates all caps to one object.

Hierarchy complicates both operations. Closing a parent Space cap: do children
survive (orphaned, severing the structural claim) or cascade-destroy (a new
destroy-propagation mechanism beyond D11's explicit per-object destroy)? Either
answer extends D11's scope beyond its settled semantics.

Under the flat model, D11 operates identically on Space caps as on every other
cap type. No new semantics.

### D26: independent VA assignment

D26 assigns each Space a VA base "at creation time" as "a property of the
Space." This reads as independent per-Space assignment. Hierarchy would require
either sub-range partitioning (children occupy portions of the parent's VA
range) or independent assignment (making the hierarchy logical rather than
structural in the VA layout). Independent assignment is already D26's model.

### A3: hierarchy is a workload assumption

A hierarchical-only model forces a tree structure on all memory organization.
Some memory patterns do not fit trees: shared libraries mapped into many
unrelated Observers, ring buffers between peers with no parent/child
relationship, or Observers sharing Spaces across multiple independent
supervisors. A3 forecloses workload assumptions; hierarchy-only is such an
assumption.

The flat model imposes no structural constraint on how Spaces relate — any
combination of Space caps per Observer is valid. A3-clean.

## Alternatives rejected

**Hierarchical (parent/child):** Rejected on five convergent grounds:

| Path | Argument                                                                                    |
| ---- | ------------------------------------------------------------------------------------------- |
| D8   | First inter-entry relationship in the flat cap table — new kernel state and structural role |
| D6   | Grouping is userspace policy, not kernel mechanism                                          |
| D4   | Hierarchy introduces implicit structural authority beyond designation                       |
| D11  | Close/destroy semantics require cascade or orphan — extends D11's scope                     |
| A3   | Tree assumption forecloses non-tree memory patterns                                         |

No settled decision or axiom requires hierarchy. The flat model creates no
tensions.

**Flat with provenance tracking:** Not rejected — deferred. Kernel-internal
provenance (tracking which Space a split originated from) has no user-visible
authority implications and could support accounting or debugging. It is
orthogonal to the cardinality question and can be added or removed later without
affecting the user-facing model.

## Non-load-bearing axioms

**A1 (Rust)** is not load-bearing. Rust's ownership model works identically with
flat or hierarchical Space caps. A1 will matter when implementing Space
split/close, but did not discriminate here.

**A2 (ARM64)** is not load-bearing. The MMU and page table hardware is
compatible with either model. D26 already absorbed A2's contribution.

**A4 (purely reactive)** is not load-bearing. Neither model requires background
management.

**A5 (leaf node)** was examined and found non-discriminating in the final
analysis. Both models absorb mechanism complexity. The distinguishing factor is
whether Space grouping is mechanism or policy — D6 settles this as policy. A5
does not discriminate between the options because the complexity at stake is
policy complexity that D6 already placed in userspace.

---

## Status

**Settled as D27.**

An Observer holds multiple independent Space caps directly in its D8 capability
table. Each Space cap is an independent entry. The kernel tracks no
parent/child, hierarchical, or structural relationships between Spaces. "Related
Spaces" (a program's code, data, heap) are a userspace convention — the same
treatment D6 gives Observer grouping.

Provenance tracking (kernel-internal metadata linking split origins) is deferred
as a potential future optimization. It is orthogonal to D27 and can be
introduced without changing the user-facing model.

Revisit if:

- D8 is revised to support inter-entry relationships (would re-open whether
  Spaces should use them)
- D6 is revised to add kernel grouping (would re-open whether Space grouping
  belongs in the kernel)
- A downstream derivation (Space split semantics, memory accounting) reveals
  that the absence of hierarchy forces essential complexity into userspace that
  flat caps cannot express
