# 056 — Scheduler callback signature: two traits, five methods

Date: 2026-04-24

## Starting point

D2 settled per-core schedulers with potentially different algorithms. D42
settled the three-value scheduling profile. D50 settled the IPC fast-path
conditions including the `should_switch_to` callback. D56 settled cross-core
placement. The scheduler callback signature — the interface the kernel calls
into for scheduling decisions — was implicit across these derivations but never
stated explicitly.

## Exploration

### Two traits, not one

The scheduler interface decomposes into two traits:

- **`Scheduler`** — per-core, hot-path decisions about which Observer runs next
  on THIS core.
- **`Placement`** — cross-core scoring, determines which core an Observer should
  run on.

They must be separate because D1 (per-core hot path touches no cross-core shared
state) conflicts with D56's placement function needing to read other cores'
atomic counters. Combining them would require the per-core Scheduler instance to
have cross-core read access — a D1 violation. Separation makes the boundary a
type-level guarantee (A1).

### Scheduler trait: five methods

Each method is forced by a distinct derivation:

**`enqueue(&mut self, observer: NonNull<Observer>)`** — An Observer has become
runnable on this core. Forced by the Benno scheduling lesson (cited in D50): run
queues must stay consistent at all times. Called while holding
Arena\<Observer\>.

**`dequeue(&mut self, observer: NonNull<Observer>)`** — An Observer is leaving
the run queue (blocking, migrating, destroying). Same consistency requirement.
Called while holding Arena\<Observer\>.

**`pick_next(&mut self) -> Option<NonNull<Observer>>`** — Select the next
Observer to run. Returns None = enter idle (D46). Called on every timer
preemption, block, and yield. Called without arena locks.

**`should_switch_to(&self, candidate: NonNull<Observer>) -> bool`** — IPC
fast-path predicate (D50 condition 5). Read-only query: should the kernel switch
immediately to this candidate, bypassing the normal pick_next cycle? Called
without arena locks. Budget: ≤50 cycles to stay within D50's ~400-cycle fast
path.

**`on_preempt(&mut self, current: NonNull<Observer>)`** — Timer has preempted
the current Observer. Gives the scheduler accounting time (virtual runtime
update, budget deduction) before pick_next. Called without arena locks.

### Why NonNull\<Observer\>, not an ID

The fast-path `should_switch_to` has a ~20-50 cycle budget. Arena lookup by ID
(lock + index) costs more than the entire budget. The kernel already has a live
pointer from the Field's waiters list. `NonNull<Observer>` communicates the
non-null invariant without implying Rust's normal borrow rules, which don't
apply inside arena-allocated structures.

### Lock discipline

`enqueue` and `dequeue` are called while holding Arena\<Observer\> — the
Observer's scheduling_state and the scheduler's run queue are updated in the
same atomic step. `pick_next`, `should_switch_to`, and `on_preempt` are called
without arena locks — forced by D53 (lock ordering: Field before Observer; the
fast path has already released Arena\<Field\> before the scheduler check).

### Placement trait

```rust
fn place(&self, observer: &Observer, snapshot: &CoreSnapshot) -> PlacementDecision
```

Returns `Local` (hot path, no IPI) or `Remote(CoreId)` (cold path, mailbox + IPI
per D56). `CoreSnapshot` is populated once from per-core atomic counters before
calling place — avoids cache-line bouncing during the scoring loop.

One `dyn Placement` per system, not per-core (it reads system-wide state).

### Trigger map

| Event                  | Callbacks                                                  |
| ---------------------- | ---------------------------------------------------------- |
| Timer preemption       | on_preempt → pick_next                                     |
| IPC fast path (D50)    | enqueue(receiver) → should_switch_to(receiver)             |
| IPC slow path          | enqueue(receiver) → pick_next                              |
| Observer blocks        | dequeue(current) → pick_next                               |
| Observer unblocks      | Placement::place → if Local: enqueue; if Remote: IPI       |
| Idle core receives IPI | enqueue(observer) → pick_next                              |
| Work stealing          | dequeue(victim) from source → enqueue on local → pick_next |

## Status

**Settled.** Two traits (Scheduler + Placement), five Scheduler methods
(enqueue, dequeue, pick_next, should_switch_to, on_preempt), one Placement
method (place). NonNull\<Observer\> arguments. Lock discipline: enqueue/dequeue
under Arena\<Observer\>, others without locks.

Does NOT settle: internal run queue structure, affinity decay curve, scoring
weights, reclassification thresholds, CoreSnapshot layout, admission control
failure handling, work stealing synchronization mechanism.
