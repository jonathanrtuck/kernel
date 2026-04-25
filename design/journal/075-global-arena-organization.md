# 075 — Global arena organization

**Date:** 2026-04-25 **Starting point:** Open question in spec.md: "CoreState
arena references for dispatch." D53 settles lock ordering and D70 settles slab
internals, but neither settles where the five Arena<T> instances live or how
cold-path code reaches them.

---

## Exploration

### The question

D53 mandates five per-type arenas with SpinLock wrappers. D70 settles their
internal structure as slab allocators. The question is organizational: where do
these five arenas live as global state, and how does the dispatch path (and
every other cold-path operation) reach them?

### Constraints from settled decisions

Every relevant parent decision is settled:

- **D53:** one SpinLock per Arena<T>, five arenas total. Lock ordering: Field <
  Observer < Pulsar. Space and Time unordered.
- **D70:** slab allocator internals, pages from root Space pool.
- **D1:** hot-path data per-core, cold-path shared under locks. Arenas are
  categorically cold-path — the hot path works with NonNull<Observer> pointers
  (current, scheduler run queues, Field waiter lists).
- **D50/D59:** fast path and scheduler never acquire arena locks. Arena
  organization is invisible to the hot path.
- **D74:** per-core state (TPIDR_EL1 → CoreState) needs only the RegisterState
  pointer. No arena references required.
- **A1 (no_std):** no heap. Globals or boot-initialized structs only.
- **A4 + D46:** no lazy init (no kernel thread). Arenas created at boot,
  available before any core creates objects.

### Current code state

Lock<T> uses PhantomData<T> — it does not wrap the arena data. The guard
provides no Deref to the protected data. Lock and arena are separate objects.
The invariant "caller must hold this arena's lock (D53)" is enforced by doc
comment convention, not by the type system.

### Options considered

**Option A — Five separate statics.** Each lock and arena pair is a module-level
static. Cold-path code imports directly. Least ceremony but scatters import
sites — changing the organization later requires touching every cold-path
function.

**Option B — Bundled struct via CoreState parameter threading.** An Arenas
struct holds all five pairs. CoreState stores `&'static Arenas`. Cold-path
functions receive the reference as a parameter. Makes shared-state access
visible in function signatures.

Rejected: violates "push complexity to the leaves." Arena organization is a leaf
concern that should not affect inter-module interfaces. Threading a constant
(every core points to the same struct) adds ceremony without information. Every
cold-path function signature grows to advertise an implementation detail.

**Option C — Bundled struct as a named global.** Same bundle as B, accessed as a
global. No parameter threading. Self-documenting namespace
(`KERNEL.arenas.field`). Single point of change if the organization needs to
change (D53 flags per-core sharding as a future revisit trigger — "isolate
uncertain decisions behind interfaces").

**Option D — Lock<T> wraps data via UnsafeCell.** Change Lock from PhantomData
to UnsafeCell<T>. LockGuard provides DerefMut<Target=T>. The type system
enforces that you hold the lock before accessing the data.

Justified by A1: "ownership maps to resource lifecycle, unsafe boundaries map to
trust boundaries." The current design leaves a safety invariant (lock-before-
access) unenforced when Rust provides the tools to enforce it. "Separate
concerns" is not a reason to leave a safety gap — the concerns (proving you hold
the lock, accessing the data) are in fact coupled (accessing without the lock is
a data race).

### Decision

**Option C + D.** A bundled global struct where Lock<T> owns the data.

The bundle (`KernelState`) collects all kernel-wide shared cold-path state: the
five arenas and the SpaceManager (root pool). These have identical access
patterns (shared, cold-path, under locks). Consistent organization.

Lock<T> becomes a proper Rust mutex — wrapping UnsafeCell<T>, guard provides
exclusive access. This eliminates the convention gap between "caller must hold
lock" and actual type-system enforcement.

### Why not per-core arena copies (Barrelfish model)?

D53 settles the base model as global per-type arenas. Per-core copies are not
foreclosed (D53 explicitly flags sharding as a revisit trigger) but would
require reopening D53 because multiple downstream decisions build on the global
assumption:

- D33 (destroy cascade): caps cross types and cores — every close during cascade
  would require cross-core arena lookup.
- ObjectId encoding: currently indexes into a single per-type arena. Per-core
  copies require encoding core affinity.
- D53's lock ordering: assumes one arena per type, not N×5 per-core locks.

Per-core sharding (front-end magazines with shared back-end) remains compatible
as a future SMP optimization within this organization.

---

## Status

**Settled as D75.** The five arenas live in a global `KernelState` struct
alongside the SpaceManager. Lock<T> is refactored to own its data (UnsafeCell),
with LockGuard providing DerefMut. Cold-path code accesses arenas through the
global; the hot path is unaffected.

Revisit if: per-core sharding is implemented (changes Lock<T> semantics —
per-core magazines behind the same Lock interface), or if a new kernel-wide
shared resource doesn't fit the KernelState bundle pattern.
