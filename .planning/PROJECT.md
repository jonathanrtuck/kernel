# Kernel Verification

## What This Is

Systematic verification infrastructure for an ARM64 microkernel. The kernel has ~80k lines of Rust with ~1,975 unit tests and 34 bare-metal userspace tests, but lacks property-based testing, coverage measurement, structured unsafe auditing, syscall fuzzing, and concurrency model checking. This milestone closes those gaps, ordered by confidence gained per effort invested.

## Core Value

Every kernel invariant is tested by at least one technique that can find bugs the developer didn't anticipate — not just bugs they thought to check for.

## Requirements

### Validated

- Existing unit tests (~1,975 `#[test]` functions across source files)
- Existing bare-metal tests (34 tests: 18 assembly + 16 Rust, including 7 SMP control-plane tests)
- Framekernel discipline (all unsafe confined to `src/frame/`, 406 SAFETY comments)
- Runtime assertions (~1,900 `assert!`/`debug_assert!` across codebase)
- Pre-commit verification gate (`scripts/verify`)

### Active

- [ ] Property-based testing for core data structures
- [ ] Structured unsafe audit of frame/ with gap remediation
- [ ] Code coverage measurement and blind-spot elimination
- [ ] Syscall-level fuzzing via bare-metal test harness
- [ ] Concurrency model checking for IPC and scheduling protocols

### Out of Scope

- Formal verification with Verus — highest effort, deferred to future milestone
- GUI test harness or visual testing — kernel is headless
- Performance regression testing — separate concern from correctness
- Userspace-level testing — this milestone targets kernel internals only

## Context

- Rust nightly, `no_std`, ARM64 target — constrains which tools are available (no Miri for bare-metal, no std-dependent fuzzing frameworks)
- Host unit tests run on `aarch64-apple-darwin`; bare-metal tests run under `hypervisor` (Apple Hypervisor.framework)
- The kernel uses capability-based security — cap encoding/decoding, handle tables, and arena allocators are high-value proptest targets
- All unsafe is in `src/frame/` with SAFETY comments — but ~37 blocks may lack comments, and no tool-assisted verification of claims exists
- SMP tests already found 3 concurrency bugs in the last session — probabilistic testing works but doesn't guarantee coverage of all interleavings

## Constraints

- **Target**: `aarch64-unknown-none` bare-metal — many Rust testing tools assume `std`; host-side testing must mirror bare-metal types
- **Build system**: Cargo with nightly features — proptest and coverage tools must integrate cleanly
- **Hypervisor**: Apple Hypervisor.framework via custom `hypervisor` binary — syscall fuzzing runs here, not QEMU
- **No external CI**: All verification runs locally via `scripts/verify` and `scripts/test`

## Key Decisions

| Decision | Rationale | Outcome |
|----------|-----------|---------|
| Proptest before Verus | Proptest finds unknown unknowns cheaply; Verus proves known properties expensively | — Pending |
| Unsafe audit when frame/ stabilizes | Audit is most valuable when the surface isn't actively changing | — Pending |
| Host-side concurrency modeling | Bare-metal has no threading library; model protocols in std-compatible harness | — Pending |

## Current Milestone: v1.0 Exhaustive Verification

**Goal:** Systematically close every testing gap in the kernel, ordered by confidence gained per effort invested.

**Target features:**
- Property-based testing (proptest) for capability encoding, arena allocation, handle tables, scheduler properties
- Structured unsafe audit of all ~443 unsafe occurrences in frame/ with gap remediation
- Code coverage measurement (cargo-llvm-cov) with targeted tests for uncovered paths
- Syscall fuzzer as bare-metal test generating random capability/IPC/memory operations
- Concurrency model checking (Loom) for IPC rendezvous, lock-free protocols, scheduling invariants

## Evolution

This document evolves at phase transitions and milestone boundaries.

**After each phase transition** (via `/gsd:transition`):
1. Requirements invalidated? — Move to Out of Scope with reason
2. Requirements validated? — Move to Validated with phase reference
3. New requirements emerged? — Add to Active
4. Decisions to log? — Add to Key Decisions
5. "What This Is" still accurate? — Update if drifted

**After each milestone** (via `/gsd:complete-milestone`):
1. Full review of all sections
2. Core Value check — still the right priority?
3. Audit Out of Scope — reasons still valid?
4. Update Context with current state

---
*Last updated: 2026-04-28 after milestone v1.0 initialization*
