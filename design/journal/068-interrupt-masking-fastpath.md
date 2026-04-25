# Journal 068 — Interrupt Masking During IPC Fast Path

Settles G05. The IPC fast path masks all interrupts (DAIF I-bit) for its
~400-cycle window. Five independent convergences from the design graph, plus
unanimous prior-art agreement across every surveyed microkernel.

## Context

D50 settled six IPC fast-path conditions and explicitly deferred interrupt
masking as an open question: "whether interrupts are masked for the ~400-cycle
fast-path window. Trades worst-case interrupt latency for scheduling-check
consistency. Implementation concern within the ~400-cycle budget."

The `.brain/explorations/G05-interrupt-masking-fastpath/` exploration evaluated
four options: (A) mask all via DAIF, (B) don't mask, (C) priority-based masking
via GICv3 ICC_PMR_EL1, (D) restartable fast path. The question decomposes along
two axes: fast-path correctness complexity and interrupt responsiveness.

## Why masking wins — five convergences

**1. D50 TOCTOU elimination.** The scheduler callback (`should_switch_to`) is
evaluated before the context switch. Without masking, an interrupt can fire
between the callback returning "true" and the TTBR0 switch completing. The
interrupt handler may wake a higher-priority Observer, making the callback's
answer stale. The fast path would complete a context switch the scheduler would
not re-approve. Under masking, this window is structurally closed — no interrupt
can invalidate the scheduler's decision mid-fast-path.

**2. Journal 023 Verus readiness.** A non-preemptible fast path is orders of
magnitude easier to specify and verify than a preemptible or restartable one.
The specification of a masked section requires: "for all states reachable by
executing instructions i_1 through i_N sequentially, invariant X holds." The
specification of a preemptible section requires: "for all states reachable by
executing any prefix of i_1 through i_N, followed by an arbitrary interrupt
handler, followed by any suffix, invariant X holds." The second is
combinatorially harder. The framekernel pattern (journal 023) depends on
precisely bounded unsafe sections — a non-preemptible fast path is a clean
trust-boundary section.

**3. A4 (purely reactive) alignment.** The fast path is a single exception
handler execution (SVC from EL0). A4 means no background threads, no kernel
preemption infrastructure. Allowing nested exceptions (EL1h IRQ during SVC)
introduces nesting complexity: the TrapFrame stack must accommodate nesting, the
IRQ handler must handle "called during fast path" as a distinct case, and the
current serial lock in `irq_handler()` would deadlock if the IRQ preempts a
`println!`. Masking avoids all of this — the fast path is a single non-nested
execution.

**4. D1 (per-core hot path) simplicity.** The fast path touches no cross-core
shared state. Masking keeps it a straight-line section with no branches for
interrupt nesting. The hot path stays minimal: `msr daifset, #2` at entry,
`msr daifclr, #2` at exit, ~2–8 cycles total overhead.

**5. Blackham et al. quantitative grounding.** Blackham et al. (EuroSys 2012)
measured non-preemptible seL4's worst-case interrupt latency at 10k–100k cycles
on Cortex-A9. The fast path's ~400-cycle masking window is 0.4–4% of the total
worst-case. The fast path is not where interrupt latency is primarily spent —
the longer kernel paths (destroy cascade, capability operations) dominate. D33's
preemptible destroy cascade already addresses the dominant contributor to
worst-case latency.

## Prior art — unanimous convergence

Every surveyed microkernel with an IPC fast path masks interrupts during the
equivalent window:

- **seL4 (classic and MCS):** Non-preemptible fast path. MCS adds preemption
  points to long kernel paths (CDT traversal), not to the fast path.
- **L4Ka::Pistachio:** Masked. Fast path is an atomic straight-line section per
  Liedtke's 1993 principles.
- **Fiasco.OC:** Masked. Priority check done before masked section; the fast
  path itself runs non-preemptible.
- **EROS/Coyotos:** Non-preemptible invocation path. Shapiro (SOSP 1999): "EROS
  uses a non-preemptible invocation path for correctness."
- **Barrelfish:** Non-preemptible dispatcher loop per core.

No surveyed microkernel has implemented a non-masked IPC fast path. The
convergence reflects 30+ years of experience that preemptible fast paths create
correctness problems disproportionate to their interrupt-latency benefit.

## Options rejected

**Option B (don't mask):** Creates D50 TOCTOU, requires nested exception
handling, dramatically increases verification complexity, has no prior art. The
interrupt latency benefit (avoiding ~200 ns delay) is negligible against the
total worst-case latency budget.

**Option C (ICC_PMR_EL1 priority masking):** Requires settling D22's deferred
interrupt priority exposure question. Creates partial TOCTOU (for high-priority
interrupts that pass the threshold). ICC_PMR_EL1 EL1-writability may be
constrained by EL3/TrustZone on some platforms. Adds a second masking mechanism
alongside DAIF. No microkernel prior art. Journal 066 settled interrupt priority
as flat — adding priority-based masking here would contradict that settlement.

**Option D (restartable fast path):** Extreme implementation complexity. Every
fast-path state mutation must be idempotent or checkpointed. The scheduler
callback must be safe to call multiple times. The IRQ handler must inspect
ELR_EL1 on every EL1 IRQ. Potential starvation under high interrupt + high IPC
load. No prior art in any surveyed kernel. ARM64 has no hardware restartable
sequence primitive.

## Sub-choice: DAIF.I only, not DAIF.IF

Mask only the I-bit (IRQ), not the F-bit (FIQ). At EL1 Non-Secure, FIQ routes to
EL3 (TrustZone Secure Monitor) via GICv3 Group 0. The kernel does not handle FIQ
— it is not ours to mask. `msr daifset, #2` (I-bit only) is correct.
`msr daifset, #6` (I+F) would be a no-op for F in most configurations but is
semantically wrong — we should not mask exceptions we do not own.

## D42 tension — accepted

High-R, high-P Observers experience up to ~400 cycles (~200 ns at 2 GHz) added
interrupt delivery latency per concurrent IPC fast-path invocation on their
core. This is a real floor on interrupt responsiveness imposed by the kernel.

Accepted because:

- The floor is bounded and deterministic (~400 cycles, not variable).
- It is 0.4–4% of measured total worst-case interrupt latency (Blackham et al.).
- Millisecond-scale RT deadlines (the common case) have multi-thousand-cycle
  jitter tolerance; 400 cycles is within tolerance.
- Microsecond-scale ultra-low-latency RT is served by dedicated cores (D2) where
  IPC fast-path frequency is low (the RT Observer is the receiver, not a busy
  server — interrupt delivery on its core is infrequent during fast paths).
- The alternative (non-masked) creates correctness complexity that undermines
  the verification goal that benefits all workloads, not just high-R Observers.

## Status

Settled. Closes G05. The IPC fast path masks IRQ via DAIF.I for its ~400-cycle
window. DAIF.I only (not F-bit). Revisit if: (a) a concrete workload is
identified where 200 ns interrupt latency floor is unacceptable AND the workload
cannot be served by dedicated-core isolation (D2), or (b) a formally verified
restartable fast path is demonstrated in any kernel, changing the complexity
calculus.
