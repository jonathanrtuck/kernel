# Kernel Design Specification

The current state of the kernel's design. Settled decisions with brief
rationale. See `design/graph.d2` for the structural map and `design/journal/`
for full exploration history.

This document is intentionally sparse. It was reset on 2026-04-15 to re-derive
contingent decisions from first principles. The previous derivation chain is
preserved under `design/archive/restart-1/` for convergence-checking — consult
it only after a fresh derivation has arrived at an answer.

---

## Axioms

These are design inputs, not decisions. They constrain everything that follows.
Labeled for reference from derivation entries' "Rests on" lines.

- **A1 — Rust (nightly, no_std).** Not a language preference — a design input.
  Ownership maps to resource lifecycle. Traits map to architecture abstraction.
  Unsafe boundaries map to trust boundaries.

- **A2 — ARM64 target.** Generic timer, GIC, EL0/EL1. The codebase is
  structured for portability (`src/arch/`); architecture-specific details live
  behind trait interfaces and do not shape the design.

- **A3 — The kernel is generic.** No assumptions about the OS or workload.
  Personal devices, servers, embedded — all viable. Workload-specific policy
  belongs in userspace.

---

## Derivations

_Empty. Populated as the fresh derivation proceeds._

### Entry template

Each derivation entry names three things: what rests on what, how settled the
entry is, and where to find the reasoning. Format:

> **Name.** One-sentence statement of what was derived.
>
> - **Rests on:** the load-bearing predecessors — axiom labels (A1, A2, …),
>   prior derivation names, and any `design/research/` docs that directly
>   shaped the derivation. Only entries the reasoning *actually invokes*, not
>   every entry that might be related. Completeness is not the goal; honesty
>   is. If a predecessor moves, this entry must be revisited.
> - **Status:** `tentative` (accepted to enable downstream exploration, may
>   move), `settled` (reasoning reviewed, revisit only on explicit trigger),
>   or `settled — revisit when X` (settled now but with a named trigger to
>   reopen).
> - **Journal:** link to the numbered journal entry containing the full
>   reasoning. Spec entries state the *conclusion*; journals carry the
>   *argument*.

No confidence numbers. Numeric scores in this kind of work turn into vibes
within a session and then start being treated as load-bearing. Qualitative
language above is the substitute.

### Relationship to philosophy

`design/philosophy.md` is not in the axioms list and is not a predecessor
listed under "Rests on." Axioms are *what we derive from*; philosophy
provides *strategies for how to derive*. When a journal entry applies a
philosophy principle to make a derivation move, it should name that
principle ("applying 'push complexity to the leaves' here…") so the
principle's role is visible without collapsing it into the dependency graph.

### Template revisit

This template itself is tentative. After 3-5 entries have landed under it,
review whether the shape fits what actually needs to be captured. Adjust if
not.

---

## Open questions

_Tracked as the derivation exposes them._

---

## Journal index

_Empty. New numbered entries begin at `001-`._

---

## Research

See `design/research/` for descriptive prior-art studies and
`design/landscape.md` for the survey of how other kernels resolved each major
design decision.
