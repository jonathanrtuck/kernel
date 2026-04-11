# design/research

Prior art studies prepared before major design decisions. These are
**descriptive** documents — they record what exists in the world, what other
systems chose, what the tradeoffs are, and what the measured data shows.

Research documents must not contain project-specific reasoning, design
preferences, or conclusions about what this kernel should do. That belongs in
`design/journal/`.

## Distinction

- **Research (here):** "Barrelfish uses per-core capability spaces replicated by
  user-mode monitors."
- **Journal:** "We're not doing that because we don't want distributed consensus
  above the kernel|userspace interface."

A research document should be reusable — a future session could read it and
reach a different conclusion than the journal did.

## Format

Each document covers one design question. Structure:

1. Frame the question
2. Survey how existing systems answer it (name systems, cite papers)
3. Include measured data where available (latency, overhead, benchmarks)
4. List tradeoffs without ranking them
5. References at the end
