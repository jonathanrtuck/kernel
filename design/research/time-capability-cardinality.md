# Time Capability Cardinality

## The Question

Can an execution unit (thread, process, domain) hold multiple time/scheduling
capabilities, or is it structurally limited to exactly one? If multiple can be
held, can more than one be simultaneously active — i.e., can more than one time
object simultaneously govern an execution unit's scheduling at a given instant?

Two sub-questions:

1. **Holding cardinality.** Can an execution unit hold capabilities referencing
   N distinct time objects (where N > 1)?
2. **Active cardinality.** Can more than one held time capability simultaneously
   govern the execution unit's scheduling?

This question is distinct from the prior-art question of whether time is a
first-class capability at all (surveyed in `time-as-kernel-object.md`). It
targets the structural multiplicity of the binding once time-as-capability is
accepted.

---

## Survey of Existing Systems

### seL4 MCS: SchedContext

The seL4 MCS TCB struct has exactly one field for the active scheduling context:
`tcbSchedContext` (a single pointer). The SchedContext struct has exactly one
back-pointer: `scTcb`. This is a 1:1 structural relationship enforced at the
data-structure level — there is no array or list, only a pointer.

**Holding cardinality:** A TCB's CSpace (flat capability table) can hold
capabilities to an arbitrary number of SchedContext objects. There is no kernel
restriction on how many SC caps a CSpace entry count allows. A TCB might
simultaneously hold caps to SC-A (its current active context), SC-B (a backup
context for rebinding), and SC-C (a context it intends to donate to a passive
server).

**Active cardinality:** Exactly one SC is bound at a time, enforced by the
single `tcbSchedContext` pointer. The MCS spec states: "TCBs can only be bound
to one scheduling context at a time, and vice versa." (seL4 MCS docs,
`docs.sel4.systems/Tutorials/mcs.html`)

**Rebinding:** Possible via `seL4_SchedContext_Unbind` +
`seL4_SchedContext_Bind`. seL4 v13.0.0 introduced lazy SC rebind, relaxing a
prior constraint that prevented an SC from being simultaneously listed on both a
TCB and a Notification object. The relaxation makes rebinding easier to arrange
without creating a window where the TCB has no SC, but it does not change the
1-active constraint.

**Summary:** N-hold, 1-active. The 1-active constraint is structural (one
pointer slot), not a policy rule checked at runtime.

---

### Coyotos: Schedule Capability

The Coyotos microkernel specification (Shapiro, `shapiro:coyotosspec`) defines a
Schedule capability (type 12) as a "first class" object representing "permission
to execute under a particular schedule."

**Holding cardinality:** Coyotos processes have a general capability store
(similar to a CSpace). Schedule capabilities can be stored in any capability
slot — there is no documented restriction on holding multiple.

**Active cardinality:** The spec states: "In order to execute instructions, a
process must name (via a capability) the schedule under which it runs." The word
"name" implies a single designated slot that identifies the active schedule for
execution purposes. Chapter 7 on Schedules in the specification is noted as
incomplete, so the precise slot mechanism is not fully documented. The
architectural model implies one "named" schedule governs execution at any
moment.

**Summary:** N-hold (implied by general cap storage), 1-named-active (implied by
the singular "naming" requirement for execution).

---

### KeyKOS: Meter Key

KeyKOS domains hold capabilities in a capability list. Meter keys represent "the
right to execute for the unit of time held by the meter." The kernel's prime
meter represents all future CPU time; sub-meters are derived by subdivision.

**Holding cardinality:** A domain's capability list can contain arbitrary
capabilities. Multiple meter keys could in principle occupy different slots in
the list.

**Active cardinality:** "A domain requires a valid meter key to be eligible to
execute" (Bomberger et al., USENIX 1992). The original KeyKOS papers describe
each domain as having one active meter — execution accounting is credited
against that meter. The OSR paper describes "a meter" (singular) as one of the
three key domain resources alongside address space and domain identity. The
mechanism for which meter governs execution when multiple are held is not fully
described in published papers; the structural design implies a designated meter
slot.

**Summary:** Multiple meter keys can be held; one governs execution at a time.

---

### EROS: Schedule Capability

EROS inherits the KeyKOS meter model, refined as "schedule capabilities" that
designate scheduling reserves. "Schedule capabilities convey the authority for a
running domain to execute instructions under a particular scheduling reserve"
(Shapiro, Smith, Farber, SOSP 1999).

**Holding cardinality:** A domain's cap table can hold multiple schedule
capabilities.

**Active cardinality:** A domain without a valid schedule capability cannot run.
The EROS SOSP paper discusses scheduling and storage allocation as capabilities
that can be exported to user space for policy, but treats each running domain as
operating under one schedule reserve at a time.

**Summary:** N-hold, 1-active (singular reserve governs execution).

---

### Composite OS: SchedContext

Composite OS decouples execution state (thread = registers + stack) from
scheduling budget (SchedContext = budget + parameters). Thread migration carries
the scheduling context: when a thread migrates into a callee's protection
domain, its SchedContext follows, so the callee runs on the caller's budget.

**Holding cardinality:** Threads hold a reference to their SchedContext. The
migration model implies one context per thread at any moment; whether a thread
can hold additional SC references is not described in published papers.

**Active cardinality:** One SchedContext per executing thread — the budget that
follows the thread during migration. Migration replaces the governing context
atomically (old context leaves, new context arrives); there is no point at which
two contexts govern one thread.

Reference: Parmer and West, "Predictable and Configurable Component-Based
Scheduling in the Composite OS," ACM TECS 2013.

**Summary:** 1-active by design; multi-hold not discussed.

---

### Zircon: Profile

Zircon Profiles are scheduling parameter templates, not budgets. Applying a
profile to a thread (`zx_object_set_profile`) replaces the thread's current
scheduling parameters. Each call overwrites the previous configuration.

**Holding cardinality:** A process can hold handles to multiple Profile objects.
There is no kernel restriction on how many Profile handles are held.

**Active cardinality:** A thread has one set of scheduling parameters at a time.
The most recently applied profile governs. There is no mechanism to stack or
simultaneously apply two profiles; `zx_object_set_profile` is a last-write-wins
parameter update, not a composition operation.

**Summary:** N-hold (via handles), 1-active-configuration (latest `set_profile`
wins). Note: Profile is a parameter template, not a time budget — it does not
track consumption state.

---

## Cross-System Pattern

Every system surveyed that separates time/scheduling into a first-class object
converges on the same structural pattern:

| System       | Holding cardinality | Active cardinality | Enforcement mechanism            |
| ------------ | ------------------- | ------------------ | -------------------------------- |
| seL4 MCS     | N (CSpace slots)    | 1                  | Single `tcbSchedContext` pointer |
| Coyotos      | N (cap store slots) | 1                  | Single "named" schedule slot     |
| KeyKOS       | N (cap list slots)  | 1                  | One active meter per domain      |
| EROS         | N (cap table)       | 1                  | One schedule reserve per domain  |
| Composite OS | N (implied)         | 1                  | SC follows thread migration      |
| Zircon       | N (handles)         | 1                  | Last `set_profile` overwrites    |

No system surveyed allows two time objects to simultaneously govern one
execution unit's scheduling. The 1-active constraint appears universally.

---

## The Space/Time Asymmetry

The most conceptually important observation for this cardinality question is
that Space and Time have different "active" semantics, even if both are held as
capabilities:

**Space:** An execution unit's address space IS the union of all its held Space
regions. All Spaces are simultaneously active — memory translation uses all of
them in parallel. If an Observer holds N Space caps, all N are live in the page
table at the same time. Holding 3 Spaces means all 3 are concurrently mapped.

**Time:** An execution unit occupies one CPU at a time. At any clock tick,
exactly one time budget is being consumed — the one governing the CPU the unit
is currently running on. Execution is inherently sequential. Even if an Observer
held N Time caps, only one can be consuming CPU cycles at any instant, because
the Observer is on one CPU.

This asymmetry means:

- Space cardinality: N-hold + N-simultaneously-active (all held Spaces are
  always in the address space)
- Time cardinality: N-hold + 1-active-per-instant (only one Time object is being
  consumed at any clock tick)

The 1-active-per-instant property is not a design choice imposed by these
kernels — it follows from sequential execution semantics. Any thread executing
on one CPU is consuming exactly one time budget, regardless of how many time
capabilities it holds.

---

## What Multi-Hold Enables

Even with 1-active semantics, holding multiple time capabilities enables
meaningful operations across all surveyed systems:

**Rebinding (mode switching).** A thread holds caps to SC-A (normal execution
budget) and SC-B (elevated-priority budget for latency-sensitive work). It
switches by unbinding SC-A and binding SC-B. seL4 MCS uses this pattern.

**Passive server delegation.** A server holds its own SC cap but executes on
donated SCs from callers. The server's cap remains held for when no caller is
active. seL4 MCS uses this pattern explicitly.

**Core migration.** SchedContexts in seL4 MCS are per-core (created under a
specific CPU's SchedControl). Migration = hold SC-for-core-0 and SC-for-core-1,
unbind from core-0's SC, bind to core-1's SC.

**Temporal delegation pipeline.** An execution unit holds a "reserve" SC (larger
budget) and creates sub-SCs for children, delegating portions of its reserve.
This mirrors KeyKOS's meter subdivision model.

---

## Tradeoffs

**Structural 1-active (single field/slot) vs. Policy 1-active (runtime check)**

Structural 1-active (seL4 MCS's single `tcbSchedContext` pointer): The kernel
data model physically cannot represent two SCs bound simultaneously. Rebinding
requires explicit unbind + bind. No per-invocation check needed; structural
impossibility is stronger than a runtime guard.

Tradeoff: Migration and rebinding are explicit two-step operations. A window
exists between unbind and rebind where the thread has no SC (and cannot run).

**N-hold vs. 1-hold**

All surveyed systems allow N-hold. The benefits (rebinding, delegation,
migration) require holding multiple caps. Restricting to 1-hold would require
releasing the current SC cap before acquiring a new one — precluding the
atomic-swap pattern.

Tradeoff: N-hold means a thread can accumulate SC references without using them.
The kernel must correctly handle the case where the thread's cap table holds SC
caps that are not bound to the thread (no active semantics for un-bound caps).

**Active = singular vs. Active = sum**

No surveyed system implements "active = sum" (where holding N time caps gives
the thread N times as much budget). The semantics are always: the active cap
governs, held-but-not-active caps are idle.

An "active = sum" model would enable multi-core parallelism for a single thread
(hold one Time cap per core, execute on all simultaneously) — but this
contradicts the thread model where one thread occupies one CPU at a time.
Parallelism across cores requires multiple threads in all surveyed systems.

---

## Measured Data

No published benchmark specifically measures multi-SC holding cost in seL4 MCS
or other systems. The relevant seL4 MCS measurement is for SC binding/unbinding
overhead, not holding cost. From Lyons et al. (EuroSys 2018):

- SC binding operation: not separately benchmarked; part of thread creation
  overhead
- Replenishment operation (at period boundary): ~50 cycles (ARM Cortex-A9)
- IPC donation fastpath overhead: zero additional cycles vs. non-MCS IPC

The `tcbSchedContext` is a pointer field in the TCB struct. Additional SC caps
in the CSpace occupy capability slots (each ~16 bytes in seL4), subject to the
CSpace capacity. No separate per-held-cap overhead beyond slot space.

---

## References

- Lyons, A., McLeod, K., Almatary, H., Heiser, G. (2018). "Scheduling-context
  capabilities: a principled, light-weight operating-system mechanism for
  managing time." EuroSys 2018.
  https://trustworthy.systems/publications/abstracts/Lyons_MAH_18.abstract
- seL4 MCS Tutorial. https://docs.sel4.systems/Tutorials/mcs.html
- seL4 MCS Release Notes (v10.1.1-MCS).
  https://docs.sel4.systems/releases/sel4/10.1.1-mcs.html
- seL4 Release Notes (v13.0.0) — lazy SC rebind.
  https://github.com/seL4/docs/blob/master/content_collections/_releases/sel4/13.0.0.md
- Bomberger, A. et al. (1992). "The KeyKOS Nanokernel Architecture." USENIX
  Annual Technical Conference 1992.
  https://css.csail.mit.edu/6.5660/2017/readings/keykos.pdf
- Shapiro, J.S., Smith, J.M., Farber, D.J. (1999). "EROS: a fast capability
  system." SOSP '99.
  https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf
- Shapiro, J.S. Coyotos Microkernel Specification.
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- Parmer, G. and West, R. (2013). "Predictable and Configurable Component-Based
  Scheduling in the Composite OS." ACM TECS.
  https://www2.seas.gwu.edu/~gparmer/pubs.html
- seL4 TCB source (tcbSchedContext field).
  https://github.com/seL4/seL4/blob/master/src/object/tcb.c
