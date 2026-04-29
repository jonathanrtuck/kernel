---
phase: 01-property-testing
plan: 03
subsystem: testing
tags: [proptest, eevdf, scheduler, property-based-testing, no_std]

# Dependency graph
requires: []
provides:
  - "EEVDF scheduler property tests verifying pick_next liveness, fairness, starvation-freedom, enqueue/dequeue consistency, and weight tracking"
affects: []

# Tech tracking
tech-stack:
  added: [proptest = "1" (dev-dependency)]
  patterns:
    - "Stack-allocated Observer arrays (no_std compatible proptest) using core::array::from_fn"
    - "Fixed-size stack-based tracking arrays replacing Vec for no_std proptest sequences"
    - "Nested mod prop_tests inside #[cfg(test)] mod tests with proptest! macro"

key-files:
  created: []
  modified:
    - Cargo.toml
    - src/time_manager/earliest_eligible_virtual_deadline.rs

key-decisions:
  - "Used stack-allocated fixed-size arrays instead of Vec throughout proptest bodies — required for no_std kernel target; proptest strategies can return Vec<T> but test body must use alloc-free constructs"
  - "prop_eevdf_eligible_first tests observable consequence (always-Some) rather than internal VET state — private fields are inaccessible; the invariant is validated indirectly, and deterministic unit tests cover the full eligible-first correctness"
  - "prop::collection::vec used only for strategy input generation (profiles, ops sequences) — proptest's std-dependent strategy machinery runs in the test harness, not in the kernel; the test body is alloc-free"

patterns-established:
  - "PROP-04 comment block pattern: // ── PROP-04: <requirement name> ── marks proptest sections for grep-based acceptance checks"

requirements-completed: [PROP-04]

# Metrics
duration: 5min
completed: 2026-04-29
---

# Phase 01 Plan 03: EEVDF Scheduler Property Tests Summary

**Six proptest properties for EEVDF scheduler covering liveness, fairness, starvation-freedom, consistency, and weight tracking — all no_std-compatible with stack-allocated Observers**

## Performance

- **Duration:** ~5 min
- **Started:** 2026-04-29T00:07:13Z
- **Completed:** 2026-04-29T00:12:23Z
- **Tasks:** 1
- **Files modified:** 3 (Cargo.toml, Cargo.lock, src/time_manager/earliest_eligible_virtual_deadline.rs)

## Accomplishments

- Added proptest = "1" as a dev-dependency (parallel with plan 01-01 which also adds it; the result is idempotent)
- Wrote 6 proptest property functions in a `prop_tests` submodule of the EEVDF test module, marked with `// ── PROP-04: EEVDF scheduler properties ──`
- All 6 tests pass under 1164-test full suite (1158 existing + 6 new)

## Task Commits

1. **Task 1: Property tests for EEVDF scheduler invariants** - `84c4a88` (test)

**Plan metadata:** (docs commit follows)

## Files Created/Modified

- `Cargo.toml` — Added `[dev-dependencies] proptest = "1"`
- `Cargo.lock` — Updated with proptest and its 60+ transitive dependencies
- `src/time_manager/earliest_eligible_virtual_deadline.rs` — Added `mod prop_tests` block inside existing `#[cfg(test)] mod tests` with 6 `proptest!` property functions

## Decisions Made

- Used stack-allocated `[Observer; N]` arrays throughout proptest bodies. The no_std kernel cannot use `Vec` in test bodies even though proptest strategies themselves can produce `Vec<T>`. All tracking (enqueued set, stack) uses fixed-size arrays.
- `prop_eevdf_eligible_first` tests the observable consequence of the eligible-first invariant (pick_next is always Some when non-empty) rather than accessing private VET/global_virtual_time fields. Deterministic unit tests (`high_responsiveness_observer_selected_first`, `interactive_scheduled_more_frequently`) cover the full eligible-first ordering.
- proptest version pinned to "1" (semver-compatible) rather than a specific patch to allow future patch updates.

## Deviations from Plan

### Auto-fixed Issues

**1. [Rule 3 - Blocking] Added proptest to Cargo.toml before writing tests**
- **Found during:** Task 1 (before implementation)
- **Issue:** Cargo.toml lacked proptest dev-dependency; tests would fail to compile without it. Plan 01-01 was supposed to add it but runs in parallel.
- **Fix:** Added `[dev-dependencies] proptest = "1"` to Cargo.toml as instructed in the parallel execution prompt.
- **Files modified:** Cargo.toml, Cargo.lock
- **Verification:** cargo test --target aarch64-apple-darwin compiled successfully with proptest available.
- **Committed in:** 84c4a88 (Task 1 commit)

**2. [Rule 1 - Bug] Replaced alloc::vec::Vec tracking with fixed-size array stacks**
- **Found during:** Task 1 (initial implementation)
- **Issue:** First draft used `alloc::vec::Vec` for tracking enqueued observer indices in `prop_eevdf_enqueue_dequeue_consistency` and `prop_eevdf_total_weight_tracks`. This requires `extern crate alloc` in no_std context and adds heap allocation. The no_std kernel avoids heap in test bodies.
- **Fix:** Replaced with fixed-size `[usize; N]` arrays used as LIFO stacks with an explicit length counter, matching the pattern used elsewhere in the kernel test suite.
- **Files modified:** src/time_manager/earliest_eligible_virtual_deadline.rs
- **Verification:** All 6 property tests pass with no compilation warnings.
- **Committed in:** 84c4a88 (Task 1 commit)

**3. [Rule 1 - Bug] Fixed prop_assert_eq! format string capturing loop variable `i`**
- **Found during:** Task 1 (first compilation attempt)
- **Issue:** `prop_assert_eq!(sched.contains(ptr), currently_enqueued[i], "observer {i}: ...")` failed to compile: `prop_assert_eq!` expands via `concat!` which cannot capture variables from surrounding scope.
- **Fix:** Extracted values into local bindings (`let in_queue = ...; let expected = ...;`) and used positional `{}` format with explicit args.
- **Files modified:** src/time_manager/earliest_eligible_virtual_deadline.rs
- **Verification:** Compiled successfully.
- **Committed in:** 84c4a88 (Task 1 commit, fixes applied before commit)

---

**Total deviations:** 3 auto-fixed (1 blocking dependency, 2 code bugs found during implementation)
**Impact on plan:** All fixes required for compilation and correctness. No scope creep.

## Issues Encountered

- Initial cargo test run was in wrong directory (/Users/user/Sites/kernel instead of worktree). Corrected by running with explicit working directory in worktree.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- PROP-04 complete: EEVDF scheduler properties verified under proptest-generated sequences
- Property test patterns established for remaining proptest plans (01-01, 01-02)
- The no_std + proptest pattern (stack arrays, proptest strategy inputs only, alloc-free test bodies) is validated and ready to apply to other subsystems

---
*Phase: 01-property-testing*
*Completed: 2026-04-29*
