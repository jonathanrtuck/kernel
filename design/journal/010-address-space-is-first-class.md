# 010 — The address space is a first-class kernel object

**Question:** What is Space at the kernel level — a first-class object that
Observers bind to, or an emergent property of individual memory-object mappings?

**Answer:** The address space is a first-class, capability-designated kernel
object. Observers bind to an address space; multiple Observers can bind to the
same one, sharing its page table tree. The vocabulary's "Space" (resource claim
/ memory budget) remains a separate concept. Working name: "address space" —
final naming deferred to public API.

---

## Prior work

No journal entry in the current chain explored this question directly. It
appeared as an explicit "does NOT settle" item in three journals:

- journal/005 (D5): "Address space structure per Observer" — one level down.
- journal/006 (D6): "Observer-Space binding model" — unsettled.
- journal/008 (D8): "Observer-Space binding and table sharing" — deferred, with
  D8 explicitly conditioning capability table sharing on this question's
  outcome.

The archive (restart-1) mentioned seL4's "TCB + VSpace + CSpace" model in
passing but did not explore the binding question.

Every surveyed system in the landscape (§6.5) treats the address space as a
first-class kernel object: seL4's VSpace, Zircon's Process/VMAR, Mach's Task,
QNX's process. No surveyed system uses an emergent model (address space as a
Observer attribute without a separate object). The migrating-threads alternative
(Spring, Composite, L4 LRPC) still uses first-class address spaces — the
execution unit migrates between them.

No specific research document on the Observer-Space binding question existed
prior to this exploration.

---

## Derivation

### The structural fork

The question reduces to: does the page table tree (the set of virtual→physical
translations the MMU uses) belong to the Observer, or does the Observer point to
it?

**Option A — First-class address space:** The page table tree is a
capability-designated kernel object. Observers hold capabilities to address
spaces and bind to one. Multiple Observers can bind to the same address space,
sharing the tree. Memory objects (D9) are mapped into the address space, not
into the Observer.

**Option B — Emergent (address space = Observer attribute):** Each Observer owns
its page tables directly. Memory objects are mapped into an Observer. "Sharing"
means multiple Observers independently mapping the same memory objects. No
structural sharing of page tables.

### Path 1: A5 — mapping consistency is interface complexity

Under the emergent model, maintaining mapping consistency across Observers that
share memory is pushed to userspace. Adding a shared mapping to N Observers
requires N syscalls and N independent page table walks. Partial failure (syscall
k of N fails) leaves inconsistent state for userspace to resolve.

Under the first-class model, one map operation into the address space object
makes the mapping visible to all bound Observers. The kernel absorbs the
consistency concern.

A5 says the kernel absorbs complexity behind a simple interface. The emergent
model pushes essential complexity (mapping consistency for shared address
spaces) into userspace. This is the same argument pattern that rejected CNode
trees in D8 and userspace-managed memory in D9.

Furthermore: if the kernel does not provide a first-class address space, user-
space will rebuild the concept. A supervisor creating a "multi-threaded process"
under the emergent model must: track which Observers are in the group, replicate
all mappings when an Observer joins, propagate new mappings to all group
members, and handle partial failures. This is a userspace address space manager
— the kernel object reimplemented outside the kernel, which is precisely the A5
antipattern.

### Path 2: D1 — hot-path TLB cost

Context switching between Observers that share memory but have independent page
tables (emergent model) requires a TTBR0 reload on every switch — different page
table roots, different ASIDs. Under the first-class model, Observers bound to
the same address space share a TTBR0 value and ASID. Context switch between them
skips the TTBR write entirely.

The direct TTBR switch cost is small (~13 cycles, pipeline flush on ARM64
Cortex-A76). The real cost is **TLB capacity pressure**. With per-Observer ASIDs
(emergent model), co-located Observers that share all mappings each need their
own TLB entries — the same virtual→physical translations duplicated under
different ASIDs. On a Cortex-A76 with 1280-entry L2 TLB, 10 Observers with a
200-page shared working set need 2000 TLB entries (emergent) vs. 200 entries
(first-class). The overflow causes page table walks at ~30 cycles per miss (to
L2 cache) or ~290+ cycles (to DRAM).

This matters for high-concurrency workloads (many-worker servers, databases with
thread pools). For workloads with few Observers, small working sets, or
infrequent switching, the difference is negligible.

Kernel-internal optimizations could partially mitigate this (shared ASIDs for
Observers with identical mappings), but such optimizations amount to the kernel
internally re-deriving the first-class address space concept — tracking "which
Observers share an address space" as an internal bookkeeping structure. The
essential complexity of tracking shared address spaces exists regardless; the
question is whether it appears at the interface or is hidden and inferred. Since
the kernel needs the concept anyway, exposing it as an interface concept is
simpler than inferring it.

### Path 3: D4 — independent delegation

If the address space is a first-class kernel object, it is capability-designated
(D4). This means address-space access (map/unmap rights) can be delegated
independently of Observer access (suspend/destroy rights). A supervisor can
grant a loader the ability to map code into an address space without granting it
control over any Observer bound to that address space.

Under the emergent model, the address space is inseparable from the Observer. To
grant someone the ability to modify an Observer's mappings, you must grant them
a capability to the Observer itself — conflating mapping authority with
execution authority.

### Supporting observations

**D6's language.** D6 says "one address space binding" — not "one address
space." The word "binding" implies something the Observer is bound TO, not
something it inherently IS. D6 also says "process is a userspace convention (a
group of Observers sharing a Space)" — "sharing" reads as multiple Observers
bound to the same object, not independently replicating mappings.

**D5 CHERI note.** D5 says "design around objects and permissions, not
page-table-specific concepts." A first-class address space object abstracts the
page table behind a capability-designated object. CHERI intra-address-space
compartmentalization is a natural extension — multiple security domains within
one address space object, without page table changes.

**Vocabulary cardinality.** The vocabulary says "An Observer correlates one or
more Spaces." If Space (vocabulary) were the address space, this would
contradict D6 ("one address space binding"). Space-as-budget (one or more memory
claims per Observer) is consistent with both the vocabulary and D6. This
confirms that Space (vocabulary) and the address space are distinct concepts.

### The "right way easy" concern

During evaluation, a concern was raised: providing first-class address spaces
might lead developers to default to "share everything" (one address space, many
Observers) out of POSIX familiarity, even though D4's per-Observer authority
model suggests selective sharing is more principled.

This concern is real but addressable at the API level:

1. **No default address space.** Observer creation requires an explicit address
   space capability. There is no "inherit creator's address space" shortcut.
2. **Equal friction.** Creating a new address space is one syscall — the same
   effort as reusing an existing one. Neither isolation nor sharing is the path
   of least resistance.
3. **Capability-gated.** Binding an Observer to an existing address space
   requires holding a capability with bind rights. Sharing cannot happen
   accidentally.
4. **Per-Observer authority preserved.** Even Observers sharing an address space
   have separate capability tables (D8). Kernel-level authority is never shared
   by default.

The object model enables both patterns (isolated and shared). The API design
determines which feels natural. This is a downstream decision, not an
object-model decision.

### Budget resolution

The page table tree backing an address space needs physical memory. Applying
D8's pattern (kernel-managed structures backed by the user's budget): the
address space creator's Space budget pays for the page table memory, just as a
memory object creator's budget pays for the object's physical backing. This is
not a new mechanism — it is D8's typed-memory-backing pattern applied to another
kernel object type.

---

## Vocabulary note

The vocabulary's "Space" is confirmed as a budget/resource-claim concept, not an
address-space concept. The "one or more Spaces" cardinality in the Observer
vocabulary entry is consistent with Space-as-budget (an Observer can hold
multiple memory resource claims) and inconsistent with Space-as-address-space
(an Observer has exactly one address space binding, per D6).

The address space object is a distinct concept. Working name: "address space."
Final naming deferred to public API design (per the vocabulary's existing naming
note).

---

## Emergent model rejected

The emergent model (address space as an Observer attribute) was rejected on
three independent paths:

| Path | Argument                                                                                  |
| ---- | ----------------------------------------------------------------------------------------- |
| A5   | Mapping consistency for shared address spaces is essential complexity pushed to userspace |
| D1   | TLB capacity pressure from per-Observer ASIDs on co-located workloads                     |
| D4   | Cannot delegate address-space access independently of Observer access                     |

Supporting: D6's language, D5's CHERI note, vocabulary cardinality, and the
observation that the kernel needs to track shared address spaces internally
regardless (for TLB shootdown at minimum), so exposing the concept at the
interface is simpler than inferring it.

The emergent model's claimed advantages — simpler object model, no lifecycle
question, natural per-Observer budget — were examined and found to be largely
illusory: userspace rebuilds the address space manager (A5 antipattern), the
lifecycle question is replaced by ad-hoc group management, and the budget
question has the same answer as D8 (creator pays).

---

## What this does NOT settle

- **Binding mutability.** Can an Observer rebind to a different address space
  after creation? D6 says "one address space binding" but does not say it is
  immutable.
- **Address space lifecycle.** When is an address space destroyed? When the last
  capability is dropped? When the last Observer unbinds?
- **Observer creation API.** Whether binding is a parameter of Observer creation
  or a separate operation. The "no default, equal friction" API guidance above
  is a design intent, not a settled interface.
- **Capability table sharing.** D8 deferred this to the Observer-Space binding
  model. Now that same-address-space Observer groups are a first-class concept,
  capability table sharing can be revisited as a D8 downstream.
- **Address space naming.** Working name only. Final name deferred to public
  API.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Rust's type system is compatible with either
model. A1 will become relevant when implementing address space structures, but
the derivation does not pass through it.

**A2 (ARM64)** provides the hardware context (TTBR0, ASIDs, TLB) that makes D1's
hot-path argument concrete, but A2 itself does not discriminate between the
options. The argument structure holds on any architecture with hardware-walked
page tables and tagged TLBs.

**A3 (generic)** was examined and found to be non-discriminating. Both the
shared-address-space pattern (multi-threaded server) and the
isolated-address-space pattern (single-Observer isolation) are expressible under
the first-class model. Neither option forecloses workloads.

**A4 (purely reactive)** is not load-bearing. Address space operations happen in
response to syscalls under either model. No background management is required by
either.

---

## Status

**Accepted as `spec.md#D10` — settled.**

Revisit if:

- A5 is revised (would re-open whether mapping consistency belongs in userspace)
- D1 is revised to remove the hot/cold split (would remove the TLB argument,
  though A5 and D4 paths remain independently sufficient)
- A downstream derivation (capability table sharing, Observer lifecycle) reveals
  that first-class address spaces force essential complexity into userspace that
  per-Observer address spaces would have avoided
