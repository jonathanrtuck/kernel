---
phase: 02-unsafe-audit
plan: "02"
subsystem: testing
tags: [unsafe, safety-comments, arm-arm, slab, mmu, page-tables]

requires:
  - phase: 01-property-testing
    provides: proptest infrastructure and verified kernel object invariants

provides:
  - Zero SAFETY comment gaps in mapping.rs, mmu.rs, boot.rs, and slab.rs
  - ARM ARM D5.10 citations for ISB and DSB ISHST in MMU-enable sequence
  - Verified asm options() policy compliance in sysreg.rs (nomem only on immutable registers)
  - Documented MaybeUninit UB risk in test SlabStore::allocate()
  - Verified no user-provided indices in page table address computation (no Spectre v1 risk)

affects:
  - 02-unsafe-audit (other plans in this phase)
  - Any future frame/ changes that touch slab.rs or mmu.rs

tech-stack:
  added: []
  patterns:
    - "SAFETY comments cover all unsafe blocks including inline unsafe expressions on a single line"
    - "ARM ARM citations placed at call sites in configure_and_enable, not just in sysreg.rs wrappers"
    - "MaybeUninit::zeroed() branches each require their own SAFETY comment, even when identical"

key-files:
  created: []
  modified:
    - src/frame/arch/aarch64/mmu.rs
    - src/frame/slab.rs

key-decisions:
  - "ARM ARM D5.10 ISB-before-TLBI and DSB-ISHST-before-TLBI comments added inline in configure_and_enable to make the ordering rationale self-contained"
  - "slab.rs insert() &mut reference derivation requires its own SAFETY comment separate from the preceding ptr::write SAFETY comment — they cover different aspects of the same operation"
  - "mapping.rs, boot.rs had zero actual gaps; plan counts reflected a previous state"

patterns-established:
  - "Every unsafe {} block needs its own SAFETY comment, even if logically related to a preceding block"

requirements-completed: [SAFE-01, SAFE-02, SAFE-03, SAFE-04, SAFE-05]

duration: 25min
completed: 2026-04-28
---

# Phase 02 Plan 02: Unsafe Audit (mapping.rs, mmu.rs, boot.rs, slab.rs) Summary

**Closed 2 SAFETY comment gaps in slab.rs and added ARM ARM D5.10 citations in mmu.rs; all four memory-subsystem files now have zero unsafe annotation gaps**

## Performance

- **Duration:** ~25 min
- **Started:** 2026-04-28T~00:00Z
- **Completed:** 2026-04-28
- **Tasks:** 2 of 2
- **Files modified:** 2 (mmu.rs, slab.rs)

## Accomplishments

- Audited all unsafe blocks in mapping.rs (9 blocks), mmu.rs (6 unsafe constructs), boot.rs (18 unsafe constructs), and slab.rs (15 bare-metal + 2 test build blocks)
- Closed 2 SAFETY comment gaps in slab.rs: `allocate()` test-build else-if branch and `insert()` reference derivation
- Added ARM ARM D5.10 citations in `configure_and_enable` for ISB (post-TTBR/TCR/MAIR writes) and DSB ISHST (pre-TLBI) to document the ordering requirement against the ARM Architecture Reference Manual
- Verified sysreg.rs asm options() are compliant: `nomem` used only on `mrs` of immutable registers (MPIDR_EL1, CNTFRQ_EL0, ID registers); omitted on all msr/dsb/isb/tlbi as required by CLAUDE.md policy
- Confirmed no user-provided indices flow into page table address computation — Spectre v1 speculation barriers not needed in mapping.rs or mmu.rs
- Verified aliasing soundness: no two `pa_to_table()` calls in mapping.rs can produce references to the same PA within the same scope; slab.rs freelist and bitmap prevent double-allocation
- Verified `for_each_mut` callback cannot cause aliasing: each loop iteration produces a reference to a distinct slot (bitmap-checked); the callback cannot re-enter the same SlabStore without taking `&mut self` which is already borrowed

## Task Commits

1. **Task 1: Audit mapping.rs and mmu.rs** - `f9a52bc` (chore)
2. **Task 2: Audit boot.rs and slab.rs** - `7f29a0d` (chore)

**Plan metadata:** (pending final commit)

## Files Created/Modified

- `src/frame/arch/aarch64/mmu.rs` - Added ARM ARM D5.10 comments for ISB and DSB ISHST in configure_and_enable
- `src/frame/slab.rs` - Added 2 missing SAFETY comments (test allocate else-if branch, insert &mut reference)

## Decisions Made

- ARM ARM citations placed inline in `configure_and_enable` rather than relying solely on the sysreg.rs wrappers — makes the MMU initialization sequence self-documenting at the call site
- Both SAFETY gaps in slab.rs were in code paths that share invariants with adjacent commented blocks; nonetheless, each unsafe expression requires its own SAFETY comment per CLAUDE.md

## Deviations from Plan

### Minor Scope Differences

**1. [Informational] mapping.rs and boot.rs had zero actual gaps**
- **Situation:** The plan counted 5 gaps in mapping.rs and 1 gap in boot.rs. Audit of the current source found 0 gaps in both files — all unsafe blocks already had SAFETY comments.
- **Assessment:** The plan was written from a prior source snapshot. Gaps were closed during Phase E implementation work. Not a problem — the audit confirmed coverage rather than creating it.
- **Action:** Full audit still performed; files confirmed clean.

---

**Total deviations:** 0 code changes outside plan scope. 2 SAFETY gaps closed as planned (both in slab.rs).

## Issues Encountered

None. All 1158 host tests + 28 bare-metal userspace tests pass after changes. Framekernel boundary (all unsafe in frame/) and speculation barriers both confirmed.

## Known Stubs

None — this was an audit-only plan with no functional changes.

## Next Phase Readiness

- Memory subsystem unsafe code is fully annotated and audited
- asm options() policy is verified compliant across sysreg.rs (no incorrect nomem)
- Aliasing analysis complete: slab freelist + bitmap, mapping.rs exclusive-access chain, boot.rs single-threaded init all verified
- MaybeUninit UB risk documented: test SlabStore::allocate() is unsafe for NonNull-containing types; insert() is the correct API

---
*Phase: 02-unsafe-audit*
*Completed: 2026-04-28*
