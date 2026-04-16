# Capability-Based Authority — 2026-04-15

Fourth derivation entry after the 2026-04-15 reset. Records the reasoning behind
`spec.md#D4`.

## Starting point

From the spec.md open questions: "Capability-based vs ACL vs ambient — not yet
derived. High-leverage: will shape Frame creation, Space transfer, Time
transfer, and the shape of every syscall."

Three candidate model families:

- **Capability-based:** authority is an unforgeable per-Frame token; holding it
  IS the permission. Designation and authority are bundled.
- **ACL-based:** authority is a per-resource list checked against the
  requester's identity. Designation and authority are separate.
- **Ambient:** authority derives from inherited context (privilege level, UID,
  group). Designation and authority are separate.

## Exploration

### Two independent derivation paths

Two unrelated axiom/derivation chains independently narrow the space to
capabilities. Neither chain references the other; both arrive at the same
conclusion.

#### Path 1: A5 + the confused deputy

A5 says the kernel "presents a simple interface and absorbs complexity behind
it, rather than exposing primitives that force complexity into userspace."

The confused deputy problem (Hardy 1988, `design/landscape.md` §1.2) is a
structural property of any interface that separates naming a resource from
proving authority over it. When these are separate, a deputy acting on behalf of
two principals cannot distinguish whose authority to use. This is not a bug in
ACL implementations — it is a property of the interface shape.

- In ACL models: a Frame names Resource Y and the kernel checks Frame identity
  against Y's access list. A deputy Frame serving two clients uses its own
  identity for both lookups — confused deputy.
- In ambient models: same structural problem, more severe.
- In capability models: the Frame presents a handle that IS both the designation
  and the authority. A deputy uses the client's handle — no confusion possible.

A5 prohibits "exposing primitives that force complexity into userspace." An
interface where every multi-client service must implement its own
authority-tracking logic to avoid confused deputies IS forcing complexity into
userspace. Capabilities are the only model family where this complexity
structurally cannot arise.

This forecloses ambient authority (A5 violation) and ACLs as the kernel↔Frame
interface (A5 violation via confused deputy).

#### Path 2: D1 + O3 + hot path

D1 says the hot path (exception entry → state update → scheduler pick →
resumption) touches no cross-core shared state. O3 says exceptions are taken on
the causing core. Syscall handling is part of the exception entry path —
authority checks during syscall handling are on the hot path.

- Capability tables are per-Frame. The running Frame's table is part of per-core
  state. Lookup is O(1) and touches only per-core data.
- ACLs are per-resource — shared data structures, writable by any
  authority-management operation on any core. Reading an ACL during syscall
  handling puts shared mutable state on the hot path, violating D1.
- Ambient privilege levels are per-Frame (per-core accessible), but ambient is
  already foreclosed by Path 1.

The only workaround for ACLs — per-core caching of per-Frame authority snapshots
— is structurally identical to a capability table. The ACL becomes a backing
store that generates capability-table-equivalent caches. At that point the
effective interface model IS capability-based.

This independently forecloses pure ACLs.

### Supporting observations

**A4 compatibility.** No kernel thread means no background authority-maintenance
entity. All authority state transitions must be explicitly triggered during
exception handling. Capability reference counting fits naturally (cleanup on
last close, triggered by Frame operations). ACL models requiring periodic
garbage collection face tension.

**A3 neutrality.** ACLs require Frame identity for permission lookup. A3 says
the kernel is generic across workloads. Some workloads (embedded,
single-purpose) have no natural identity model. Capabilities work without
identity — the handle IS the authority.

**A1 structural correspondence.** Capabilities map naturally to Rust's ownership
model: capability = owned reference, transfer = move, clone = explicit clone,
drop = close, attenuation ≈ `&T`/`&mut T`. The kernel implementation can
leverage the type system for compile-time authority-flow correctness. Not forced
by A1, but a structural alignment that makes capabilities cheaper to implement
correctly in Rust.

### Non-load-bearing axiom disclaimers

**A2 is not load-bearing here.** A2 answers "what hardware does the kernel
target?"; this entry answers "what authority model does the kernel use?" The
capability decision would be the same on any architecture. D1 (which A2
partially supports) IS load-bearing in Path 2, but A2 itself is not — Path 2
rests on D1's structural constraint, not on ARM64-specific properties.

**A1 is not load-bearing here.** A1's Rust/ownership correspondence is a
supporting observation (capabilities are cheaper to implement correctly in
Rust), not a derivation dependency. The capability decision would hold in any
implementation language. The work is done by A5 + D1 + O3.

**A5 is not load-bearing for the hot-path argument (Path 2).** A5 is
load-bearing in Path 1 (confused deputy). It is NOT load-bearing in Path 2. Path
2 rests on D1 + O3 — the mechanical constraint that hot-path authority checks
must use per-core data. A5 answers "what complexity placement does the
kernel↔Frame interface require?"; Path 2 answers "what data-access pattern does
the hot path permit?" The work in Path 2 is done by D1 and O3 alone.

This matters because the two paths provide genuinely independent evidence. If A5
were load-bearing in both, a challenge to A5 would undermine both arguments
simultaneously. With A5 load-bearing only in Path 1, a challenge to A5 still
leaves Path 2 standing.

### Convergence with the archived chain

The archived restart-1 chain arrived at capability-based authority in journal
004 (context-relationships) and explored capability representation in detail in
journal 006. The archive's primary reasoning path was the confused deputy
argument (our Path 1). The current derivation adds a second independent path via
D1 (which the archive did not have — the archive's SMP derivation in journal 005
came after the capability decision).

Three independent convergences:

1. A5 + confused deputy (current Path 1, archive journal 004)
2. D1 + O3 + hot path (current Path 2, new)
3. Archive journal 004's exploration from a different starting point

Per the philosophy: "When independent paths converge, trust the convergence."

### What this derivation does NOT settle

The derivation settles the model family (capability-based) but not the specific
flavor. Three choices are deferred as separate explorations:

1. **Scope of capability mediation.** Everything through capabilities (seL4/EROS
   style) vs. resources through capabilities with operations as direct syscalls
   (Zircon style). Determines syscall surface shape.

2. **Capability table structure.** Kernel-managed opaque handle table (Zircon)
   vs. capability to a table object (seL4 CNode). Determines who controls the
   authority-space structure.

3. **Revocation model.** Close-only (refcount), authoritative destroy,
   derivation tracking (CDT), or generation numbers. Each has different cost
   profiles under D1 and O2.

These are one level down — internals of HOW capabilities work, not WHETHER to
use them. The philosophy says "resist defining next-level internals until the
current-level interfaces are as stable as you can make them." The current-level
answer (capabilities) is stable; the next-level choices can proceed
independently.

### What IS settled for downstream derivations

A Frame proves it is allowed to perform an operation by presenting an
unforgeable, per-Frame handle (capability) that designates the resource AND
carries the permitted operations. The kernel resolves the handle, checks the
rights, and proceeds or rejects.

Properties that downstream derivations can rely on:

- **Per-Frame:** each Frame has its own authority state, independent of other
  Frames. Handle N in Frame A and handle N in Frame B are unrelated.
- **Unforgeable:** a Frame cannot fabricate a capability. It acquires
  capabilities at creation or via explicit transfer from another Frame.
- **Designation = authority:** there is no separate naming step. The capability
  IS the name.
- **Rights-bearing:** each capability carries a set of permitted operations. The
  kernel enforces the rights on every use.
- **Transferable:** capabilities can be passed between Frames (mechanism TBD).
- **Hot-path compatible:** capability resolution is per-core, O(1) lookup, no
  cross-core shared state.

## Status

**Accepted as `spec.md#D4` — settled.**

Revisit only if A5 is revised AND D1 is revised simultaneously (either alone
leaves at least one derivation path intact), or if a new model family emerges
that provides confused-deputy prevention AND per-Frame hot-path data
organization AND explicit-trigger-compatible lifecycle.

**Open sub-questions (deferred):**

- Scope of capability mediation (everything vs. resources-only)
- Capability table structure (kernel-managed vs. CNode-style)
- Revocation model (refcount, destroy, CDT, generation numbers)
- Whether Time is a first-class capability
- Bootstrap: how the first Frame acquires initial capabilities
- Capability-Frame relationship (interacts with "what unit runs in a Frame")
