# D83 — Per-core data organization

**Question:** What does each core own locally? What is the assembly-visible
PerCoreData layout?

**Rests on:** D1 (per-core hot path), D46 (core lifecycle), D56 (placement), D74
(register save target).

**Status:** settled.

## Decisions

### PerCoreData: assembly-visible indirection struct

TPIDR_EL1 points to a `#[repr(C)]` `PerCoreData` struct with two fields at known
offsets:

```text
offset 0: register_state_ptr (*mut RegisterState)  — assembly reads
offset 8: core_state_ptr     (*mut u8)             — Rust reads
```

Total size: 16 bytes. Assembly loads `register_state_ptr` with a single `ldr`
from the TPIDR_EL1 value (no offset arithmetic). The Rust exception handler
dereferences `core_state_ptr` to reach `CoreState<S>`.

The generic parameter `S: Scheduler` is erased in PerCoreData (`*mut u8`)
because the `#[repr(C)]` struct cannot name the concrete scheduler type — it
must be a fixed layout for assembly. The Rust side casts back to the concrete
type.

**Why separate from CoreState:** CoreState is generic over `S: Scheduler` and
may grow arbitrarily. Assembly needs a fixed, tiny struct at a known layout. One
pointer chase (PerCoreData → CoreState) on the Rust side is negligible — it
happens once per exception entry after register save completes. This is the same
pattern as Linux's `current_task` pointer in `task_struct` accessed via per-cpu
data.

### DeadlineEntry and per-core deadline array

Each `CoreState<S>` carries:

- `deadlines: [Option<DeadlineEntry>; 32]` — fixed-size, no dynamic allocation
- `deadline_count: usize` — tracks active entries

`DeadlineEntry` is 24 bytes: `deadline_ticks: u64`, `pulsar_id: ObjectId`,
`field_id: ObjectId`, `badge: Badge`. Fields carry enough context to construct
the D63 fire message without touching the global Pulsar arena on the hot path.

**Dense packing invariant:** Active entries occupy indices `0..deadline_count`,
removal uses swap-with-last. This gives O(1) removal and O(n) scan — acceptable
for n <= 32 on a timer interrupt path.

**Hard cap of 32:** Reject at CreatePulsar if the target core's deadline array
is full. 32 is generous: typical interactive workloads use 2-5 timers per
application, and a core runs one Observer at a time. The fixed array avoids
dynamic allocation on the hot path and keeps the per-core footprint bounded
(32 \* 32 = 1024 bytes for the Option array).

### Pointer path

```text
TPIDR_EL1 (set at boot, updated never)
  └─► PerCoreData                     [#[repr(C)], 16 bytes, frame/cores.rs]
       ├─ register_state_ptr ──────►  RegisterState (current Observer's save area)
       │                               [updated on every context switch]
       └─ core_state_ptr ─────────►  CoreState<S>  [safe Rust, core_manager.rs]
            ├─ core_id
            ├─ current: Option<NonNull<Observer>>
            ├─ scheduler: S
            ├─ deadlines: [Option<DeadlineEntry>; 32]
            └─ deadline_count: usize
```

## Rejected alternatives

**TPIDR_EL1 directly to CoreState** (previous state): CoreState is generic, so
assembly cannot know its layout. The PerCoreData indirection costs one pointer
chase but decouples assembly from Rust generics permanently.

**Deadlines in a separate struct:** No benefit — they are per-core hot-path data
(scanned on every timer interrupt). Splitting them out would add another
indirection for no structural gain.

**Dynamic deadline allocation (Vec/linked list):** Rejected. The hot path (timer
interrupt handler) must not allocate. A fixed array with a hard cap is simpler,
bounded, and deterministic.

**Sorted deadline array / min-heap:** Premature optimization. With n <= 32,
linear scan is fast enough (~32 comparisons per timer interrupt). A sorted
structure would complicate insertion and removal for negligible gain at this
scale.

## Implementation

- `src/frame/cores.rs`: `PerCoreData` struct with compile-time offset
  assertions, `read_per_core_data()`, updated `read_core_state()` and
  `read_core_state_mut()` to go through PerCoreData.
- `src/core_manager.rs`: `DeadlineEntry` struct, `MAX_DEADLINES_PER_CORE`
  constant, deadline fields added to `CoreState<S>`.
- Tests: layout assertions (size, alignment, field offsets), raw byte access
  verification, field roundtrip, dense packing invariant, interaction with
  existing dispatch.

## Interface changes

- `read_core_state()` and `read_core_state_mut()` now go through PerCoreData
  instead of treating TPIDR_EL1 as a direct CoreState pointer. The safe
  interface (`current_core()` / `current_core_mut()` in `core_manager.rs`) is
  unchanged.
- `CoreState<S>` has two new fields (`deadlines`, `deadline_count`). All
  existing CoreState constructors updated.
- New public function `read_per_core_data()` for direct PerCoreData access (used
  by future EL0 exception handler in Task 2.3).

## Reference check

Matches `.claude/implementation-plan-layers-1-3.md`:

- Task 1.4: DeadlineEntry struct, MAX_DEADLINES_PER_CORE = 32, fields in
  CoreState. Implemented as specified.
- Task 2.1: PerCoreData `#[repr(C)]` with register_state_ptr at offset 0,
  core_state_ptr at offset 8. Implemented as specified.
