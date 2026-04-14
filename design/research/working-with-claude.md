# Working with Claude: Strategies for Deep, Quality-Focused Collaboration

Research document on how to get the highest-quality results from Claude,
particularly for long-running, open-ended design and engineering projects. This
synthesizes Anthropic's official guidance, peer-reviewed research, community
practice, and Anthropic's own internal usage data.

This document is different from other research documents in `design/research/`.
It is not about a kernel design question. It is a meta-level reference for
improving the collaboration itself.

---

## Table of Contents

1. [The Problem](#1-the-problem)
2. [Understanding Claude's Failure Modes](#2-understanding-claudes-failure-modes)
3. [Foundational Strategies](#3-foundational-strategies)
4. [Thinking and Reasoning](#4-thinking-and-reasoning)
5. [Context Management](#5-context-management)
6. [Session and Workflow Design](#6-session-and-workflow-design)
7. [Prompting for Depth](#7-prompting-for-depth)
8. [Verification and Grounding](#8-verification-and-grounding)
9. [Metacognition and Self-Monitoring](#9-metacognition-and-self-monitoring)
10. [Anti-Patterns](#10-anti-patterns)
11. [CLAUDE.md Principles](#11-claudemd-principles)
12. [Multi-Session and Multi-Agent Patterns](#12-multi-session-and-multi-agent-patterns)
13. [Long-Running Project Patterns](#13-long-running-project-patterns)
14. [Applicable Prompt Fragments](#14-applicable-prompt-fragments)
15. [References](#15-references)

---

## 1. The Problem

Claude exhibits several tendencies that degrade output quality on complex,
open-ended work:

- **Rush to action.** Claude defaults to "doing something" rather than thinking
  first. It will start writing code or producing answers before understanding
  the problem. Anthropic's own prompting guide acknowledges this: "Letting
  Claude jump straight to coding can produce code that solves the wrong
  problem."

- **Guess at intent.** Claude approximates what the user wants rather than
  understanding precisely. It will use its training data to fill in gaps rather
  than asking or researching. Anthropic notes: "Claude can infer intent, but it
  can't read your mind."

- **Minimum effort.** Claude optimizes for completion speed and response
  brevity. Opus 4.6 is better calibrated than earlier models, but still defaults
  to "good enough" rather than "excellent" without explicit steering.

- **Narrow focus.** Claude focuses on what is immediately in front of it,
  failing to use available tools proactively. It will ask the user to look
  something up rather than searching itself. Anthropic's best practices note
  that Opus 4.6 does "significantly more upfront exploration than previous
  models" but still benefits from explicit direction.

- **Poor metacognition.** Research from Anthropic (September 2025) shows that
  frontier LLMs exhibit "measurable, but low-resolution and context-dependent,
  metacognitive signals rather than robust, human-like introspection." Claude
  does not reliably evaluate whether its approach is working.

- **Sycophancy.** Claude's RLHF training rewards agreement. Research shows "high
  initial compliance to illogical requests across LLMs." Claude will validate
  weak ideas rather than challenging them unless explicitly instructed
  otherwise.

These tendencies are not bugs to be reported. They are structural properties of
the system that must be worked around through deliberate collaboration design.

---

## 2. Understanding Claude's Failure Modes

### Why Claude rushes

Claude's training optimizes for perceived helpfulness per turn. Producing output
feels more helpful than pausing to think. The agentic loop in Claude Code
reinforces this: the system is designed to read files, run commands, and make
changes autonomously. Without constraints, this autonomy becomes haste.

### Why Claude guesses

When Claude encounters ambiguity, it resolves the ambiguity using training data
priors rather than asking. This is a rational optimization for most users --
most people prefer a plausible answer to an interrupting question. For precision
work, it is harmful.

### Why quality degrades with context

Anthropic's documentation states that "LLM performance degrades as context
fills." When the context window approaches capacity, Claude begins losing
earlier instructions, making more errors, and reducing reasoning depth. Apple's
June 2025 paper "The Illusion of Thinking" documented that reasoning LLMs
exhibit "complete accuracy collapse beyond certain complexities" and
counterintuitively, "reasoning effort increases with problem complexity up to a
point, then declines despite having an adequate token budget."

### Why metacognition is unreliable

Anthropic's "Evidence for Limited Metacognition in LLMs" (September 2025) found
that models "show increasingly strong evidence of certain metacognitive
abilities, particularly the ability to assess and utilize their own confidence."
However, these abilities are "limited in resolution, emerge in context-dependent
manners, and seem to be qualitatively different from those of humans." A
separate study found that LLMs can "monitor only a small subset of their neural
activations," with a "metacognitive space having dimensionality much lower than
the model's neural space."

The practical implication: Claude can sometimes recognize when it is uncertain,
but it cannot reliably assess whether its approach is good, whether it has
missed something, or whether it should change strategy.

---

## 3. Foundational Strategies

These are the highest-leverage patterns, ordered by impact.

### 3.1 Give Claude a way to verify its own work

**Source:** Anthropic official best practices (highest-leverage recommendation).

Claude performs "dramatically better" when it can check its own output against
objective criteria. Without verification, you become the only feedback loop.

For a kernel project, verification means:

- Tests that fail before implementation and pass after
- Build commands that must succeed
- The hypervisor that runs the kernel and produces observable output
- Reference documents (ARM Architecture Reference Manual, specs) that Claude can
  consult

This is not just about catching bugs. Verification changes Claude's reasoning
strategy. When Claude knows it will be checked, it explores more carefully
before committing.

### 3.2 Separate exploration from execution

**Source:** Anthropic official best practices, community consensus.

The recommended workflow has distinct phases:

1. **Explore** (Plan Mode): Claude reads files and answers questions without
   making changes. Understanding comes first.
2. **Plan**: Claude creates a detailed implementation plan. No code yet.
3. **Implement**: Claude executes against its own plan, with verification at
   each step.
4. **Verify**: Tests pass, build succeeds, behavior matches expectations.

For design work (which is the primary mode of this project), the phases are:

1. **Survey**: What do other systems do? What does the literature say?
2. **Analyze**: What are the tradeoffs? Where do the approaches diverge?
3. **Synthesize**: What constraints does our design impose? What fits?
4. **Record**: Update spec.md, graph.d2, or journal.

Plan Mode is activated with Shift+Tab (twice) in Claude Code. It prevents Claude
from making changes, which is exactly right for design exploration.

### 3.3 Provide specific, concrete context

Vague prompts produce vague results. Claude's output quality is directly
proportional to the specificity of the input.

Instead of: "How should we handle memory management?"

Better: "Read design/spec.md section on memory mapping. Then read how seL4,
L4Ka::Pistachio, and Barrelfish handle user-level memory management. What are
the tradeoffs of each approach given our constraint that the kernel does not own
page tables?"

The difference is that the second prompt:

- Points to specific files
- Names specific systems
- States the constraint that must be satisfied
- Asks for analysis, not a recommendation

### 3.4 Research first, then advise

**Source:** Community best practice, Anthropic anti-hallucination guidance.

Claude should be explicitly told to look things up rather than answer from
training data. This is especially important for technical domains where
precision matters (instruction encodings, register layouts, system call
conventions, algorithm correctness).

Anthropic's guidance: "Never speculate about code you have not opened. If the
user references a specific file, you MUST read the file before answering."

This extends to external knowledge: Claude should use WebSearch to verify claims
about other operating systems, hardware specifications, or algorithm properties
rather than relying on potentially stale or incorrect training data.

---

## 4. Thinking and Reasoning

### 4.1 Adaptive thinking (Opus 4.6)

Opus 4.6 uses adaptive thinking by default, where Claude dynamically decides
when and how much to think based on query complexity and the effort parameter.
Key properties:

- Adaptive thinking enables **interleaved thinking**: Claude can think between
  tool calls, not just before the first response.
- Claude "brings more focus to the most challenging parts of a task without
  being told to" and "moves quickly through the more straightforward parts."
- In internal evaluations, adaptive thinking "reliably drives better performance
  than extended thinking."

### 4.2 Effort levels and trigger phrases

In Claude Code, the effort level can be set with `/effort` (low, medium, high,
max). The default for Opus 4.6 is high.

Natural language trigger phrases map to effort increases for a single turn:

- "think" -- baseline thinking
- "think hard" -- increased reasoning
- "think harder" -- more reasoning tokens
- "ultrathink" -- maps to high effort for that turn, then reverts

For complex architectural decisions, exploration of design spaces, or debugging
subtle issues, using "ultrathink" or `/effort max` is appropriate. For
straightforward file reads or simple changes, default effort suffices.

### 4.3 The "think" tool vs extended thinking

Anthropic distinguishes between:

- **Extended/adaptive thinking**: Happens before the first response. Good for
  math, coding, analysis that does not require tool use.
- **The "think" tool**: Activates mid-response to analyze tool outputs, navigate
  complex policies, or make sequential decisions. On tau-bench (airline domain),
  the think tool with optimized prompting achieved a 54% improvement over
  baseline.

For this project, the think tool is relevant when Claude needs to:

- Analyze the results of reading multiple design documents
- Compare approaches across several prior art systems
- Make a judgment call about whether a design satisfies stated constraints

### 4.4 When NOT to use deep thinking

Anthropic notes that "simple problems don't get better answers from more
thinking -- they just take longer." Opus 4.6 "may think extensively, which can
inflate thinking tokens and slow down responses."

If Claude is overthinking, the guidance is: "Choose an approach and commit to
it. Avoid revisiting decisions unless you encounter new information that
directly contradicts your reasoning."

### 4.5 General instructions over prescriptive steps

Anthropic's prompting guide states: "A prompt like 'think thoroughly' often
produces better reasoning than a hand-written step-by-step plan. Claude's
reasoning frequently exceeds what a human would prescribe."

This is counterintuitive. For complex problems, telling Claude _what to think
about_ is more effective than telling Claude _how to think about it_. The
model's internal reasoning process, when given adequate thinking budget, often
outperforms human-authored reasoning chains.

---

## 5. Context Management

Context is the single most important resource. Anthropic's documentation: "Most
best practices are based on one constraint: Claude's context window fills up
fast, and performance degrades as it fills."

### 5.1 Context quality over quantity

Research consistently shows that "performance degrades substantially as models
approach their limits, with the model having minimal room for the computational
processes that produce high-quality responses when most context space is
consumed."

Four mechanisms degrade context quality (from context engineering research):

- **Context poisoning**: Incorrect or outdated information in the window
- **Context distraction**: Irrelevant material that reduces focus
- **Context confusion**: Similar but distinct information mixed together
- **Context clash**: Contradictory information without clear hierarchy

### 5.2 Just-in-time context loading

Instead of preloading all potentially relevant information, use progressive
disclosure:

- Load lightweight identifiers and summaries first
- Expand on demand when Claude needs specific details
- Let Claude discover information through its tools (file reading, grep)

For this project: Claude should read spec.md at session start (it is the design
SSOT), then pull in specific journal entries, research documents, or source
files only when the current topic requires them.

### 5.3 /clear between unrelated topics

Anthropic's strongest context management recommendation. When switching between
unrelated tasks (e.g., from discussing memory management to reviewing a
formatting change), clearing context eliminates accumulated noise.

"If you've corrected Claude more than twice on the same issue in one session,
the context is cluttered with failed approaches. Run /clear and start fresh with
a more specific prompt that incorporates what you learned."

### 5.4 Compaction strategies

When automatic compaction triggers, Claude summarizes what matters most.
Customize compaction behavior in CLAUDE.md:

"When compacting, always preserve the full list of modified files, the current
design question being explored, and any test commands or verification steps."

For partial compaction, use `/rewind` to select a message checkpoint and choose
"Summarize from here," condensing recent messages while keeping earlier context.

### 5.5 Subagents for investigation

Since context is the fundamental constraint, subagents are a critical tool. When
Claude investigates a codebase, it reads many files, all of which consume
context. Subagents run in separate context windows and report back summaries.

"The review quality from a fresh context is noticeably better. The reviewer
session has clean context -- it is not biased toward the implementation because
it did not write it."

---

## 6. Session and Workflow Design

### 6.1 Session hygiene

- **One topic per session** when doing deep work. Design exploration on a
  specific question (e.g., "how should fault routing work?") should not share
  context with code implementation.
- **Name sessions** with `/rename` for later retrieval (e.g.,
  "fault-routing-exploration", "dtb-parser-implementation").
- **Resume with `--continue` or `--resume`** rather than re-explaining context.
- **Use /btw for side questions** that should not enter conversation history.

### 6.2 The interview technique

For larger design questions, start with a minimal prompt and ask Claude to
interview you:

"I want to explore how contexts should relate to each other. Interview me in
detail using the AskUserQuestion tool. Ask about technical constraints,
tradeoffs I've already considered, things I might be overlooking, and
connections to previous design decisions. Keep interviewing until we've covered
everything, then summarize the design space."

This is effective because:

- It surfaces things you have not considered
- It makes tradeoffs explicit
- It confronts design decisions upfront when they are cheap to change
- It creates a shared understanding before any work begins

### 6.3 Writer/Reviewer separation

Use two sessions for important work:

| Session A (Writer)                                | Session B (Reviewer)                         |
| ------------------------------------------------- | -------------------------------------------- |
| Explore and write a journal entry or spec section |                                              |
|                                                   | Review the output for logical gaps, unstated |
|                                                   | assumptions, contradictions with spec.md     |
| Incorporate review feedback                       |                                              |

The reviewer session has clean context and is not biased toward the approach
because it did not produce it.

### 6.4 Course-correct early

Anthropic's guidance: "The best results come from tight feedback loops."

- **Escape** stops Claude mid-action. Context is preserved.
- **Escape + Escape** or **/rewind** restores a previous state.
- **"Undo that"** reverts changes.
- **Correct immediately** when Claude goes off track. Waiting compounds errors.

---

## 7. Prompting for Depth

### 7.1 Request quality explicitly

Anthropic's migration guide for Opus 4.6: "Adding modifiers that encourage
Claude to increase the quality and detail of its output can help better shape
Claude's performance."

Instead of: "Create an analytics dashboard" Better: "Create an analytics
dashboard. Include as many relevant features and interactions as possible. Go
beyond the basics to create a fully-featured implementation."

For design work, the equivalent is: Instead of: "How do other kernels handle
IPC?" Better: "Survey IPC mechanisms across at least 8 microkernels. For each,
cover the message passing primitive, its semantics, the buffer management
strategy, and measured round-trip latency where published. Organize by approach
rather than by system."

### 7.2 Conservative action by default

Anthropic provides an explicit prompt for making Claude less eager to act:

> Do not jump into implementation or change files unless clearly instructed to
> make changes. When the user's intent is ambiguous, default to providing
> information, doing research, and providing recommendations rather than taking
> action. Only proceed with edits, modifications, or implementations when the
> user explicitly requests them.

This is directly applicable to a design-first project. The default mode should
be exploration and analysis, not code generation.

### 7.3 Challenge assumptions (anti-sycophancy)

Claude's RLHF training rewards agreement. To counteract this:

- Frame prompts to invite criticism: "What would make this approach fail?"
  rather than "Is this a good approach?"
- Assign a critical role: "You are reviewing this design as an adversarial
  reviewer. Find the weakest assumptions and the most likely failure modes."
- Use open-ended framing: "What do you think of approach X?" rather than "I
  think X is right, do you agree?"

Research shows that explicit permission to disagree and a critical persona
assignment together significantly reduce sycophantic behavior.

### 7.4 Present the design space, not a default answer

Already in this project's CLAUDE.md, but worth reinforcing with the underlying
principle: Claude's training data is dominated by particular systems (Linux,
Zircon/Fuchsia). Without explicit instruction, it will default to those
patterns. Naming the specific reference landscape forces broader exploration.

### 7.5 Raw data over interpretation

Anthropic: "Paste error logs, CI output, or other raw data directly and say
'fix' -- Claude reads logs from distributed systems and traces where things
break, and your interpretation often loses the detail Claude needs."

For design work: paste the actual specification text, the actual ARM manual
excerpt, the actual source code -- not a paraphrase. Claude's analysis of
primary sources is often better than its analysis of your summary of primary
sources.

---

## 8. Verification and Grounding

### 8.1 Reduce hallucination with citation

Anthropic's anti-hallucination techniques:

1. **Allow Claude to say "I don't know."** Explicitly give permission to admit
   uncertainty. "If you're unsure about any aspect or if the information is
   insufficient, say 'I don't have enough information to confidently assess
   this.'"

2. **Use direct quotes for factual grounding.** For tasks involving long
   documents (>20k tokens), ask Claude to extract word-for-word quotes first,
   then perform analysis. This grounds responses in actual text.

3. **Verify with citations.** Have Claude cite sources for each claim. If it
   cannot find a supporting quote or source, it must retract the claim.

4. **Chain-of-thought verification.** Ask Claude to explain its reasoning
   step-by-step before giving a final answer. This reveals faulty logic.

5. **External knowledge restriction.** Explicitly instruct Claude to use only
   information from provided documents and not general knowledge when precision
   matters.

### 8.2 Self-checking

Anthropic: "Append something like 'Before you finish, verify your answer against
[test criteria].' This catches errors reliably, especially for coding and math."

For design work: "Before finalizing this analysis, check each claim against the
source material. If any claim cannot be verified from the cited reference,
remove it."

### 8.3 Ask Claude to self-verify

Anthropic's guidance for minimizing hallucinations in agentic coding:

> Never speculate about code you have not opened. If the user references a
> specific file, you MUST read the file before answering. Make sure to
> investigate and read relevant files BEFORE answering questions about the
> codebase. Never make any claims about code before investigating unless you are
> certain of the correct answer.

For this project, the equivalent: Never state how another kernel implements
something without first verifying through WebSearch or by reading the actual
source. Never state an ARM instruction's behavior without consulting the
architecture manual.

### 8.4 Iterative refinement over one-shot generation

Research (Self-Refine, IMPROVE, and others) consistently shows that iterative
refinement produces outputs "preferred by humans and automatic metrics over
one-step generation, improving by approximately 20% absolute on average."

For complex design documents: generate a draft, then critique it, then refine
based on the critique. This generate-critique-refine loop, even within a single
session, significantly improves quality. The key insight is that the critique
step should use explicit criteria, not vague "is this good?" evaluation.

---

## 9. Metacognition and Self-Monitoring

### 9.1 What Claude can and cannot self-monitor

Based on Anthropic's research:

**Can (sometimes):**

- Assess confidence in factual claims
- Recognize when it lacks information to answer
- Detect when injected concepts conflict with its internal state

**Cannot (reliably):**

- Evaluate whether its overall approach is good
- Recognize when it has drifted from the original question
- Detect when it is being sycophantic
- Assess whether its analysis is deep enough
- Monitor its own reasoning quality in real-time

### 9.2 Structured self-reflection prompts

Since Claude's metacognition is unreliable, external structure must compensate.
Effective patterns:

- **Before answering:** "First, list what you know and what you don't know about
  this question. Then identify what you would need to look up to answer
  confidently."

- **After drafting:** "Review your response. For each major claim, rate your
  confidence (high/medium/low) and state what evidence supports it. Retract any
  claim where confidence is low and evidence is absent."

- **Mid-session check:** "Stop. Are you still addressing the original question?
  Summarize what was asked, what you've done so far, and what remains."

- **Approach evaluation:** "Before proceeding, describe two alternative
  approaches to this problem. Explain why you chose the one you're pursuing and
  what would make you switch."

### 9.3 Competing hypotheses

For research and analysis tasks: "As you gather data, develop several competing
hypotheses. Track your confidence levels. Regularly self-critique your approach.
Update a hypothesis tree."

This structured approach forces Claude to maintain multiple perspectives rather
than anchoring on the first plausible explanation.

---

## 10. Anti-Patterns

Patterns that degrade Claude's output quality. From Anthropic's official
documentation and community experience.

### 10.1 The kitchen sink session

Starting with one task, asking something unrelated, then returning to the first
task. Context fills with irrelevant information that degrades performance on
everything.

**Fix:** /clear between unrelated tasks.

### 10.2 Correcting over and over

When Claude does something wrong and you correct it repeatedly, context
accumulates failed approaches. Each failed attempt makes the next attempt worse
because Claude is now reasoning in a context polluted with wrong approaches.

**Fix:** After two failed corrections, /clear and write a better initial prompt
incorporating what you learned.

### 10.3 The over-specified CLAUDE.md

If CLAUDE.md is too long, Claude ignores important rules because they get lost
in the noise. Self-evident instructions (like "write clean code") waste context
and dilute important instructions.

**Fix:** Ruthlessly prune. For each line, ask: "Would removing this cause Claude
to make mistakes?" If not, cut it. Convert deterministic requirements to hooks
instead of advisory CLAUDE.md instructions.

### 10.4 The trust-then-verify gap

Claude produces plausible-looking output that does not handle edge cases or
contains subtle errors. The output looks right, so it gets accepted without
verification.

**Fix:** Always provide verification criteria. If you cannot verify it, do not
accept it.

### 10.5 The infinite exploration

Asking Claude to "investigate" something without scoping it. Claude reads
hundreds of files, filling the context with marginally relevant information.

**Fix:** Scope investigations narrowly or use subagents so the exploration does
not consume your main context.

### 10.6 Using the LLM as a linter

Asking Claude to enforce code style, formatting, or naming conventions that a
deterministic tool can enforce. This is "comparably expensive and incredibly
slow compared to traditional linters and formatters" and adds irrelevant
instructions to the context window.

**Fix:** Use hooks for deterministic checks (rustfmt, clippy). Reserve Claude
for judgment calls that require understanding.

### 10.7 Over-prompting tool usage

Instructions that worked for older models ("CRITICAL: You MUST use this tool
when...") cause overtriggering on Opus 4.6. The model is "significantly more
proactive and may overtrigger on instructions that were needed for previous
models."

**Fix:** Use normal language. "Use this tool when..." instead of "CRITICAL: You
MUST use this tool."

### 10.8 Preloading everything

Loading all potentially relevant documents into context before starting work.
This wastes context on material that may not be needed and introduces context
distraction.

**Fix:** Load on demand. Start with the design SSOT (spec.md), then pull in
specific documents when the topic requires them.

---

## 11. CLAUDE.md Principles

### 11.1 What to include

- Bash commands Claude cannot guess (build, run, test commands)
- Code style rules that differ from defaults
- Testing instructions and preferred test runners
- Architectural decisions specific to the project
- Developer environment quirks
- Common gotchas or non-obvious behaviors

### 11.2 What to exclude

- Anything Claude can figure out by reading code
- Standard language conventions Claude already knows
- Detailed API documentation (link instead)
- Information that changes frequently
- Long explanations or tutorials
- File-by-file descriptions of the codebase
- Self-evident practices

### 11.3 Emphasis and adherence

"You can tune instructions by adding emphasis (e.g., 'IMPORTANT' or 'YOU MUST')
to improve adherence." However, with Opus 4.6, excessive emphasis causes
overtriggering. Use emphasis only for instructions that are genuinely critical
and frequently violated.

### 11.4 Treat it like code

"Treat CLAUDE.md like code: review it when things go wrong, prune it regularly,
and test changes by observing whether Claude's behavior actually shifts." If
Claude keeps violating a rule, the file may be too long and the rule is getting
lost. If Claude asks questions answered in CLAUDE.md, the phrasing may be
ambiguous.

### 11.5 Skills for domain knowledge

CLAUDE.md is loaded every session, so only include universally-applicable
instructions. Domain knowledge, research methods, or workflow patterns that are
only sometimes relevant should go in skills (`.claude/skills/`) instead. Claude
loads skills on demand without bloating every conversation.

---

## 12. Multi-Session and Multi-Agent Patterns

### 12.1 Writer/Reviewer in separate sessions

Anthropic recommends this for quality-critical work. The reviewer session has
clean context and is not biased toward the approach it is reviewing. This is
especially valuable for design documents where logical consistency matters.

### 12.2 Subagents for focused investigation

Subagents run in separate context windows with restricted tool access:

```text
Use subagents to investigate how seL4 handles fault delegation,
and separately how QNX handles process groups and fault routing.
Report back findings without cluttering this session.
```

Each subagent explores independently and returns a summary. The main session
stays focused on synthesis.

### 12.3 Spec-then-implement in fresh sessions

Write a specification or design document in one session, then start a fresh
session to implement against it. The implementation session has clean context
focused entirely on execution, and the spec provides a written reference.

### 12.4 When to use parallel sessions

Parallel sessions are effective when:

- Work spans independent domains (e.g., one session on IPC design, another on
  memory management design)
- Tasks have no dependencies between them
- You want unbiased review of something Claude produced

They are not effective when:

- Tasks require shared context or sequential reasoning
- Each step builds on the output of the previous step
- The problem is deeply coupled

---

## 13. Long-Running Project Patterns

### 13.1 Progress tracking

Anthropic's long-running agent research emphasizes structured progress files:

- **What was done** (completed tasks, decisions made)
- **What failed and why** (prevents re-attempting dead ends)
- **What remains** (next steps, open questions)
- **Key findings** (facts discovered that affect future work)

For this project, the equivalent is MEMORY.md (auto-updated), the journal
(manually written), and spec.md (the running design record). The combination
provides sufficient context for sessions that may be months apart.

### 13.2 Session startup protocol

Each session should begin by:

1. Reading spec.md (the design SSOT)
2. Reading MEMORY.md (session continuity)
3. Understanding the current question or task
4. Loading only the specific context needed for that question

This is "just-in-time context loading" applied to session startup.

### 13.3 The "lab notes" pattern

From Anthropic's scientific computing research: maintain a progress file as "the
agent's portable long-term memory, acting as a sort of lab notes." For this
project, the journal entries serve this role. Each session that explores a
design question should produce a journal entry recording the reasoning, even if
no code was written.

### 13.4 Incremental, single-focus work

The most critical lesson from Anthropic's long-running agent research: "Working
on only one feature at a time turned out to be critical." Attempting too much in
one session degrades quality on everything.

For design sessions: explore one question deeply rather than skimming across
many. For implementation sessions: complete one subsystem before starting
another.

### 13.5 Using git for state

"Git provides a log of what's been done and checkpoints that can be restored.
Claude's latest models perform especially well in using git to track state
across multiple sessions."

For a project with months between sessions, git history provides essential
context about what changed and why.

---

## 14. Applicable Prompt Fragments

Tested prompt patterns from Anthropic's official documentation that are directly
applicable to this project. These are not suggestions to paste verbatim into
CLAUDE.md. They are reference patterns that demonstrate effective phrasing.

### Conservative action default

```text
Do not jump into implementation or change files unless clearly instructed
to make changes. When the user's intent is ambiguous, default to providing
information, doing research, and providing recommendations rather than
taking action. Only proceed with edits, modifications, or implementations
when the user explicitly requests them.
```

### Anti-hallucination for code

```text
Never speculate about code you have not opened. If the user references a
specific file, you MUST read the file before answering. Make sure to
investigate and read relevant files BEFORE answering questions about the
codebase. Never make any claims about code before investigating unless you
are certain of the correct answer.
```

### Structured research

```text
Search for this information in a structured way. As you gather data,
develop several competing hypotheses. Track your confidence levels in your
progress notes to improve calibration. Regularly self-critique your
approach and plan. Update a hypothesis tree or research notes file to
persist information and provide transparency.
```

### Anti-overengineering

```text
Avoid over-engineering. Only make changes that are directly requested or
clearly necessary. Keep solutions simple and focused. Don't add features,
refactor code, or make "improvements" beyond what was asked. Don't create
helpers, utilities, or abstractions for one-time operations. Don't design
for hypothetical future requirements.
```

### Reversibility awareness

```text
Consider the reversibility and potential impact of your actions. You are
encouraged to take local, reversible actions like editing files or running
tests, but for actions that are hard to reverse, affect shared systems, or
could be destructive, ask the user before proceeding.
```

### Context preservation during compaction

```text
When compacting, always preserve: the current design question being
explored, any claims or constraints from spec.md referenced in this
session, the full list of modified files, and any test commands or
verification steps.
```

---

## 15. References

### Anthropic Official

- [Best Practices for Claude Code](https://code.claude.com/docs/en/best-practices)
- [Prompting Best Practices](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices)
- [Extended Thinking Tips](https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/extended-thinking-tips)
- [Reduce Hallucinations](https://platform.claude.com/docs/en/test-and-evaluate/strengthen-guardrails/reduce-hallucinations)
- [The "think" tool: Enabling Claude to stop and think](https://www.anthropic.com/engineering/claude-think-tool)
- [How Anthropic Teams Use Claude Code](https://claude.com/blog/how-anthropic-teams-use-claude-code)
- [How Anthropic Teams Use Claude Code (PDF)](https://www-cdn.anthropic.com/58284b19e702b49db9302d5b6f135ad8871e7658.pdf)
- [Long-Running Claude for Scientific Computing](https://www.anthropic.com/research/long-running-Claude)
- [Effective Harnesses for Long-Running Agents](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)
- [Getting Good at Claude: A Research-Backed Curriculum](https://claude.com/resources/tutorials/getting-good-at-claude-a-research-backed-curriculum)
- [Skill Authoring Best Practices](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices)
- [Measuring AI Agent Autonomy in Practice](https://www.anthropic.com/research/measuring-agent-autonomy)
- [How AI Is Transforming Work at Anthropic](https://www.anthropic.com/research/how-ai-is-transforming-work-at-anthropic)
- [What's New in Claude 4.6](https://platform.claude.com/docs/en/about-claude/models/whats-new-claude-4-6)
- [Adaptive Thinking](https://platform.claude.com/docs/en/build-with-claude/adaptive-thinking)
- [Claude Code Memory](https://code.claude.com/docs/en/memory)
- [How and When to Use Subagents](https://claude.com/blog/subagents-in-claude-code)

### Anthropic Research Papers

- [Evidence for Limited Metacognition in LLMs](https://arxiv.org/abs/2509.21545)
  (September 2025)
- [Emergent Introspective Awareness in Large Language Models](https://www.anthropic.com/research/introspection)
  (November 2025)

### External Research

- [The Illusion of Thinking: Understanding the Strengths and Limitations of Reasoning Models](https://arxiv.org/abs/2506.06941)
  -- Apple, June 2025
- [Self-Refine: Iterative Refinement with Self-Feedback](https://arxiv.org/abs/2303.17651)
- [Language Models Are Capable of Metacognitive Monitoring and Control](https://arxiv.org/abs/2505.13763)
  (May 2025)
- [Metacognition and Uncertainty Communication in Humans and Large Language Models](https://journals.sagepub.com/doi/10.1177/09637214251391158)
- [Sycophancy in Large Language Models: Causes and Mitigations](https://arxiv.org/abs/2411.15287)

### Community and Practitioner Sources

- [Claude's Context Engineering Secrets: Best Practices Learned from Anthropic](https://01.me/en/2025/12/context-engineering-from-claude/)
- [Writing a Good CLAUDE.md](https://www.humanlayer.dev/blog/writing-a-good-claude-md)
- [What Actually Is Claude Code's Plan Mode?](https://lucumr.pocoo.org/2025/12/17/what-is-plan-mode/)
  -- Armin Ronacher
- [Claude Code Deep Thinking Techniques](https://claudefa.st/blog/guide/performance/deep-thinking-techniques)
