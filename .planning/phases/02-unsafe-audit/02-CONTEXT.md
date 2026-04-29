# Phase 2: Unsafe Audit - Context

**Gathered:** 2026-04-28
**Status:** Ready for planning
**Mode:** Auto-generated (infrastructure phase — code audit, no user-facing behavior)

<domain>
## Phase Boundary

Structured audit of every unsafe block in frame/. Close SAFETY comment gaps, verify each comment's precondition against all callers, check aliasing soundness, validate inline assembly options() against ARM ARM, confirm speculation barriers for user-provided index dereferences, and document per-file audit status (clean / fixed / known-risk).

</domain>

<decisions>
## Implementation Decisions

### Claude's Discretion
All implementation choices are at Claude's discretion — pure infrastructure phase. Use ROADMAP phase goal, success criteria, and codebase conventions to guide decisions.

Key technical context:
- frame/ has ~443 unsafe occurrences across 14 files with ~406 SAFETY comments (~37 gap)
- Largest gaps by file: mapping.rs (5), cores.rs (5), mod.rs (4), exception.rs (3), slab.rs (2), lock.rs (2), capabilities.rs (2)
- Files with 0 or negative gaps (more SAFETY than unsafe) may have extra comments from multi-line blocks — still need audit of comment accuracy
- The project's CLAUDE.md has strict rules about unsafe: every block needs SAFETY comment, nomem only with ARM ARM justification, speculation barriers for user-provided indices
- Per memory: SB barrier discipline required in frame/ for user-provided index dereferences (Spectre v1)
- Audit findings should be documented per-file, not just as code changes

</decisions>

<code_context>
## Existing Code Insights

### Files to Audit (by gap size)
| File | unsafe | SAFETY | Gap |
|------|--------|--------|-----|
| cores.rs | 81 | 76 | 5 |
| mapping.rs | 14 | 9 | 5 |
| mod.rs | 8 | 4 | 4 |
| exception.rs | 11 | 8 | 3 |
| slab.rs | 16 | 14 | 2 |
| lock.rs | 5 | 3 | 2 |
| capabilities.rs | 6 | 4 | 2 |
| boot.rs | 19 | 18 | 1 |
| mmu.rs | 8 | 7 | 1 |
| mmio.rs | 4 | 3 | 1 |
| cpu.rs | 5 | 4 | 1 |
| fields.rs | 46 | 52 | -6 (extra comments) |
| dtb.rs | 2 | ? | check |

### Established Patterns
- SAFETY comments use format: `// SAFETY: <invariant description>`
- Inline asm has explicit options() with comments justifying each flag
- `scripts/verify` already checks framekernel boundary and speculation barriers

### Integration Points
- Audit report goes in `.planning/phases/02-unsafe-audit/` as a markdown doc
- Any code fixes (adding SAFETY comments, fixing options()) are committed to src/frame/

</code_context>

<specifics>
## Specific Ideas

No specific requirements — infrastructure phase. Refer to ROADMAP success criteria:
1. Zero unsafe blocks without SAFETY comments
2. Every SAFETY precondition confirmed against all call sites
3. No reachable aliasing violations
4. Every asm options() justified against ARM ARM
5. Speculation barriers for all user-provided index dereferences
6. Per-file audit record

</specifics>

<deferred>
## Deferred Ideas

None — discussion stayed within phase scope.

</deferred>
