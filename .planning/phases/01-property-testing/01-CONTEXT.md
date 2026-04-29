# Phase 1: Property Testing - Context

**Gathered:** 2026-04-28
**Status:** Ready for planning
**Mode:** Auto-generated (infrastructure phase — no user-facing behavior)

<domain>
## Phase Boundary

Add proptest-based property testing for core kernel data structures. Targets: capability Handle encode/decode roundtrip, Rights bitmask algebraic laws, Arena allocation invariants, EEVDF scheduler properties, and capability slot arithmetic. All tests run as host-side `#[cfg(test)]` modules via `cargo test --target aarch64-apple-darwin`. Must integrate cleanly with `scripts/verify`.

</domain>

<decisions>
## Implementation Decisions

### Claude's Discretion
All implementation choices are at Claude's discretion — pure infrastructure phase. Use ROADMAP phase goal, success criteria, and codebase conventions to guide decisions.

Key technical context for decisions:
- `proptest` is the standard Rust property testing crate; prefer it over `quickcheck` for its shrinking and composable strategies
- Tests live as `#[cfg(test)]` modules inside existing source files (per project convention)
- The kernel is `no_std` but host tests run with `std` via the `aarch64-apple-darwin` target
- proptest requires `std` — this is fine because it only runs in `#[cfg(test)]` on the host target
- Existing test modules already exist in `capability.rs`, `arena.rs`, and `time_manager/*.rs`

</decisions>

<code_context>
## Existing Code Insights

### Proptest Targets
- `capability.rs:203` — `Handle::encode(self) -> u64` and `Handle::decode(raw: u64) -> Handle` (D77 ABI)
- `capability.rs:69` — `Rights(u16)` with bitwise operations (union, intersection, subset, attenuation via D52)
- `capability.rs:232` — `SlotTag::abi_matches` — 48-bit mask comparison
- `arena.rs:39` — `Arena<T>` with allocate/insert/get/get_mut/free/for_each_mut
- `time_manager/earliest_eligible_virtual_deadline.rs:61` — `EarliestEligibleVirtualDeadline` scheduler
- `time_manager/round_robin.rs:33` — `RoundRobin` scheduler
- `capability.rs:45` — `CAP_ABSENT = u64::MAX` sentinel, `MAX_HANDLE_INDEX = 0xFFFF`

### Established Patterns
- Test modules at bottom of each file, gated with `#[cfg(test)]`
- Existing tests use standard `assert_eq!`/`assert!` macros
- No external test dependencies currently (just `core` and `alloc`)

### Integration Points
- `Cargo.toml` — add proptest as dev-dependency
- `scripts/verify` — already runs `cargo test`; proptest integrates automatically
- Each target file's `#[cfg(test)] mod tests` — add proptest cases alongside existing tests

</code_context>

<specifics>
## Specific Ideas

No specific requirements — infrastructure phase. Refer to ROADMAP success criteria:
1. Capability encoding roundtrips for any valid input
2. Arena allocation invariants across any alloc/free sequence
3. Rights bitmask algebraic laws under proptest
4. EEVDF scheduler properties (eligible threads picked, deadline ordering)
5. Slot arithmetic never overflows

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>
