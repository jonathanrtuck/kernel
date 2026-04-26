---
name: implement
description: >-
  Autonomous MVP plan implementation. Reads mvp-status.json, implements the next
  incomplete item, commits, updates status, and continues. Designed for /loop.
---

# Autonomous Implementation

You are implementing the kernel MVP plan without human interaction. This command
is your authorization to read, write, build, test, and commit. Do not ask the
user anything. Do not stop to confirm. Do not summarize what you did and wait.

## Each iteration

1. Read `.claude/mvp-status.json` — find the next `pending` item.
2. Read `.claude/mvp-plan.md` — find that item's section for context and fix
   description.
3. Read every source file you will modify AND every file that calls into it.
4. Implement the fix. Follow CLAUDE.md protocols (SAFETY comments on unsafe,
   no unsafe outside frame/, Spectre barriers where required).
5. Run `scripts/verify`. This runs clippy, bare-metal build, and all tests.
6. If verify passes: commit with message format below, then update status to
   `done`.
7. If verify fails: read the error, fix it, re-run. Up to 3 fix attempts per
   item. If still failing after 3 attempts, update status to `blocked` with
   the error in the `note` field, and move to the next item.
8. After updating status, the loop fires the next iteration automatically.

## Commit messages

```
fix(phase-N): 0.X — short description (D-numbers)

One-line explanation of what was wrong and why the fix is correct.
```

This command is explicit authorization to commit after each item. The
pre-commit hook runs the full verification gate — nothing broken can be
committed.

## When to stop the loop

- All items in the current phase are `done` or `blocked`. Run a bare-metal
  boot (`hypervisor target/aarch64-unknown-none/debug/kernel --no-gpu
  --timeout 10`) as the phase gate, then set the phase status to `complete`
  and advance `current_phase`.
- The next phase has unresolved `⚠ DECIDE` items. Set phase status to
  `blocked:decisions` and stop (do not call ScheduleWakeup).
- All phases are `complete`. Stop.

## Rules

- **Never ask the user anything.** Not "should I continue?", not "does this
  look right?", not "which approach?". The plan has the answers. If it
  doesn't, mark the item blocked and move on.
- **Never change the plan.** If the fix described in the plan is wrong or
  incomplete, implement it anyway, note the issue in the status `note` field,
  and move on. The plan is the spec; deviations are bugs to report, not
  decisions to make.
- **Never take shortcuts.** If the plan says "create a unified delivery path,"
  create a unified delivery path. Don't add a wakeup call to the existing
  split path and call it done.
- **Never silently work around a hard problem.** If something is genuinely
  difficult (e.g., the slab allocator wiring), spend the time. The plan
  already identified what's hard. Doing the easy version isn't implementing
  the plan.
- **One item per commit.** Don't batch. Don't split. One logical change, one
  commit, one status update.
- **Read before writing.** Every time. Read the file, read its callers, read
  the tests. Then write.

## On test failures

The pre-commit hook rejects bad commits, so you will encounter test failures
during development. This is expected.

1. Read the full error output. Don't just grep for "FAILED."
2. If the failure is in code you just changed: fix it.
3. If the failure is in an existing test you didn't touch: your change broke
   something downstream. Trace the call chain. Fix the root cause, not the
   test.
4. If the failure is a pre-existing issue unrelated to your change: note it
   in the status file but don't let it block your commit. Use
   `--no-verify` ONLY for genuinely pre-existing failures, and document
   exactly which test and why in the commit message. (This should be rare.)
5. After 3 failed fix attempts on the same item: mark blocked, move on.

## On bare-metal failures

If `hypervisor` hangs or panics during a phase gate:

1. Check serial output for the last line printed.
2. Check if the failure is in boot (before any test output) or during a
   specific test scenario.
3. If boot failure: likely a Phase 1 issue (TTBR, page tables, slab). Read
   the boot path in `main.rs` and `frame/boot.rs`.
4. If scenario failure: the serial output should indicate which scenario.
   Trace that code path.
5. After 3 attempts: mark the phase gate as blocked with full diagnostic.

## Status file format

```json
{
  "current_phase": 0,
  "phases": {
    "0": { "status": "in_progress" },
    "1": { "status": "pending" }
  },
  "items": {
    "0.1":  { "status": "pending", "note": "" },
    "0.2":  { "status": "pending", "note": "" }
  }
}
```

Valid statuses: `pending`, `in_progress`, `done`, `blocked`, `skipped`.
Phase statuses: `pending`, `in_progress`, `complete`, `blocked:decisions`.

When starting an item, set it to `in_progress`. On completion, set to `done`.
Always write the status file immediately after the state change — it is your
recovery mechanism after context compaction.
