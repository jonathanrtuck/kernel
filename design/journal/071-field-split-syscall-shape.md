# Journal 071 — Field split syscall shape: split-to-new only

Settles D45's deferred question: "whether split-to-new and split-to-existing are
one syscall or two." Also updates D48's field_split signature and promotes D19's
multi-receive from "not foreclosed" to "planned."

**Decision:** field_split is split-to-new only:
`field_split(cap, badge_range, space_cap) → new_field_cap`. Split-to-existing is
deferred (not foreclosed). Multi-receive (D19 Option C) is promoted to planned,
covering the two-Field case that split-to-existing would otherwise serve.

---

## Starting point

D45 settles the Field split mechanism (badge-range routing with
fallback-on-destroy) but explicitly defers syscall shape: "whether split-to-new
and split-to-existing are one syscall or two." D48 records the provisional
signature `field_split(cap, badge_range, dest_field)`, which implies
split-to-existing. G08 explored the full design space across four options.

---

## The design space (G08 summary)

Four options for Field split's syscall surface:

1. **Split-to-new only.** `field_split(cap, badge_range, space_cap) → new_cap`.
   Consistent with Space and Time split. Covers top-down delegation. Cannot
   express merge (routing into an existing Field).

2. **Split-to-existing only.** `field_split(cap, badge_range, dest_field)`.
   Covers IRQ+IPC integration and merge. Top-down delegation requires
   pre-creating the destination (two syscalls).

3. **Two explicit syscalls.** Both variants with distinct names and signatures.
   Best type safety and error semantics. Commits to both interfaces upfront.

4. **One syscall, type dispatch.** Kernel infers the operation from the cap type
   of the destination argument. Preserves D48's count but creates D7 tension
   (typed operations with variable-type parameters).

Time and Space split are effectively settled as split-to-new by structural
constraints (D38 linearity for Time, D26 VA base assignment for Space). The
genuine choice is concentrated in Field split.

---

## Why split-to-new only

**Both variants are individually additive.** Split-to-new can be added to a
system with only split-to-existing, and vice versa, without breaking existing
callers. Neither forecloses the other. The question is which to start with.

**Split-to-new serves the common case.** Top-down delegation — supervisor splits
off a badge range for a new entity — is the canonical use case (D22, D41). Every
surveyed system with memory/resource split uses this form. Split-to-existing
serves the rarer case: routing into an already-running service.

**The two-Field problem has a planned solution.** The primary motivation for
split-to-existing is that a driver receiving a second Field from split-to-new
cannot wait on both simultaneously (D19 dissolved multi-field wait as a kernel
primitive). But D19 explicitly preserved the multi-receive escape hatch: "The
stateless multi-receive syscall (Option C) can be added at any time without
architectural disruption." D19 further recommends the Observer wait state be
built to accommodate N-field blocking. With multi-receive planned, a driver
holding two Fields can receive on both. Split-to-existing becomes a convenience
(one Field instead of two, supervisor-only routing control) rather than a
structural necessity.

**Starting with split-to-existing would commit to an unvalidated interface.** No
surveyed system uses split-to-existing for IPC endpoints. The authority
coherence question — does a send cap on the destination Field constitute consent
to be a routing target? — is unresolved (G08 evaluate phase flagged this as a
potential D4 concern warranting separate exploration). Starting with the
universally validated form avoids committing to an interface that may need
revision once the authority question is settled.

**D45's merge pattern is deferred, not lost.** Combine decomposes into
split-to-existing + destroy (D45). Without split-to-existing, merge is not
expressible through the syscall surface. This is accepted: multi-receive means
services needing traffic from multiple badge ranges can receive on multiple
Fields. Field consolidation becomes a future ergonomic optimization, not a
structural requirement.

---

## Settled signature

```text
field_split(cap, badge_range, space_cap) → new_field_cap
```

- `cap`: receive cap with split right on the source Field
- `badge_range`: low..=high (D71)
- `space_cap`: consumed for the new Field's backing (D32)
- Returns: cap to the new Field (receive + all rights)

Consistent with `space_split(cap, size) → new_cap` and
`time_split(cap, amount) → new_cap`. All three split operations create a new
object and return a cap to it.

---

## Multi-receive status change

D19's multi-receive (Option C) is promoted from "explicitly not foreclosed" to
"planned." D19's design recommendation — Observer wait state should accommodate
N-field blocking — becomes a requirement for the initial Observer
implementation, not just a forward-compatibility suggestion.

This does not settle multi-receive's syscall signature or semantics. Those
remain to be derived when the implementation approaches.

---

## Effect on D45

D45's mechanism is unchanged: badge-range routing with fallback-on-destroy. What
changes is the exposed syscall surface. Split-to-new is the only variant
available. Split-to-existing and combine-via-split-to-existing are deferred.

D45's description of split-to-existing enabling "IRQ + IPC on one Field" is
reframed: that use case is served by multi-receive (two Fields, one receive
point) rather than by split-to-existing (one Field, routing converges traffic).
The routing mechanism itself still supports both variants internally — only the
syscall surface is restricted.

---

## Effect on D48

field_split signature changes from `(cap, badge_range, dest_field)` to
`(cap, badge_range, space_cap) → new_field_cap`. Operation count stays at 25.

---

## Deferred: split-to-existing

Split-to-existing is not foreclosed. When/if added, it would be a second
operation (G08 Option 3) with a distinct name (e.g. `field_route`) and its own
error domain. The authority coherence question (routing-target consent) should
be explored before it is settled. The Observer wait-state design (N-field
blocking per D19) means there is no structural urgency.
