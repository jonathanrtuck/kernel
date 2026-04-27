# Kernel Review Configuration

This file configures both the built-in `/review` command and provides context
for `/review-kernel` (the multi-pass pipeline).

## Severity Definitions (kernel-specific)

**Important** — reserve for findings that would:

- Violate memory safety (UB, use-after-free, double-free, out-of-bounds)
- Allow capability escape (access without valid capability)
- Cause a data race under SMP (incorrect atomics, missing barriers)
- Violate the framekernel boundary (unsafe behavior reachable from safe code)
- Crash the kernel (unwrap on fallible path, infinite loop in exception handler)

**Nit** — at most five per review. Worth fixing but not blocking:

- Missing or stale SAFETY comment
- Overly broad rights check (correct but grants more than needed)
- Style inconsistency with surrounding code

**Pre-existing** — bugs not introduced by this PR. Flag with `pre-existing:`
prefix.

## What to Skip

CI already catches these — do not duplicate:

- Clippy warnings (enforced by `scripts/verify`)
- Formatting (rustfmt runs as a post-edit hook)
- Framekernel boundary violations (checked by `scripts/verify`)
- Test failures (run by `scripts/verify`)

## Kernel-Specific Rules

1. Every `unsafe` block must have a `// SAFETY:` comment. The comment must name
   a specific invariant and what breaks if violated. "This is safe" is not a
   SAFETY comment.

2. Inline asm `options(nomem)` is only valid for `mrs` of truly immutable
   registers (ID registers, CNTFRQ). All other asm must omit `nomem`. A wrong
   `nomem` is a CRITICAL finding — LLVM will silently reorder memory accesses.

3. Arena accesses must check generation. Any path that accesses an arena slot
   without verifying the generation counter is a use-after-free vector.

4. Capability resolution must check rights. Any path that resolves a handle
   without verifying the caller has the required rights is a privilege
   escalation.

5. User-provided indices into kernel arrays must have a speculation barrier (SB)
   after the bounds check (Spectre v1).

6. Atomic orderings: Release stores must pair with Acquire loads. Relaxed is
   only for non-synchronizing operations. Under ARM64's weak memory model, wrong
   orderings are silent until SMP load.

7. No `.unwrap()` on fallible operations in kernel code paths. A panic in the
   kernel is a crash.

## Evidence Bar

Every finding must include:

- Exact `file:line` citation
- The invariant or contract violated
- A concrete scenario that triggers the bug

"This could potentially be an issue" is not a finding. If you cannot describe
the trigger, do not report it.
