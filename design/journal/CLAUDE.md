# design/journal

Numbered exploration entries that record the reasoning behind design
decisions. These are **prescriptive** documents — they start from a question,
work through the tradeoffs in context of this kernel, and arrive at (or
defer) a decision.

Journal entries capture the thinking process: what was considered, what was
rejected, why, and what remains open. They reference research documents for
the factual foundation but contain the project-specific reasoning that
research documents must not.

## Distinction

- **Research (`design/research/`):** "ARM64 has hardware TLB broadcast via
  TLBI IS variants. DSB ISH stalls the initiating core until completion."
- **Journal (here):** "This means our hybrid model has almost no software
  broadcast cost on ARM64, which strengthens the case for shared memory +
  IPI over full Barrelfish."

## Format

Each entry is numbered sequentially. Structure:

1. **Title and date.**
2. **Starting point.** What question, from where in the tree of prior
   exploration.
3. **Exploration.** The reasoning itself — alternatives considered, rejected
   paths, discoveries. When a philosophy principle does load-bearing work in
   the reasoning, name it explicitly ("applying 'find the abstraction that
   absorbs the edge cases' here…"). This makes the philosophy's role
   traceable without requiring it to live in the dependency graph.
4. **Status.** What was decided, tentatively accepted, or left open. If a
   decision lands, it moves to `design/spec.md` with the provenance
   template there; the journal retains the full reasoning.

## Relationship to spec.md

Spec entries state conclusions; journals carry the arguments. A spec entry
is short and names its load-bearing predecessors. The journal is long and
carries the reasoning the spec entry's `Rests on` line compresses.

When a spec entry moves, the journal that produced it stays. When a journal
entry supersedes a prior one, the prior journal is preserved (exploration
history is the record) but the spec entry is updated.
