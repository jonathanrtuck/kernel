---
phase: 01-property-testing
plan: 01
subsystem: testing
tags: [proptest, property-based-testing, capability, rights, handle, slot-arithmetic]

requires: []
provides:
  - proptest dev-dependency in Cargo.toml enabling property-based tests on host target
  - 16 property tests in src/capability.rs covering Handle encode/decode roundtrip, Rights algebra, and slot arithmetic bounds
  - PROP-01: Handle::encode/decode roundtrip for any valid index and 48-bit slot_tag
  - PROP-03: Rights bitmask algebraic laws (idempotent, commutative, associative union; attenuate; contains)
  - PROP-05: Slot arithmetic bounds never overflow for valid inputs
  - PROP-06: proptest integrated into cargo test pipeline via standard #[test] runner
affects: [01-02-PLAN, 01-03-PLAN]

tech-stack:
  added: [proptest = "1"]
  patterns:
    - proptest! blocks grouped by requirement ID with comment headers
    - use proptest::prelude::* alongside use super::* in test module
    - one proptest! macro block per requirement group, multiple test functions inside

key-files:
  created: []
  modified:
    - Cargo.toml
    - src/capability.rs

key-decisions:
  - "Proptest strategies use full u16 range for Rights algebra (not just 14 valid bits) to test laws on arbitrary bit patterns"
  - "CAP_ABSENT sentinel verified as #[test] not proptest! since it is a single deterministic check, not a property over a range"
  - "proptest! blocks split into three groups by requirement ID for traceability"

patterns-established:
  - "Requirement traceability: each proptest! block is preceded by a // -- PROP-NN: ... -- comment"
  - "Proptest strategies inline in proptest! macro signatures using Rust range syntax"

requirements-completed: [PROP-01, PROP-03, PROP-05, PROP-06]

duration: 8min
completed: 2026-04-28
---

# Phase 01 Plan 01: Property Testing — Capability Module Summary

**proptest dev-dependency added and 16 property tests written for Handle encode/decode roundtrip, Rights algebraic laws, and slot arithmetic bounds — all passing under scripts/verify**

## Performance

- **Duration:** ~8 min
- **Started:** 2026-04-28T00:00:00Z
- **Completed:** 2026-04-28T00:08:00Z
- **Tasks:** 2
- **Files modified:** 2 (Cargo.toml, src/capability.rs)

## Accomplishments

- Added proptest = "1" to [dev-dependencies]; project compiles with proptest available in all test builds
- 16 property test functions across 3 proptest! blocks in src/capability.rs test module
- PROP-01 (4 tests): Handle::encode then decode roundtrips for any valid index/slot_tag; SlotTag::abi_matches is reflexive and ignores bits 48..63
- PROP-03 (9 tests): Union idempotent, commutative, associative; identity; attenuate as bitwise AND; contains reflexive and monotone; empty contains only empty
- PROP-05 (3 tests): Low-16-bit index preservation, bounded decoded index, CAP_ABSENT decodes to 0xFFFF
- scripts/verify passes: 1174 host tests + 28 userspace tests, clippy clean, framekernel boundary intact

## Task Commits

1. **Task 1: Add proptest dev-dependency** - `a829969` (chore)
2. **Task 2: Property tests for Handle, Rights, slot arithmetic** - `8053585` (test)

## Files Created/Modified

- `Cargo.toml` - Added [dev-dependencies] section with proptest = "1"
- `src/capability.rs` - Added use proptest::prelude::* and 175 lines of property tests in existing #[cfg(test)] mod tests block

## Decisions Made

- Used full u16 range for Rights algebra properties to test algebraic laws on arbitrary bit patterns, not just the 14 valid bits. Algebraic laws must hold universally; restricting to valid masks would miss edge cases.
- CAP_ABSENT sentinel verified as a standard #[test] rather than proptest! because it is a single deterministic value, not a property over a range.
- Strategies kept inline in proptest! macro headers using Rust range syntax (0u32..=MAX_HANDLE_INDEX) for readability.

## Deviations from Plan

None - plan executed exactly as written.

## Issues Encountered

None.

## User Setup Required

None - no external service configuration required.

## Next Phase Readiness

- proptest infrastructure is in place; Plans 02 and 03 can use proptest without adding any dependencies
- Pattern established: proptest! blocks grouped by requirement ID with comment headers for traceability
- All PROP-01, PROP-03, PROP-05, PROP-06 requirements satisfied

---
*Phase: 01-property-testing*
*Completed: 2026-04-28*

## Self-Check: PASSED

- `src/capability.rs` exists and contains 3 proptest! blocks
- `Cargo.toml` contains [dev-dependencies] and proptest = "1"
- Commits a829969 and 8053585 exist in git log
- All 16 prop_ tests pass; scripts/verify exits 0
