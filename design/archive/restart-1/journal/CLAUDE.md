# design/journal

Numbered exploration entries that record the reasoning behind design decisions.
These are **prescriptive** documents — they start from a question, work through
the tradeoffs in context of this kernel, and arrive at (or defer) a decision.

Journal entries capture the thinking process: what was considered, what was
rejected, why, and what remains open. They reference research documents for the
factual foundation but contain the project-specific reasoning that research
documents must not.

## Distinction

- **Research (`design/research/`):** "ARM64 has hardware TLB broadcast via TLBI
  IS variants. DSB ISH stalls the initiating core until completion."
- **Journal (here):** "This means our hybrid model has almost no software
  broadcast cost on ARM64, which strengthens the case for shared memory + IPI
  over full Barrelfish."

## Format

Each entry is numbered sequentially. Structure:

1. Title and date
2. Starting point (what question, from where)
3. Exploration (reasoning, rejected alternatives, discoveries)
4. Status (what was decided, what's tentatively accepted, what's still open)

Entries reference `design/research/` for prior art and `design/spec.md` for
settled decisions. When a journal entry leads to a settled decision, that
decision moves into spec.md — the journal retains the reasoning history.
