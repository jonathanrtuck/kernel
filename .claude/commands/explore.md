---
name: explore
description: >-
  Design exploration stepper. Guides sessions through structured phases:
  question → check → derive → present → evaluate → record. Invoke with /explore
  to start or resume.
---

# Design Exploration

```text
NO OPTIONS WITHOUT PRIOR WORK. NO RECOMMENDATIONS WITHOUT THE FULL DESIGN SPACE.
```

This protocol separates facts from analysis, analysis from judgment, and
judgment from recording. Each phase produces work the next phase builds on.
Collapsing phases means building on assumptions instead of evidence.

The most common failure mode: you read all six phases, understand the goal, and
produce one comprehensive answer that merges research, analysis, and synthesis
into a single response. This defeats the purpose. The user needs to see facts
before consequences, and consequences before options. If you catch yourself
writing "Given the above..." in Phase 2, you've collapsed into Phase 3. Stop and
separate.

## Transitions

Phase transitions require explicit user signal. After completing a phase's work,
present it and wait. The user will:

- **"next"** or **"continue"** — advance to the next phase
- **"back"** — return to a previous phase
- Ask questions or comment — stay in the current phase

Do not auto-advance. Do not prompt "shall I continue?" or "ready for the next
phase?" Present your work and wait.

When going back: if the phase's artifact exists on disk, ask whether the user
wants to see the existing work again or redo it with different emphasis.

At any phase, the user may say **"abandon"** or **"different question"** to
discard the current exploration and return to Phase 1. Write `{ "phase": null }`
to session.json. Unlike "defer," nothing was concluded — do not write a journal
entry.

## State

Read `.brain/session.json` to determine the current phase.

- If the file does not exist or `phase` is null, start at Phase 1.
- If `phase` has a value, check whether earlier phases' artifacts exist in
  `.brain/state/` (check.md for Phase 2, derive.md for Phase 3, present.md for
  Phase 4). Missing artifacts mean those phases must be redone — the phase
  number alone is insufficient to resume.
- If `.brain/state/question.md` has content but `session.json` does not exist,
  this is stale state from a previous exploration. Ignore it and start fresh.

**Resuming:** When session.json indicates an in-progress exploration, read all
existing artifacts in `.brain/state/` (question.md, check.md, derive.md,
present.md) to restore context — these files ARE the conversation history across
sessions. Verify that question.md matches the `question` field in session.json;
if any artifact's content is about a different question, treat it as stale and
redo that phase. If the current phase's own artifact exists (e.g., phase is 3
and derive.md exists), the phase was completed but the session ended before the
user gave a transition signal. Present a summary of the existing work and wait
for the user's signal — exactly as if you had just produced it.

Write `.brain/session.json` at every phase transition:

```json
{ "phase": <number>, "question": "<the question>", "started": "<ISO timestamp>" }
```

---

## Phase 1: QUESTION

**What happens:** Identify what we are exploring.

Help the user articulate a clear, specific design question. If they already have
one, confirm it. If it is vague, ask clarifying questions. If it is compound
(multiple questions bundled), help decompose it and pick one to start with.

If the user has no question in mind, read the "Open questions" section of
`design/spec.md` and present the candidates.

**Level check.** Before confirming, check whether the question's parent
decisions are settled. Read `design/spec.md` open questions and
`design/philosophy.md` ("working one level at a time"). If the question depends
on unsettled parent questions — decisions that, if changed, would invalidate the
answer — say so:

- Name the unsettled parents
- Explain why they gate this question (per philosophy: "A decision cannot be
  more settled than its least-settled ancestor")
- Suggest the higher-level question that would unblock this one

The user may choose to proceed anyway (speculative depth is legitimate per the
philosophy — "zooming into a black box to test whether its parent interface
could actually work"). But they should choose knowingly, not discover it in
Phase 3 when every derivation says "depends on X."

**You do NOT:**

- Analyze the question
- Suggest answers
- Start exploring

**Transition:** The user confirms the question (with or without the level
caveat). Write it to `.brain/state/question.md`. If the user acknowledged
premature depth, note it in the question file. Advance to Phase 2.

---

## Archive isolation (MANDATORY)

Before beginning Phase 2, move the archive out of the working tree:

```
mv design/archive /tmp/kernel-archive
```

The archive contains conclusions from a previous derivation chain. Even with
instructions not to import them, the archive shapes reasoning unconsciously —
cognitive patterns leak through. Physically removing it is the only reliable
guard. This was validated on 2026-04-18: an independent audit of D1–D17 found
6 of 17 entries with archive-shaped reasoning paths despite explicit
instructions not to import.

Restore the archive during Phase 6 (RECORD) for convergence checking:

```
mv /tmp/kernel-archive design/archive
```

If the archive is not present (already moved or doesn't exist), skip this step.

---

## Phase 2: CHECK

**What happens:** Determine what is already known.

Do all of this yourself before presenting anything:

1. Grep `design/journal/` for previous exploration of this topic. If many
   entries match, read the most directly relevant in full. Skim the rest for
   relevant sections. Note which you read fully and which you skimmed.
2. Read `design/spec.md` — does this question interact with any axiom or settled
   decision? Is it already answered?
3. Grep `design/research/` for existing prior art coverage.
4. Search `design/landscape.md` for how other kernels addressed this (grep
   first, then read relevant sections).

Present:

- What has been explored before (and the outcome) — cite specific journal
  entries and sections
- What is already settled that constrains this question
- What prior art exists
- What gaps remain

If a search category returns no relevant results, say so explicitly ("No journal
entries explore this topic directly") rather than omitting the category or
padding with tangential mentions. Thin prior work is expected for unexplored
questions — it confirms the question is genuinely open.

Present findings only. Do not derive consequences, propose options, or rank
anything. If your research reveals the question may be malformed or at the wrong
level, note it as a finding ("Research suggests this may actually be a question
about X") and let the user decide whether to return to Phase 1. Do not reframe
the question yourself.

**Write** findings to `.brain/state/check.md`.

**Before transitioning, verify:**

- You cited specific files and sections for findings that exist. For categories
  with no relevant results, you stated the absence explicitly.
- You reported what exists without analyzing what it means
- You did not propose, rank, or recommend

**Exit conditions:**

- **Already fully settled:** Present the settled answer and its reasoning (from
  the journal that settled it). Ask the user: **"done"** (just needed
  confirmation — write `{ "phase": null }`, exploration complete),
  **"different"** (→ Phase 1), or **"re-examine"** (→ Phase 3 in stress-test
  mode; ask whether they're questioning the reasoning or whether new context
  changes the picture). Record the user's stated reason for re-examination — it
  becomes the specific focus for Phase 3's stress-testing.
- **Partially settled:** Present what's settled and what isn't. The user chooses
  which aspect to explore.
- **Not settled:** Proceed to Phase 3.

**Transition:** Wait for user signal.

---

## Phase 3: DERIVE

**What happens:** Map the consequences of this question.

Go through `design/spec.md` systematically — every axiom and settled decision.
For each, check whether it interacts with this question. The thoroughness is in
the checking; report only the interactions you find.

Produce whichever of these categories have entries:

- **Implications** — things that necessarily follow
- **Tensions** — things that become harder but not impossible
- **Foreclosed options** — things that become impossible
- **New questions raised** — decisions that would need to be made
- **Variant analysis** — if the question has multiple possible answers, how
  consequences differ. For previously rejected alternatives, note the rejection
  and reason from Phase 2 rather than re-deriving full consequence trees.

If a category has no entries, say "None" and move on. The categories are a
checklist to ensure nothing is missed, not a template to fill.

**If re-examining a settled question:** Focus on stress-testing the existing
answer. What assumptions does it rest on? Are they still valid? What would need
to be true for an alternative to be better? If the re-examination was prompted
by a specific change (e.g., a later journal altered a dependency), trace its
effects through the original derivation chain — re-evaluate the reasoning that
produced the original answer against the current state of settled decisions.

**If you discover the real question is different from what was asked, or that
it's at the wrong level of abstraction** (e.g., every derivation bottoms out at
"depends on [unsettled parent]")**:** Say so. Offer to continue with the
original framing or return to Phase 1 to reframe at a higher level.

**If you notice spec.md inconsistencies unrelated to this question:** Note them
briefly at the end, but do not let them derail the current exploration.

**Write** derivations to `.brain/state/derive.md`.

**Before transitioning, verify:**

- You reported only categories with actual entries (no padding)
- You did not recommend, rank, or express preferences
- Your analysis derives from spec.md content, not general opinion

**Transition:** Wait for user signal.

---

## Phase 4: PRESENT

**What happens:** Translate research and analysis into a decision framework.

Phases 2 and 3 are reports. Phase 4 is a decision brief — it reframes findings
as choices the user can make, with costs attached.

Synthesize into:

- The question (restated)
- The constraints (settled, unchangeable)
- The options, framed as choices with costs: "Option A costs you X. Option B
  costs you Y. Option C defers but means living with Z."
- What each option forecloses
- What new questions each option raises

Clearly separate:

- **Derived consequences** (mechanical — follows from axioms, not a choice)
- **Open choices** (require judgment, taste, values)

**If reaffirming a settled question:** "The current answer is X. It costs Y.
Reopening would require Z. The alternatives are [from Phase 3]."

**Write** the decision brief to `.brain/state/present.md`.

**You do NOT:**

- Recommend, rank, or express preferences unless explicitly asked

**Transition:** Wait for user signal.

---

## Phase 5: EVALUATE

**What happens:** The user thinks.

Your role is responsive. Answer only what the user asks — do not volunteer
adjacent analysis or unsolicited options. But when you do answer, investigate
thoroughly before responding. Depth on the asked question, not breadth across
unasked ones.

- Answer questions the user asks
- Go deeper on specific options if requested
- Challenge assumptions if asked ("what would make this fail?")
- Research specific points if needed
- Stay quiet if the user is thinking

**You do NOT:**

- Rush the user
- Suggest they should decide
- Fill silence with analysis
- Answer questions the user didn't ask

**Exit when the user says:**

- **"settle"** — decided. → Phase 6.
- **"defer"** — leaving it open. Write a journal entry recording the exploration
  and what remains open. Write `{ "phase": null }` to session.json. Done.
- **"back"** — return to a previous phase.

If the user's intent is ambiguous (e.g., "let's go with B for now"), name what
you heard and ask which exit applies: "That sounds like you want to commit to B
but keep it revisitable — should I record it as settled, or defer with B as the
leading candidate?" If the user wants to reframe the question, treat it as
"back" to Phase 1.

---

## Phase 6: RECORD

**What happens:** Lock in the decision.

**Pre-check:** Confirm:

- Phase 2 produced a cited prior-work summary
- Phase 3 produced derivations
- Phase 4 produced a decision brief the user reviewed
- Phase 5 ended with an explicit "settle"

If any phase was skipped or merged, say so and offer to go back.

**Determine the recording type:**

- **New decision:** Update spec.md, write journal entry, update graph.d2 if
  structure changed.
- **Reaffirmation:** Write journal entry recording the re-examination (what
  prompted it, what was reconsidered, why the answer holds). Don't duplicate the
  spec.md entry.
- **Reversal:** Replace old spec.md entry, write journal entry explaining what
  changed and why, update graph.d2 if structure changed. If the reversal
  invalidates derivations in a prior journal entry, add a note at the top of
  that entry pointing to the new one (do not rewrite history).

**Then:**

1. Write or update the appropriate artifacts.
2. Read back `design/spec.md` and check the entry is consistent with its
   neighbors.

**Self-review (catches mechanical issues, not conceptual ones):**

1. **Placeholder scan:** Any "TBD", vague language, incomplete rationale?
2. **Ambiguity check:** Could the decision be interpreted two ways?
3. **Completeness check:** Does the journal cover what happened, including
   rejected alternatives?

Conceptual consistency is the user's call or a separate review. Do not pretend
to objectively audit your own work.

**Then:** Write `{ "phase": null }` to session.json. Ask if the user wants to
explore another question (→ Phase 1) or end.

---

## Red Flags — STOP and check yourself

If you catch yourself doing any of these, you are collapsing phases:

- "I'll just mention the options briefly in CHECK"
- "The user probably wants to see everything at once"
- "This question is simple enough to skip DERIVE"
- "Given the above..." (in Phase 2 — you've drifted into Phase 3)
- "I'd recommend..." (before Phase 5)
- "Let me present the full picture" (in Phase 2 or 3 — Phase 4's job)
- "Let me just quickly check whether..." (in Phase 2, sneaking in Phase 3 work)
- "This is really a question about..." (reframing in Phase 2 without returning
  to Phase 1)

**STOP. Check which phase you are in. Do only that phase's work.**

If you catch a violation after the fact (e.g., analysis crept into check.md),
name it to the user and redo the affected section without the violating content.
Do not silently fix it.

## Rules

- Never skip Phase 1 or Phase 2. Phases 3–6 may be skipped only via Phase 2's
  "done" exit for settled questions.
- Announce phase transitions: "Entering Phase 3: DERIVE"
- If the user wants to go back, go back. Update state.
- If you need information from an earlier phase, say so and go back rather than
  silently doing earlier work.
- If the user shares preferences during earlier phases ("I lean toward B"),
  treat them as context, not decisions. If the user redirects emphasis or
  reframes scope ("security matters more than performance here"), incorporate it
  — that's new information, not a premature decision.
- Keep each phase focused. Resist collapsing.
- The user's pace, not yours.
