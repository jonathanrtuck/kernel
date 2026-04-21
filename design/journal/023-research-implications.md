# Journal 023 — Research implications: bleeding-edge OS landscape (2022–2026)

## Starting point

The research document `design/research/bleeding-edge-os-landscape.md` surveys
2022–2026 OS research across verified development, Rust as architectural
boundary, capability innovations, IPC mechanisms, async-first design, static
architectures, persistent systems, compartmentalization, zero-copy patterns,
hardware-software co-design, and WASM execution substrates.

This entry evaluates which concepts fit the kernel's settled decisions (D1–D22),
axioms (A1–A5), and philosophy — and which explicitly don't. The purpose is to
identify course adjustments worth making now (before they become expensive to
retrofit) and conceptual tools that should inform future derivation without
being imported as decisions.

This is not a derivation entry. No D-number is assigned. The analysis informs
future derivations and architectural discipline.

---

## Structural fits: concepts that reinforce settled decisions

### Framekernel pattern (Asterinas) → A1 trust boundaries

Asterinas confines all `unsafe` Rust to a core library (OSTD, ~14% of codebase).
The remaining 86% — all kernel services — is safe Rust against OSTD's API. The
result: 14% TCB vs. Theseus's 62% vs. Tock's 44%.

A1 states "unsafe boundaries map to trust boundaries." The kernel is already
building in this direction: D8 (kernel-managed flat table), D9 (kernel-managed
memory objects), D12 (fault dispatch), D21 (fault handler in cap table) all push
unsafe hardware interaction behind safe interfaces. The framekernel insight is
to make this an **explicit architectural boundary** — a named core module that
contains all unsafe, with every other kernel component written in safe Rust
against that core's API.

This is an organizational principle, not a feature. It doesn't change any
derivation. It makes A1's trust-boundary promise auditable and enables
verification (see below).

### Verification readiness (Atmosphere, TickTock, Verus, Flux)

Atmosphere (SOSP 2025) proved a full L4-style Rust microkernel with Verus at
7.5:1 proof-to-code ratio in ~2 person-years. TickTock (SOSP 2025) used Flux
refinement types on Tock and found 7 previously unknown isolation bugs,
including paths to full OS compromise — in a kernel already using Rust's type
system for isolation.

The implication: if the unsafe core is confined (framekernel pattern), it is
small enough to eventually verify with Verus. The safe kernel code gets
type-system guarantees for free. The cost of structuring for verifiability is
near-zero now; the cost of retrofitting later is a rewrite.

No ARM64 verified microkernel exists yet (Atmosphere is x86-64 only). This
kernel's public-domain stance and educational goals make verification readiness
a natural fit for all three audiences.

**Not proposed:** adopting Verus or Flux now. Only: structuring code so that
adopting them later is a leaf-node addition, not an architectural change.

### Ownership-transfer IPC (PLOS 2023) → D13 + A1 + D9

Academic work extending Rust's ownership transfer semantics across process
boundaries. When a message is sent, ownership of the backing memory transfers to
the receiver — the type system enforces that the sender can't access it after
send. Zero-copy falls out as a type-system invariant.

D13 settled queued fields with direct-switch fast path. D9 settled variable-size
kernel-managed memory objects. D8 settled per-Observer flat cap tables.
Capability transfer already moves designation; extending it to move the memory
object's backing is a natural extension. The D8 flat table already tracks
per-Observer ownership; moving an entry from one table to another maps directly.

**Timing pressure:** the message format is still open. If settled without
considering ownership transfer, retrofitting is expensive. This concept should
be evaluated as part of the message format derivation.

---

## Conceptual tools: not implementations, but frames for future derivation

### Capability graph as complete system state (TreeSLS)

TreeSLS (SOSP 2023 Best Paper) uses the capability tree as the single structure
governing all system state. Persist the tree and you've persisted the system.

The kernel is not building persistence. But the conceptual discipline is
powerful: D4 + D8 + D9 + D10 + D14 + D13 already make capabilities the sole
designation mechanism for Observers, memory objects, address spaces, and fields.
If this discipline is maintained — every kernel object reachable only through
the capability graph — structural properties follow:

- Debugging: walk the graph to see complete system state.
- Leak detection: anything unreachable through capabilities is reclaimable.
- Migration: transfer a subgraph and you've transferred a workload.
- Auditing: the graph is a complete description of who can do what to whom.

D21's choice to put the fault handler in the cap table rather than a struct
field is an example of this discipline already in action. The principle: don't
create kernel-internal references that bypass the capability graph.

### Time-as-capability (seL4 MCS, S3K)

seL4's MCS model and S3K both make scheduling time a transferable capability. An
Observer donates execution time to another via IPC.

The kernel's Time vocabulary already describes Time as "a claim to a portion of
a specific logical core's scheduling time." Two open questions touch this:

- **Time migration across cores:** if Time is a capability, migration is a
  transfer (close on source core, create on destination).
- **Time reclamation on Observer destroy:** if Time is a capability, the answer
  follows from D11's base revocation — close returns the capability to the
  holder that delegated it.

Making Time a capability-designated kernel object (joining Space, Observer,
Coordinate System, field) would be consistent with D4's architecture. Not
settled here — but noted as a frame that may dissolve several open questions
simultaneously.

---

## Validation: research confirming existing decisions

### io_uring security lesson → D13 + D18

Google disabled io_uring on Android, ChromeOS, and internal servers after 60% of
their 2022 bug bounty exploits targeted it. The attack surface of a
general-purpose async interface is large.

D13's fixed-capacity bounded queues, D18's error-to-sender (no per-field policy
modes, no kernel-level coalescing), and D13's rejection of the SQ/CQ pattern are
all structurally simpler than io_uring. The research confirms the instinct:
simplicity in the IPC mechanism reduces attack surface.

### Type-system isolation needs hardware backup → D5

TickTock found 7 isolation bugs in Tock's type-system-based capsule isolation.
Theseus (type-only, no hardware) trusts the compiler absolutely — a compiler bug
or unsound unsafe block breaks all isolation.

D5's requirement for MMU-backed hardware isolation is validated. The emerging
research consensus matches: type-system isolation is valuable alongside hardware
isolation, not as a replacement. The framekernel pattern (Asterinas-style)
implements this: safe Rust for structural isolation, MMU for enforcement.

---

## Explicit non-fits (and why)

These concepts were evaluated and rejected as incompatible with settled
decisions. Recording them prevents re-discovery and re-evaluation.

- **Theseus type-only isolation:** contradicts D5 (MMU required). TickTock's
  7-bug discovery confirms type-system isolation alone is insufficient.
- **MnemOS kernel-as-async-executor:** contradicts A4 (purely reactive, no event
  loop, no kernel thread).
- **Hubris fully static architecture:** contradicts A3 (generic kernel, no
  workload assumptions). But Hubris's 2,000 LOC validates the microkernel size
  target.
- **io_uring SQ/CQ pattern:** validated as an anti-pattern by Google's security
  experience. D13 + D18 are structurally simpler.
- **WASM userspace (k23):** A3-compatible as a userspace concern, not a kernel
  concern. Noted for eventual non-POSIX OS layer.
- **NrOS per-NUMA-node replication:** potentially relevant to D3's Space manager
  implementation (a leaf-node concern), not to the kernel's architecture. D1's
  per-core hot path already captures the structural benefit.
- **Declarative OS interfaces:** fundamentally different interaction model.
  Interesting for the non-POSIX OS layer, not for the kernel's syscall surface.

---

## Status

No derivation settled. Implications recorded:

1. **Framekernel discipline** — confine all unsafe to a named core module.
   Organizational principle flowing from A1. Enables verification readiness.
   Recommended as an explicit architectural discipline during implementation.
2. **Ownership-transfer IPC** — evaluate during message format derivation.
   Timing-sensitive: settling message format without considering ownership
   transfer forecloses the zero-copy invariant.
3. **Capability graph completeness** — maintain the discipline that every kernel
   object is reachable only through the capability graph. Already in action
   (D21); should be maintained consciously.
4. **Time-as-capability frame** — evaluate when deriving Time migration and
   reclamation. May dissolve multiple open questions simultaneously.
5. **Spec.md annotations** — open questions for message format, Time migration,
   and Time reclamation annotated with research connections.
