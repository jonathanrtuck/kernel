# Phase D Execution Protocol

How leaf implementations are produced, tested, reviewed, and composed. This
document governs all Phase D autonomous work. Agents receive it as an
operational constraint, not optional context.

Created: 2026-04-25. Agreed between designer and Claude.

---

## Principles (non-negotiable)

These are derived from `design/philosophy.md` and the designer's stated values.
They override any agent's impulse to "just get the test green."

1. **Only the correct solution.** Not the easiest. Not the quickest. The one
   that matches the spec derivation AND handles every edge case the derivation
   didn't think to mention. When in doubt, investigate — don't guess.

2. **Fix root causes, not symptoms.** If a test fails, understand why. If the
   borrow checker complains, understand the ownership constraint it's surfacing.
   Never silence a diagnostic without understanding its cause.

3. **Make the right way the easy way.** The implementation should make incorrect
   usage a compile error wherever possible. Prefer types that eliminate invalid
   states over runtime checks that detect them.

4. **The spec drives the code.** Every `todo!()` body has a doc comment citing
   derivations. The implementation must satisfy those derivations. If the
   derivation is ambiguous, stop and flag it — don't quietly settle it.

5. **The compiler is a verifier.** When Rust's type system or borrow checker
   forces a structural change (split a struct, add a lifetime, change
   ownership), that is the medium surfacing a design constraint. Record it as a
   journal entry, not a silent code fix.

6. **No contamination from prior implementations.** The `implementation-v1`
   branch is off-limits. Each module is derived from `spec.md`, not copied from
   a previous attempt. The only exception: pure Rust-language questions ("how do
   I express X in Rust") that have nothing to do with kernel design.

---

## Wave structure

Three waves. Each wave implements modules at the same depth in the call graph.
Within a wave, modules do not call each other — they can be implemented in
parallel. Between waves, dependencies flow strictly downward.

### Wave 1 — Data Structures

Leaf modules with no cross-module calls in their `todo!()` bodies.

| Module          | todo!() count | What gets implemented                                                                                          | Key derivations                 |
| --------------- | ------------- | -------------------------------------------------------------------------------------------------------------- | ------------------------------- |
| `arena.rs`      | 4             | Slab allocator: allocate, get, get_mut, free                                                                   | D53, D67, D70                   |
| `capability.rs` | 9             | Table: resolve, resolve_mut, allocate_slot, free_slot, install_at, install, close, begin_cascade, cascade_step | D4, D8, D11, D17, D33, D51, D67 |
| `field.rs`      | 7             | Queue: enqueue, dequeue. Waiters: add_waiter, remove_waiter, pop_waiter. Routing: resolve_route, add_route     | D13, D18, D45, D54, D71         |

**Total:** 20 `todo!()` bodies.

**Why these three together:** Arena provides the backing store for all kernel
objects. Table provides the capability resolution that gates every syscall.
Field provides the queue and waiter mechanics that IPC builds on. None of these
call each other's `todo!()` methods — Arena is used by the arena's _callers_ (in
frame/ and higher layers), Table operates on Entry structs without touching
Field or Arena, and Field operates on its own queue and linked lists.

**Wave 1 is the foundation.** Errors here propagate to every subsequent wave.
Testing must be exhaustive.

### Wave 2 — Composition

Modules that compose Wave 1 interfaces. May call into Wave 1 modules.

| Module             | todo!() count | What gets implemented                                             | Composes                                                                    |
| ------------------ | ------------- | ----------------------------------------------------------------- | --------------------------------------------------------------------------- |
| `communication.rs` | 4             | send, receive, call, reply_recv                                   | Field (enqueue, dequeue, pop_waiter, add_waiter), Observer (block, unblock) |
| `space_manager.rs` | 4             | allocate_pages, return_pages, assign_va, type_conversion_overhead | Arena (slab page source)                                                    |

**Total:** 8 `todo!()` bodies.

**Why these together:** Communication orchestrates Field and Observer operations
for IPC. SpaceManager mediates between the root pool and Arena slab pages. They
don't call each other.

**Wave 2 is where cross-module correctness risks live.** D50's fast-path
conditions span Field (waiters), Observer (state transitions), and Scheduler
(approval callback). D18's pending-list draining requires Field dequeue to
trigger pending delivery. Integration tests here are critical.

### Wave 3 — Orchestration

Top-level dispatch and algorithm implementations. Depends on everything below.

| Module            | todo!() count | What gets implemented                                                      | Composes                                       |
| ----------------- | ------------- | -------------------------------------------------------------------------- | ---------------------------------------------- |
| `core_manager.rs` | 6             | current_core[_mut], dispatch_ipc, dispatch_typed, handle_timer, handle_irq | communication, capability, Observer, Scheduler |
| Scheduler impl(s) | new file(s)   | Concrete Scheduler trait implementation(s)                                 | Observer (priority/profile reads)              |
| Placement impl    | new file(s)   | Concrete Placement trait implementation                                    | CoreSnapshot                                   |

**Total:** 6 `todo!()` bodies + new files.

**Why last:** CoreManager is the entry point for every exception. It calls
communication for IPC, capability for typed ops, and Scheduler for scheduling
decisions. It cannot be tested until everything below works. Scheduler and
Placement are behind trait interfaces — true leaf nodes that can't break
anything above them.

---

## Testing protocol

Two testing roles per wave. They are briefed differently and run independently.

### Role A: Spec Verifier

**Purpose:** Validate that the implementation does what the design says.

**Briefing includes:**

- The module source (types + method signatures + doc comments)
- The specific derivations cited in doc comments (extracted from spec.md)
- The module's section of `src/CLAUDE.md` for context
- `design/philosophy.md` as operational constraint
- This protocol document

**Briefing does NOT include:**

- Implementation details of other modules
- The implementation-v1 branch
- Spec sections beyond the cited derivations (to prevent scope creep)

**What they write:**

- One or more tests per derivation claim cited in the module
- Tests named to trace the claim: `test_d13_fifo_ordering`,
  `test_d18_full_queue_returns_error`, `test_d67_generation_mismatch`
- Tests for stated invariants: "R + T <= 128", "refcount 0 or 1 for Time",
  "circular queue preserves FIFO order"

**What they do NOT write:**

- Implementation code (they write tests first; the implementation agent fills
  the `todo!()` bodies to pass the tests)

### Role B: Adversarial Tester

**Purpose:** Find every way the implementation can fail, corrupt state, or
violate an invariant.

**Briefing includes:**

- The module source (types + method signatures + doc comments)
- The instruction: "This module has been implemented. Your job is to find bugs.
  Test boundary conditions, off-by-one errors, wrap-around behavior, state
  corruption from interleaved operations, integer overflow, underflow, invariant
  violations after sequences of valid operations. Assume the implementation has
  bugs. Find them."
- General Rust safety guidance (no UB, no panics on valid inputs)
- This protocol's testing section (for naming conventions)

**Briefing does NOT include:**

- Derivation numbers or spec references
- The design rationale
- What the implementation is "supposed to do" beyond what the type signatures
  and doc comments state

**What they write:**

- Boundary condition tests: empty queue dequeue, full queue enqueue, single-
  element queue, capacity-1 queue, capacity-MAX queue
- Wrap-around tests: enqueue/dequeue cycles that force the circular buffer head
  past the end
- Sequence tests: interleaved add_waiter/remove_waiter/pop_waiter in every
  order, including removing a waiter that was already popped
- State machine abuse: calling resume on every state, block on every state,
  fault on every state, calling them twice
- Arithmetic edge cases: split(0), split(MAX), split(all), merge with self,
  merge with non-adjacent, overflow in size calculations
- Invariant checks after operations: is the queue length consistent with
  enqueue/dequeue count? Is the cap table count consistent with allocate/free
  count?

**Naming convention:** `test_adversarial_<module>_<what>`. Example:
`test_adversarial_field_wraparound_after_n_cycles`.

### Implementation Agent

**Purpose:** Fill `todo!()` bodies to pass both Spec Verifier and Adversarial
tests.

**Briefing includes:**

- The module source (types + method signatures + doc comments)
- The specific derivations cited in doc comments
- `design/philosophy.md` as operational constraint
- The existing tests in the module
- This protocol's principles section
- For Wave 2+: the implemented Wave 1 modules (read-only reference)

**Workflow:**

1. Read all tests (existing + Spec Verifier + Adversarial)
2. Implement the `todo!()` bodies
3. Run `scripts/verify` — must pass
4. Run `cargo test` — all tests must pass
5. If a test fails, fix the implementation, not the test (unless the test is
   provably wrong — in which case, explain why)

---

## Review gates

### Within-wave review (automated)

After each module's implementation is complete:

1. **Code review agent** checks: style compliance, missing error paths, unsafe
   discipline (must remain zero outside frame/), doc comment accuracy
2. **`scripts/verify`** must pass: clippy clean, bare-metal build, all tests,
   framekernel boundary

### Between-wave review (human + automated)

Before starting the next wave:

1. **Integration test agent** writes cross-module tests for the completed wave.
   These exercise the interfaces that the next wave will depend on. Examples for
   Wave 1:
   - Allocate an arena slot, install a cap entry pointing to it, resolve the
     cap, free the slot, verify the cap reports stale generation
   - Enqueue messages until full, dequeue one, verify pending delivery triggers,
     verify FIFO order preserved across the boundary
   - Full Table lifecycle: allocate slots, install entries, close entries with
     badge tracking, verify close results, run cascade

2. **Human review** at wave boundary. The designer evaluates:
   - Does the data structure choice make sense? (circular buffer, freelist,
     sorted array — are these the right representations?)
   - Do the tests actually test what matters, or are they tautological?
   - Are there invariants the tests don't cover?
   - Does the implementation feel right — or does it feel like "the simplest
     thing that passes"?

3. **Wave acceptance.** The designer explicitly approves before the next wave
   begins. No silent progression.

---

## Cross-cutting concerns

These span multiple modules and don't emerge from unit tests alone. They must be
verified through integration tests written at wave boundaries.

### D50 — Fast-path conditions (Wave 2 gate)

The fast path requires all six conditions simultaneously. Test the composition:

- Send to Field with waiting receiver → WokeReceiver (not Enqueued)
- Call with 0-cap message + waiting receiver + scheduler approves → DirectSwitch
- Call with 1-cap message + waiting receiver → slow path (Enqueued)
- ReplyRecv where reply wakes the original caller → verify round trip
- Scheduler denies switch → falls back to enqueue despite waiter present

### D53 — Lock ordering (Wave 3 design constraint)

Arena lock ordering (Field < Observer < Pulsar) is a design constraint on
core_manager's dispatch paths. Verify by inspection that:

- IPC paths acquire Field arena before Observer arena
- Fault paths release Observer before acquiring Field (or never hold both)
- Pulsar fire acquires Field, never Observer

### D33 — Preemptible cascade (Wave 1 + Wave 2 integration)

Cascade exercises Table::close iterating over entries, each close potentially
triggering badge-closure, refcount decrement, and recursive destroy. Test:

- Single Observer destroy with mixed cap types in table
- Observer holding cap to another Observer (chain of length 2)
- Cascade step count is bounded (does not process entire table in one step)
- Object is dead (no new cap resolution succeeds) before cascade begins

### D18 — Pending list draining (Wave 2)

When a Field queue is full and the kernel needs to deliver a fault message, the
message goes to the pending list. On dequeue, the pending list is drained. Test:

- Fill queue → fault delivery goes to pending → dequeue one → pending message
  delivered → verify order

---

## Structural rules

### Framekernel boundary

All unsafe code lives in `frame/`. The crate-level `#![deny(unsafe_code)]` with
`#[allow(unsafe_code)]` on `mod frame` enforces this at compile time.
`scripts/verify` checks it as belt-and-suspenders.

If an implementation requires unsafe (e.g., Arena slab internals, intrusive list
pointer manipulation), it MUST go in `frame/`. The safe module defines the
interface; `frame/` provides the implementation.

**This means:** Arena<T>'s `allocate`, `get`, `get_mut`, `free` methods may need
their implementations to live in frame/ if they involve raw pointer
manipulation. The `todo!()` bodies in `arena.rs` would become thin wrappers
around frame/ functions. This is expected — the autonomous plan notes that
"unsafe slab internals live inside frame/" (arena.rs line 36).

### Borrow-checker feedback

When the borrow checker forces a structural change:

1. Stop. Do not work around it with unsafe.
2. Understand what ownership constraint is being surfaced.
3. If the change is within a single module (e.g., split a struct, change a field
   from `&mut` to returned value), make the change and note it.
4. If the change requires modifying a module interface (e.g., adding a lifetime
   parameter, changing a method signature), **stop and report to the designer.**
   Interfaces are architectural decisions in this project.

### New files

Wave 3 may create new files for Scheduler and Placement implementations. These
go in `src/time_manager/` as sibling files (e.g., `round_robin.rs`). They
implement the traits defined in `time_manager/mod.rs`. They do not modify the
trait interface.

No other waves should create new files. All implementation goes into existing
`todo!()` bodies.

---

## Agent spawn order

For each wave:

```md
1. Spec Verifier agents (one per module, parallel)
2. Adversarial agents (one per module, parallel, after #1 merges tests)
3. Implementation agents (one per module, parallel, after #2 merges tests)
4. Code review agents (one per module, parallel, after #3)
5. Integration test agent (one per wave, after all modules pass)
6. Human review (designer evaluates, accepts or requests changes)
```

Steps 1 and 2 produce tests only. Step 3 fills `todo!()` bodies. Step 4 reviews
the implementation. Step 5 verifies cross-module composition. Step 6 gates
progression to the next wave.

Within each step, modules are independent and can be parallelized.

---

## Success criteria

Phase D is complete when:

1. All `todo!()` bodies in the kernel domain modules are filled
2. `scripts/verify` passes (clippy, build, test, framekernel boundary)
3. Test coverage >= 80% across kernel domain modules
4. All D50, D53, D33, D18 cross-cutting integration tests pass
5. The designer has reviewed and accepted all three waves
6. No known bugs marked `Status: open-bug` introduced during Phase D
7. Design documents (`spec.md`, `graph.d2`) updated if any structural changes
   were forced by the borrow checker or implementation discovery
