# 043 — Observer minimum schema

2026-04-22. Starting from the explicit open question in spec.md: "Observer
minimum schema. The concrete field set (register state, L0 page table pointer,
capability table pointer, cached scheduling aggregate, scheduling state,
pending-list linkage, scheduling profile) needs formal derivation."

All parent decisions settled: D6 (Observer = execution unit), D14
(capability-held), D20/D21 (fault handler as cap-table entry), D29/D30 (Time as
capability-held, multi-Time in regular slots), D35 (creation API, inert state),
D39 (five-state machine, nine rights), D42 (three-value scheduling profile), D18
(pending-list linkage), D19 (multi-field wait), D32 (type conversion model),
D11/D33 (revocation, destroy cascade).

---

## The question

What concrete fields must the Observer kernel struct contain? Nine derivations
have settled what goes elsewhere (cap table, per-core scheduler, structural
backing). This derivation synthesizes the minimum field set.

---

## Physical split: metadata struct + structural backing (D32/D35)

D32 says per-object kernel metadata comes from root Space — "bounded per object,
fixed-size, small relative to the object's functional backing." D35 says
creation consumes a Space entirely into structural backing: cap table pages, L0
page table root, register save area.

The Observer is therefore two physical regions:

- **Metadata struct** — from root Space. Small. Contains pointers to structural
  backing plus scalar fields (scheduling state, aggregate, profile, reference
  count).
- **Structural backing** — from the consumed Space. Contains the register save
  area (~816 bytes), cap table pages, and L0 page table root.

The metadata struct references the structural backing via pointers. The register
save area (816 bytes: 31 GP × 8, SP/PC/PSTATE × 8, TPIDR × 8, 8 alignment
padding, 32 SIMD/FP × 16, FPCR/FPSR × 8) violates D32's "small" constraint if
placed inline in root-Space
metadata. Pointer indirection adds ~4 cycles (L1 hit) on context switch —
negligible against the ~400-cycle fast-path floor (D13). The metadata struct
stays small (~80–100 bytes); the scheduler's working set (state + aggregate +
profile) fits in one or two cache lines.

---

## Forced fields (eight clusters, mechanically derived)

Each field is required by at least one settled derivation. Absence would violate
that derivation.

### 1. Register save pointer (D6/D35/D32)

D6: one register state. D35: register save area is part of structural backing
(consumed Space). D32: root-Space metadata is "bounded, small" — ~816 bytes of
registers cannot be inline. The metadata struct holds a pointer to the register
save area in structural backing. Hot-path: loaded on every context switch.

### 2. TTBR0 value (D5/D26/D1)

D5: per-Observer L0 page table. D26: root pointed to by TTBR0_EL1, containing
entries only for Spaces the Observer holds caps to. D1: context switch is
hot-path. The physical address of the L0 root must be loadable without chasing
through structural backing. One u64.

### 3. Cap table pointer (D4/D8)

D8: kernel-managed flat array. D35: cap table pages are structural backing. The
metadata struct holds a pointer to the array base. D8's table-full fault means
the table can grow — the pointer must be updatable. Cap table capacity tracking
(for table-full detection) is an implementation detail: could be alongside this
pointer in the struct or in the table's own header. Deferred to implementation.

### 4. Scheduling state (D39)

D39: five states — inert, runnable, blocked, faulted, externally-suspended.
Suspended co-occurs with blocked and faulted. Encoding: primary state enum
{inert, runnable, blocked, faulted} plus suspended boolean flag. Resume clears
the suspension; if an underlying block/fault remains, the Observer stays in that
state.

### 5. Cached scheduling aggregate (D30/D31/D36)

D30/D31: kernel maintains the cached sum of held Time compute units. D36: units
are normalized, hardware-independent integers. D1: scheduler reads it hot-path
(O(1)). Updated cold-path on Time cap installation or removal. One integer.

### 6–8. Scheduling profile: responsiveness, throughput, precision (D42)

D42: three values sharing a fixed per-Observer budget. D39: modify-scheduling
right gates external modification. D1: scheduler reads hot-path. Three integers.

These are the only scheduling profile fields. There are no separate "base" and
"effective" values. The kernel stores and reads one set. If userspace wants to
adjust them dynamically (e.g., a supervisor boosting a server's responsiveness
before dispatching a latency-sensitive request), it uses modify-scheduling
(D39). Scheduling inheritance during IPC is a userspace concern, not a kernel
mechanism — the server's optimal scheduling profile is determined by the
server's current work, not by who requested it.

### 9. Wait-state linkage (D18/D19)

D18: intrusive linked list through Observer objects for blocked-on-receive and
fault-pending-delivery. D34 confirms O(1) unlink required (destroy must unlink
from pending lists). D19: multi-field wait should be accommodated from the
start.

Representation: a Rust enum with inline single-field variant (prev + next
pointers + field reference, no allocation) and an allocated multi-field variant
(pointer to a list of wait entries). The single-field variant covers the
overwhelmingly common case (receive from one Field) with zero allocation. The
multi-field variant supports future multi-receive without schema rework.

The enum encoding is A1-idiomatic: Rust enum-as-state-machine with
compiler-enforced exhaustive matching. src/CLAUDE.md preference for explicit
state transitions applies.

### 10. Reference count (D11/D33)

D11: "refcount on holder-drop" — close drops a reference. D33: "objects reaching
refcount zero are destroyed too." One integer tracking outstanding capability
references to this Observer.

---

## What is NOT in the struct (and why)

| Candidate                          | Where it lives                               | Why                                                |
| ---------------------------------- | -------------------------------------------- | -------------------------------------------------- |
| Fault handler                      | Cap table, reserved slot (D21)               | D11 destroy, D17 badge-closure, D8 ABA             |
| Reply field                        | Cap table, reserved slot (D16 + D21 pattern) | Same three D21 arguments apply                     |
| Time caps                          | Cap table, regular slots (D30)               | Capability-held objects                            |
| Algorithm-specific scheduler state | Per-core, in scheduler (D2)                  | Observer carries abstract profile only             |
| VA mappings                        | Kernel-internal page tables (D26)            | Kernel-managed, not per-Observer data              |
| Grouping/parent links              | None (D6)                                    | No kernel grouping                                 |
| Effective/base scheduling split    | None                                         | Scheduling inheritance is a userspace concern      |
| Core assignment                    | Per-core scheduler (D31)                     | Transient — re-decided on each runnable transition |

### Core assignment: transient, not persistent

D31 makes core assignment kernel-internal. The kernel decides which core an
Observer runs on each time it transitions to runnable. This is transient
placement, not persistent assignment — no core ID field in the Observer struct.

Arguments for transient:

- Every wake-up is an implicit migration opportunity. The kernel can optimize
  placement continuously based on current load, core capacity, and cache state.
- The placement decision (~50–200 cycles to check candidate cores) is small
  relative to the IPI cost (~1000–5000 cycles) that cross-core wake-ups pay
  regardless of persistent vs. transient.
- Transient can sometimes avoid IPIs by placing locally (saving ~2000 cycles),
  which persistent assignment cannot do.
- Cache affinity (the main argument for persistent assignment) can be maintained
  as a per-core scheduler hint ("recent Observers on this core") without a
  per-Observer field.

The D13 direct-switch fast path is unaffected — both Observers end up on the
same core regardless of assignment model.

### Reply field: cap-table reserved slot

D16 settles pre-allocated reply field per Observer. D21 established the pattern
of reserved cap-table slots for per-Observer Field references. The three
arguments that settled D21 (fault handler as cap-table entry) apply identically
to the reply field:

1. D11 destroy-invalidation: cap-table walk finds and invalidates automatically
2. D17 badge-closure: cap close fires lifecycle notifications generically
3. D8 ABA protection: slot tag prevents stale references

The reply field is a regular Field object held at a second reserved cap-table
slot. D16's open question on allocation policy (pre-allocated at creation vs.
lazy) is compatible with either approach — the slot exists in the table, and may
be populated at creation or on first Call().

---

## Costs

- **Pointer chase on context switch.** The register save area is in structural
  backing, accessed via a pointer in the metadata struct. One dependent load (~4
  cycles L1 hit) per context switch. This is ~1% of the D13 fast-path floor
  (~400 cycles). The alternative (registers inline in metadata struct) would
  bloat root-Space metadata by ~816 bytes per Observer, breaking D32's "bounded,
  small" constraint and polluting scheduler cache lines with data only touched
  at switch boundaries.

- **Wait-state enum overhead.** The enum tag adds ~1–2 cycles per wait-state
  manipulation (discriminant check). The multi-field variant requires allocation
  from an allocation source not yet determined (per-core slab, root Space, or
  Observer Space). The single-field variant (common case) has zero allocation
  overhead.

- **No kernel-side scheduling inheritance.** The Call/Reply fast path does no
  profile manipulation — simpler and faster. The cost: userspace must explicitly
  manage scheduling adjustment when needed (extra modify-scheduling syscalls).
  This is consistent with A5 — the kernel provides the modify-scheduling
  mechanism; userspace provides the policy for when to use it.

---

## What this does NOT settle

- **Wait-state allocation source for multi-field.** Where do extra WaitEntry
  nodes come from when an Observer blocks on N > 1 Fields? Per-core slab, root
  Space, Observer's Space? Downstream of this derivation.
- **Cap table capacity tracking placement.** In the Observer struct alongside
  the pointer, or in the table's own header. Implementation optimization,
  deferred.
- **Register save area layout within structural backing.** How the consumed
  Space is carved into cap table pages, L0 page table root, and register save
  area. D32/D35 define the components; internal layout is implementation.
- **Budget size and encoding.** D42 deferred: 100 points, 256, or other.
- **Default scheduling profile for newly-created Observers.**
- **Self-reference capabilities.** Whether an Observer holds a cap to itself.

---

## Archive convergence

Archive journal/004 derived a "Context model schema" with: register state, TTBR,
runnable/blocked state, fault handler capability, pending message state. This
derivation's forced fields are a superset:

**Convergent:**

- Register state (both: per-Observer, save/restore on switch)
- Page table root / TTBR (both: per-Observer, loaded on switch)
- Scheduling state (both: tracks runnable/blocked; this chain adds inert,
  faulted, suspended from D35/D39)
- Pending message / wait-state linkage (both: tracks blocking; this chain adds
  multi-field from D19)

**Divergent:**

- Fault handler: archive had it as a struct field; this chain moved it to cap
  table (D21). Three structural arguments (D11, D17, D8) drove the move.
- Cached scheduling aggregate: absent in archive. Present here because D30
  (multi-Time) creates the need — the archive had exactly-one-Time.
- Scheduling profile (R, T, P): absent in archive. Archive had structured timing
  declarations (mode + parameters) on a separate "Time Shape" object. This chain
  puts qualitative profile on the Observer (D42) and quantitative allocation on
  Time (D36).
- Reference count: absent in archive. Present here from D11/D33 (close/destroy
  semantics).
- Capability table pointer: archive had it (CSpace binding). Same concept,
  different representation (D8 flat array vs. archive's CNode tree).

Divergences are explained by downstream decisions made after the archive froze:
D21 (fault handler → cap table), D30 (multi-Time → aggregate), D42 (profile on
Observer), D11/D33 (refcount from revocation model).

---

## Axioms

**A1 (Rust):** Load-bearing. The wait-state enum uses Rust's enum-as-state-
machine pattern. The physical split (metadata struct + structural backing via
pointer) maps to Rust ownership: the metadata struct owns a reference to the
structural backing. src/CLAUDE.md preferences (concrete types, explicit state
transitions, simple lifetimes) are directly relevant.

**A2 (ARM64):** Load-bearing for register state content. The ARM64 register file
(31 GP + SP/PC/PSTATE + 32 SIMD/FP + FPCR/FPSR + TPIDR_EL0) determines the
register save area size (~816 bytes). TTBR0_EL1 is the specific ARM64 register
holding the L0 page table root.

**A3 (generic):** Not directly load-bearing. A3's work is done through ancestors
(D42 — profile spans all workload types; D39 — state machine handles all
lifecycle patterns).

**A4 (purely reactive):** Indirectly load-bearing. A4 means the Observer struct
must contain everything the kernel needs to resume an Observer on exception
return — there is no kernel thread to lazily compute anything. This reinforces
the cached aggregate and the TTBR0 value as struct fields rather than derived
values.

**A5 (kernel absorbs complexity):** Load-bearing for the "no kernel-side
inheritance" decision. A5 says the kernel provides mechanism (modify-scheduling)
and absorbs the complexity of scheduling interpretation. But the policy of when
to adjust scheduling — including IPC inheritance — is a userspace concern. The
kernel's mechanism is sufficient; absorbing the policy would not reduce
essential complexity, only shift it from one userspace protocol (explicit
adjust) to a kernel mechanism (automatic inheritance) with equivalent
expressiveness.
