# Coverage Report

**Date:** 2026-04-28 **Overall:** 92.81% line coverage (22,407 lines measured,
1,610 uncovered) **Tool:** cargo-llvm-cov 0.6.x (LLVM source-based coverage)
**Command:**
`cargo llvm-cov --target aarch64-apple-darwin --summary-only --no-cfg-coverage`
**Tests run:** 1,158

---

## Files Below 50% Line Coverage

| File                              | Line Coverage | Lines | Uncovered | Classification |
| --------------------------------- | ------------- | ----- | --------- | -------------- |
| `frame/arch/aarch64/serial.rs`    | 0.00%         | 53    | 53        | hardware-only  |
| `frame/arch/aarch64/timer.rs`     | 0.00%         | 17    | 17        | hardware-only  |
| `frame/arch/aarch64/platform.rs`  | 0.00%         | 15    | 15        | hardware-only  |
| `frame/arch/aarch64/gic.rs`       | 31.58%        | 95    | 65        | hardware-only  |
| `frame/arch/aarch64/entropy.rs`   | 0.00%         | 35    | 35        | hardware-only  |
| `frame/arch/aarch64/mod.rs`       | 13.04%        | 46    | 40        | hardware-only  |
| `frame/arch/aarch64/sysreg.rs`    | 20.83%        | 96    | 76        | hardware-only  |
| `frame/arch/aarch64/exception.rs` | 0.00%         | 106   | 106       | hardware-only  |
| `frame/arch/aarch64/mmio.rs`      | 0.00%         | 13    | 13        | hardware-only  |
| `time_manager/mod.rs`             | 0.00%         | 48    | 48        | hardware-only  |

**All 10 below-50% files are hardware-only.**

---

### Classification Details

**`frame/arch/aarch64/serial.rs` — 0% (hardware-only)** Pure PL011 UART driver.
`putc()` calls `mmio::write32()` and `mmio::read32()` on physical device
addresses. The `SerialGuard` acquires a lock and reads
`LOCK_ENABLED`/`SERIAL_LOCK` atomics, but the entire write path terminates in
UART MMIO reads/writes. There is no logic separable from hardware access.

**`frame/arch/aarch64/timer.rs` — 0% (hardware-only)** ARM generic timer driver.
Every function (`init`, `tick`, `tick_count`) writes or reads `CNTV_TVAL_EL0`,
`CNTV_CTL_EL0`, or reads the atomic `TICK_COUNT`. The actual timer programming
uses `sysreg::set_cntv_tval_el0()` — a single-instruction inline asm MSR. The
`tick_count()` function only reads `TICK_COUNT` atomically, which could
theoretically be tested, but the counter only advances via hardware-triggered
interrupts. Not practically testable in isolation.

**`frame/arch/aarch64/platform.rs` — 0% (hardware-only)** Provides hardware base
addresses as constants and DTB-discovered values. The `init()` function is gated
with `#[cfg(target_os = "none")]` — it reads physical memory via a raw DTB
pointer and calls `frame::firmware::dtb::scan()`. The getter functions
(`core_count`, `ram_base`, `ram_size`, etc.) load from atomics initialized by
`init()`; they are trivially testable but contain no logic worth covering. The
critical `init()` path cannot be exercised without a real or simulated DTB.

**`frame/arch/aarch64/gic.rs` — 31.58% (hardware-only)** GICv3 distributor,
redistributor, and CPU interface driver. The 31.58% that IS covered are the
pure-logic helper functions: `redist_base_for_core()`, `is_sgi()`, constant
sanity checks — all tested in `#[cfg(test)]` at the bottom of the file. The
remaining 65 uncovered lines are `init_distributor()`, `init_redistributor()`,
`init_cpu_interface()`, and `send_sgi()` — all of which write to GIC MMIO
registers or program ICC system registers via inline asm. These require live GIC
hardware (or HVF emulation) to exercise.

**`frame/arch/aarch64/entropy.rs` — 0% (hardware-only)** Hardware RNG via the
`RNDR` instruction (FEAT_RNG) with timer jitter fallback. `init()` reads
`ID_AA64ISAR0_EL1` via inline asm; `random_u64()` calls `sysreg::rndr()` which
issues `mrs {val}, s3_3_c2_c4_0`; `jitter_u64()` calls `sysreg::cntpct_el0()` to
read the physical counter. All paths depend on privileged system register access
unavailable on the macOS host.

**`frame/arch/aarch64/mod.rs` — 13.04% (hardware-only)** The 13% covered are
`cntfrq_el0()` and `cntvct_el0()` wrappers which happen to be called through
other paths. Everything else — `disable_interrupts()`, `restore_interrupts()`,
`dump_panic_registers()`, `halt()`, `enable_pmu_el0()`, `tpidr_el1()`,
`set_tpidr_el1()`, `signal_panic()` — all issue inline asm or write MMIO. They
are arm-specific boot/exception utilities.

**`frame/arch/aarch64/sysreg.rs` — 20.83% (hardware-only)** System register
accessor macros. The ~20% that IS covered consists of registers read through
other paths (e.g., `cntfrq_el0`, `mpidr_el1` called by `cpu.rs` and `timer.rs`).
The uncovered 76 lines are write accessors (`set_vbar_el1`, `set_sctlr_el1`,
`set_tcr_el1`, `set_ttbr0_el1`, `set_ttbr1_el1`, `set_mair_el1`, ICC registers)
and barrier instructions (`dsb_sy`, `dsb_ish`, `dsb_ishst`, `tlbi_vmalle1is`,
`tlbi_vae1is`, `tlbi_vale1is`, `tlbi_aside1is`) — all single-instruction inline
asm requiring real hardware context.

**`frame/arch/aarch64/exception.rs` — 0% (hardware-only)** Exception vector
table and trap frame dispatch. The assembly vector table is included via
`#[cfg(target_os = "none")]`. All Rust functions in this file
(`el0_exception_handler`, `el1h_exception_handler`, etc.) are called only from
the assembly entry stubs and require the CPU to have taken an exception at EL0
or EL1. No host-side simulation of exception entry is possible without a
hypervisor harness.

**`frame/arch/aarch64/mmio.rs` — 0% (hardware-only)** Volatile MMIO read/write
primitives (`read32`, `write8`, `write32`). These functions accept a physical
address and perform `core::ptr::read_volatile`/`core::ptr::write_volatile`.
Calling them on the host with arbitrary addresses would fault. They are tested
transitively via gic.rs and serial.rs under bare-metal execution.

**`time_manager/mod.rs` — 0% (hardware-only)** The `SchedulerAlgorithm` enum and
trait implementations dispatch to `RoundRobin` and
`EarliestEligibleVirtualDeadline`. The 0% coverage is because
`SchedulerAlgorithm` methods (`enqueue`, `dequeue`, `pick_next`,
`should_switch_to`, `on_preempt`) are called only from
`CoreState::dispatch_ipc()` and `handle_timer()` — which in turn are only
invoked from the exception handler running on bare metal. The individual
scheduler implementations (`round_robin.rs`,
`earliest_eligible_virtual_deadline.rs`) have their own `#[cfg(test)]` suites.
The dispatch enum itself has no host-executable code paths because `CoreState`
cannot be constructed without a bare-metal environment.

---

## Improvement Candidates (above 50%, significant gaps)

| File                                                 | Line Coverage | Lines | Uncovered | Uncovered Areas                                                                                                                                           |
| ---------------------------------------------------- | ------------- | ----- | --------- | --------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `core_manager.rs`                                    | 91.04%        | 7,876 | 706       | VM fault translation, call_reply_recv, unreachable paths, observer_read_pc, observer_save/take_saved_syscall, observer_extend_cap_table                   |
| `frame/arch/aarch64/mmu.rs`                          | 79.31%        | 290   | 60        | MMU init sequence (init, init_secondary, configure_and_enable), helper functions (l3_index, linker_addr, ttbr_base_address, page_size, kernel_l2_root_pa) |
| `frame/cores.rs`                                     | 85.58%        | 721   | 104       | translate_vm_fault, call_reply_recv, observer_read_pc, observer_save_syscall, observer_take_saved_syscall, observer_extend_cap_table                      |
| `frame/slab.rs`                                      | 90.72%        | 194   | 18        | Slab::insert (new slot + freelist paths), Slab::for_each_mut                                                                                              |
| `time_manager/earliest_eligible_virtual_deadline.rs` | 94.39%        | 392   | 22        | Edge cases in EEVDF selection, preemption accounting                                                                                                      |
| `frame/arch/aarch64/psci.rs`                         | 82.35%        | 34    | 6         | PSCI call paths (cpu_on, cpu_off, system_reset)                                                                                                           |
| `communication.rs`                                   | 96.48%        | 965   | 34        | Error branches in send/receive/call/reply_recv edge cases                                                                                                 |
| `lib.rs`                                             | 95.15%        | 763   | 37        | init sequence, panic handler paths                                                                                                                        |
| `syscall.rs`                                         | 93.16%        | 190   | 13        | Unused SVC dispatch branches                                                                                                                              |

---

### Candidate Analysis

**`core_manager.rs` — 706 uncovered lines (largest gap)**

The uncovered code in `core_manager.rs` falls into two groups:

1. **VM fault translation (`translate_vm_fault`, lines 427-453):** Walks the
   Observer's capability table to find a Space containing the faulting address.
   Currently unreachable from host tests because fault delivery tests use
   synthetic fault caps rather than walking live cap tables under a real page
   fault.

2. **`call_reply_recv` wrapper (lines 554-572):** Wraps
   `communication::reply_recv` with raw pointer field access. Untested because
   reply_recv integration tests use the safe `communication::reply_recv`
   directly.

3. **`unreachable_unchecked` branches (lines 489, 523):** Defense-in-depth
   branches after exhaustive match arms in dispatch paths — by construction
   unreachable on correct inputs.

4. **Observer register helpers (lines 911-919, 952-980):** `observer_read_pc`,
   `observer_save_syscall`, `observer_take_saved_syscall` — called from
   `frame/cores.rs` but only exercised when `CoreState` dispatches a
   TypedOperation with a current Observer. These are indirectly tested in
   integration tests but the specific code paths via `frame/cores.rs` do not
   execute host-side (they require a live `RegisterState` pointer).

5. **`observer_extend_cap_table` (lines 994+):** Capability table extension on
   Observer resize — no test exists for this path yet.

**`frame/arch/aarch64/mmu.rs` — 60 uncovered lines**

Three categories:

1. **Init functions (`init`, `init_secondary`, `configure_and_enable`):** Write
   to SCTLR*EL1, TCR_EL1, TTBR0/1_EL1, MAIR_EL1, and perform TLB invalidation.
   These cannot run on the host — they would attempt to program real ARM system
   registers. The `#[cfg(target_os = "none")]` gate is not applied here, but the
   functions call
   `sysreg::set*\*`which are no-ops on the host (they compile to the inline asm`msr`
   instructions, which trap on Apple Silicon when not in a VM).

2. **Helper functions (`l3_index`, `linker_addr`):** `l3_index` is purely
   arithmetic — testable but not yet tested. `linker_addr` casts a linker symbol
   pointer to a `usize` — only meaningful with the real binary layout.

3. **Pure utilities (`ttbr_base_address`, `page_size`, `kernel_l2_root_pa`):**
   `ttbr_base_address` and `page_size` are `const fn` computations — both
   testable via host unit tests. `kernel_l2_root_pa` reads a raw pointer value
   from the L2_ROOT static — testable as a size assertion but semantically only
   meaningful on bare metal.

**`frame/cores.rs` — 104 uncovered lines**

The uncovered functions in `frame/cores.rs` are unsafe wrappers around Observer
raw pointers:

- `translate_vm_fault`: same as core_manager.rs — requires live cap table walk
- `call_reply_recv`: raw-pointer variant of communication::reply_recv
- `observer_read_pc`: reads PC from RegisterState via raw pointer cast
- `observer_save_syscall` / `observer_take_saved_syscall`: read/write
  `saved_syscall` field via raw pointer
- `observer_extend_cap_table`: extends cap table via raw pointer

These functions are called from the bare-metal exception dispatch path.
Host-side tests for `core_manager.rs` use test helpers that bypass
`frame/cores.rs`. Targeted tests could cover these by constructing valid
`Observer` instances with pinned memory.

**`frame/slab.rs` — 18 uncovered lines**

Two gaps:

1. **`Slab::insert` (lines 89-107):** The `insert` method (which allocates from
   freelist or grows) has 0% coverage despite `Slab` being otherwise
   well-tested. The existing tests exercise `allocate` (which delegates to
   `Arena::allocate`), `get`, `get_mut`, and `free` directly but never call
   `insert`. A targeted test calling `slab.insert(value)` would cover both the
   freelist reuse path and the push path.

2. **`Slab::for_each_mut` (lines 145-151):** Iterates all live slots with a
   mutable callback. No test calls this method. A simple test checking that
   `for_each_mut` visits exactly the allocated slots would cover this.

**`time_manager/earliest_eligible_virtual_deadline.rs` — 22 uncovered lines**

EEVDF scheduler edge cases. The 22 uncovered lines are in:

- Tie-breaking in `pick_next` when multiple observers have equal virtual
  deadlines
- `on_preempt` accounting when the run queue is empty at preemption time
- Edge case in `should_switch_to` when the candidate has a later deadline than
  current

**`frame/arch/aarch64/psci.rs` — 6 uncovered lines**

Three PSCI HVC call sites (`cpu_on`, `cpu_off`, `system_reset`) use
`#[cfg(target_os = "none")]` gating. The HVC instruction cannot be issued on the
host. The pure logic portions (function ID constants, return code decoding) are
covered by the existing `#[cfg(test)]` suite at 100%.

**`communication.rs` — 34 uncovered lines**

Scattered error branches in `send`, `receive`, `call`, and `reply_recv`. These
are mostly `CapError` variants that communication functions propagate but that
existing tests don't exercise (e.g., `CapError::TableFull` during cap transfer
in `call`).

**`lib.rs` — 37 uncovered lines**

The kernel init sequence (`kernel_main`) and panic handler paths. `kernel_main`
calls `mmu::init()`, `gic::init()`, `timer::init()` — all hardware-only. The
`#[panic_handler]` calls `serial::Writer` and
`frame::arch::dump_panic_registers()` — hardware-only.

---

## Summary

- **10 files below 50%** — all hardware-only (bare-metal AArch64 only; cannot be
  tested via `cargo test --target aarch64-apple-darwin`)
- **9 improvement candidates** above 50% with notable uncovered paths
- **706 uncovered lines in core_manager.rs** is the largest absolute gap but
  represents the highest-value target for new tests

---

## Recommended Targets for Plan 03

Ordered by coverage impact (most uncovered testable lines first):

### Priority 1: `frame/slab.rs` — `insert` and `for_each_mut` (~18 lines, trivial)

`Slab::insert` and `Slab::for_each_mut` are pure Rust with no unsafe or hardware
dependencies. Tests can construct a `Slab<u32>` directly. Expected effort: 2-3
new test functions, ~15 minutes.

**Specific targets:**

- `Slab::insert` with freelist slot reuse
- `Slab::insert` with new slot push
- `Slab::insert` at capacity returns `AllocError::OutOfMemory`
- `Slab::for_each_mut` visits exactly the allocated (non-freed) slots

### Priority 2: `frame/arch/aarch64/mmu.rs` — pure utility functions (~15 lines)

Three pure computations that run on the host:

- `l3_index(va)` — pure arithmetic, one test per boundary (0, 1, PAGE_SIZE-1,
  PAGE_SIZE)
- `ttbr_base_address(ttbr)` — bitmask extraction, test with known values
- `page_size()` — const fn, assert equals PAGE_SIZE

`linker_addr` and `kernel_l2_root_pa` are tied to the binary layout — do not
test. `init`, `init_secondary`, `configure_and_enable` are hardware-only — do
not test.

### Priority 3: `frame/cores.rs` — observer helpers via test harness (~40 lines)

The unsafe observer helpers (`observer_read_pc`, `observer_save_syscall`,
`observer_take_saved_syscall`) can be tested by constructing an `Observer` with
a properly initialized `RegisterState` (the host test harness already does this
for other cores.rs tests). This requires pinning memory for the `RegisterState`
but no hardware.

**Specific targets:**

- `observer_read_pc`: construct Observer with known PC, verify read
- `observer_save_syscall` / `observer_take_saved_syscall`: roundtrip test
- `observer_extend_cap_table`: extend from small to large capacity, verify
  bounds

### Priority 4: `time_manager/earliest_eligible_virtual_deadline.rs` — edge cases (~22 lines)

EEVDF edge cases require setting up scheduler state with specific virtual
deadlines. The test infrastructure for EEVDF already exists in the file's
`#[cfg(test)]` module. The uncovered paths are:

- Tie-breaking: two observers with equal `virtual_deadline` — verify
  deterministic selection
- `on_preempt` with empty queue — verify no panic
- `should_switch_to` with later-deadline candidate — verify returns false

### Priority 5: `communication.rs` — CapError propagation (~15 lines)

The uncovered `CapError` branches in IPC are exercised by constructing cap table
entries in specific invalid states. The existing `communication.rs` test
infrastructure makes this straightforward — add tests that trigger
`CapError::TableFull` during `call` cap transfer.

### Not recommended for Plan 03

- `core_manager.rs` 706 uncovered lines: most are either `unreachable_unchecked`
  branches (untestable by design) or require constructing complete kernel state
  with live RegisterState pointers. The ROI is lower than the above targets.
- All 10 below-50% hardware-only files: require bare-metal execution, not
  testable via host runner.
