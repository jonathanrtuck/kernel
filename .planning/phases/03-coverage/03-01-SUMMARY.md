---
phase: 03-coverage
plan: 01
subsystem: testing
tags: [cargo-llvm-cov, coverage, shell, verify]

# Dependency graph
requires: []
provides:
  - scripts/coverage: per-file line/function/region coverage in 4 output formats
  - scripts/verify: informational line coverage percentage after test gate
affects:
  - 03-02
  - 03-03

# Tech tracking
tech-stack:
  added: [cargo-llvm-cov (0.8.5)]
  patterns:
    - "Coverage as informational metric: measured and reported, never a gate"
    - "JSON summary for programmatic extraction, text output for human reading"

key-files:
  created:
    - scripts/coverage
  modified:
    - scripts/verify

key-decisions:
  - "Use default text output for scripts/coverage (human-readable per-file table) and JSON --summary-only for scripts/verify (clean jq parse)"
  - "Coverage is informational only — never fails verify regardless of percentage"
  - "Parse third percentage field in TOTAL line (line coverage) using awk field iteration with pct_count"

patterns-established:
  - "scripts/coverage --json: machine-readable output for downstream tooling (03-02 gap analysis)"
  - "scripts/verify coverage step: cargo llvm-cov --json --summary-only | jq for single-value extraction"

requirements-completed:
  - COV-01
  - COV-04

# Metrics
duration: 15min
completed: 2026-04-29
---

# Phase 03 Plan 01: Coverage Tooling Summary

**`scripts/coverage` with 4 output modes (default/--html/--text/--json) and `scripts/verify` reporting line coverage as an informational metric after the test gate**

## Performance

- **Duration:** ~15 min
- **Started:** 2026-04-29T00:30:00Z
- **Completed:** 2026-04-29T00:45:54Z
- **Tasks:** 2
- **Files modified:** 2

## Accomplishments

- Created `scripts/coverage` following the same style as `scripts/verify` (color codes, pass/fail/info helpers, `set -eo pipefail`)
- Default mode produces the cargo-llvm-cov per-file text table plus an `info  overall line coverage: XX.XX%` summary line
- `--html` flag generates `target/llvm-cov/html/index.html` and prints its path
- `--text` and `--json` flags produce annotated source and machine-readable JSON respectively
- Added 5-line coverage step to `scripts/verify` between "all tests" and framekernel check — runs `cargo llvm-cov --json --summary-only` and prints the line percentage via `jq`
- Current kernel line coverage: 92.81%

## Task Commits

Each task was committed atomically:

1. **Task 1: Create scripts/coverage** - `fd3cfc5` (feat)
2. **Task 2: Integrate coverage into scripts/verify** - `babd61b` (feat)

**Plan metadata:** (docs commit follows)

## Files Created/Modified

- `scripts/coverage` — four-mode coverage script; informational, never a gate
- `scripts/verify` — 5-line addition reporting line coverage percentage after test pass

## Decisions Made

- Used plain text output for `scripts/coverage` default mode because the cargo-llvm-cov text table is already well-formatted for human consumption. Extracts the overall percentage from the TOTAL line using awk (third percentage field = line coverage column).
- Used `--json --summary-only` in `scripts/verify` so `jq` can extract a single clean number rather than parsing the text table, which avoids brittle column-offset parsing.
- Coverage informational-only: no threshold logic added. Adding a gate would require a settled threshold decision; the current ~93% is a baseline to track, not enforce.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 1 - Bug] `--summary-only` requires a format flag**
- **Found during:** Task 1 (scripts/coverage creation)
- **Issue:** The plan says to run `cargo llvm-cov --summary-only` in default mode, but `--summary-only` is only valid with `--json`, `--lcov`, or `--cobertura`. Running without a format flag would error.
- **Fix:** Default mode uses plain text output (no `--summary-only`), which gives the same per-file table. `--summary-only` is used only in `scripts/verify` paired with `--json`.
- **Files modified:** `scripts/coverage`
- **Verification:** `scripts/coverage` runs and produces per-file table with overall percentage. `scripts/verify` uses `--json --summary-only` correctly.
- **Committed in:** `fd3cfc5` (Task 1 commit)

---

**Total deviations:** 1 auto-fixed (1 bug — cargo-llvm-cov flag constraint)
**Impact on plan:** Fix was necessary for correctness; outcome matches plan intent exactly.

## Issues Encountered

None beyond the `--summary-only` flag constraint above.

## Known Stubs

None — scripts are fully wired to cargo-llvm-cov and produce real data.

## Next Phase Readiness

- `scripts/coverage --json` is ready for 03-02 to consume per-file coverage data and identify uncovered paths
- 03-03 (gap-targeted tests) can use `scripts/coverage` to measure test effectiveness after each addition

## Self-Check: PASSED

- `scripts/coverage` exists: FOUND
- `scripts/coverage` executable: FOUND
- `scripts/verify` contains "line coverage:": FOUND (verified by running scripts/verify)
- Task commits exist: fd3cfc5 FOUND, babd61b FOUND

---
*Phase: 03-coverage*
*Completed: 2026-04-29*
