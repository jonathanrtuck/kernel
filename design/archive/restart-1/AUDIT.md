# restart-1 — audit notes for convergence-checking

This file collects gotchas that help a reader use the archive correctly under
the current (post-2026-04-15) axiom/philosophy split. The archive is frozen;
this file is not a correction to it but a translation layer.

## Leaf-node reasoning: two scopes, one word

The archive treats "kernel is a leaf node" as a single concept — a direct
application of the philosophy principle "push complexity to the leaves." The
current `design/spec.md` separates that single concept into two:

1. **A5 (axiom).** The kernel is a leaf relative to userspace. Complexity
   belongs kernel-side of the kernel|userspace boundary, not pushed outward.
   Decides which side of that one boundary a concern sits on.

2. **Fractal philosophy principle.** Within the kernel's own tree,
   topology-specific or algorithm-specific complexity belongs behind an
   interface in an interior leaf, not in the kernel's skeleton. Same principle,
   different scope. Not an axiom; named inline in journals where it does work.

The archive uses "leaf node" in both scopes without distinguishing them. A
reader pulling archive reasoning into a current derivation should ask, for each
"leaf node" invocation, which scope the reasoning actually rests on:

- **External scope** (cite A5 in the new chain): archive `spec.md:50-52`;
  `journal/002-communication-flows.md:72-87` ("kernel is the leaf node"
  correction); `journal/011-reply-routing-and-fault-resume.md:54` (keeping
  reply-routing complexity kernel-side).
- **Internal scope** (cite the philosophy principle in the new chain, inline —
  not under "Rests on"): archive `spec.md:91, 118` (scheduling algorithm as
  swappable leaf inside); `journal/001:108`, `003:36/145`, `005:75`, `009:41`
  (components as leaves within the kernel).

The archive's reasoning is not wrong under its own framing. It just uses one
concept where the new chain uses two. Translate when consulting.

## Why the distinction matters

Axioms appear in "Rests on" lines; philosophy principles do not. A derivation
that cites A5 when the work is actually being done by the fractal internal
principle inflates A5's apparent weight and hides the real load-bearing
strategy. This exact trap was caught mid-chain in
`design/journal/003-space-manager-interface.md` and corrected there and in
`design/spec.md#D3`. The archive's derivations predate the distinction and will
read cleanly only after the translation above.

## Scope of this file

- It does **not** edit the archive.
- It does **not** invalidate archive reasoning.
- It does **not** replace convergence-checking; it supports it.
- It **does** call out the specific place archive vocabulary maps two-to-one
  onto current vocabulary, so a reader doesn't import the conflation back into
  the live chain.

Add further entries below as future audits find other archive framings that need
similar translation.
