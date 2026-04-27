# Benchmark & Workload Infrastructure

Plan for stress testing and performance characterization of the kernel. Assumes
the D26 auto-mapping path (ObserverInstallCap → wire_space_mapping) is complete:
installing a Space cap maps it, removing it unmaps it, CreateObserver allocates
a proper L1 table.

The existing syscall surface (25 operations) is sufficient for all workloads. No
kernel API changes are needed — only test infrastructure and benchmark binaries.

---

## Implementation strategy: breadth-first by layer

Each layer is fully complete and tested before the next begins. The layers are
ordered by dependency: each builds only on primitives proven in the layer below.

```text
Layer 1  Kernel mechanism (BRK #0x48)
   ↓     complete: handler prints structured data, resumes Observer
Layer 2  Full syscall surface (25 wrappers)
   ↓     complete: every wrapper ABI-verified, all typed op constants
Layer 3  Measurement primitives (Stats, Stopwatch, benchmark, cycles, burn)
   ↓     complete: single-Observer bench_emit → scripts/bench → results table
Layer 4  scripts/bench runner
   ↓     complete: builds, runs, collects BENCH lines, formats output
Layer 5  Multi-Observer harness (ChildBuilder, Barrier, share_field)
   ↓     complete: spawn + IPC verified with integration test
Layer 6  Benchmark binaries (Phases 5–8 workloads)
         each binary uses only proven primitives from Layers 1–5
```

Layers 1–2 can be done in a single session (small, mechanical). Layer 3 depends
on Layer 1 (cycles reads CNTVCT, bench_emit uses BRK #0x48). Layer 4 depends on
Layers 1+3 (parses BENCH lines, validates Stats output). Layer 5 depends on
Layer 2 (CreateObserver, ObserverInstallCap, ObserverWriteRegisters wrappers).

---

## Layer 1 — Kernel mechanism: BRK #0x48

**Problem.** Tests are pass/fail via BRK #0x42. Benchmarks need to report
quantitative results (latency distributions, throughput, cycle counts).

**Approach.** Add a non-terminal BRK opcode that emits structured data to serial
and resumes execution (unlike #0x42 which exits). This keeps the syscall surface
clean — the reporting channel is test infrastructure, not kernel API.

### Kernel change

BRK #0x48 ("benchmark data point"): the exception handler reads x0–x3 from the
current Observer's saved RegisterState, prints a structured line to serial, and
returns `DispatchResult::Resume` (does not exit). Format:

```console
    BENCH <x0> <x1> <x2> <x3>
```

Four u64 values, hex-encoded, space-separated. The tag (x0) identifies the
metric; x1 holds the value; x2/x3 are available for units or metadata. Userspace
decides the encoding.

Implementation: one match arm in `handle_el0_sync` (exception.rs), same pattern
as the existing BRK #0x43–#0x47 handlers. Read registers via
`read_typed_registers` (reuses the same RegisterState access pattern), print via
`println!`, advance PC via `observer_advance_pc`, return
`DispatchResult::Resume(observer)`.

### Layer 1 done-criteria

- BRK #0x48 prints a BENCH line and resumes execution (not exits)
- A minimal test binary emits a BENCH line then passes via BRK #0x42
- Existing test suite still passes

---

## Layer 2 — Complete syscall wrappers

**Problem.** tests/src/lib.rs wraps 6 of 25 syscalls. Multi-Observer benchmarks
need the rest.

All signatures below are verified against the kernel's actual dispatch paths in
core_manager.rs. The register layout is D47: x0–x3 = args, x4 = label/op_code,
x5 = target handle, x6 = user cap, x7 = reply info.

### Missing IPC wrappers

```rust
/// Call = Send + block-on-reply (SVC #3).
///
/// ABI: x0-x3 = data, x4 = label, x5 = target field handle,
/// x6 = user cap handle (CAP_ABSENT if none), x7 = reply badge.
/// Blocks until reply arrives. Returns the reply message.
fn call(
    handle: u64,
    label: u64,
    data: [u64; 4],
    user_cap: u64,
    reply_badge: u64,
) -> Message;

/// ReplyRecv = reply + receive next (SVC #4).
///
/// ABI: x0-x3 = reply data, x4 = reply label, x5 = reply field handle
/// (send-once cap), x6 = user cap (CAP_ABSENT if none),
/// x7 = receive field handle.
///
/// Sends reply via x5, then blocks on receive from x7. Returns the
/// next received message.
fn reply_receive(
    reply_handle: u64,
    recv_handle: u64,
    label: u64,
    data: [u64; 4],
    user_cap: u64,
) -> Message;
```

These are the hot-path operations for ping-pong benchmarks (D50 fast-path).

### Missing typed operation wrappers

All use the existing `typed_syscall(op, target, [args])` unless noted.

| Op  | Name                   | Wrapper + notes                                                                                                                                                                  |
| --- | ---------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| 0   | ObserverResume         | `fn observer_resume(handle: u64) -> TypedResult`                                                                                                                                 |
| 1   | ObserverInstallCap     | `fn observer_install_cap(observer: u64, source_cap: u64) -> TypedResult`                                                                                                         |
| 2   | ObserverWriteRegisters | `fn observer_write_registers(observer: u64, pc: u64, sp: u64, x0: u64, pstate: u64) -> TypedResult` — args = [pc, sp, x0, pstate & NZCV_MASK]                                    |
| 3   | ObserverReadRegisters  | `fn observer_read_registers(observer: u64) -> RegistersResult` — **custom wrapper** (returns x0=PC, x1=SP, x2=x0, x3=PSTATE; generic typed_syscall only captures x0)             |
| 4   | ObserverSuspend        | `fn observer_suspend(handle: u64) -> TypedResult`                                                                                                                                |
| 5   | ObserverChangeHandler  | `fn observer_change_handler(observer: u64, handler_field: u64, badge: u64) -> TypedResult` — args[0] = field handle, args[1] = badge                                             |
| 6   | ObserverSetScheduling  | `fn observer_set_scheduling(observer: u64, r: u64, t: u64) -> TypedResult`                                                                                                       |
| 8   | Clone                  | `fn clone_cap(handle: u64) -> TypedResult`                                                                                                                                       |
| 10  | Mint                   | `fn mint(handle: u64, badge: u64) -> TypedResult`                                                                                                                                |
| 12  | SpaceMerge             | `fn space_merge(handle: u64, adjacent: u64) -> TypedResult` — args[0] = adjacent Space handle                                                                                    |
| 14  | FieldSplit             | `fn field_split(field: u64, space: u64, badge_low: u64, badge_high: u64) -> TypedResult` — args = [space_handle, badge_low, badge_high] (Space consumed for new sub-Field)       |
| 15  | TimeSplit              | `fn time_split(handle: u64, units: u64) -> TypedResult`                                                                                                                          |
| 16  | CreatePulsar           | `fn create_pulsar(space: u64, field: u64, badge: u64, duration_ns: u64, period_ns: u64) -> TypedResult` — args = [field, badge, duration_ns, period_ns] (badge before durations) |
| 18  | CreateObserver         | `fn create_observer(space: u64, handler_field: u64, handler_badge: u64) -> TypedResult` — args = [handler_field, handler_badge]; R/T use kernel defaults, set via SetScheduling  |

Constants for all op codes (OP_OBSERVER_RESUME through OP_RESOURCE_REQUEST).
Existing wrappers (space_split, create_field, close, clock_read, destroy,
resource_request) are unchanged.

### ObserverReadRegisters custom wrapper

```rust
struct RegistersResult {
    pub ok: bool,
    pub pc: u64,
    pub sp: u64,
    pub x0: u64,
    pub pstate: u64,
}

fn observer_read_registers(observer: u64) -> RegistersResult {
    // SVC #0 with op_code=3, target=observer. On success:
    // x0=PC, x1=SP, x2=target's x0, x3=PSTATE. On error: x0 negative.
    // Must capture x0–x3 (generic typed_syscall only captures x0).
}
```

### Layer 2 done-criteria

- All 25 operations have wrappers
- Constants for all 20 typed op codes
- Wrappers compile and link in the tests/ crate
- Existing tests still pass (wrappers are additive)

---

## Layer 3 — Measurement primitives

Reusable building blocks in tests/src/lib.rs (or a tests/src/bench.rs module).
These are the foundation every benchmark depends on — they must be correct
before any benchmark is written.

### Benchmark reporting (depends on Layer 1)

```rust
/// Direct counter read — no syscall overhead.
/// Requires clock_access=true (D66), which the root Observer has.
/// Child Observers MUST NOT call this (faults at EL0).
fn cycles() -> u64;

/// Emit one benchmark data point via BRK #0x48.
fn bench_emit(tag: u64, value: u64, meta0: u64, meta1: u64);
```

### Statistics

```rust
const MAX_SAMPLES: usize = 1024;

struct Stats {
    min: u64,
    max: u64,
    sum: u64,
    count: u32,
    samples: [u64; MAX_SAMPLES],
}

impl Stats {
    fn new() -> Self;
    fn record(&mut self, value: u64);
    fn mean(&self) -> u64;
    /// Requires sorting the samples array (insertion sort, O(n²) but
    /// n ≤ 1024 and only called once after measurement completes).
    fn median(&self) -> u64;
    fn p99(&self) -> u64;
    /// Emit summary via bench_emit. Emits 5 BENCH lines:
    /// tag+0=min, tag+1=median, tag+2=p99, tag+3=mean, tag+4=count.
    fn emit(&self, tag: u64);
}
```

MAX_SAMPLES is bounded — benchmarks run a fixed iteration count and store raw
samples for percentile computation. No heap allocation. The sort is done once
after all samples are recorded (not on the hot path).

### Timing harness

```rust
struct Stopwatch { start: u64 }
impl Stopwatch {
    fn start() -> Self;        // calls cycles()
    fn elapsed(&self) -> u64;  // cycles since start
    fn lap(&mut self) -> u64;  // elapsed, then reset
}

/// Run `body` for `warmup + measure` iterations. First `warmup` are
/// discarded. Returns Stats over the `measure` iterations.
fn benchmark<F: FnMut()>(warmup: u32, measure: u32, body: F) -> Stats;
```

### Compute calibration

```rust
/// Burns approximately `n` counter ticks in a tight loop.
///
/// CONSTRAINT: calibration reads CNTVCT_EL0 (requires clock_access=true).
/// Must be called from the root Observer for calibration. If children
/// need burn_cycles, the root calibrates first and passes the
/// cycles_per_iteration ratio via x0 at spawn time.
///
/// The calibration loop measures cycles-per-iteration of the inner loop
/// and adjusts the iteration count accordingly.
fn burn_cycles(n: u64);
```

### Layer 3 done-criteria

- cycles() reads CNTVCT_EL0 correctly (verified by comparison with clock_read)
- bench_emit produces correct BENCH lines (verified via hypervisor serial)
- Stats computes correct min/max/mean/median/p99 (unit-testable pure logic)
- Stopwatch timing is monotonic and plausible
- benchmark() discards warmup iterations correctly
- burn_cycles calibration produces stable results across repeated calls

---

## Layer 4 — scripts/bench

Dedicated benchmark runner, separate from scripts/test. Builds the kernel,
builds benchmark binaries (convention: tests/src/bin/bench\_\*.rs), runs each
under the hypervisor, collects BENCH lines from serial output, formats a results
table.

### Protocol

Benchmark binaries:

- Named `bench_*.rs` (distinct from test binaries)
- Emit zero or more `BENCH <x0> <x1> <x2> <x3>` lines via BRK #0x48
- Signal completion via BRK #0x42 (same as tests)

scripts/bench:

- Builds kernel + benchmark binaries
- Runs each under hypervisor with --no-gpu --timeout (longer than tests — 30s)
- Captures BENCH lines from serial output
- Parses hex values, groups by tag
- Formats results table

Example output:

```console
    bench_pingpong      min=380  median=412  p99=520  mean=415  (10000 iters)
    bench_fanout_4      min=890  median=945  p99=1200 mean=960  (1000 iters)
```

scripts/test continues to run correctness tests only (bench\_\* files excluded).
scripts/bench is opt-in and slower.

### Layer 4 done-criteria

- scripts/bench builds and runs benchmark binaries
- BENCH lines are correctly parsed from serial output
- Results table is formatted and readable
- scripts/test excludes bench\_\* binaries
- At least one trivial benchmark (e.g. bench_noop: emit one BENCH line + pass)
  validates the full pipeline end-to-end

---

## Layer 5 — Multi-Observer harness

With D26 working, spawning a child Observer is a sequence of existing syscalls.
This layer wraps that sequence into ergonomic helpers. This is the hardest layer
— it composes many primitives and crosses trust boundaries.

### Child entry point convention

All benchmark Observers share one binary. The root Observer creates children and
sets their PC to different `fn() -> !` functions in the same binary. Since the
child's Space caps map to the same VA as the parent's (D26: "the base is a
property of the Space — all holders see the same Space at the same VA"),
function pointers are valid across Observers.

For code access: the root Observer clones its own code Space cap (or the root
Space, depending on the test) and installs it in the child via
ObserverInstallCap. The kernel maps it at the same VA in the child's page table.
Same for the stack Space — split a new one, install it, and set SP to point
within it.

### Helpers

```rust
struct ChildBuilder {
    structural_space: u64,     // consumed by CreateObserver
    handler_field: u64,        // fault handler Field
}

impl ChildBuilder {
    fn new(structural_space: u64, handler_field: u64) -> Self;

    // Create child, install parent's code Space, allocate + install
    // a stack Space, set PC to `entry`, set SP to top of stack Space,
    // set x0 to `arg`. Does NOT resume — caller controls when.
    fn build(self, entry: fn(u64) -> !, arg: u64) -> u64;  // returns Observer handle
}

// Create child, resume immediately. Convenience for simple cases.
fn spawn(entry: fn(u64) -> !, arg: u64) -> u64;

// Install a shared Field in both parent and child cap tables.
// Returns the handle in the child's table (parent already has it).
fn share_field(child: u64, field: u64) -> u64;
```

### Barrier synchronization

N Observers rendez-vous before measurement starts. Prevents measuring Observer
creation overhead in the benchmark results.

Implementation: a shared Field. Each participant Sends a "ready" message, then
Receives N-1 messages. When all have sent + received, the barrier is released.
Simple, uses only existing IPC, no new kernel mechanism.

**CONSTRAINT:** The shared Field must have queue capacity ≥ N-1 (every
participant Sends before any Receives, so N-1 messages may be in-flight). Size
the Field's backing Space accordingly.

```rust
struct Barrier { field: u64, count: u64 }
impl Barrier {
    fn new(field: u64, participant_count: u64) -> Self;
    fn wait(&self);  // Send + Receive loop
}
```

### IPC patterns (building blocks for benchmarks)

```rust
/// Tight Receive → reply-via-Send loop. Echoes data back.
/// Intended as a child Observer's entry point.
fn echo_server(field_handle: u64) -> !;

/// Tight Call loop, timing each round-trip. Reports Stats.
fn call_loop(field_handle: u64, iterations: u32) -> Stats;
```

### Measurement architecture

The root Observer (parent) does all timing. Children don't need clock_access —
they just do work and signal completion via IPC. This avoids the clock_access
gap on child Observers and keeps measurement centralized.

Pattern:

1. Root creates children, installs shared Fields
2. Root records `start = cycles()`
3. Root resumes all children
4. Children do work, Send completion message when done
5. Root Receives completions, records `end = cycles()`
6. Root computes and emits stats

For per-operation timing (e.g. individual IPC round-trips), the root IS one of
the participants — it does Call/ReplyRecv in a timed loop against a child echo
server.

### Layer 5 done-criteria

- ChildBuilder spawns a child Observer that executes at a given PC
- Child can access code and stack via installed Space caps
- share_field installs a Field cap in both parent and child
- Barrier synchronizes N Observers (tested with N=2)
- echo_server + call_loop complete a full ping-pong cycle
- All of the above verified by a dedicated integration test
  (tests/src/bin/bench_harness_test.rs or similar) that passes via BRK #0x42

---

## Layer 6 — Benchmark binaries

Each benchmark is a Rust binary in tests/src/bin/bench\_\*.rs. Only written
after the primitives they depend on are proven. Independent of each other — can
be implemented in any order.

### IPC workloads

#### bench_pingpong

Two Observers, one Field. Root creates echo-server child, then runs
`call_loop()` against it. Measures Call/ReplyRecv round-trip latency.

This is the D50 fast-path benchmark — the most important number for a
microkernel. Target: ~400 cycles per round-trip.

Reports: min, median, p99, mean latency in counter ticks.

#### bench_pingpong_slow

Same topology but attaches a user_cap to force the slow path. Measures the cost
difference between fast-path and slow-path IPC.

#### bench_send_receive

Single Observer, self-send. Measures raw Send + Receive (no cross-Observer
switch). Isolates queue enqueue/dequeue cost from context-switch cost.

#### bench_fanout

One server Observer, N worker Observers (N = 2, 4, 8). Server receives requests
via one Field, dispatches work via FieldSplit routing to per-worker Fields.
Measures per-worker latency and aggregate throughput.

#### bench_fanin

N producer Observers, one consumer. Producers Send to a shared Field as fast as
possible. Consumer Receives. Measures throughput and queue-full backpressure
rate.

#### bench_pipeline

Chain of 2, 4, 8 Observers. Each receives, transforms data, sends to next.
Measures end-to-end latency and per-hop cost. Shows whether the scheduler
cooperates with data-flow direction.

#### bench_queue_pressure

Fast sender, slow receiver (receiver burns cycles between Receives). Measures
queue-full rate, pending-list behavior (D18), and whether the sender blocks or
drops gracefully.

### Scheduler workloads

#### bench_fairness

N identical compute-bound Observers (burn_cycles in a loop), each recording how
many iterations they complete in a fixed wall-clock window. Root measures total
elapsed time, children report iteration counts via IPC. Under round-robin, all
should get equal shares.

#### bench_profiles

Observers with different R/T values (D57). Same compute loop. Measures actual
CPU share per Observer. Under the current round-robin scheduler, profiles should
have NO effect — this benchmark establishes the baseline for when a
profile-aware scheduler is implemented.

#### bench_mixed_load

Half the Observers do tight IPC (ping-pong), half do compute (burn_cycles).
Measures both IPC latency and compute throughput. Reveals whether one class
starves the other under load.

#### bench_yield_cost

Tight yield loop (Yield + increment counter). Measures the overhead of voluntary
preemption — how many cycles the kernel spends on a no-op reschedule.

#### bench_preemption_latency

One Observer blocked on Receive. N compute-bound Observers running. A producer
Sends to the blocked Observer's Field. Measures time from Send to the blocked
Observer actually executing — this is wake-to-run latency under contention.

### Capability & resource workloads

#### bench_resolve

Observer with cap table at 10%, 50%, 90% capacity. Performs typed syscalls (e.g.
ClockRead) to measure resolution cost at different table occupancy. D77
resolution is a fixed 5-check sequence — this verifies it's truly O(1)
regardless of table fullness.

#### bench_cap_churn

Tight loop: SpaceSplit → CreateField → Close. Measures create/destroy
throughput. Stresses the arena allocator and freelist management.

#### bench_cascade_delete

Build an N-deep object graph (Observer → Space → Field → ...), then Destroy the
root. Measures total cascade time (D33/D98 preemptible cascade). Reports
per-object cost and whether preemption points cause measurable overhead.

#### bench_space_fragmentation

Many small SpaceSplit calls, then SpaceMerge to recombine. Measures allocation
latency as the free list becomes fragmented. Relevant for long-running systems
where memory churn accumulates.

### Timer, fault, and SMP workloads

#### bench_pulsar_jitter

Create N Pulsars at different periods (1ms, 5ms, 10ms). Measure actual delivery
time vs. requested period over many cycles. Reports jitter (stddev of period
error). Stresses the per-core deadline array (D83, 32-entry limit).

#### bench_timer_storm

Create Pulsars that all expire within the same tick window. Measures delivery
spread — how long between the first and last delivery when many fire
simultaneously. Stresses the kernel-as-sender path and queue capacity.

#### bench_fault_roundtrip

Observer deliberately accesses an unmapped address, fault handler receives the
VmFault message (LABEL_VM_FAULT), modifies the faulting Observer's registers to
skip the fault, resumes it. Measures the full fault→deliver→handle→resume
round-trip.

#### bench_fault_under_load

Same as bench_fault_roundtrip but with concurrent IPC traffic on other
Observers. Measures whether fault delivery introduces latency interference to
unrelated IPC.

#### bench_cross_core_ipc

Two Observers pinned to different cores. Send on core 0, Receive on core 1.
Measures cross-core IPC latency (D56 mailbox + IPI path, no D50 fast-path since
different cores). Compare against same-core bench_pingpong to quantify the
cross-core tax.

#### bench_core_contention

N cores all sending to the same Field simultaneously. Measures throughput
degradation as core count increases. Stresses the Arena<Field> lock (D53).

---

## Implementation notes

### File organization

All benchmarks live in tests/src/bin/bench\_\*.rs. They use the same build
system, linker script, and test protocol as existing tests. The only distinction
is the naming convention (bench\_\* prefix) and that they emit BENCH lines via
BRK #0x48 in addition to BRK #0x42 for pass.

### Clock_access constraint

Child Observers created via CreateObserver have clock_access=false (D66). Only
the root Observer can read CNTVCT_EL0. All timing is done by the root Observer.
Children do work and report via IPC — they don't measure.

If a benchmark requires per-child timing, the child uses Send to deliver its raw
iteration count to the root, which divides elapsed time by total iterations.

### burn_cycles in child Observers

burn_cycles calibration reads CNTVCT_EL0 (clock_access=true). Children cannot
calibrate themselves. Two options:

1. Root calibrates, passes cycles_per_iteration ratio to child via x0 at spawn.
   Child's burn_cycles uses the pre-calibrated ratio.
2. Children use a fixed iteration count (no calibration). Less accurate but
   simpler. Acceptable when relative timing matters more than absolute.

Option 1 is preferred for benchmarks measuring fairness or scheduling share.

### Core affinity

ObserverSetScheduling controls R/T/P profile but not core pinning. For SMP
benchmarks that need Observers on specific cores, we rely on the placement
function (D56) scoring. If deterministic pinning is needed, it's a separate
design decision — not a benchmark infrastructure concern.

### Iteration counts

Benchmarks should run enough iterations for stable statistics. Guideline:

- Micro-benchmarks (ping-pong, yield): 10,000+ iterations, 100 warmup
- Meso-benchmarks (fanout, pipeline): 1,000+ iterations, 50 warmup
- Macro-benchmarks (cascade, fragmentation): 100+ iterations, 10 warmup

These are tunable per-benchmark. The benchmark() harness handles warmup
automatically.
