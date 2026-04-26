# Gap Closure Plan: From Verified Interfaces to Working Kernel

Status: **approved** (ready for autonomous execution) Created: 2026-04-26
Reviewed: 2026-04-26 (ARM64 engineer, capability skeptic, execution pragmatist)

---

## Current state

102 derivations settled. 851 tests passing. 60K lines. All unsafe confined to
frame/ (140 blocks). Framekernel boundary holding. The kernel has verified
inter-module contracts — every interface is tested — but several syscall paths
return `InvalidState` instead of doing real work, and there are no multi-step
integration tests that compose syscalls into workflows.

## Goal

Reach the simplest correct implementation with distinct, independent leaf nodes.
Every settled derivation has a working code path. Every code path has tests.
Intentionally deferred items (badge tracking map, cross-core scheduling, ASID
recycling) stay deferred. The result is a kernel where autonomous iteration on
leaf algorithms (scheduling, hot-path performance) can begin, driven by
userspace stress tests.

## Feasibility assessment

**Fully autonomous.** All items below are either mechanical wiring of settled
interfaces, design decisions with clear rationale, or test infrastructure.

Resolved during plan review and multi-perspective agent review:

- **WriteRegisters/ReadRegisters:** Inline in syscall args (PC, SP, x0, PSTATE).
  This is a **design decision (D103)**, not a forced conclusion. Arguments: A5
  (kernel absorbs complexity — don't leak RegisterState layout across ABI), D35
  (composable setup needs only 2-4 values), and simplicity (no buffer
  resolution, no Space cap arithmetic). D97 originally specified full
  RegisterState transfer and deferred the memory protocol; D103 resolves that
  deferral as inline-only for now. Full 816-byte transfer deferred to future
  debugging/checkpoint extension. **Known gap:** handlers cannot modify x1-x7 on
  a faulted Observer — typed syscall returns use x0 (D49), so no concrete
  scenario requires this today, but the gap is acknowledged. **SECURITY: PSTATE
  must be masked to NZCV only (0xF000_0000).** Unmasked PSTATE allows userspace
  to set SPSR_EL1.M bits, causing eret to enter EL1 (kernel privilege
  escalation). ARM ARM: only bits [31:28] (N, Z, C, V) are safe for EL0 to
  control.
- **ResourceRequest:** Fault-routed for non-root Observers (D31 pager chain).
  **Root-Observer special case:** when the root Observer (handler = kernel)
  makes a ResourceRequest, the kernel allocates directly from SpaceManager pool
  — it doesn't fault-route to itself. D31 explicitly says "allocate from pool or
  deny" for the kernel-as-root-pager case.
- **Pager chain depth:** The chain does NOT recurse on the kernel stack —
  `deliver_fault()` enqueues a message into a Field and returns. So kernel stack
  overflow is not a risk. The chain unrolls through scheduling rounds. **Open
  question (not closed):** liveness under live-but-perpetually-faulted handlers
  is unresolved. "Bounded by arena capacity" is technically true but does not
  guarantee timely progress. D68 handles dead handlers (cap invalidation). The
  live-but-stalled case is a liveness concern, not a safety concern — documented
  for future exploration.
- **Async IPC justification:** Already derived in D13 from A3 (generic requires
  both patterns) + D12 (faults require async delivery). No gap.
- **Block/unblock in IPC dispatch:** Required for IPC state machine correctness.
  An Observer whose `state` field says Runnable but is actually linked in a
  Field's waiter list creates scheduler inconsistency. Additionally, fault
  delivery (D100) calls `observer.fault()` which checks state — wrong intern
  state causes wrong transitions. Existing frame/ helpers
  (`observer_set_blocked`, `observer_unblock` in cores.rs) make this ~6 lines.

**Intentionally skipped:** Badge tracking map (D17), cross-core scheduling
(D56), ASID recycling (D101), level-triggered IRQ ack, per-core arena sharding.
These are optimization/extension concerns that need usage data, not
implementation.

---

## Phase 0: Hypervisor boot proof (AUTONOMOUS)

**Why first:** 851 unit tests run on aarch64-apple-darwin. The bare-metal code
paths — exception vectors, MMU enable, timer setup, GIC init — are
`#[cfg(target_os = "none")]` and untested. Expert review identified that this
class of gap can hide real bugs (the test suite tests `build_tcr_split()` but
not the boot path's `configure_and_enable()` — two independent TCR
constructions). One integration test that boots the kernel and exits cleanly is
worth more than 200 unit tests for catching hardware-path bugs.

**Good news:** the kernel already boots and exits cleanly today. Phase 0
formalizes this into a repeatable test.

### 0a. Boot + exit test

Build kernel, run via hypervisor, verify serial output includes boot banner,
verify clean exit (PSCI SYSTEM_OFF or timeout without panic).

```sh
hypervisor target/aarch64-unknown-none/debug/kernel --no-gpu --timeout 5
```

Parse serial output for expected markers. Fail if panic or hang.

### 0b. Boot + timer IRQ test

Verify the kernel takes at least one timer interrupt. **Note:** `handle_timer`
currently emits zero serial output — there is nothing to parse. Two options:

1. Add a one-line `println!` in the timer path (simple, slightly noisy)
2. Design a test binary that counts timer interrupts via a counter register and
   reports the count on exit

Option 1 is sufficient for Phase 0. The test binary approach is Phase 4
territory.

### 0c. Defensive assertions

- PA alignment `debug_assert!` in page_table.rs descriptor construction
- RegisterState compile-time size assertion (coupling between Rust struct and
  assembly offsets)
- Document in `configure_and_enable()` why EPD1=1 is correct for the TTBR0-only
  identity map (distinct from `build_tcr_split()`'s EPD1=0 for the future
  TTBR0/TTBR1 split)

**Estimate:** ~50 lines of test infrastructure, ~20 lines of assertions.

---

## Phase 1: Close syscall stubs (AUTONOMOUS)

### 1a. SpaceSplit — create new Space from split

**Location:** core_manager.rs:1492 **What exists:** `space.split()` works
(mutates source, returns new VA/size). Arena allocation pattern proven in
CreateObserver/CreateField/CreatePulsar. Cap table install pattern proven in
InstallCap. **What's missing:** After split succeeds, allocate arena slot →
construct new Space → install cap in sender's table → return handle. **Note:**
Space has an `l3_table_pa` field that CreateField doesn't deal with. For host
tests this can be 0 (following SpaceMerge pattern). The agent must not create a
bare-metal soundness hole. **Estimate:** ~60-80 lines. More complex than
CreateField due to Space fields.

### 1b. TimeSplit — create new Time from split

**Location:** core_manager.rs:1771 **What exists:** `time.split()` works. Same
arena/cap pattern as SpaceSplit. **What's missing:** Identical to SpaceSplit but
for Time arena. **Estimate:** ~60-80 lines.

### 1c. ClockRead — read virtual timer counter

**Location:** core_manager.rs:1867 **Design:** Settled by D47/D49. No cap rights
required (D52: `Rights::empty()`). **Frame/ helper already exists:**
`read_counter_ticks()` at frame/cores.rs:786-788 calls
`crate::frame::arch::cntvct_el0()`. No new frame/ code needed. CNTVCT_EL0
(virtual counter) is correct — the kernel builds all timer logic around the
virtual timer, and CNTPCT_EL0 (physical) may trap to EL2 in the hypervisor.
**Must also set** `observer.clock_access = true` on the calling Observer. This
controls CNTKCTL_EL1.EL0VCTEN on the next context restore (exception.S:430-432).
Without it, the EL0 process gets access denied on a future direct
`mrs CNTVCT_EL0`. **Estimate:** ~15 lines in dispatch only.

### 1d. ResourceRequest — fault-routed resource acquisition

**Location:** core_manager.rs:2010 **Design:** D31 pager chain. Two paths:

**Non-root case:** Construct `FaultType::ResourceRequest { resource, quantity }`
from args, deliver via `deliver_fault()` to the Observer's handler Field. The
handler (userspace pager) decides whether to grant resources. **What exists:**
`FaultType::ResourceRequest` variant (fault.rs:54-58), `deliver_fault()`, fault
message construction. **What's missing:** Read resource type from args[0],
quantity from args[1], construct FaultType, call
`self.dispatch_fault(fault, kernel_state)`. Function signatures align cleanly.

**Root-Observer case:** When the handler is the kernel (root Observer's handler
cap at slot 0 → kernel-managed Field), the kernel allocates directly from
SpaceManager pool. D31: "allocate from pool or deny." Implementation: detect
root handler → `kernel_state.space_manager.acquire().allocate_pages()` →
construct new Space in arena → install cap → return handle. If pool exhausted,
return error (deny).

**Estimate:** ~40 lines (15 for fault routing + 25 for root allocation).

### 1e. WriteRegisters/ReadRegisters — inline register transfer

**Location:** core_manager.rs:1101, 1109 **Design decision (D103):** Inline in
syscall args.

**ABI layout** (from TypedRegisters in syscall.rs:96-100):

- x5 = target_handle (target Observer cap)
- x0 = args[0] → PC (target's ELR_EL1)
- x1 = args[1] → SP (target's SP_EL0)
- x2 = args[2] → x0 (target's initial x0 — note: caller's x2 carries what
  becomes the target's x0, a non-obvious ABI wrinkle)
- x3 = args[3] → PSTATE (target's SPSR_EL1, **masked**)

**SECURITY — PSTATE masking (mandatory):**

```rust
const PSTATE_USER_MASK: u64 = 0xF000_0000; // NZCV only
let safe_pstate = args[3] & PSTATE_USER_MASK;
```

Unmasked PSTATE allows setting SPSR_EL1.M[4:0] to EL1 mode, causing eret to
enter kernel privilege. ARM ARM D1.7: SPSR_EL1 is restored verbatim by eret.
Only bits [31:28] (NZCV condition flags) are safe for userspace to control.

**WriteRegisters:** Validate target is Inert or Faulted (D39). Read args, apply
PSTATE mask, write into target Observer's RegisterState via frame/ helper.
Existing helpers in frame/cores.rs follow established pattern.
**ReadRegisters:** Read PC/SP/x0/PSTATE from target Observer's RegisterState,
write into caller's result registers. **Estimate:** ~40 lines each, plus ~20
lines of frame/ helpers.

### 1f. Destroy cascade — preemptible version

**Location:** core_manager.rs:1219 (current synchronous loop) **Design:** D98
fully specified. `begin_cascade()` and `cascade_step()` exist and are tested.

**Structural additions needed (found during review):**

1. Add `destroyer_ptr: Option<NonNull<Observer>>` to `CascadeContinuation`
   (capability.rs:396) — handle_timer needs this to re-enqueue the destroyer
   when cascade completes.
2. Add
   `observer_cascade_step(ptr: NonNull<Observer>, state: &mut CascadeState) -> bool`
   to frame/cores.rs — analogous to how `observer_close_cap` wraps
   `Table::close`.
3. Update `make_core_state()` and related test constructors (~40 call sites) if
   CascadeContinuation field addition changes CoreState construction.

**Placement in handle_timer:** After Pulsar deadline scan, before
`schedule_next()`. Must not run before Pulsar scan (cascade_step could free a
Pulsar slot, staling the scan). Must run before schedule_next so the unblocked
destroyer appears in the run queue.

**Estimate:** ~100-120 lines including structural additions and test updates.

### 1g. Block/unblock synchronization in IPC dispatch

**Location:** core_manager.rs:742, 772, 908, 922 **Why required:** IPC state
machine correctness. An Observer in `Runnable` state that is actually linked in
a Field's waiter list creates scheduler inconsistency. Also affects fault
delivery (D100): `observer.fault()` checks state and rejects transitions from
wrong states. **Implementation:** Existing helpers `observer_set_blocked()` and
`observer_unblock()` in frame/cores.rs take `NonNull<Observer>` — which all four
TODO sites already have in scope (`receiver_ptr`, `server_ptr`, etc.).
**Estimate:** ~6 lines. Replace each TODO with the appropriate helper call.

### Phase 1 tests

Each stub gets at minimum:

- Happy path: operation succeeds, result accessible
- Error paths: arena full, cap table full, insufficient resources, wrong state
- Integration: split → use new object → destroy original → new object still
  works
- WriteRegisters: verify PC/SP/x0/PSTATE written correctly; verify PSTATE
  masking (set M bits, verify they are stripped); verify rejection when target
  is Runnable
- ReadRegisters: verify returned values match what was written
- ResourceRequest non-root: verify fault message delivered to handler Field with
  correct resource type and quantity
- ResourceRequest root: verify kernel allocates from pool, returns valid Space
  cap; verify denial when pool exhausted
- ClockRead: verify non-zero return; verify clock_access bit set on Observer

---

## Phase 2: Integration test infrastructure (AUTONOMOUS)

The 851 tests are unit tests — they verify individual interfaces. No test
composes multiple syscalls into a workflow. This phase builds the test
infrastructure and writes the scenario tests.

### 2a. Test scenario builder

A helper that constructs a full kernel context for multi-step tests:

```rust
struct TestScenario {
    kernel_state: KernelState,
    core: CoreState<RoundRobin>,
    root_space: ObjectId,
    root_time: ObjectId,
    root_observer: NonNull<Observer>,
}
```

Methods for common operations: `grant_send_cap()`, `create_observer()`,
`dispatch_and_resume()`, `assert_observer_state()`, etc.

Strong existing patterns: `make_kernel_state()`, `make_core_state()`,
`make_sender_with_cap()`, `make_space_in_arena()`, `make_field_in_arena()` are
all present in core_manager.rs test module.

**Estimate:** ~150 lines of test infrastructure.

### 2b. Multi-step workflow tests

Scenarios that exercise the full lifecycle:

1. **Create + IPC + Destroy:** Create Field → create two Observers →
   Send/Receive between them → destroy Field → verify Observers unblocked with
   fault

2. **Fault delivery chain:** Observer executes invalid syscall → fault
   dispatched to handler Field → handler Observer receives fault message → reads
   fault cap → resumes faulted Observer

3. **Space split + map + use:** Split Space → create Observer in new Space →
   Observer runs → destroy original Space → new Space still valid

4. **Capability revocation cascade:** Observer A holds cap to Field → Observer B
   holds derived cap → close A's cap → verify B's derived cap also closed →
   verify cascade was preemptible (timer could interrupt)

5. **Timer + Pulsar lifecycle:** Install Pulsar deadline → advance timer →
   verify fire message delivered → verify repeating Pulsar rearms → destroy
   Pulsar → verify no more fires

6. **Nested IPC (Call/Reply chain):** A calls B → B calls C → C replies to B → B
   replies to A → verify all three resume correctly with correct message data

7. **Resource request flow:** Observer calls ResourceRequest → kernel delivers
   fault to handler → handler grants Space via IPC → original Observer receives
   resources

**Estimate:** ~500 lines of test code across 7 scenarios.

### 2c. Observer module coverage

observer.rs has 9 test functions for 596 lines. Missing coverage:

- `block()` state transitions (Runnable → various WaitStates)
- `suspend()` + `resume()` interactions
- `fault()` delivery and state transition
- `set_scheduling()` profile assignment
- `revoke()` compute removal and state cleanup

**Estimate:** ~15 additional tests, ~200 lines.

### 2d. Concurrency contention tests

D1 designs an SMP kernel. D53 defines lock ordering. Add multi-threaded tests
using `std::thread`:

- Two threads contending on the same arena (allocate/free races)
- Cap revocation racing against concurrent IPC send on the same Field

**Scoped down from original plan:** D53 lock ordering is documented but NOT
enforced at runtime (`Lock::acquire()` has no per-thread order tracking). Cannot
test ordering validation that doesn't exist. Lock ordering enforcement is a
separate future work item. These tests validate data-race safety of the
primitives under contention only.

**Estimate:** ~120 lines.

---

## Phase 3: Frame boundary test coverage (AUTONOMOUS)

### 3a. Testable frame/ code

| Module                | Lines | Current tests | Testable on host?                        |
| --------------------- | ----- | ------------- | ---------------------------------------- |
| frame/fields.rs       | 563   | 0             | Partially — routing table logic is pure  |
| frame/cores.rs        | ~400  | 17            | Partially — register marshalling helpers |
| frame/firmware/dtb.rs | 247   | 23            | Yes (already tested)                     |
| frame/slab.rs         | 421   | 0             | Partially — bitmap math, page accounting |
| frame/mapping.rs      | 156   | 0             | Yes — VA assignment is pure arithmetic   |
| frame/capabilities.rs | 171   | 0             | Partially — cap table structural backing |

**Priority:** fields.rs routing table logic (used by IRQ dispatch), mapping.rs
VA assignment (used by Space operations), slab.rs bitmap accounting.

### 3b. Not testable on host (bare-metal only)

| Module                          | Lines | Why                          |
| ------------------------------- | ----- | ---------------------------- |
| frame/arch/aarch64/exception.rs | 522   | Vector table, EL transitions |
| frame/arch/aarch64/timer.rs     | 64    | System register access       |
| frame/arch/aarch64/sysreg.rs    | 328   | System register wrappers     |
| frame/arch/aarch64/serial.rs    | 134   | MMIO writes                  |
| frame/boot.rs                   | 391   | Whole-kernel init sequence   |

These get tested via the hypervisor in Phase 4, not via `cargo test`.

**Estimate:** ~25 new tests, ~300 lines for testable frame/ code.

---

## Phase 4: Userspace test runner (AUTONOMOUS)

D102 established the foundation: flat binary loading via DTB `--module`, root
Observer creation, EL0 entry. The 16-byte fallback binary proves the path works.
This phase builds a proper test harness on top of it.

### 4a. Test binary protocol

Convention for test binaries:

- Exit with `svc #5` (Yield) + x0 = 0 means PASS
- Exit with `brk #N` means FAIL (N = error code)
- Serial output via kernel print (kernel logs Observer state on fault/exit)
- Test binary is a flat aarch64 binary, loaded at a fixed VA

### 4b. Test runner script

**Toolchain:** Apple clang (`/usr/bin/clang --target=aarch64-unknown-none`) for
assembly, `rust-lld` (from nightly toolchain at
`~/.rustup/toolchains/nightly-aarch64-apple-darwin/lib/rustlib/aarch64-apple-darwin/bin/rust-lld`)
for linking flat binaries. No `aarch64-none-elf-as` needed.

A script that:

1. Assembles a test binary from a `.S` source file via clang + rust-lld
2. Builds the kernel
3. Runs `hypervisor kernel --module test.bin --no-gpu --timeout 5`
4. Parses serial output for PASS/FAIL
5. Reports results

### 4c. Initial userspace test suite

Single-syscall tests that verify the kernel's ABI:

| Test             | What it exercises                                    |
| ---------------- | ---------------------------------------------------- |
| yield_returns    | Yield syscall returns to caller                      |
| clock_read       | ClockRead returns non-zero timer value               |
| send_receive     | Two-observer IPC (needs multi-observer bootstrap)    |
| cap_close        | Close a cap, verify handle becomes invalid           |
| fault_handler    | Trigger fault, verify handler receives message       |
| space_split      | Split Space, verify both halves accessible           |
| resource_request | Request resources, verify fault delivered to handler |

### 4d. Multi-observer bootstrap

**Depends on Phase 1e (WriteRegisters)** — the bootstrap sets Observer B's PC
and SP, which IS the WriteRegisters syscall.

D102 sketched multi-Observer bootstrap but only root Observer is created. For
IPC tests, extend the boot path:

- Root Observer creates Field (CreateField syscall)
- Root Observer creates second Observer (CreateObserver)
- Root Observer installs send cap in Observer B's table (InstallCap)
- Root Observer writes B's registers (WriteRegisters — inline PC/SP/x0)
- Root Observer resumes Observer B
- Both Observers execute, IPC between them

**Estimate:** ~200 lines of test runner, ~50 lines per test binary, ~7 initial
tests.

---

## Phase 5: Journal entries for design decisions (AUTONOMOUS)

Short journal entries documenting resolutions for items deferred by the D-chain.

### D103: WriteRegisters/ReadRegisters — inline register protocol

**Decision (not observation):** Resolves D97's deferred memory region
designation. Inline in syscall args: PC, SP, x0, PSTATE (masked to NZCV).

Arguments for: A5 (don't leak RegisterState layout across ABI), D35 (composable
setup needs 2-4 values), simplicity (no buffer resolution). Arguments against:
D97 originally specified full batch transfer; inline cannot modify x1-x7
(handlers needing arbitrary register modification must use a future buffer-based
extension).

This is a design choice with tradeoffs, not a derivation-forced conclusion. The
inline approach covers all concrete use cases today (initial setup needs
PC/SP/x0; fault resolution needs PC; typed returns use x0). The buffer extension
is not foreclosed.

### D104: ResourceRequest dispatch — dual-path implementation

**Settles:** ResourceRequest implementation. Two paths forced by D31:

- Non-root: fault-route to handler Field (same as hardware faults)
- Root: kernel allocates directly from SpaceManager pool (D31: "allocate from
  pool or deny")

Root detection: check if handler cap at slot 0 points to kernel-managed Field.

### D105: Pager chain — no kernel-stack recursion; liveness is open

**Observation (partial):** deliver_fault() enqueues into a Field and returns —
the chain does not recurse on the kernel stack. Stack overflow is not a risk.
The chain unrolls through scheduling rounds.

**Open question (not closed):** Liveness under live-but-perpetually-faulted
handlers is unresolved. "Bounded by arena capacity" is technically true but does
not guarantee timely progress. D68 handles dead handlers (cap invalidation
detects destroyed Fields). The live-but-stalled case is a liveness concern for
future exploration, possibly via Pulsar watchdog (D44 supervision pattern from
D68).

---

## Phase 6: Benchmark harness + stress tests (AFTER PHASES 0–4)

### 6a. Benchmark infrastructure

Cycle counting via CNTVCT_EL0 in the hypervisor. Measure:

- IPC round-trip latency (Send + Receive between two Observers)
- Syscall entry/exit cost (Yield round-trip)
- Cap resolution cost (handle → object reference)
- Timer interrupt handling latency

Report as cycle counts per operation. These validate the cost assumptions from
D1 (cold-path reads "effectively free under cache coherence") and D50 (fast-path
IPC at rendezvous speed).

### 6b. Stress test workloads

| Test                | What it stresses                              |
| ------------------- | --------------------------------------------- |
| IPC throughput      | Tight send/receive loop, measures raw latency |
| Observer churn      | Create/destroy in a loop, catches leaks       |
| Space fragmentation | Split/merge repeatedly, catches fragmentation |
| Timer storm         | Many Pulsars with short deadlines             |
| Fault recovery      | Repeated fault + restart by handler           |
| Mixed workload      | All above combined, multiple Observers        |

**Estimate:** ~100 lines per stress test, ~600 lines total.

---

## Execution order and dependencies

```text
Phase 0 (boot proof) ← FIRST: prove the kernel boots before building on it
  │
Phase 1 (close stubs) ← mechanical wiring + design decisions
  ├── 1a SpaceSplit   ─┐
  ├── 1b TimeSplit     ─┤
  ├── 1c ClockRead     ─┤── no inter-dependencies, but 1f and 1g
  ├── 1d ResourceReq   ─┤   both touch core_manager.rs — do sequentially
  ├── 1e WriteRegisters─┤   if parallel agents, not parallel file edits
  ├── 1f Cascade       ─┤
  └── 1g Block/unblock ─┘
         │
Phase 2 (integration tests) ← needs Phase 1 for full scenarios
  ├── 2a Scenario builder ─┐
  ├── 2b Workflow tests    ─┤── 2b–2d depend on 2a
  ├── 2c Observer coverage ─┤
  └── 2d Contention tests  ─┘
         │
Phase 3 (frame tests) ← independent of Phase 2, can run in parallel
         │
Phase 4 (userspace runner) ← needs Phase 0 + Phase 1 (especially 1e)
  ├── 4a Protocol
  ├── 4b Runner script
  ├── 4c Initial suite
  └── 4d Multi-observer bootstrap ← explicit dependency on Phase 1e
         │
Phase 5 (journal entries) ← independent, can run anytime
         │
Phase 6 (benchmarks + stress tests) ← needs Phase 4 runner
```

**Phases 1, 3, and 5 can run in parallel.** Phase 2 needs Phase 1. Phase 4 needs
Phases 0 and 1 (especially 1e). Phase 6 needs Phase 4. Within Phase 1, items
1a-1e are independent, but 1f and 1g both modify core_manager.rs — do them
sequentially if in the same worktree.

## Cleanup during execution

- Remove `// TODO: remove` markers in gic.rs:31 and sysreg.rs:19
- Update src/CLAUDE.md gap list (cap table capacity is solved, IRQ routing is
  solved, Pulsar deadline queue is solved, WriteRegisters/ReadRegisters resolved
  via D103, ResourceRequest resolved via D104)
- Ensure the 1 ignored test (D17 badge tracking) stays ignored with clear
  explanation
- Document in configure_and_enable() why EPD1=1 is correct for the TTBR0-only
  identity map (distinct from build_tcr_split()'s EPD1=0 for the future split)

## What "done" looks like

- Zero syscall stubs for settled derivations
- Hypervisor proves the kernel boots and takes interrupts
- Integration tests cover 7+ multi-step workflows
- Contention tests validate arena safety under concurrent access
- Observer module has >80% method coverage
- Frame boundary testable code has tests
- Userspace test runner can build, run, and report on .S test binaries
- 7+ userspace tests exercise the syscall ABI
- Benchmark harness measures IPC latency, syscall cost, cap resolution
- All leaf algorithms behind trait interfaces, swappable for optimization
- scripts/verify still passes
- Test count: targeting ~1050+ (currently 851)
- Three journal entries (D103–D105) document design decisions and open questions
- PSTATE masking enforced on all register write paths
