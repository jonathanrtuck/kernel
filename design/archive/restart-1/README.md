# restart-1 — frozen 2026-04-15

The first post-restart derivation chain. Archived to re-derive contingent
decisions more carefully from first principles.

## Scope of this chain

The axioms (Rust/no_std, ARM64, generic kernel) were settled before this chain
started and remain settled in `design/spec.md` at the root. Everything below
them in this archive is contingent and up for re-derivation.

## Contents

- `spec.md` — Settled decisions as of 2026-04-13. Components (reactor, space
  manager, scheduler, context model), cross-cutting decisions (capability
  naming, fault routing via chains, badges, reply routing, four object types,
  ten syscalls, subdivision-based creation), and open questions.
- `graph.d2` — Structural map matching spec.md.
- `journal/` — 13 numbered entries (`001-component-exploration` through
  `013-object-creation-and-context-handles`) recording the reasoning that
  produced spec.md.
- `claims.toml` — Pre-restart design decisions (28 claims), itself already
  archived relative to restart-1.
- `AUDIT.md` — Translation notes for reading archive reasoning under the current
  (post-2026-04-15) axiom/philosophy split. Read first if pulling archive
  reasoning into a live derivation.

## What to preserve across restarts

Nothing in this directory is authoritative for the current design. But two
classes of content here are worth referencing:

- **Dead-end markers.** If a fresh derivation starts down a path journaled here
  as rejected, the reasoning for rejection is here.
- **Convergence candidates.** If a fresh derivation arrives at a decision
  structurally identical to one here, that convergence is evidence the decision
  captures something real about the problem shape.

Otherwise, re-derive from the axioms forward.
