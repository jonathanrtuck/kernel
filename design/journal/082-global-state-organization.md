# 082 — Global state organization

**Date:** 2026-04-25 **Starting point:** D75 settled that the five per-type
arenas and SpaceManager live in a bundled global `KernelState` struct. D82
settles the concrete struct definition, its location, and access pattern from
the dispatch path.

---

## Exploration

### The question

What is the `KernelState` struct? Where does it live? How does the dispatch path
access it?

D75 answers the organizational question ("bundled global, Lock<T> owns data")
but leaves the concrete layout, location, boot initialization contract, and safe
accessor as unsettled implementation details. D82 closes these gaps.

### Constraints from settled decisions

- **D53:** Five arenas with lock ordering: Field < Observer < Pulsar. Space and
  Time unordered.
- **D70:** Arena<T> is the per-type slab allocator.
- **D75:** KernelState bundles arenas and SpaceManager. Lock<T> wraps
  UnsafeCell<T>; LockGuard provides DerefMut. Cold-path only. Hot path never
  touches it.
- **D1:** Hot-path data per-core (CoreState), cold-path shared (KernelState).
- **D3/D31:** SpaceManager is kernel-wide shared state, same access pattern as
  arenas.
- **D46:** BSP initializes globals before secondary core activation.
- **A1:** no_std, no heap. MaybeUninit for boot-time initialization.
- **Framekernel discipline (journal 023):** All unsafe in frame/. The
  MaybeUninit static and assume_init_ref are genuinely unsafe.

### Fields

The five arenas, each wrapped in Lock with D53 ordering:

1. `fields: Lock<Arena<Field>>` — LockOrder::Field
2. `observers: Lock<Arena<Observer>>` — LockOrder::Observer
3. `pulsars: Lock<Arena<Pulsar>>` — LockOrder::Pulsar
4. `spaces: Lock<Arena<Space>>` — LockOrder::Space (unordered)
5. `times: Lock<Arena<Time>>` — LockOrder::Time (unordered)

Plus the SpaceManager:

6. `space_manager: Lock<SpaceManager>` — LockOrder::Space (same unordered
   category; SpaceManager is co-located with Space operations and does not
   participate in the Field-Observer-Pulsar ordering chain)

The IRQ routing table (Task 1.3) will be added to KernelState when that
derivation is implemented. D82 settles the struct with these six fields.

### Location

The KernelState type definition lives in `src/kernel_state.rs` — safe Rust,
outside frame/. The struct contains no unsafe code; it is a plain bundle of
Lock-wrapped arenas plus a Lock-wrapped SpaceManager.

The global static lives in `src/frame/mod.rs`:

```rust
static mut KERNEL_STATE: MaybeUninit<KernelState> = MaybeUninit::uninit();
```

MaybeUninit + assume_init_ref is genuinely unsafe — it belongs in the trusted
boundary (frame/). frame/ exports a safe accessor:

```rust
pub fn kernel_state() -> &'static KernelState
```

The accessor is valid after BSP initialization (D46: before secondary cores).
Calling it before initialization is a bug that would read uninitialized memory.
The SAFETY comment documents this invariant.

### Access from dispatch

Global function call, not CoreState field. This was a deliberate choice:

- **Avoids inflating CoreState:** CoreState is generic over S: Scheduler and
  per-core. Adding a &'static KernelState reference inflates every core's state
  and requires threading the lifetime through the generic.
- **Avoids 'static lifetime in test setup:** Tests construct CoreState locally.
  If CoreState held &'static KernelState, tests would need to leak allocations
  or use static-lifetime test fixtures.
- **Matches D1:** Hot path uses CoreState exclusively. Cold path calls
  kernel_state() when it needs arenas. The access pattern naturally separates.

### Boot initialization contract

main.rs provides:

1. Empty arenas (Arena::new() with empty slab stores)
2. SpaceManager initialized from DTB-discovered RAM

frame/ provides:

1. `init_kernel_state(state: KernelState)` — writes to the MaybeUninit static
2. `kernel_state() -> &'static KernelState` — safe accessor

Sequence: main.rs constructs a KernelState value, calls
frame::init_kernel_state(), then activates secondary cores. The value is moved
into the static — no references to stack-local data.

### KernelState::new()

Constructor takes empty arenas and a SpaceManager. Tests construct it locally
without needing the global static. This is the testability seam: domain logic
tests create a KernelState on the stack and pass references to it. Only the boot
path and the global accessor use the frame/ static.

### Rejected alternatives

- **KernelState in CoreState:** rejected per D75 rationale. Inflates CoreState,
  threads 'static through generic, complicates test setup.
- **Free-standing arena globals (no bundle):** rejected per D75. Scatters
  organization, loses single-point-of-change for future sharding.
- **Lazy initialization (OnceCell pattern):** rejected. A4 + D46 means boot-time
  init is sufficient. Lazy init adds runtime checks on every access for a
  property that is structurally guaranteed by boot ordering.

---

## Status

**Settled as D82.** KernelState struct in `src/kernel_state.rs` with six
Lock-wrapped fields (five arenas + SpaceManager). Global static in frame/ using
MaybeUninit. Safe accessor `frame::kernel_state()`. Constructor
`KernelState::new()` for test setup. No changes to settled interfaces D1-D76.

Revisit if: IRQ routing table is added (extends the struct — Task 1.3), or if
per-core sharding changes the Lock semantics (D53 revisit trigger).
