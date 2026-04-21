# 035 — Observer creation API shape

2026-04-21

## Starting point

D14 opened "Observer creation API shape" as a downstream question. D31 settled
the creation mechanism (`create_observer(space_cap, config) → observer_cap`),
D32 settled the type conversion model (Space consumed entirely as structural
backing), and D20/D21 settled the fault handler as a mandatory cap-table entry.
The remaining open sub-questions: create-then-configure vs. all-params-upfront,
initial PC/SP provision, initial capability set, and whether create and start
are separate operations.

## Settled constraints entering this derivation

- D32: creation consumes one Space cap entirely (type conversion into cap table,
  L0 page table root, register save area). The Space is gone; the Observer
  exists.
- D20/D21/D12: fault handler field cap + badge mandatory at creation time.
  Kernel rejects creation without handler. Installed as cap-table write at
  reserved slot index (D21).
- D31: Time acquired post-creation through pager chain. Not a creation
  parameter.
- D26: no address space object. Page table populated automatically as Space caps
  are acquired. No VSpace binding step.
- D7/A4: creation is a typed kernel syscall, synchronous.

## Exploration

### Foreclosed options

Five creation models from the landscape are foreclosed by settled decisions:

- **Fork+exec** (QNX, Minix 3): D6 (no kernel process concept), D4 (no ambient
  authority), D27 (flat Space cardinality). Fork implies implicit state
  inheritance; D4 requires explicit capability transfer.
- **Constructor/image-stamp** (EROS): D31 (factory caps rejected), D4 (no
  factory indirection).
- **Manifest-based** (Singularity): A3 (generic, no install-time compilation
  assumptions).
- **VSpace binding as creation step** (seL4 TCB_SetSpace): D26 dissolved the
  address space object.
- **Time as required creation parameter**: D31 (pager chain acquisition).

### The creation Space does not provide executable memory

D32's type conversion means the Space consumed at creation becomes structural
backing — cap table pages, L0 page table root, register save area. The
Observer's code, data, and stack live in different Spaces, held as caps in the
Observer's cap table. After creation, the Observer has structure but no
accessible memory.

The Space size determines the Observer's initial cap table capacity. A minimum
size is required for fixed structures; additional space becomes empty cap table
slots. The relationship is deterministic — the kernel can expose a query for how
many slots a given Space size yields (or the inverse), paralleling D25's
principle that essential constraints should be visible.

### The Observer cannot execute without a code Space cap

Under D26, PC is `base_of(Space) + offset`. With no Space caps, no virtual
addresses are valid. The D28 fault message format carries Space identity +
offset; with no Space, the fault is degenerate. The Observer must hold at least
one Space cap for code before execution is meaningful.

This creates a gap between creation and executability: the Observer needs code
Space caps installed before it can run. This gap is the forcing function for the
create/start separation question.

### The fault handler is a creation parameter under any model

D20: "enforced at creation time." Even a create-then-configure model must
include the fault handler at create, not as a later step. The handler + badge
collapse into the create call regardless of the overall API shape.

### Cap installation is a general-purpose operation

The pager/fault-handler workflow requires installing caps into another
Observer's table throughout the Observer's lifetime — not just at creation. When
a child Observer faults and needs a new Space, the supervisor installs a Space
cap into the child's table, then calls resume(). This is the same primitive that
would install initial caps before the first start.

Cap installation is therefore an Observer operation (a right in the Observer
cap's rights mask), not a creation-specific mechanism. It must work on both
inert and faulted Observers. The mechanism is D8-consistent: the kernel manages
slot allocation internally and returns the slot number.

### Create-then-configure vs. all-params-upfront

D14 named these as the two poles. Three considerations resolve the question:

1. **Forecloses nothing.** Separate create and start means all-params creation
   can be built as a userspace library wrapping the sequence (create + install
   caps + write registers + resume). The reverse — decomposing an atomic create
   into inspectable steps — cannot be built outside the kernel. Choosing the
   decomposable option preserves the option space.

2. **No new kernel surface.** Every operation in the multi-step sequence exists
   independently of creation: `install_cap` for fault resolution,
   `write_ registers` for debugging/inspection, `resume` for fault recovery.
   Creation introduces only the create syscall itself.

3. **Syscall overhead is negligible on this cold path.** Observer creation is
   structurally heavyweight (D6: cap table, page table root, register save area,
   scheduling state). The additional EL0→EL1 round trips for 4–6 syscalls add
   ~1,000–2,000 cycles to an operation already in the microsecond range.
   Workloads requiring higher-frequency spawning use userspace-internal
   concurrency (green threads inside one Observer), not kernel Observer
   creation.

A5 tension exists: multi-step assembly pushes a sequence to userspace. But "A5
absorbs complexity" means the kernel provides simple, composable primitives —
not necessarily that every operation is a single syscall. A large all-params
call with a variable-length cap list and many failure modes is arguably more
complex to use correctly than a sequence of focused operations with specific
errors. The userspace library is the A5-consistent answer: a `spawn()` function
that wraps the sequence.

### Minimal create parameters

The create call includes only what is structurally required at creation time:

- **Space cap** (D32 — the resource consumed for structural backing)
- **Fault handler field cap + badge** (D20/D21 — enforced at creation)

PC/SP are set via a separate write-registers operation. This ordering is
natural: PC is only meaningful after code Space caps are installed (the PC
refers to an address within a Space). Setting PC before the code Space exists
means the kernel stores a value it cannot validate until resume.

Initial capabilities (code Spaces, data Spaces, Fields, optionally Time) are
installed via the general-purpose `install_cap` operation. Time caps can be
optionally installed this way — D31 establishes the pager chain as the fallback
acquisition mechanism, not a prohibition on early provision. Under D30, Time
caps are regular cap table entries, structurally identical to Space caps.

## Decision

**D35 — Observer creation API: minimal create, separate start, composable
operations.**

Observer creation is a minimal typed kernel syscall:

```text
create_observer(space_cap, handler_field_cap, badge) → observer_cap [inert]
```

The Space cap is consumed entirely (D32 type conversion). The handler field cap
and badge are installed at the reserved cap-table slot (D21). The Observer is
created in an inert state — it has structure (cap table, page table root,
register save area) and a fault handler, but is not scheduled.

The caller configures the Observer using general-purpose operations before
starting it:

- `observer_install_cap(observer_cap, source_cap) → slot` — installs a cap into
  the Observer's table. Kernel manages slot allocation (D8-consistent). Usable
  at any time: pre-start setup, fault resolution, dynamic delegation. Requires
  an "install-cap" right on the Observer cap.
- `observer_write_registers(observer_cap, pc, sp, ...)` — sets register state.
  Requires a "write-registers" right on the Observer cap.
- `observer_resume(observer_cap)` — transitions the Observer from inert to
  runnable (D14). Requires the "resume" right.

A typical creation sequence:

```text
observer = create_observer(backing_space, handler_field, badge)
observer_install_cap(observer, code_space)
observer_install_cap(observer, data_space)
observer_install_cap(observer, stack_space)
observer_write_registers(observer, entry_pc, stack_top, ...)
observer_resume(observer)
```

### What this settles

- Create and start are separate operations. Creation produces an inert Observer;
  `resume()` starts it.
- Creation parameters are minimal: backing Space + fault handler field cap +
  badge.
- PC/SP are set via a separate register-write operation.
- Initial capabilities are installed via a general-purpose cap-install operation
  that also serves fault resolution and dynamic delegation.
- The cap-install operation uses kernel-managed slot allocation (D8-consistent).
- Time may be optionally installed before start; the pager chain (D31) is the
  fallback, not the only path.

### What this does NOT settle

- **Observer rights model.** D35 confirms that install-cap, write-registers, and
  resume are Observer operations requiring rights. The complete rights set
  (suspend, inspect, modify scheduling properties, change fault handler) is one
  level down.
- **Observer minimum schema.** The concrete struct fields are a separate
  derivation. D35 constrains it: the struct must support an inert state
  (not-yet-scheduled).
- **Specific syscall encoding.** Register conventions, error codes, how
  source_cap is identified in the install-cap call — implementation details.
- **Reply field allocation timing.** D16 opened this (pre-allocated at creation
  vs. lazy on first Call). D35 is compatible with either: pre-allocation would
  require the creation Space to be large enough; lazy requires no creation-time
  impact.
- **Cap-install slot selection policy.** Whether the caller can request a
  specific slot vs. always kernel-chosen. D8 says kernel-managed; the default is
  kernel-chosen.

### Costs

- **Multi-step creation.** 4–6 syscalls for a typical Observer. Each is an
  EL0→EL1 round trip (~200–500 cycles). Total overhead ~1–3 µs. Observer
  creation is a cold-path operation (D1); this cost is negligible relative to
  the structural weight of Observer creation itself.
- **Userspace assembly sequence.** A5 tension: the kernel provides primitives,
  not a complete spawn operation. Mitigated by userspace libraries wrapping the
  common pattern.

## Archive convergence

The archive used all-params-upfront:
`create_context(space, time, fault_handler, ...) → context_handle`. The archive
also listed "create_context parameters" as an open question. This derivation
diverges: minimal create + composable operations instead of all-params. The
divergence is explained by: (1) D31 removes Time from creation parameters
(archive included it); (2) D26 removes VSpace binding (archive had TTBR as a
context struct field); (3) the cap-install operation for fault resolution
(identified during this derivation) makes initial-cap installation a reuse of
existing machinery rather than a creation-specific concern. The archive lacked
this structural reuse argument because its creation model was designed before
the pager protocol was explored.

## Status

Settled — revisit if D32 is revised (changes the type conversion model that
makes creation a Space-consuming operation), if D20/D21 are revised (changes the
fault handler requirement at creation), or if the Observer rights model
derivation reveals that the install-cap / write-registers decomposition creates
essential complexity that a richer create call would have avoided.
