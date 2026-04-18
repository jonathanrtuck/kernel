# 006 — An Observer is a single schedulable execution unit

**Question:** What is the concrete schedulable execution unit in this kernel?
Thread, process, actor, capability-holder, or something else?

**Answer:** An Observer is a single schedulable execution unit — one register
state, one program counter, one Time, one capability table, one address space
binding. "Process" is not a kernel concept; it is a userspace convention (a
group of Observers sharing a Space). The kernel provides no Observer-grouping
mechanism.

---

## Prior work

The archive (restart-1) used "Context" where the current chain uses "Observer."
The archive's Context was structurally equivalent to what this entry derives:

- archive/journal/004: Context model schema — register state, TTBR,
  runnable/blocked, fault handler capability, pending message state. Single
  schedulable entity. No kernel process concept.
- archive/journal/007: "The scheduler picks a Time, not a Context." Time
  capabilities carry scheduling properties; the scheduler is a Time allocator.
- archive/journal/013: Context became a capability-held object type.

The current chain had not re-derived this. D1–D5 tell us what an Observer _has_
(address space, capability table, scheduling properties, Time) but not what it
_is_.

---

## Derivation

### Part 1: Observer = single schedulable entity (derived, not chosen)

Two constraint paths converge:

**Path 1: Vocabulary + D2.** The vocabulary commits: "An Observer correlates one
or more Spaces but exactly one Time." Time is a scheduling allocation — a
fraction of a logical core's scheduling time. D2 says "the scheduler that
selects which Observer resumes on a core is per-core." Combining these:

- One Time = one scheduling allocation = one schedulable entity
- The scheduler selects Observers (D2)
- Therefore an Observer is a single schedulable entity

An Observer with multiple independent execution points would need multiple
Times, which the vocabulary explicitly forecloses: "not as a single Observer
with multiple Times."

**Path 2: Vocabulary SMT paragraph.** "SMT-concurrent workloads, when hardware
supports them, are expressed as multiple Observers sharing a Space, each with
its own Time on its own logical core." The vocabulary was designed for this:
multiple execution units in shared memory are multiple Observers, not one
Observer with internal concurrency.

### Concrete Observer state

D1–D5 together define what the kernel saves and restores per Observer:

- General-purpose registers (x0-x30, SP, PC, PSTATE)
- Floating-point/SIMD registers (v0-v31, FPCR, FPSR)
- TTBR0_EL1 value — address space root (D5)
- Capability table pointer (D4)
- Scheduling state (runnable, blocked, etc.)
- Time binding (which Time allocation this Observer holds)
- Fault handler capability (archive/journal/004 derivation, not yet re-derived
  in current chain but structurally sound)

This is structurally equivalent to seL4's TCB: a fully-specified execution
context with bindings to VSpace and CSpace. The Observer doesn't map cleanly to
"thread" or "process" — it has thread-like properties (single execution point,
schedulable) and process-like properties (own capability table, own address
space binding).

### What traditional concepts map to

- **Multi-threaded process:** multiple Observers sharing a Space
- **Single-threaded process:** one Observer, one Space
- **Actor:** one Observer, one Space, communication via IPC
- **Event loop:** one Observer doing cooperative multiplexing internally
  (userspace, invisible to kernel)
- **Green threads / coroutines:** cooperative scheduling within an Observer's
  Time allocation (userspace, invisible to kernel)

A3 (generic) is satisfied: no paradigm is assumed or foreclosed.

### Part 2: No kernel-level grouping mechanism (settled by judgment)

The derived part (Observer = execution unit) leaves one choice: does the kernel
provide a grouping mechanism ("process" as a kernel concept)?

**The A5 test:** Is process-level grouping essential complexity that the kernel
must absorb, or policy that belongs in userspace?

The test reduces to: can an Observer always be stopped by an entity holding the
right capabilities, without the Observer's cooperation? Under D4, yes.
Capabilities are unforgeable and kernel-resolved. A destroy capability for an
Observer works regardless of the Observer's state. Forceful termination requires
only the right capability, not the target's cooperation.

Therefore:

- "Kill group" = hold destroy capabilities for N Observers, iterate and destroy.
  Under A4 (reactive), sequential destroys within a single kernel invocation
  leave no window for remaining Observers to execute — they only run when the
  kernel resumes them, which it won't if it's destroying them.
- "Account group resources" = sum the Observers you created. The capability
  graph IS the group structure.
- "Limit group resources" = control what resources each Observer receives at
  creation.

Grouping is neither essential complexity (D4 capabilities handle lifecycle) nor
workload-universal (A3 — not all workloads need groups). The kernel provides
mechanism (Observers, capabilities, Spaces); userspace provides policy (how to
group, what groups mean).

**Landscape check:** seL4 validates the no-kernel-process approach. "Process" is
a userspace convention in seL4 (TCB + VSpace + CSpace wired together). CAmkES,
sel4test, and all seL4-based systems build process abstractions without kernel
support. Zircon added processes as kernel objects for ergonomic reasons, not
because the bare model was insufficient — Zircon targets a specific consumer OS
workload (tension with A3's genericity).

---

## Costs

- **Observer creation weight.** Every preemptive "thread" is a full Observer
  with its own capability table binding, scheduling state, and Time allocation.
  Heavier than POSIX pthread_create. seL4 TCB creation (the closest analogue) is
  not heavy in practice — it is a capability retype operation. Green threads and
  coroutines remain free (userspace, no kernel involvement).

- **Shared authority requires explicit setup.** Per-Observer capability tables
  (D4) mean Observers sharing a Space have separate authority by default. The
  traditional "all threads share a handle table" model requires either: (a)
  explicit capability sharing between Observers, or (b) shared capability table
  structures (whether this is possible is an open question about capability
  table structure). This is a different default than programmers expect — safer
  (confused deputy protection at the execution-unit level) but less ergonomic
  for the common multi-threaded case.

- **No kernel "kill group" or process-level accounting.** Userspace must track
  and manage Observer groups. The friction is low (capability graph provides the
  structure), but it is friction that Zircon-style kernels absorb.

---

## What this does NOT settle

- **Observer minimum schema.** The concrete fields are outlined above but not
  formally derived in the current chain. In particular, the fault handler
  capability comes from the archive (journal/004) and should be re-examined.

- **Observer-Space binding model.** When and how an Observer binds to a Space.
  At creation only, or rebindable? Interacts with Observer-Space cardinality.

- **Observer lifecycle.** Create, destroy, suspend, resume. Whether Observer is
  a capability-held object type (archive journal/013 said yes). Interacts with
  D4 and the open question about scope of capability mediation.

- **Can Observers share capability tables?** D4 requires per-Observer capability
  tables, but whether the underlying table structure can be shared (like seL4
  TCBs sharing a CSpace) is open. Determines multi-threading ergonomics.

- **Capability table structure.** Now more urgent: it determines how Observers
  in a shared-Space group interact with authority.

---

## Rejected alternatives

| Alternative                              | Foreclosed by | Reason                                                                               |
| ---------------------------------------- | ------------- | ------------------------------------------------------------------------------------ |
| Observer-as-process (containing threads) | Vocabulary    | One Time per Observer; multiple schedulable entities need multiple Observers         |
| Multi-Time Observers                     | Vocabulary    | Explicitly forbidden ("not as a single Observer with multiple Times")                |
| Sub-Observer schedulable units           | D2            | Scheduler selects Observers, not sub-Observer entities                               |
| Kernel-level process/grouping            | A3 + judgment | Not essential (D4 handles lifecycle), not universal (A3), low friction for userspace |

---

## Axioms not load-bearing here

A1 (Rust) is not load-bearing. The execution unit model is language-independent.
Rust's type system will be relevant when implementing Observer structures, but
the derivation does not pass through A1.

A2 (ARM64) is not directly load-bearing for the top-level question. It defines
the register file (what "register state" contains concretely) but does not push
toward or away from any execution unit model. The derivation rests on
vocabulary, D2, D4, and A3.

A5 is load-bearing for Part 2 (no grouping) but not for Part 1 (Observer =
execution unit). Part 1 is forced by vocabulary + D2 alone.

---

## Audit note (2026-04-18)

Flagged by independent audit: vocabulary "one Time per Observer" may be
inherited from archive; diagnostic pattern mirrors archive/004. Independent
re-derivation (archive physically removed from tree) confirmed the conclusion is
axiom-forced: A3 + A5 reject kernel grouping, D2 schedules Observers directly,
D4 + D14 enable userspace grouping, D10 is the shared-memory anchor. Conclusion
stands; structural similarity to the archive's reasoning is noted.
