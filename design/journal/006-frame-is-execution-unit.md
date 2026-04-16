# 006 — A Frame is a single schedulable execution unit

**Question:** What is the concrete execution unit inside a Frame? Thread,
process, actor, capability-holder, or something else?

**Answer:** A Frame is a single schedulable execution unit — one register state,
one program counter, one Time, one capability table, one address space binding.
"Process" is not a kernel concept; it is a userspace convention (a group of
Frames sharing a Space). The kernel provides no Frame-grouping mechanism.

---

## Prior work

The archive (restart-1) used "Context" where the current chain uses "Frame." The
archive's Context was structurally equivalent to what this entry derives:

- archive/journal/004: Context model schema — register state, TTBR,
  runnable/blocked, fault handler capability, pending message state. Single
  schedulable entity. No kernel process concept.
- archive/journal/007: "The scheduler picks a Time, not a Context." Time
  capabilities carry scheduling properties; the scheduler is a Time allocator.
- archive/journal/013: Context became a capability-held object type.

The current chain had not re-derived this. D1–D5 tell us what a Frame _has_
(address space, capability table, scheduling properties, Time) but not what it
_is_.

---

## Derivation

### Part 1: Frame = single schedulable entity (derived, not chosen)

Two constraint paths converge:

**Path 1: Vocabulary + D2.** The vocabulary commits: "A Frame correlates one or
more Spaces but exactly one Time." Time is a scheduling allocation — a fraction
of a logical core's scheduling time. D2 says "the scheduler that selects which
Frame resumes on a core is per-core." Combining these:

- One Time = one scheduling allocation = one schedulable entity
- The scheduler selects Frames (D2)
- Therefore a Frame is a single schedulable entity

A Frame with multiple independent execution points would need multiple Times,
which the vocabulary explicitly forecloses: "not as a single Frame with multiple
Times."

**Path 2: Vocabulary SMT paragraph.** "SMT-concurrent workloads, when hardware
supports them, are expressed as multiple Frames sharing a Space, each with its
own Time on its own logical core." The vocabulary was designed for this:
multiple execution units in shared memory are multiple Frames, not one Frame
with internal concurrency.

### Concrete Frame state

D1–D5 together define what the kernel saves and restores per Frame:

- General-purpose registers (x0-x30, SP, PC, PSTATE)
- Floating-point/SIMD registers (v0-v31, FPCR, FPSR)
- TTBR0_EL1 value — address space root (D5)
- Capability table pointer (D4)
- Scheduling state (runnable, blocked, etc.)
- Time binding (which Time allocation this Frame holds)
- Fault handler capability (archive/journal/004 derivation, not yet re-derived
  in current chain but structurally sound)

This is structurally equivalent to seL4's TCB: a fully-specified execution
context with bindings to VSpace and CSpace. The Frame doesn't map cleanly to
"thread" or "process" — it has thread-like properties (single execution point,
schedulable) and process-like properties (own capability table, own address
space binding).

### What traditional concepts map to

- **Multi-threaded process:** multiple Frames sharing a Space
- **Single-threaded process:** one Frame, one Space
- **Actor:** one Frame, one Space, communication via IPC
- **Event loop:** one Frame doing cooperative multiplexing internally
  (userspace, invisible to kernel)
- **Green threads / coroutines:** cooperative scheduling within a Frame's Time
  allocation (userspace, invisible to kernel)

A3 (generic) is satisfied: no paradigm is assumed or foreclosed.

### Part 2: No kernel-level grouping mechanism (settled by judgment)

The derived part (Frame = execution unit) leaves one choice: does the kernel
provide a grouping mechanism ("process" as a kernel concept)?

**The A5 test:** Is process-level grouping essential complexity that the kernel
must absorb, or policy that belongs in userspace?

The test reduces to: can a Frame always be stopped by an entity holding the
right capabilities, without the Frame's cooperation? Under D4, yes. Capabilities
are unforgeable and kernel-resolved. A destroy capability for a Frame works
regardless of the Frame's state. Forceful termination requires only the right
capability, not the target's cooperation.

Therefore:

- "Kill group" = hold destroy capabilities for N Frames, iterate and destroy.
  Under A4 (reactive), sequential destroys within a single kernel invocation
  leave no window for remaining Frames to execute — they only run when the
  kernel resumes them, which it won't if it's destroying them.
- "Account group resources" = sum the Frames you created. The capability graph
  IS the group structure.
- "Limit group resources" = control what resources each Frame receives at
  creation.

Grouping is neither essential complexity (D4 capabilities handle lifecycle) nor
workload-universal (A3 — not all workloads need groups). The kernel provides
mechanism (Frames, capabilities, Spaces); userspace provides policy (how to
group, what groups mean).

**Landscape check:** seL4 validates the no-kernel-process approach. "Process" is
a userspace convention in seL4 (TCB + VSpace + CSpace wired together). CAmkES,
sel4test, and all seL4-based systems build process abstractions without kernel
support. Zircon added processes as kernel objects for ergonomic reasons, not
because the bare model was insufficient — Zircon targets a specific consumer OS
workload (tension with A3's genericity).

---

## Costs

- **Frame creation weight.** Every preemptive "thread" is a full Frame with its
  own capability table binding, scheduling state, and Time allocation. Heavier
  than POSIX pthread_create. seL4 TCB creation (the closest analogue) is not
  heavy in practice — it is a capability retype operation. Green threads and
  coroutines remain free (userspace, no kernel involvement).

- **Shared authority requires explicit setup.** Per-Frame capability tables (D4)
  mean Frames sharing a Space have separate authority by default. The
  traditional "all threads share a handle table" model requires either: (a)
  explicit capability sharing between Frames, or (b) shared capability table
  structures (whether this is possible is an open question about capability
  table structure). This is a different default than programmers expect — safer
  (confused deputy protection at the execution-unit level) but less ergonomic
  for the common multi-threaded case.

- **No kernel "kill group" or process-level accounting.** Userspace must track
  and manage Frame groups. The friction is low (capability graph provides the
  structure), but it is friction that Zircon-style kernels absorb.

---

## What this does NOT settle

- **Frame minimum schema.** The concrete fields are outlined above but not
  formally derived in the current chain. In particular, the fault handler
  capability comes from the archive (journal/004) and should be re-examined.

- **Frame-Space binding model.** When and how a Frame binds to a Space. At
  creation only, or rebindable? Interacts with Frame-Space cardinality.

- **Frame lifecycle.** Create, destroy, suspend, resume. Whether Frame is a
  capability-held object type (archive journal/013 said yes). Interacts with D4
  and the open question about scope of capability mediation.

- **Can Frames share capability tables?** D4 requires per-Frame capability
  tables, but whether the underlying table structure can be shared (like seL4
  TCBs sharing a CSpace) is open. Determines multi-threading ergonomics.

- **Capability table structure.** Now more urgent: it determines how Frames in a
  shared-Space group interact with authority.

---

## Rejected alternatives

| Alternative                           | Foreclosed by | Reason                                                                               |
| ------------------------------------- | ------------- | ------------------------------------------------------------------------------------ |
| Frame-as-process (containing threads) | Vocabulary    | One Time per Frame; multiple schedulable entities need multiple Frames               |
| Multi-Time Frames                     | Vocabulary    | Explicitly forbidden ("not as a single Frame with multiple Times")                   |
| Sub-Frame schedulable units           | D2            | Scheduler selects Frames, not sub-Frame entities                                     |
| Kernel-level process/grouping         | A3 + judgment | Not essential (D4 handles lifecycle), not universal (A3), low friction for userspace |

---

## Axioms not load-bearing here

A1 (Rust) is not load-bearing. The execution unit model is language-independent.
Rust's type system will be relevant when implementing Frame structures, but the
derivation does not pass through A1.

A2 (ARM64) is not directly load-bearing for the top-level question. It defines
the register file (what "register state" contains concretely) but does not push
toward or away from any execution unit model. The derivation rests on
vocabulary, D2, D4, and A3.

A5 is load-bearing for Part 2 (no grouping) but not for Part 1 (Frame =
execution unit). Part 1 is forced by vocabulary + D2 alone.
