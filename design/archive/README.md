# design/archive

Frozen artifacts from previous derivation chains. These are not the current
source of truth — `design/spec.md` and `design/graph.d2` at the design root are.
This directory exists so that previous reasoning remains readable on demand
without polluting the default context of a fresh session.

## Purpose

Two reasons to preserve previous chains rather than deleting them:

1. **Convergence evidence.** The project's philosophy holds that when
   independent reasoning paths arrive at the same answer, that convergence is
   stronger evidence than any single argument. That test is only runnable if the
   prior path is still readable.

2. **Dead-end memory.** A fresh derivation that runs into the same dead-end a
   previous derivation explored should be able to discover that it has been
   tried and why it was abandoned — without being biased by that memory during
   the initial derivation.

## Rules

- **Do not auto-load archive content into session context.** Sessions start from
  the axioms in `design/spec.md` and derive forward.
- **Do not reference archived decisions as authoritative.** They document what a
  previous chain concluded, not what is true.
- **Consult archived content after a fresh derivation has reached an answer**,
  to check convergence or to avoid re-discovering a known dead-end.

## Contents

- `restart-1/` — Frozen on 2026-04-15. The derivation that ran from the original
  restart through 13 journal entries (capability representation, context model,
  message shape, badges, object types, syscalls). Reset for more careful
  derivation of contingent decisions.
