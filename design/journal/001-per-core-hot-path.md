# Per-Core Hot Path, Shared Cold Path — 2026-04-15

First derivation entry after the 2026-04-15 reset. Records the reasoning behind
`spec.md#D1`.

## Starting point

The reset left axioms A1–A5 in place and the Derivations section empty. The
first question to close: how does the kernel operate across multiple cores?

This question was also the only major open item at the end of the archived
restart-1 chain (see `design/archive/restart-1/journal/005-smp.md`). That
chain's reasoning is available as convergence-check data but is not imported
here — the current entry reasons from A1–A5 and the observations, not from
archived conclusions.

## Exploration

### The spectrum

Four positions exist in the synchronization-model space
(`design/research/smp.md` §1):

1. **Big kernel lock (BKL).** One core in the kernel at a time.
2. **Fine-grained locking.** Multiple locks; cores serialize per-resource.
3. **Hybrid: per-core hot path, shared cold path.** High-frequency work is
   per-core without shared state; infrequent cross-core work uses shared
   structures (lock-free or locked as appropriate).
4. **Full multikernel (Barrelfish).** N independent kernel instances, no shared
   kernel state; coordination via explicit message-passing, often delegated to
   userspace monitors.

Each has a known cost profile on ARM64. Research cites ~23% overhead from BKL's
barrier instructions alone on ARM — before contention. Fine-grained locking on
ARM is worse (>70% in some measurements) because every locked operation pays
barrier cost. The hybrid avoids locks on the hot path entirely; the multikernel
avoids them structurally but pays a coordination cost elsewhere.

### Rejecting BKL

BKL fails on two grounds. First, the ARM barrier overhead is prohibitive for a
kernel where context-switch frequency is high. Second — and more important — BKL
violates A4's spirit when paired with multi-core execution: a single global lock
forces serialization through a structure that has no reason to be global. The
hot path for an exception on core 0 has no semantic dependency on what core 1 is
doing; BKL manufactures a dependency for implementation convenience.

seL4's acceptance of BKL is understandable in its context (formal verification
of concurrent code is much harder than sequential), but we are not verifying the
kernel formally, so that tradeoff doesn't apply.

### Rejecting full multikernel

Barrelfish is philosophically close to this project in several ways — the "CPU
driver" is small, message-passing is the primary coordination mechanism, and
per-core execution avoids shared-state synchronization. The convergence is not
accidental.

But the multikernel's defining choice — pushing cross-core coordination _into
userspace_ via monitor processes that run distributed consensus — is exactly
what A5 forbids. "Kernel is a leaf node" means the kernel absorbs complexity
rather than exposing primitives that force userspace to solve
distributed-systems problems. Capability revocation across cores, in a
multikernel, becomes a distributed-consensus problem userspace has to handle.
That is a large, subtle piece of complexity on the wrong side of the interface.

The Lozi et al. 2016 paper ("The Linux Scheduler: A Decade of Wasted Cores")
provides strong evidence that _avoiding_ cross-core coordination entirely is
worse than coordinating cheaply: cores sitting idle while others are overloaded
cost 13–23% throughput in typical workloads, with pathological cases showing up
to 138× degradation. On our target hardware (cache-coherent ARM SoCs, 4–8+
cores), the cost of sending an IPI is ~1–2µs; the cost of failing to rebalance
is larger. This argues against the multikernel's "no cross-core coordination at
all" stance.

### Where we land

The hybrid model: the hot path is per-core and touches no cross-core shared
state; infrequent cross-core work routes through an explicitly shared cold path.

Hot path (per-core, no shared state):

- Exception entry.
- Frame state update.
- Scheduler pick.
- Resumption.

Cold path (shared, infrequent):

- Frame migration between cores.
- Cross-core message delivery.
- Shared resource allocation (the Space manager interface, D3).
- Capability operations that cross cores.

A4 does significant load-bearing work here: without a kernel thread, there is no
entity that could poll a mailbox on another core. The only way to wake another
core's kernel is an IPI (O2). That in turn makes per-core handlers the only
viable design — each core handles its own exceptions, including IPIs from other
cores.

A5 rules out the multikernel because the multikernel's price for no shared state
is userspace-side coordination complexity. The hybrid keeps that complexity
kernel-side, where A5 says it belongs.

A2 (cache-coherent ARM64) makes the cold path cheap. Reads from shared
structures that are rarely written stay in cache-line Shared state across cores;
reads are effectively free as long as no other core is writing. This is
precisely the access pattern of cold-path data (rarely written, read on demand).

### SMT

On SMT hardware (not the current A2 target, but possible future hardware),
"core" in this derivation means logical core. Each logical core has its own Core
manager, own exception state, own targetable IPI line. Hardware interleaves
sibling logical cores' instructions at the pipeline level; the kernel's
scheduling is unaffected by this.

Cross-sibling concerns (trust group placement, power coordination) are handled
via read-mostly shared state within a physical core — not by introducing a
shared per-physical-core scheduler. A shared scheduler would reintroduce
hot-path contention, defeating the purpose of the per-core structure.

### Convergence with the archived chain

The archived restart-1 chain (journal 005) landed at the same hybrid model from
a different starting point. The archive framed the decision around a specific
regime ("small cache-coherent SoC"). This entry frames it around A4 + A5 + O2
directly, which is load-bearing across A2's full scope rather than a subset. The
conclusion converges; the reasoning differs in what it rests on.

## Status

**Accepted as `spec.md#D1` — tentative.**

Tentative because actual implementation will surface issues the spec derivation
cannot predict: mailbox layout, IPI latency under contention, cache-line
bouncing patterns on the shared cold-path structures. The decision is stable
enough to derive further entries on top of, and should be revisited only if
implementation reveals that the hot/cold split leaks (e.g., a cold-path
operation turns out to be frequent enough to contend the shared read-set).

**Open sub-questions:**

- Concrete mailbox shape for cross-core requests (size, placement, ABI).
- Migration protocol detail: how a Frame moves between cores' state without
  losing in-flight messages.
- IPI semantics: synchronous-response (target ACKs before sender proceeds) vs.
  fire-and-forget. Different answers for different operation types.
- Whether any cold-path operation proves frequent enough to require promotion to
  per-core (with shared coordination as the exception).
