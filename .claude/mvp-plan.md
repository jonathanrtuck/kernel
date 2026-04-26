# MVP Plan

Derived from: seven-agent audit (`remaining-work.md`), three expert reviews
(source code, design/architecture, plan realism), spec-compliance audit against
settled derivations, and four-agent review (functionality gaps, expert
assessment, decision audit, autonomy strategy). Created 2026-04-26, revised
2026-04-26.

---

## Definition

A microkernel that fully and correctly implements all settled design decisions
(D1-D105). Every interface is defined. Every leaf node has the simplest correct
implementation — not optimized, but not wrong. No runtime errors where the spec
requires faults. No unhandled edge cases. No violated invariants.

Single-core MVP is acceptable only because SMP requires settling unsettled
interface decisions first (D1 IPI semantics). SMP is Phase 4, not deferred.

### Exit criteria

1. Root Observer creates 2 child Observers, each with a private address space
2. Parent and child complete a Send/Receive IPC roundtrip with full message
   fidelity (4 data words + label + badge)
3. Child faults on an unmapped address; parent receives a VmFault message on the
   handler Field with correct Space index and byte offset
4. Pulsar fires; a blocked Observer wakes and receives the timer message
5. Parent destroys child; cascade completes and Space cap is returned to parent
6. Cap table growth: child fills its table, handler receives CapTableFull fault,
   provides Space, table grows, original operation retries
7. Badge-closure: parent closes last cap with badge B to a tracked Field; Field
   receives badge_closure notification
8. Secondary core boots via PSCI, runs its own Observer, IPIs work for
   cross-core operations
9. No panics, no memory corruption, no deadlocks, no leaked arena slots, no ABA
   hazards, no scheduler corruption

### Honest assessment

- **Architecture:** sound. No redesign needed.
- **Interfaces:** mostly settled. SMP interfaces (IPI semantics, mailbox
  protocol) are the main gap.
- **Functions:** most exist and are individually tested (1045 host tests, 18
  bare-metal userspace tests).
- **Integration:** pieces not wired together. Memory pipeline (D88-D91) has
  tested functions with zero callers.
- **Bugs:** 5 critical, 5 medium — all identified, fixable without redesign.
- **Spec violations:** 5 items in Category 6 that were misclassified as
  "defense-in-depth" are actually violated invariants from settled derivations.
- **Missing protocols:** cap table growth fault loop (D40), badge-closure
  notifications (D17), deferred fault delivery linkage (D18).
- **Design decisions:** all 12 settled (2026-04-26). No blockers remain.

---

## Decisions (Settled 2026-04-26)

| ID     | Decision                                  | Choice                                                                             |
| ------ | ----------------------------------------- | ---------------------------------------------------------------------------------- |
| D-0.3  | Handler validation failure escalation     | **(a) destroy Observer** — matches D68 Case C                                      |
| D-1.1  | Bare-metal slab initialization sequence   | **(b) pre-allocate critical slabs at boot** — no lazy-init edge cases              |
| D-2.5  | `return_backing_space()` called on destroy | **Verified** — called in all 3 cascade paths. No wiring needed.                   |
| D-3.1a | Growth slot constant value                | **(a) `u32::MAX`** — never conflicts with user slots                               |
| D-3.1b | Retry logic after cap table growth        | **(a) kernel replays transparently** on handler resume                             |
| D-3.1c | Nested growth failure                     | **(a) escalate → destroy** — prevents infinite recursion, D68 chain terminus       |
| D-3.2a | Badge refcount map data structure         | **Leaf node — simplest correct impl.** Sorted vec or linear scan; internal to Field, swappable later |
| D-3.2b | Badge map allocation timing               | **(a) at Field creation** when `badge_tracking=true`                               |
| D-3.2c | Closure notification on full queue        | **(a) deferred delivery** — D18 pattern, never lost                                |
| D-4.0a | IPI semantics                             | **(a) fire-and-forget** — eventual consistency by next scheduler round             |
| D-4.0b | Mailbox layout                            | **(b) per-core circular queue** — handles multiple in-flight IPIs (TLB + work steal) |
| D-4.0c | IPI request format                        | **(a) typed enum** — type-safe, no performance concern at exception level          |

---

## Phase 0 — Correctness

**Gate:** all existing tests pass, no known bugs or invariant violations remain
in code that runs today.

### Bugs in active code paths

#### 0.1 Send-once cap consumption [CRITICAL]

_audit: 1.1 / D51, D16_

`dispatch_ipc` Send path never checks `is_send_once()` and never closes the
slot. Reply caps are reusable indefinitely.

**Fix:** after successful send of a send-once cap, close the slot.

**Expert note:** this is a semantic gap, not a typo — the send-once lifecycle
wasn't fully internalized during initial coding. Verify that all Send paths
(direct fast-path and slow-path queue) both check and consume.

#### 0.2 Unified message delivery path [CRITICAL]

_audit: 1.2 / D13. Upgraded from "IRQ/timer waiter wakeup" based on expert
review._

`handle_irq` and `handle_timer` call `target_field.enqueue(message)` directly
instead of going through the communication path. If a receiver is blocked on the
Field, the message is enqueued but the receiver stays blocked.

**Root cause (expert review):** two separate delivery paths exist (IPC path via
`communication::send()` and kernel-as-sender path via direct `enqueue()`). Only
the IPC path handles waiter wakeup. An experienced kernel engineer would insist
on a single delivery path for all producers.

**Fix:** extract the waiter-wakeup sequence into a shared `deliver_to_field()`
function that both the IPC path and the kernel-as-sender path call. This
function must: (1) check for a blocked waiter, (2) if present, pop waiter +
write message to registers + unblock + enqueue to scheduler, (3) if no waiter,
enqueue to queue. This eliminates the class of bugs where a new producer path
forgets wakeup.

#### 0.3 Fault handler validation ordering [CRITICAL]

_audit: 1.3 / D12_

`observer_set_faulted()` is called before the handler Field is looked up. If
lookup fails, the Observer is stuck Faulted with no recovery path (non-root).

**Expert note:** classic "side effect before precondition check." Validate
preconditions first, then transition state.

**Fix:** validate handler Field BEFORE setting Faulted. If invalid, escalate.

> **SETTLED (D-0.3):** Escalate = destroy Observer (D68 Case C).

#### 0.4 Fault delivery unblock [CRITICAL]

_audit: 1.6 / D14_

When fault delivery wakes a receiver, `scheduler.enqueue` is called but
`observer_unblock` is not. The receiver is enqueued while still in Blocked
state.

**Fix:** call `observer_unblock` before `scheduler.enqueue` in the WokeReceiver
path. (This fix should become unnecessary if 0.2 creates a unified delivery
path, but verify.)

#### 0.5 IRQ message drop on full queue [CRITICAL]

_gap analysis: new finding / D22, D18_

When an interrupt message cannot be enqueued to a full driver Field,
`handle_irq` silently drops the message. The interrupt stays masked (send-once
ack is never delivered). The device is effectively dead.

Unlike faults (which at least have the Deferred outcome path, even if unwired),
interrupts have no fallback at all.

**Fix:** apply the same `deliver_to_field()` function from 0.2. If the queue is
full, use the D18 deferred delivery mechanism (pending list linkage). If
deferred delivery is not yet wired for this phase, at minimum: do not drop the
message — keep it in a pending slot so the next `receive()` drains it.

#### 0.6 ResourceRequest split rollback [MEDIUM]

_audit: 1.4 / D104_

When cap install fails after Space split, the source Space's size is already
reduced. Pages lost.

**Fix:** attempt cap install before committing the split, or merge-back on
failure.

#### 0.7 SpaceMerge arena slot leak [MEDIUM]

_audit: 1.5 / D41_

After merge, the source Space remains allocated as a zero-size ghost.

**Fix:** free the source Space arena slot after merge completes.

#### 0.8 Cascade dequeue [LOW]

_audit: 1.8 / D98_

`observers.free(target_id)` doesn't remove the Observer from the scheduler
queue. Dangling reference if the Observer was Runnable when Destroy was called.

**Fix:** dequeue from scheduler before freeing the arena slot.

### Invariant violations (were miscategorized as "defense-in-depth")

#### 0.9 SlotTag u32 wrap — ABA hazard [D11]

_audit: Cat 6_

D11 requires ABA prevention via generational tags. A `u32` SlotTag wraps after
2^32 slot reuses, re-creating exactly the ABA hazard the tag exists to prevent.

**Expert note:** wraps in ~4 seconds at 1 billion frees/second. Not a
performance concern — an invariant violation. The fact this shipped suggests ABA
wasn't fully internalized during initial coding.

**Fix:** upgrade SlotTag to `u64`. Wrap becomes astronomically unlikely (~584
years at 1 billion frees/second).

#### 0.10 Scheduler queue duplicate prevention [D2]

_audit: Cat 6_

D2's per-core scheduler assumes each Observer is enqueued at most once. No check
exists. Double-enqueue corrupts the queue.

**Expert note:** the current code assumes the dispatcher is perfect. One bug in
dispatch logic silently corrupts the queue.

**Fix:** check-and-reject on enqueue (return error or no-op if already present),
or make enqueue idempotent by checking membership first.

#### 0.11 Slab MaybeUninit soundness

_audit: Cat 6 + code review_

`MaybeUninit::zeroed().assume_init()` for types containing NonNull is UB
regardless of caller discipline. Miri would flag it.

**Fix:** return a raw pointer from the allocator; let the caller write through
it before wrapping in `Some`. Or use a builder pattern that guarantees
initialization.

#### 0.12 Overlapping routing range prevention [D45]

_audit: Cat 6_

Binary search on overlapping badge ranges picks an arbitrary winner. The spec
does not define precedence, and nondeterministic routing is not acceptable.

**Fix:** validate no intersection at `add_route` time. Reject overlapping ranges
with an error.

#### 0.13 Boot invariant check is debug-only [D75]

_expert review: new finding_

`KERNEL_STATE_INITIALIZED` AtomicBool is checked only in `debug_assert!`. In
release builds, calling `init_kernel_state` twice silently overwrites global
state.

**Expert note:** a production-grade system never gates critical invariants on
debug builds. The cost is one atomic read on a cold path — negligible.

**Fix:** promote `debug_assert!` to `assert!` in `frame/mod.rs` for the
`KERNEL_STATE_INITIALIZED` check. Scan for any other `debug_assert!` calls that
guard boot-sequencing or one-time-init invariants and promote those too.

### Deferred fault delivery linkage [D18]

#### 0.14 Wire pending list for deferred fault delivery

_gap analysis: new finding / D18, D12_

When a fault message cannot be enqueued because the handler Field's queue is
full, `dispatch_fault` returns `FaultDeliveryOutcome::Deferred`. The code
comment explicitly states: "Pending list linkage is not yet wired — the fault
stays deferred until the handler Field drains a slot."

**Impact:** if a faulting Observer encounters a full handler Field queue, the
fault is lost forever. The Observer deadlocks silently in Faulted state with no
recovery path.

**Fix:** wire the pending list linkage so that when `receive()` drains a slot
from a Field, any pending fault entries linked to that Field are delivered. This
is required for Phase 2.3 (fault handling under realistic conditions) and Phase
3.1 (cap table growth, which adds fault pressure).

### Phase 0 verification

```sh
cargo test --target aarch64-apple-darwin   # all tests pass
scripts/verify                              # clippy + build + boundary check
```

---

## Phase 1 — Memory Pipeline

**Gate:** a second Observer can be created with its own TTBR0 address space, and
installing a Space cap makes the Space's pages visible in that Observer's
virtual address space.

Strict dependency chain: 1.1 → 1.2 → 1.3 → 1.4 → 1.5.

### 1.1 TTBR1 kernel linear map (D88)

_audit: 2.1_

Build a TTBR1 page table that identity-maps (with offset) all RAM and device
MMIO for kernel use. Switch TCR from `configure_and_enable()` to
`build_tcr_split()`. Convert the ~59 `pa as *mut T` casts in `frame/` to
`phys_to_virt(pa)`.

The pa-to-virt conversion is mechanical but high blast radius. Do it
methodically: grep, convert, test after each file.

**Prerequisite:** bare-metal slab allocator must be wired to SpaceManager page
allocation (currently a stub that panics).

> **SETTLED (D-1.1):** Pre-allocate critical slabs at boot. Boot sequence:
> init SpaceManager → request pages → hand to each slab → arenas work.

**Verification:** kernel boots with split TTBR. Root Observer still runs EL0
code. `hypervisor ... --no-gpu --timeout 5` exits cleanly.

### 1.2 Per-Observer L1 page tables (D89)

_audit: 2.2 / depends on 1.1_

- `CreateObserver`: allocate L1 page from SpaceManager, set `page_table_root`
- Boot: create L1 for root Observer (currently hardcoded to 0)
- `__restore_observer`: already swaps TTBR0 using `make_ttbr0(asid, l1_pa)`
- Destroy: free L1 page

**Verification:** `CreateObserver` returns a non-zero `page_table_root`. A
second Observer context-switches with a different TTBR0 value.

### 1.3 L3 table allocation and population (D90)

_audit: 2.3 / depends on 1.1_

- Space creation: allocate L3 page, call `populate_l3_at_pa()`, set
  `l3_table_pa` (currently always 0)
- `SpaceSplit`: allocate new L3 for the new Space
- Destroy: free L3 page

Must handle `OutOfMemory` gracefully — return error, don't panic.

**Verification:** new Space has a non-zero `l3_table_pa`. Host tests for L3
population still pass.

### 1.4 Map/unmap wiring (D91)

_audit: 2.4 / depends on 1.2 + 1.3_

`map_space_in_observer()` and `unmap_space_from_observer()` are complete and
tested. **Zero callers.**

- `InstallCap(Space)`: call `map_space_in_observer`
- `Close(Space)`: call `unmap_space_from_observer` (currently a comment)
- TLB invalidation after unmap (DSB ISH + TLBI)

**Verification:** after `InstallCap(Space)`, the Observer's L1 table has an
entry pointing to the Space's L3. After `Close(Space)`, the entry is cleared.

### 1.5 VmFault translation (D61)

_audit: 2.5 / depends on 1.4_

Translate FAR (Fault Address Register) to `(Space cap slot, byte offset)` by
scanning the faulting Observer's cap table for the Space whose VA range contains
the FAR. Distinguish read/write/execute from ESR bits.

**Verification:** a deliberate page fault in EL0 produces a fault message with
correct Space index and byte offset. Host test + bare-metal test.

### Phase 1 verification

```sh
cargo test --target aarch64-apple-darwin
scripts/verify
hypervisor target/aarch64-unknown-none/debug/kernel --no-gpu --timeout 5
```

---

## Phase 2 — Integration Proof (single-core)

**Gate:** exit criteria 1-5 pass on bare metal. Two Observers, IPC, faults,
timers, destroy — all working.

No new subsystems. This phase wires Phase 0 + Phase 1 together and tests the
full flow end-to-end.

> **VERIFIED (D-2.5):** `return_backing_space()` is called in all 3 cascade
> paths (zero-cap immediate, single-batch immediate, multi-batch preemptible).
> No wiring needed.

### 2.1 Two-Observer boot

Root Observer splits Space, creates child Observer (own L1, ASID), installs
Space cap (triggers map), writes registers (D103 inline protocol), resumes
child.

**Exit criterion 1:** child runs EL0 code in its own address space.

### 2.2 IPC roundtrip

Root creates Field, installs in both Observers. Child sends (4 words + label +
badge). Parent receives. Verify fidelity.

**Exit criterion 2:** all message fields match.

### 2.3 Fault handling

Child touches unmapped address. VmFault translated (D61), delivered to handler
Field. Parent inspects fault message.

**Exit criterion 3:** fault type, Space index, byte offset, access type correct.

**Gap analysis note:** this scenario also exercises 0.14 (deferred fault
delivery) if the handler Field queue happens to be non-empty. The test should
verify fault delivery works both when the handler is waiting (fast path) and
when it must queue (slow path).

### 2.4 Timer fire

Pulsar armed with short deadline. Child blocks on Receive. Timer fires, message
enqueued, child wakes (Phase 0.2 fix).

**Exit criterion 4:** child wakes, receives `timer_fire` message.

### 2.5 Observer destroy + cleanup

Parent destroys child. Cascade runs. Space cap returned to parent (D32).

**Exit criterion 5:** backing memory reclaimed, no leaked arena slots.

**Gap analysis note:** if Space arena allocation fails during
`return_backing_space()`, the function returns 0 without the return cap. The
Observer is already freed — no recovery possible. For MVP with small test
scenarios this is acceptable, but document as a known limitation for
long-running systems.

### Phase 2 verification

```sh
hypervisor target/aarch64-unknown-none/debug/kernel --no-gpu --timeout 10
# Serial output: each scenario prints PASS/FAIL. All pass.
```

---

## Phase 3 — Protocol Completeness

**Gate:** all settled spec decisions that affect Observer lifecycle, IPC, and
resource management are correctly implemented. No spec-required behavior is
stubbed or returns an error where a fault is specified.

### 3.1 Cap table growth fault protocol [D8/D40]

_audit: Cat 7 → reclassified as spec violation_

D40 settles: "the kernel faults the Observer; the fault handler provides more
memory, then retries." Current code returns `TableFull` error — a different
protocol entirely.

This is the highest-risk implementation task in the plan. The retry-after-growth
path must reconstruct the original operation's context. All three design
decisions are settled.

> **SETTLED (D-3.1a):** Growth slot = `u32::MAX`. Never conflicts with user slots.
>
> **SETTLED (D-3.1b):** Kernel saves syscall context, replays transparently on
> handler resume. Handler doesn't need to re-issue.
>
> **SETTLED (D-3.1c):** Nested growth failure = escalate → destroy faulting
> Observer. Prevents infinite recursion, matches D68 chain terminus.

Implement:

- Define growth slot constant (`u32::MAX`)
- When `allocate_slot` returns TableFull in a dispatch path, save syscall
  context on Observer, deliver `FaultType::CapTableFull` to handler Field
- Block the faulting Observer (like VmFault delivery)
- Handler calls `ObserverInstallCap` with Space cap targeting growth slot
- Kernel detects growth-slot install, consumes Space for new table pages
- Kernel retries original operation (transparent replay on resume)
- Handle nested failure (escalate → destroy faulting Observer)

**Exit criterion 6.**

### 3.2 Badge-closure notifications [D17]

_audit: 3.3 → reclassified as spec violation_

D17 settles: "when the last send cap with badge B to field E is closed, the
kernel enqueues a closure notification." Opt-in per Field, but MUST fire when
enabled.

> **SETTLED (D-3.2a):** Leaf node — simplest correct implementation (sorted vec
> or linear scan). Internal to Field, swappable behind stable interface later.
>
> **SETTLED (D-3.2b):** Allocate at Field creation when `badge_tracking=true`.
>
> **SETTLED (D-3.2c):** Deferred delivery on full queue (D18 pattern). Never lost.

Implement:

- Add per-badge refcount map inside Field (simplest correct impl, allocated
  when `badge_tracking` is true at creation)
- Increment on cap install with matching badge
- Decrement on close
- When refcount reaches zero, enqueue `Message::badge_closure(badge)` to the
  Field's receive queue (deferred delivery on full queue per D18 pattern)

**Exit criterion 7.**

### 3.3 Field split routing [D45]

_audit: 3.1_

Send resolves primary Field but never calls `resolve_route`. Messages to split
Fields go to wrong queue.

**Gap analysis note:** until this is fixed, `FieldSplit` is a no-op from the
message perspective. Blocks IRQ delegation patterns and any field-splitting use
case.

### 3.4 Field destroy routing cleanup [D55] — USE-AFTER-FREE

_audit: 3.2. Severity upgraded by gap analysis._

Destroy never walks the `back_pointer_head` list. Stale routing entries
accumulate.

**Gap analysis upgrade:** this is not a cleanup task — it is a use-after-free.
When a split Field is destroyed, routing entries in source Fields point to freed
arena memory. A subsequent message matching those badge ranges dereferences a
dead Field pointer, causing corruption or panic.

### 3.5 Space unmap on last cap close [D24]

_audit: 3.5 / depends on Phase 1_

Close handler returns success without unmapping. Observer retains MMU access
after losing authority.

### 3.6 Destroy returns Space cap [D32/D98] — VERIFY

_audit: 3.4_

Conflicting audit findings. Verify: does the Observer destroy cascade path
actually call `return_backing_space()`? If yes, this is already done. If not,
wire it in. (See also D-2.5 pre-flight check.)

### 3.7 Nested fault handling [D39/D68]

_gap analysis: new finding_

If an Observer's fault handler itself faults while handling a fault, what
happens? `dispatch_fault` does not check whether the current state is already
Faulted before transitioning. A second fault would attempt Faulted → Faulted,
which the five-state machine (D39) does not define.

This becomes concrete in Phase 3.1: cap table growth requires the handler to
perform operations that can themselves fault.

**Fix:** before setting Faulted, check current state. If already Faulted,
escalate per D-0.3 decision (likely destroy). Document the state transition
rules for double-fault in a test.

### Phase 3 verification

```sh
cargo test --target aarch64-apple-darwin
scripts/verify
hypervisor target/aarch64-unknown-none/debug/kernel --no-gpu --timeout 10
# New scenarios: cap table growth, badge-closure notification
```

---

## Phase 4 — SMP

**Gate:** secondary cores boot, run Observers, and cross-core operations (TLB
invalidation, scheduling migration) work correctly. Exit criterion 8.

SMP is not a leaf-node optimization. D1's per-core hot path, D53's lock
ordering, D56's cross-core scheduling, and D46's core lifecycle all cross module
interfaces. The IPI protocol is an unsettled design decision in D1 that must be
resolved before implementation.

### 4.0 Settle IPI interface design [D1 open question]

Before writing code, record these settled decisions as derivation(s) in the
journal.

> **SETTLED (D-4.0a):** Fire-and-forget IPI. Core A sends SGI, continues.
> Eventual consistency by next scheduler round. D56 work-stealing checks are
> stale by definition — scheduling quality issue, not correctness.
>
> **SETTLED (D-4.0b):** Per-core circular queue (not single-entry struct).
> Multiple IPIs can be in-flight simultaneously (TLB invalidation + work steal).
>
> **SETTLED (D-4.0c):** Typed enum — `IpiRequest { WorkSteal,
> ObserverMigration, TlbInvalidation, RoutingEntryCleanup }`. No performance
> concern at exception level.

### 4.1 Fix TPIDR_EL1 on secondary cores [remaining-work 4.1]

`cpu.rs:134` writes `core_id as u64` to TPIDR_EL1 instead of a PerCoreData
pointer. Any exception on a secondary core is UB.

### 4.2 Fix fatal_exception interrupt masking [remaining-work 4.2]

`disable_irqs()` only masks DAIF.I. SError during crash dump re-enters the
handler. Mask SError and FIQ as well.

### 4.3 Secondary core boot [D46]

Activate secondary cores via PSCI CPU_ON. Each core initializes its own
CoreState, scheduler, and deadline array. Idle cores sleep (WFI).

### 4.4 Cross-core scheduling [D56]

Wire the `Placement` trait into dispatch. Build `CoreSnapshot` from cross-core
state reads. Implement IPI send/receive for Observer migration and remote
wakeup.

### 4.5 Lock ordering verification under contention [D53]

Add multi-threaded host tests that spawn threads, acquire locks in the D53 order
(Field < Observer < Pulsar), and verify no deadlock under contention.

### 4.6 Cross-core TLB invalidation

Wire `TLBI` broadcast (VMALLE1IS, VALE1IS) into Space unmap and ASID wrap paths.
Verify with multi-core test: unmap on core A, access on core B must fault.

### Phase 4 verification

```sh
cargo test --target aarch64-apple-darwin   # includes multi-threaded lock tests
scripts/verify
hypervisor target/aarch64-unknown-none/debug/kernel --no-gpu --timeout 15
# Multi-core scenario: two cores, each running an Observer, cross-core IPC
```

---

## Risks and Open Questions

### Bare-metal slab allocator

The test-build slab uses `Vec`-backed storage. The bare-metal slab is a stub
that panics. Phase 1 must wire bare-metal slab to SpaceManager page allocation
before any arena operations work on real hardware. D-1.1 is settled
(pre-allocate at boot). Could be the single hardest piece of Phase 1.

### Pager chain liveness (D105)

A chain of live-but-stalled fault handlers can prevent system progress without
triggering any error. D68's Pulsar watchdog is workable but optional per spec.
For MVP, root Observer is the only pager and is trusted. For production, needs a
formal resolution.

### Pager chain resource acquisition circularity

If every handler in a resource-request chain is trying to grow its own cap
table, the chain stalls. The design defers to userspace sophistication
(pre-allocation, layered paging). Acceptable given D68 is optional. Decision
D-3.1c (nested growth → destroy) provides a hard backstop.

### Observer destroy Space reconstruction failure

_gap analysis: new finding_

If Space arena allocation fails during `return_backing_space()`, the function
returns 0 without a return cap. The Observer is already freed — no recovery is
possible. In a long-running system that destroys many Observers, this leaks
backing memory with no reclamation path.

**For MVP:** acceptable — short-lived test scenarios won't exhaust the Space
arena. **For production:** the SpaceManager needs a reserved pool or fallback
for return-on-destroy operations that cannot fail.

### Pending list unbounded growth

_gap analysis: new finding_

When many Observers fault simultaneously and the handler is slow to drain, the
pending list grows without bound. Each pending entry lives in the faulting
Observer's WaitEntry struct (no external allocation), but a buggy handler that
never calls receive could cause all system Observers to accumulate pending
entries.

**For MVP:** acceptable — root Observer as sole handler won't exhibit this.
**For production:** consider a pending list depth limit per Field.

### Cap table growth retry complexity

The D40 growth protocol (fault → handler → grow → retry) is the highest-risk
implementation task. D-3.1a/b/c are settled. The retry-after-growth path
must reconstruct the original operation's context. Edge cases: what if the
handler's OWN table is full? Decision D-3.1c resolves this with escalation.

### Destroy cascade vs. cap table growth conflict

_gap analysis: new finding_

If an Observer has a fault handler that is itself, and its table has many caps,
the destroy operation initiates a cascade while the handler is the Observer
being destroyed. Circular dependency. Unlikely in well-formed systems but
possible in adversarial setups.

**For MVP:** acceptable — test scenarios won't create self-handling Observers.
**For production:** destroy should validate handler != self before initiating
fault-based cascade.

### "Framekernel" framing

Expert review consensus: the unsafe-in-frame/ discipline is good engineering
practice, but it is code organization, not hardware-enforced isolation (like
CHERI). Don't oversell externally.

---

## Deferrable Leaf Nodes

These items have settled interfaces and correct-but-unoptimized implementations
that satisfy the spec. They can be improved behind stable interfaces without
changing how modules interact. Listed here so they aren't forgotten.

### Fast-path assembly (D50/D69)

_Spec: "implementation-only — no separate mechanism"_

D50 settles the six conditions for fast-path eligibility. The code correctly
identifies when conditions are met. D50 explicitly defers assembly optimization.
Current full save/restore is correct, costs ~600-800 cycles instead of ~400.

**When to do it:** after IPC performance measurement shows the save/restore is
the bottleneck. Requires careful register-passthrough in `frame/arch/` — high
risk of subtle corruption if done wrong.

### ASID selective invalidation (beyond D101)

_Spec (D101): "wrap triggers full TLB broadcast and counter reset"_

Full flush on ASID wrap IS the spec. Selective invalidation (tracking live
ASIDs, only flushing stale ones) is a pure optimization. Current implementation
is correct.

**When to do it:** after SMP is working, if TLB flush cost is measurable in
cross-core scenarios.

### Demand paging / lazy PTE population (D40 internal)

_Spec (D40): "Lazy PTE population: kernel-internal per D12"_

Pre-allocation at Space creation is spec-valid. Lazy population (fault on first
touch, populate PTE on demand) is an optimization the kernel MAY do internally.

**When to do it:** when workloads need sparse address spaces or overcommit. Not
needed for MVP's small test scenarios.

### Supervision Field (D68)

_Spec (D68): "optional creation-time configuration parameter"_

D68 explicitly marks supervision as optional and lists mandatory-vs-optional as
unsettled. Not implementing it is valid.

**When to do it:** when pager chain liveness (D105) needs a concrete resolution.
D68's Pulsar watchdog is the leading candidate mechanism.

### KASLR

_Not in spec. Questionable threat model._

Kernel address space layout randomization. Entirely contained in `frame/boot/`
and `frame/arch/aarch64/mmu.rs`. With capability-based authority, the kernel's
VA layout is never exposed to userspace — the KASLR threat model (leaking kernel
addresses) is weaker than in syscall-based kernels.

**When to do it:** if the kernel ever exposes address information through timing
side channels or fault messages. Currently no evidence of this.

### Structural backing conservation optimization (D32)

_Spec (D32): "destroy returns Space cap"_

The return-Space-on-destroy protocol is implemented (verify in Phase 2.5). What
could be optimized: the SpaceManager's page tracking. Currently a byte counter
that ignores base addresses on return. A real frame allocator (bitmap/buddy)
would detect double-free and enable specific-frame reclamation.

**When to do it:** when the system runs long enough for frame-level accounting
to matter. The byte counter is correct for MVP's short-lived test scenarios.

---

## Spec Document Debt

Not blocking MVP but should be addressed at milestone boundary per CLAUDE.md
rule 6.

- D49 text contradicts D103 on WriteRegisters protocol — update D49
- D77-D92 and D103-D105 exist only as journal entries, not spec headings
- D62 cross-arena refcount vs. D53 lock ordering carve-out not formally
  reconciled
- Badge-closure + D45 routing interaction (open question from D64)
- D68 supervision Field not in D35/D95 creation protocol

---

## Reference

This plan consumes `remaining-work.md` as its audit input, plus four-agent
review (functionality gaps, expert assessment, decision audit, autonomy
strategy).

| remaining-work.md                   | This plan                         |
| ----------------------------------- | --------------------------------- |
| Category 1 (bugs 1.1-1.8)           | Phase 0 (0.1-0.8)                 |
| Category 2 (D88-D91 memory)         | Phase 1                           |
| Category 3 (spec gaps)              | Phase 3                           |
| Category 4 (bare-metal)             | Phase 4 (4.1, 4.2) + Risks        |
| Category 5 (spec docs)              | Spec Document Debt                |
| Category 6 (defense-in-depth)       | Phase 0 (0.9-0.13) — reclassified |
| Category 7 (intentionally deferred) | Deferrable Leaf Nodes             |

Items added by four-agent review:

- 0.5 IRQ message drop on full queue (gap analysis — new critical bug)
- 0.13 Boot invariant check debug-only (expert review — promoted to Phase 0)
- 0.14 Wire deferred fault delivery linkage (gap analysis — D18 not wired)
- 3.7 Nested fault handling (gap analysis — double-fault undefined)
- 3.4 severity upgraded to use-after-free (gap analysis)
- 0.2 reframed as unified delivery path (expert review — root cause is split
  code paths, not just missing wakeup)
- 12 design decisions requiring input (decision audit — consolidated in
  "Decisions Requiring Input" section)
- 3 new risks (Observer destroy Space reconstruction, pending list unbounded,
  destroy cascade vs. growth conflict)

Items reclassified from original plan:

- Cap table growth: deferred → Phase 3.1 (spec requires fault protocol)
- Badge-closure: Phase 3 optional → Phase 3.2 (spec requires notification)
- SlotTag wrap: Cat 6 → Phase 0.9 (D11 invariant violation)
- Scheduler duplicate: Cat 6 → Phase 0.10 (D2 invariant violation)
- Routing overlap: Cat 6 → Phase 0.12 (nondeterministic behavior)
- SMP: deferred → Phase 4 (crosses interfaces)
