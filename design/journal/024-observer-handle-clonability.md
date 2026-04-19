# Observer Handle Clonability — 2026-04-19

## Starting point

D14 (Observer is a capability-held kernel object type) opens six downstream
questions. One of them: "Observer handle clonability. Clonable: multiple
independent lifecycle managers, flexible delegation. Non-clonable (like Time):
exactly one manager, enables handle = handler unification."

The question gates the Observer rights model: clonability determines whether
rights can be separated across multiple caps (resume-only to one entity,
destroy-only to another) or must all coexist on a single handle.

All parent decisions are settled: D14 (Observer is capability-held), D4
(capability-based authority), D8 (flat table with rights mask), D11 (close +
destroy + ABA tag), D6 (Observer definition, no kernel grouping). No unsettled
ancestors.

## Exploration

### What the landscape shows

Every surveyed capability system with execution-unit handles makes them
clonable:

- seL4: TCB capabilities can be copied via seL4_CNode_Copy like any other
  capability. Multiple entities can hold caps to the same TCB. No type-specific
  restriction on copying.
- Zircon: thread/process handles can be duplicated via zx_handle_duplicate.
  ZX_RIGHT_DUPLICATE controls whether a given handle can be further duplicated —
  this is per-capability, not per-type.
- Mach: task/thread ports can be copied.
- EROS/KeyKOS: start/resume keys can be copied.
- Genode: parent has absolute authority over children; authority flows
  parent-to-child via capability distribution.

No surveyed system uses non-clonable execution-unit handles. Non-clonable would
be a novel position relative to the landscape.

### Systematic derivation against axioms and settled decisions

Checked every axiom (A1–A5), observation (O1–O4), and derivation (D1–D22) for
interactions. Most have no interaction with handle clonability. The interacting
entries:

**D4 (capability-based authority).** Attenuation — minting derived caps with
reduced rights — requires creating a new cap, which is a clone. Non-clonable
forecloses D4 attenuation for Observer handles. Observer would be the only
kernel object type where the standard attenuation mechanism does not work.

**D8 (flat capability table).** The table currently has no type-specific
behavior. Non-clonable requires the kernel to enforce, on every clone or
transfer attempt, that no other slot across all tables already holds a cap to
this Observer. Options: global search (expensive), per-Observer "holder exists"
flag (new internal state), or move-only transfer semantics for Observer type.
All add type-specific logic to what is currently type-agnostic.

**D11 (base revocation).** Close decrements refcount. With non-clonable,
refcount is always 1; close removes the sole reference. The Observer remains
alive but unreachable through the capability graph — no entity can resume,
destroy, or interact with it. This orphan risk requires either auto-destroy on
last-cap-close (collapsing the close/destroy distinction for Observers) or
kernel orphan detection.

**D12/D20 (fault delegation and fault handler attachment).** D20 says fault
messages include "the faulting Observer's capability handle via cap transfer."
Non-clonable means this transfer must be a move (original holder loses cap) or a
special-case token. Move means the creator cannot destroy the faulting Observer
while the pager handles the fault. If the pager crashes, the Observer is
orphaned — no entity holds a cap to it.

**D10, D15, D9 (type consistency).** Address space handles are implicitly
clonable (D10: multiple Observers bind to the same one). Endpoint handles are
explicitly clonable (D15: "clone receive caps to multiple worker Observers").
Memory object handles are implicitly clonable (D9: sharing through capability
transfer). Non-clonable Observer handles would make Observer the sole exception
among five kernel object types.

**D14 (rights separation).** With non-clonable, resume and destroy authority
cannot be separated across entities — both operations are on the sole handle.
The standard capability pattern for separating concerns (different caps with
different rights masks) requires cloning.

**A3 (generic kernel).** Some workloads need multiple independent lifecycle
managers (delegated kill authority, monitoring, fault tolerance with backup
managers). Non-clonable forces all multi-manager patterns into userspace
indirection. A3 doesn't forbid this, but the pressure is real.

**A5 (kernel absorbs complexity).** The A5 question cuts both ways. Non-clonable
pushes multi-manager complexity to userspace (A5 violation?). Clonable absorbs
refcount management in the kernel (accidental complexity?). The resolution: the
refcount mechanism already exists for all other kernel object types via D11.
Clonable Observer handles use an existing mechanism. Non-clonable Observer
handles require three new mechanisms (uniqueness enforcement, fault delivery
workaround, orphan prevention). A5's interface simplicity principle — "the
kernel presents a simple interface and absorbs complexity behind it" — favors
the option that requires fewer new mechanisms, not more.

A1 (Rust) maps cleanly to both options (single ownership vs. Arc-style shared
ownership). Not load-bearing. A2, A4 have no interaction.

### Archive convergence

The archive (restart-1/journal/013) explored this question and left it open,
tied to "handle = handler unification": if context handles are non-clonable,
then the handle holder is necessarily the fault handler, eliminating the
separate fault handler field.

The archive found this attractive but deferred it because "the kernel can't
'send to the holder' — it needs a Wormhole for fault message delivery." The
delivery mechanism was unclear.

The current chain independently settled this: D20 (per-Observer fault handler
attachment) and D21 (fault handler is a cap-table entry at a reserved slot). The
fault handler is a separate endpoint cap — it is not the Observer handle holder.
The unification concept is dissolved by D20/D21. The primary motivation for
non-clonable in the archive no longer applies.

Both chains converge toward clonable. The archive would have reached the same
conclusion once it settled the fault handler mechanism, which it had not yet
done.

### The case for non-clonable

Non-clonable provides one concrete benefit: kernel-enforced single-manager
invariant. Exactly one entity holds authority over each Observer.

This benefit was tested against concrete workloads:

- **Supervisor/child hierarchy:** parent holds sole handle, can destroy child.
  Works identically with clonable — the parent simply doesn't distribute the
  handle. No kernel enforcement needed.
- **Debugger attachment:** needs an attenuated Observer cap (inspect-only).
  Under non-clonable, must proxy through sole manager. Under clonable, manager
  mints an inspect-only handle directly.
- **Fault-tolerant backup manager:** hot standby holds a cap ready to take over.
  Under non-clonable, impossible (only one holder at a time). Under clonable,
  both hold caps from the start.

Every scenario where single-manager helps, clonable achieves the same result
through standard capability discipline. But scenarios where non-clonable hurts
have no workaround except userspace indirection.

D11's authoritative destroy further weakens the single-manager argument: destroy
kills the object regardless of holder count. Multiple holders don't prevent
teardown. The "someone else holds a reference" concern doesn't apply.

### A3 is not load-bearing

A3 (generic kernel) creates tension with non-clonable but does not force
clonability. A3 answers "the kernel must support diverse workloads"; it does not
answer "the Observer handle mechanism must support diverse authority patterns."
The work is done by D4 (attenuation foreclosed), D8 (uniformity broken), D11
(orphan risk), D12/D20 (fault delivery complication), and the type-consistency
argument (sole exception among five types) — all structural consequences of
settled decisions, not workload diversity.

### A5 is not the primary load-bearing axiom

A5 creates tension in both directions and does not independently resolve the
question. The resolution comes from observing that clonable uses an existing
mechanism (D11 refcount) while non-clonable requires three new mechanisms. A5's
"simple interface" principle aligns with clonable, but the decisive arguments
are the structural consequences above, not A5 alone.

## The duplicate-control right

An intermediate position — clonable with a duplicate-control right (Zircon's
ZX_RIGHT_DUPLICATE) — was considered. A right in D8's rights mask controls
whether a given handle can be further duplicated. The creator distributes
non-duplicable handles to limit proliferation.

This is functionally clonable (all D4/D8/D11/D12 mechanisms work) with an
optional policy lever. It is not a separate position from clonable — it is
clonable plus a deferrable rights-mask addition. The right can be added to all
kernel object types uniformly whenever the rights model is derived, without
affecting the clonability decision.

## Status

**Settled.** Observer capabilities are clonable. Observer handles follow uniform
capability rules — clone, attenuate, transfer — identically to every other
kernel object type (endpoints, address spaces, memory objects).

Five convergent arguments:

1. D4 attenuation requires cloning (foreclosed by non-clonable)
2. D8 uniformity requires no type-specific exceptions (broken by non-clonable)
3. D12/D20 fault delivery requires cap-copy (requires new mechanism under
   non-clonable)
4. D11 close under non-clonable creates orphan risk (requires new mechanism)
5. Type consistency: all other kernel object types are clonable (Observer would
   be sole exception)

Non-clonable's sole benefit (kernel-enforced single-manager) is achievable
through capability discipline under clonable, while non-clonable's structural
costs are not avoidable.

Archive convergence: archive left the question open tied to handle = handler
unification; D20/D21 dissolve that concept, removing the motivation for
non-clonable. Landscape convergence: 100% of surveyed capability systems make
execution-unit handles clonable.

Revisit if D11 is revised (changes the refcount/destroy model that makes
multi-holder safe), if D20/D21 are revised (reopens the handle = handler
unification that motivated non-clonable), or if a downstream derivation
(Observer rights model, Observer creation API) reveals that clonability creates
essential complexity that non-clonable would have avoided.
