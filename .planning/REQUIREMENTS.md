# Requirements: Kernel Verification

**Defined:** 2026-04-28
**Core Value:** Every kernel invariant is tested by at least one technique that can find bugs the developer didn't anticipate.

## v1 Requirements

Requirements for this milestone. Each maps to roadmap phases.

### Property Testing

- [ ] **PROP-01**: Capability encoding roundtrips correctly for all valid ObjectType/Rights/badge combinations
- [ ] **PROP-02**: Arena allocation invariants hold: no double-alloc, freed slots reusable, no ID overlap
- [ ] **PROP-03**: Rights bitmask operations are consistent (union, intersection, subset, attenuation)
- [ ] **PROP-04**: EEVDF scheduler properties hold: eligible threads always picked, virtual deadline ordering maintained
- [ ] **PROP-05**: Capability slot arithmetic never overflows or produces invalid indices
- [ ] **PROP-06**: proptest integrated into `cargo test` and `scripts/verify` pipeline

### Unsafe Audit

- [ ] **SAFE-01**: Every unsafe block in frame/ has a SAFETY comment (close the ~37-block gap)
- [ ] **SAFE-02**: Each SAFETY comment's precondition is verified against all callers
- [ ] **SAFE-03**: No aliasing violations in unsafe code (mutable references, pointer casts)
- [ ] **SAFE-04**: All inline assembly options() are correct (nomem/nostack/preserves_flags justified per ARM ARM)
- [ ] **SAFE-05**: Speculation barriers present for all user-provided index dereferences in frame/
- [ ] **SAFE-06**: Audit findings documented with per-file status (clean / fixed / known-risk)

### Coverage

- [ ] **COV-01**: Code coverage measurement produces per-file line and branch coverage reports
- [ ] **COV-02**: Identify all source files with <50% line coverage
- [ ] **COV-03**: Write targeted tests to bring uncovered critical paths above 80%
- [ ] **COV-04**: Coverage measurement integrated into scripts/verify or available as scripts/coverage

### Fuzzing

- [ ] **FUZZ-01**: Bare-metal test harness generates random valid syscall sequences
- [ ] **FUZZ-02**: Fuzzer covers capability operations (Create, Mint, Clone, Destroy, Close)
- [ ] **FUZZ-03**: Fuzzer covers IPC operations (Call, Send, Receive with random payloads)
- [ ] **FUZZ-04**: Fuzzer covers memory operations (Space mapping, Field split/transfer)
- [ ] **FUZZ-05**: Kernel does not panic or hang under any generated sequence (liveness + safety)
- [ ] **FUZZ-06**: Fuzzer reproducible via seed for regression testing

### Concurrency

- [ ] **CONC-01**: IPC rendezvous protocol modeled and verified for all sender/receiver interleavings
- [ ] **CONC-02**: Lock-free data structures in frame/ modeled for linearizability
- [ ] **CONC-03**: Scheduler queue operations verified under concurrent enqueue/dequeue
- [ ] **CONC-04**: Observer lifecycle (create/suspend/resume/destroy) verified under concurrent access
- [ ] **CONC-05**: Models run as host-side tests integrated into `cargo test`

## v2 Requirements

Deferred to future milestone.

### Formal Verification

- **VERUS-01**: Core capability operations proven correct with Verus annotations
- **VERUS-02**: Arena allocator proven free of undefined behavior
- **VERUS-03**: Page table mapping operations proven to respect capability authority

## Out of Scope

| Feature | Reason |
|---------|--------|
| Verus formal verification | Highest effort; other techniques cover more ground sooner |
| Performance regression testing | Separate concern from correctness; benchmark harness already exists |
| Userspace test coverage | This milestone targets kernel internals only |
| GUI/visual testing | Kernel is headless; all verification is terminal-based |
| CI/CD integration | All verification runs locally; no external CI exists |

## Traceability

Updated during roadmap creation.

| Requirement | Phase | Status |
|-------------|-------|--------|

**Coverage:**
- v1 requirements: 27 total
- Mapped to phases: 0
- Unmapped: 27

---
*Requirements defined: 2026-04-28*
*Last updated: 2026-04-28 after initial definition*
