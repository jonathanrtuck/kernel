---
phase: 03-coverage
plan: "02"
subsystem: coverage
tags: [coverage, analysis, classification]
dependency_graph:
  requires: [03-01]
  provides: [COVERAGE-REPORT.md]
  affects: [03-03]
tech_stack:
  added: []
  patterns: [cargo-llvm-cov, per-file classification]
key_files:
  created:
    - .planning/phases/03-coverage/COVERAGE-REPORT.md
  modified: []
decisions:
  - "All 10 below-50% files are hardware-only (AArch64 bare-metal only)"
  - "exception.rs and mmio.rs discovered as additional 0% files not in CONTEXT"
  - "time_manager/mod.rs classified hardware-only: SchedulerAlgorithm enum only callable from bare-metal exception handler"
  - "Priority order for Plan 03: slab.rs (trivial), mmu.rs pure helpers, cores.rs observer helpers, eevdf edge cases, communication.rs CapError branches"
metrics:
  duration: "15 minutes"
  completed: "2026-04-28"
  tasks_completed: 1
  files_created: 1
---

# Phase 3 Plan 2: Coverage Measurement and Classification Summary

**One-liner:** Fresh llvm-cov run classifying all 10 below-50% files as hardware-only with 5 priority targets for Plan 03 test writing.

## What Was Done

Ran `cargo llvm-cov --target aarch64-apple-darwin --summary-only --no-cfg-coverage` against 1,158 tests. Parsed per-file output, read source for all below-50% files to verify classification, examined text coverage for improvement candidates.

**Result:** `.planning/phases/03-coverage/COVERAGE-REPORT.md` — 327 lines covering:
- 10 files below 50% (all hardware-only)
- 9 improvement candidates with specific uncovered functions/branches
- 5 priority targets ordered by coverage impact for Plan 03

## Coverage Numbers

| Metric | Value |
|--------|-------|
| Overall line coverage | 92.81% |
| Lines measured | 22,407 |
| Uncovered lines | 1,610 |
| Tests run | 1,158 |

## Files Below 50% — All Hardware-Only

| File | Coverage | Reason |
|------|----------|--------|
| `frame/arch/aarch64/serial.rs` | 0% | UART MMIO via physical address |
| `frame/arch/aarch64/timer.rs` | 0% | ARM generic timer (MSR/MRS inline asm) |
| `frame/arch/aarch64/platform.rs` | 0% | `init()` gated `#[cfg(target_os = "none")]` |
| `frame/arch/aarch64/gic.rs` | 31.58% | MMIO + ICC system registers; pure helpers covered |
| `frame/arch/aarch64/entropy.rs` | 0% | RNDR instruction + cntpct_el0 |
| `frame/arch/aarch64/mod.rs` | 13.04% | Boot/exception utilities (inline asm) |
| `frame/arch/aarch64/sysreg.rs` | 20.83% | Write accessors + barrier instructions |
| `frame/arch/aarch64/exception.rs` | 0% | Vector table entry — bare-metal only |
| `frame/arch/aarch64/mmio.rs` | 0% | `read_volatile`/`write_volatile` on physical addrs |
| `time_manager/mod.rs` | 0% | SchedulerAlgorithm only callable from exception handler |

## Deviations from Plan

### Auto-fixed Issues

None.

### Discoveries

**1. Two additional 0% files not in CONTEXT**

The initial context listed 8 below-50% files. The fresh coverage run revealed two more:
- `frame/arch/aarch64/exception.rs` — 0% (106 lines uncovered)
- `frame/arch/aarch64/mmio.rs` — 0% (13 lines uncovered)

Both are hardware-only. Documented in COVERAGE-REPORT.md. No impact on classification.

**2. mmu.rs reclassified as improvement candidate (not below-50%)**

CONTEXT listed mmu.rs as potentially below-50%. Actual coverage: 79.31% — above the threshold. Treated as improvement candidate.

## Known Stubs

None. COVERAGE-REPORT.md contains real data from a live coverage run.

## Self-Check

- [x] COVERAGE-REPORT.md exists at `.planning/phases/03-coverage/COVERAGE-REPORT.md`
- [x] Contains "hardware-only" classification
- [x] Contains "Improvement Candidates" section
- [x] All 10 below-50% files listed and classified
- [x] Commit f3450aa verified

## Self-Check: PASSED
