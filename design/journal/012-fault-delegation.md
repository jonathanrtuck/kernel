# 012 — Fault delegation to userspace pager Observers

**Date:** 2026-04-18 **Starting point:** D5 (MMU-backed virtual memory) deferred
fault delegation as "one level down." D6 listed the fault handler capability
field as tentative, noting it was from the archive and "not yet re-derived in
current chain." D9 noted "does not foreclose fault delegation via IPC, which is
a separate question." This exploration gates the IPC model — if faults are
delegated, pager traffic becomes a first-class IPC workload.

---

## The question

Page faults are a hardware fact under D5. The kernel must handle them. Three
models exist in the landscape:

1. **Kernel-internal** (QNX, Redox): the kernel resolves faults itself.
2. **Full delegation** (seL4, L4, Mach): faults are delivered to userspace pager
   threads/Observers via IPC.
3. **Self-paging** (Nemesis, Barrelfish): faults are reflected back to the
   faulting Observer itself.

A fourth candidate, **hybrid** (kernel resolves simple faults, delegates complex
ones), has partial precedent in Zircon but no principled microkernel
formulation.

---

## Self-paging is foreclosed

Self-paging pushes fault-handling complexity into every Observer. Under A5
(kernel absorbs complexity, presents a simple interface), this is an O4 (a)
violation: essential complexity moved to userspace. Every Observer would need to
be its own pager, even when its workload has nothing to do with memory
management. Foreclosed.

---

## A4 + A3 foreclose pure kernel-internal

Two axioms interact to structurally constrain kernel-internal resolution:

**A4 (purely reactive)** means no kernel thread exists. Background paging work —
page scanning, eviction candidate selection, prefetching, write-back batching —
cannot run inside the kernel. If the kernel handles faults internally, ALL
paging work must happen synchronously during exception handling. Eviction during
a fault means: the faulting Observer's exception handler must synchronously
select a victim, write it back (if dirty), reclaim the frame, and only then map
the faulted page. Fault latency becomes unbounded under memory pressure.

**A3 (generic kernel)** means paging policy varies by workload. Embedded systems
may pin all memory. Servers want application-aware eviction. Personal devices
want app-lifecycle-aware policies. Real-time systems need bounded fault latency.
A generic kernel cannot embed a single paging policy. If kernel-internal, the
kernel must either:

- Hardcode one policy → violates A3 (forecloses workloads that need different
  policies).
- Expose a policy-configuration interface → adds interface surface that A5
  resists. A policy-configuration framework (eviction strategy selection, page
  source configuration, prefetch hints) is likely larger than a simple fault
  notification protocol.

These are independent paths. A4 constrains the kernel's execution model; A3
constrains the kernel's policy space. Both point away from kernel-internal.

---

## Hybrid's boundary problem

The hybrid model (kernel resolves "simple" faults, delegates "complex" ones)
avoids IPC overhead for common cases. But the boundary between "simple" and
"complex" is itself a policy decision:

- An embedded system wants ALL faults to be "simple" (everything pre-committed).
- A server may want custom handling even for committed pages (to track access
  patterns for its own eviction decisions).
- The boundary is workload-dependent — which is the exact problem A3 exists to
  prevent.

Additionally, the hybrid model requires two code paths in the kernel (internal
resolution AND delegation dispatch), adding complexity that neither pure model
carries. No clean microkernel precedent exists for this as a principled design
(Zircon is partially hybrid but is a substantially larger kernel).

The hybrid is not foreclosed — it could be added later as a transparent
kernel-internal optimization if profiling shows that trivial faults (committed-
but-unmapped page in a known memory object) dominate and the IPC overhead is
measurable. The key insight is that this optimization requires no interface
changes: the kernel can short-circuit the pager roundtrip for cases where the
pager's response is predictable. Designing for delegation first preserves this
option; designing for hybrid first embeds policy.

---

## Three independent paths converge on delegation

Applying "when independent paths converge, trust the convergence" (philosophy):

1. **A4 path:** No kernel thread → no background paging → pager Observers with
   their own Time allocations are the only way to do background page management.
2. **A3 path:** Generic workload → no single policy → paging policy belongs in
   userspace where each workload implements its own.
3. **A5 path (net):** The fault dispatch interface (one notification protocol)
   is smaller than a policy-configuration interface. Policy complexity lives in
   pager Observers, which are leaf nodes. A5 says push complexity to leaves;
   pagers are leaves.

Each path arrives at delegation independently. The A4 path is about execution
model. The A3 path is about policy flexibility. The A5 path is about interface
size. They do not share premises.

---

## Archive convergence

The restart-1 chain chose full delegation with:

- Per-Context fault handler capability
- Fault handler chains (Context → handler → … → kernel as root)
- Badge alongside fault handler for multiplexing
- The kernel as root fault handler (bootstrap case)

The current derivation arrives at the same conclusion from re-derived axioms.
The archive's specific mechanism choices (chains, badges, root handler) are data
points for downstream questions, not settled in this entry.

---

## The decision

**The kernel delegates all page faults to userspace pager Observers.** The
kernel's role is fault dispatch: detect the fault, identify the faulting
Observer, deliver a fault notification to the designated pager Observer, and
resume the faulting Observer when the pager replies. The kernel does not contain
paging policy (eviction, prefetch, page source selection, write-back).

This is the dominant microkernel pattern (seL4, L4, Mach). It satisfies A3 (any
workload implements its own paging), A4 (pagers use their own Time for
background work), and A5 (small dispatch interface; policy in leaf-node pagers).

**Costs accepted:**

- IPC overhead on every fault (minimum: 1 IPC + 1 syscall roundtrip per fault,
  due to D9's kernel-managed memory objects).
- Every Observer needs a fault handler (or inherits one from its creator).
- Bootstrap requires a root-case mechanism (the kernel must handle faults for
  the initial Observer before any userspace pager exists).

**Nothing structural is foreclosed.** A kernel-internal fast path for trivial
faults can be added later as a transparent optimization without interface
changes — the decision is about where the interface sits, not about forbidding
optimization below it.

---

## A5 is not load-bearing in the usual direction

A5 ("kernel absorbs complexity") might seem to push toward kernel-internal fault
handling — "the kernel should handle faults so userspace doesn't have to." But
the derivation does not pass through A5 in that direction. A4 and A3 do the
structural work. A5's net contribution is positive for delegation: the dispatch
interface is smaller than a policy-configuration interface. A5 is load-bearing,
but in the direction of confirming delegation's interface economics, not driving
the choice. The choice is driven by A4 + A3.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Rust's type system will shape the fault
message representation and pager interface implementation, but does not push
toward or away from delegation. A1 becomes relevant one level down (fault
message types, pager trait).

**A2 (ARM64)** provides the hardware context (ESR_EL1, FAR_EL1, exception
vectors) but does not choose between models. All surveyed ARM64 kernels use one
of the three models; the hardware supports all.

---

## What remains open

These are downstream questions — one level below this decision, to be explored
separately.

- **Fault handler attachment.** Does the fault handler attach to the Observer or
  the address space (D10)? Per-Observer allows different fault policies for
  Observers sharing an address space; per-address-space avoids redundant
  handling. The archive chose per-Observer (per-Context).

- **Pager unavailability.** What happens when the pager is destroyed, blocked,
  or unresponsive? Fault handler chains (archive model) or double-fault-kills?

- **Root/bootstrap case.** The kernel must handle faults for the initial
  Observer. The archive used "kernel as root fault handler" — the one place the
  kernel does internal resolution.

- **Fault message contents.** What information is delivered? Fault address
  (FAR_EL1), fault type (ESR_EL1 syndrome), access type (read/write/execute),
  memory object reference, Observer identity.

- **Pager reply/resume mechanism.** How does the pager signal "fault resolved,
  resume"? Reply capability (seL4), exception channel operation (Zircon), or
  something shaped by this kernel's IPC model. Tightly coupled with IPC design.

- **D7 classification.** Is fault notification IPC (kernel-as-sender) or a
  dedicated mechanism? Shapes IPC design — the original motivating question.

- **Observer minimum schema.** D6 lists the fault handler capability field as
  tentative. This entry confirms the field is structurally required (every
  Observer must have a fault handler designation).

---

## Rejected alternatives (summary)

| Alternative            | Foreclosed by         | Reason                                               |
| ---------------------- | --------------------- | ---------------------------------------------------- |
| Self-paging            | A5 + O4 (a)           | Pushes fault complexity into every Observer          |
| Kernel-internal (pure) | A4 + A3               | No background paging; no single policy fits all      |
| Hybrid (designed-in)   | A3 (boundary problem) | Simple/complex boundary is workload-dependent policy |

Note: Hybrid as a transparent optimization (below the delegation interface) is
not rejected — only hybrid as a designed-in interface-level distinction.

---

## Audit note (2026-04-18)

Flagged by independent audit (HIGH severity): (1) potential mischaracterization
of archive conclusion, (2) derivation gap — A4 alone does not foreclose
synchronous inline resolution by the exception handler. Independent
re-derivation (archive physically removed from tree) clarified: A4 forecloses
background paging, not synchronous resolution. But A3 + A4 combined force
delegation at the interface level — A3 rejects a single hardcoded policy, and
the hybrid boundary is workload-dependent (A3). The kernel remains free to
optimize behind the interface (resolve trivial faults inline). Conclusion
stands; the "Kernel- internal rejected by A4 + A3" row in the table above should
be read as "A3 + A4 combined at the interface level," not "A4 alone forecloses
all inline resolution."
