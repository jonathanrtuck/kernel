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

- **Rust (nightly, no_std).** Not a language preference — a design input.
  Ownership maps to resource lifecycle. Traits map to architecture abstraction.
  Unsafe boundaries map to trust boundaries.

- **ARM64 target.** Generic timer, GIC, EL0/EL1. The codebase is structured for
  portability (`src/arch/`); architecture-specific details live behind trait
  interfaces and do not shape the design.

- **The kernel is generic.** No assumptions about the OS or workload. Personal
  devices, servers, embedded — all viable. Workload-specific policy belongs in
  userspace.

---

## Derivations

_Empty. Populated as the fresh derivation proceeds. Each entry here must be
justified by journal entries in `design/journal/` that derive it from the
axioms above or from previously-settled entries in this file._

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
