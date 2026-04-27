---
name: review-kernel
description: >-
  Multi-pass expert kernel review. Spawns parallel specialized agents for
  mechanical correctness, invariant analysis, SMP/concurrency, and adversarial
  testing, then validates all findings independently. Based on research into LLM
  code review limitations — decomposed passes, evidence-gated findings, and
  independent validation to counter the 22-48% false positive rate on correct
  code.
---

# Kernel Review Pipeline

```text
ANALYSIS ONLY. NO FIX PROPOSALS. FIND FIRST, FIX SECOND.
```

Research shows that asking an LLM to both find bugs and propose fixes in the
same pass degrades finding accuracy (overcorrection effect). Every agent in this
pipeline identifies issues — none proposes solutions.

## Evidence Standard (applies to ALL passes)

Every finding MUST include ALL of these fields or it is automatically rejected:

- **ID**: `P{pass}-{seq}` (e.g., `P1a-001`, `P3-002`)
- **Severity**: `CRITICAL` / `HIGH` / `MEDIUM` / `LOW`
  - CRITICAL: memory safety violation, capability escape, data race, UB
  - HIGH: incorrect behavior under valid inputs, security boundary violation
  - MEDIUM: incorrect behavior under edge-case inputs, missing validation
  - LOW: defensive gap, hardening opportunity, style concern
- **Location**: `file_path:line_number` (exact line, not approximate)
- **Invariant**: which specific property/contract is violated
- **Trigger**: a concrete scenario (syscall sequence, input value, timing
  condition) that causes the bug to manifest
- **Confidence**: `HIGH` / `MEDIUM` / `LOW` (self-assessed)

Findings that lack a file:line citation or concrete trigger are **noise, not
signal** — reject them. "This could potentially be an issue" is not a finding.

## Anti-Patterns (what agents must NOT do)

1. **No persona prompting.** Do not "act as Linus" or "think like an seL4
   verifier." Specify criteria, not characters. Research shows expert personas
   reduce coding accuracy.
2. **No fix proposals.** Analysis only. The overcorrection effect means
   find-and-fix in one pass degrades finding quality.
3. **No suspicion-based findings.** If you cannot cite the exact line and
   describe a concrete trigger, do not report it.
4. **No reimagining the architecture.** Review what exists against its own
   stated invariants. "I would have designed this differently" is not a finding.
5. **No training-data ARM semantics.** For inline asm, read the actual sysreg.rs
   policy comments and CLAUDE.md rules in-context. Do not rely on memorized
   instruction behavior.

## Scope

`$ARGUMENTS` determines what to review:

- **(empty)**: review changes since last tag or `HEAD~5`, whichever is larger
- **`full`**: review the entire `src/` tree
- **file paths**: review only those files
- **`git:<ref>`**: review changes since that git ref (e.g., `git:abc123`)

## Execution

Spawn passes 1a–1d and passes 2–4 as **parallel agents** (7 total). Wait for all
to complete. Then spawn pass 5 (validation) with the collected findings.

All review agents should use `subagent_type: "Explore"` — they need Read, Grep,
Glob, Bash but must NOT modify files. This also prevents accidental edits.

After validation, present the consolidated report to the user.

---

## Pass 1a: Framekernel Boundary

**Goal**: Verify that no unsafe code exists outside `frame/`.

**Files to read first**:

- `src/lib.rs` (crate-level deny/allow attributes)
- `src/main.rs` (entry point — `#[unsafe(no_mangle)]` is expected)

**Criteria**:

1. `#![deny(unsafe_code)]` is present at crate root
2. `#[allow(unsafe_code)]` appears ONLY on `mod frame`
3. No `unsafe` blocks, `unsafe fn`, `unsafe impl`, or `unsafe trait` outside
   `frame/` — except `#[unsafe(no_mangle)]` on the entry point
4. No `#[allow(unsafe_code)]` attributes anywhere outside `frame/`

**Method**: `grep -rn "unsafe" src/ --include="*.rs"` excluding `src/frame/`,
then manually verify each hit is a comment or the known exceptions.

---

## Pass 1b: SAFETY Comment Audit

**Goal**: Every unsafe block has a SAFETY comment, and the comment accurately
describes the invariant the code relies on.

**Files to read**: ALL `.rs` files in `src/frame/`

**Criteria**:

1. Every `unsafe { ... }` block has a `// SAFETY:` comment immediately above
2. The comment names a specific invariant (not "this is safe because we
   checked")
3. The comment describes what would break if the invariant were violated
4. If the unsafe block was recently modified, the SAFETY comment still holds for
   the current code (not stale from a previous version)
5. `unsafe impl` blocks (Send, Sync) have justification comments

**Red flags**:

- SAFETY comment that says "see above" without being specific
- SAFETY comment that describes what the code does instead of why it's safe
- Missing SAFETY comment on any unsafe block
- SAFETY comment that references a condition the code doesn't actually check

---

## Pass 1c: Inline Assembly Correctness

**Goal**: Every inline asm block has correct options, clobbers, and constraints.

**Files to read first** (these contain the project's asm policy):

- `src/frame/arch/aarch64/sysreg.rs` (nomem policy documentation)
- `src/frame/arch/aarch64/CLAUDE.md` (asm rules)
- Root `CLAUDE.md` (nomem policy)

**Then read ALL files containing `asm!`** and check each block against:

**Criteria**:

1. **nomem correctness**: `nomem` is used ONLY on `sysreg_read_const!` (truly
   immutable registers: ID registers, CNTFRQ). All other asm blocks omit
   `nomem`. If `nomem` appears anywhere else, it is a CRITICAL finding — LLVM
   will reorder memory accesses past the instruction.
2. **nostack correctness**: `nostack` is safe for single-instruction wrappers
   that don't touch the stack. Verify each usage.
3. **Clobber correctness**: instructions that modify condition flags must
   declare them. `msr` to registers that affect execution state must have
   appropriate clobbers.
4. **Register constraints**: output registers match the instruction semantics.
   `mrs` outputs to a GPR; `msr` takes a GPR input.
5. **Barrier placement**: `dsb`/`isb` sequences appear where architecturally
   required (after TTBR writes, after VBAR writes, around TLB invalidation).

---

## Pass 1d: Bounds, Overflow, and Cast Safety

**Goal**: No unchecked array indexing, arithmetic overflow, or lossy casts.

**Files to read**: all of `src/` (focus on `frame/capabilities.rs`,
`frame/slab.rs`, `arena.rs`, `capability.rs`)

**Criteria**:

1. Every array/slice index is either bounds-checked or provably in-bounds (with
   the proof cited in a comment)
2. Arithmetic on capacity/size/count values uses checked/wrapping/saturating ops
   or is provably non-overflowing
3. Casts between integer types (`as u32`, `as usize`) do not silently truncate —
   either the value is provably in range or a checked cast is used
4. Speculation barriers (SB) are present after bounds checks on user-provided
   indices (Spectre v1 — see `frame/capabilities.rs` for the pattern)

---

## Pass 2: Invariant & Contract Review

**Goal**: Public API contracts are maintained across function boundaries. The
capability graph is complete. Type-state transitions are correct.

**Files to read**: read `src/CLAUDE.md` for the module map, then read every
module's public API (types, method signatures, doc comments).

**Criteria**:

1. **Capability graph completeness**: every kernel object (Observer, Space,
   Time, Field, Pulsar) is reachable ONLY through the capability table. No
   function takes a raw `ObjectId` and accesses an arena directly without going
   through capability resolution first (except internal kernel paths that are
   documented).
2. **Pre/postcondition maintenance**: method doc comments state preconditions;
   callers satisfy them. Return types accurately represent possible outcomes.
3. **Error path completeness**: every `Result` is handled. No `.unwrap()` on
   fallible operations in kernel paths (panics are kernel crashes).
4. **Generation checks**: arena objects use generational indices. Every access
   must check the generation to prevent use-after-free. Look for paths that skip
   generation checking.
5. **Rights checks**: capability operations must verify the caller has the
   required rights before proceeding. Look for paths that skip rights checking.
6. **Type-state correctness**: Observer state transitions (Running → Blocked →
   Ready, etc.) follow the defined state machine. No impossible transitions.

---

## Pass 3: Concurrency & SMP Correctness

**Goal**: Atomic orderings, lock discipline, and barrier placement are correct
for ARM64's weak memory model.

```text
WARNING: Research shows LLM accuracy on concurrency under relaxed memory models
drops dramatically (F1: 0.65-0.80). Mark every finding in this pass as
Confidence: MEDIUM at best unless the issue is a clear violation of a
documented ordering protocol. Do not hallucinate ordering bugs.
```

**Files to read**: `frame/lock.rs`, `frame/cores.rs`, `kernel_state.rs`,
`time.rs`, `lib.rs`, `observer.rs`, `frame/arch/aarch64/cpu.rs`,
`frame/arch/aarch64/mmu.rs`

**Criteria**:

1. **Acquire/Release pairing**: every `store(..., Release)` has a corresponding
   `load(..., Acquire)` on the reading side. Relaxed loads/stores are used only
   when ordering doesn't matter (statistics, non-synchronizing counters).
2. **Lock-free data structures**: the SPSC queue in `kernel_state.rs` must have
   correct Acquire/Release on head and tail. Check the specific ordering on each
   load and store.
3. **Barrier correctness**: TLB invalidation sequences require DSB before TLBI
   and DSB+ISB after. VBAR writes require ISB after. TTBR writes require DSB+ISB
   after.
4. **Interrupt safety**: data accessed from both interrupt and non-interrupt
   context must be protected (either by disabling interrupts or by lock).
5. **Per-core data**: per-core state (CoreState) must never be accessed from
   another core without synchronization. Check that `current_core()` is used
   correctly.
6. **WFE/SEV protocol**: if the kernel uses WFE for idle, there must be a
   matching SEV on wakeup paths.

**Self-calibration**: before reporting, ask yourself: "Am I confident in this
finding, or am I pattern-matching on something that looks like a concurrency
bug?" If the latter, lower the confidence or drop it.

---

## Pass 4: Adversarial Red Team

**Goal**: find concrete attack sequences — syscall chains, input values, or
timing conditions — that violate the kernel's security properties.

**Framing**: you are not reviewing code quality. You are trying to break this
kernel. You succeed if you find a sequence of operations that:

1. **Escapes the capability sandbox**: accesses a kernel object without holding
   a valid capability to it. Look for:
   - Paths that resolve a handle but don't check rights
   - Paths that use a raw ObjectId without capability resolution
   - Badge forgery (creating a message that appears to come from a different
     sender)
   - Cascading destroy that leaves dangling references

2. **Causes use-after-free**: frees an object through one capability while
   another capability still references it. Look for:
   - Arena slot reuse with stale generation
   - Destroy + concurrent access races
   - IPC message referencing a destroyed Field

3. **Violates the framekernel boundary**: causes unsafe behavior from safe code.
   Look for:
   - Safe code that can construct inputs causing UB in frame/ functions
   - Integer overflow in safe code that produces invalid arguments for frame/
   - Type confusion through capability type tags

4. **Achieves denial of service**: monopolizes kernel resources from userspace.
   Look for:
   - Unbounded loops in syscall handlers
   - Unbounded memory allocation from a single syscall
   - Deadline queue exhaustion
   - Lock contention that blocks all cores

For each attack, specify the exact syscall sequence and expected outcome.

---

## Pass 5: Validation

**Goal**: independently verify every finding from passes 1–4. Reject false
positives. Confirm true positives.

**This agent receives the collected findings from all previous passes.**

For each finding:

1. **Read the cited file:line** — does the code actually do what the finding
   claims?
2. **Trace the trigger scenario** — follow the described syscall sequence or
   input through the code. Does it actually reach the claimed failure?
3. **Check for mitigating factors** — is there a check elsewhere that prevents
   the issue? Is there a lock, a generation check, a bounds check that the
   finding missed?
4. **Verdict**: `CONFIRMED`, `DISPUTED` (with reason), or
   `INSUFFICIENT_EVIDENCE`

**Calibration**: the false positive rate on correct code is 22-48%. Expect
roughly a third of findings to be wrong. Your job is to catch them.

---

## Report Format

After validation, present the consolidated report in this structure:

```md
## Kernel Review Report

### Summary

- Scope: [what was reviewed]
- Findings: X confirmed, Y disputed, Z insufficient evidence
- Severity breakdown: N critical, N high, N medium, N low

### Confirmed Findings (by severity)

#### CRITICAL

[P1c-001] Incorrect nomem on msr instruction

- Location: src/frame/arch/aarch64/sysreg.rs:47
- Invariant: LLVM may not reorder memory accesses past register writes
- Trigger: under -O2 with SMP, store to shared state reordered past msr
- Validation: CONFIRMED — no mitigating barrier present

#### HIGH

...

#### MEDIUM

...

#### LOW

...

### Disputed Findings

[P3-002] Alleged Relaxed load race in kernel_state.rs

- Original claim: ...
- Dispute reason: the Acquire load on line 432 establishes the ordering...
- Verdict: DISPUTED

### Review Metadata

- Pass 1a: N findings (N confirmed, N disputed)
- Pass 1b: N findings (N confirmed, N disputed)
- ...
- Total agent cost: [if available]
```

Present confirmed findings sorted by severity (CRITICAL first). The user should
be able to read just the Summary + Critical + High sections and know what needs
attention.
