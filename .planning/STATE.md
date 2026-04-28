# State

## Current Position

Phase: Not started (defining requirements)
Plan: —
Status: Defining requirements
Last activity: 2026-04-28 — Milestone v1.0 started

## Project Reference

See: .planning/PROJECT.md (updated 2026-04-28)

**Core value:** Every kernel invariant is tested by at least one technique that can find bugs the developer didn't anticipate.
**Current focus:** Defining requirements

## Accumulated Context

- Kernel has ~80k lines, ~1,975 unit tests, 34 bare-metal tests
- All unsafe in frame/ (443 occurrences, 406 SAFETY comments)
- SMP tests already found 3 concurrency bugs — probabilistic testing works but doesn't exhaust state space
- User priority ordering: proptest > unsafe audit > coverage > fuzzing > Loom > Verus (out of scope)
- User wants fully autonomous execution — no involvement in testing decisions
