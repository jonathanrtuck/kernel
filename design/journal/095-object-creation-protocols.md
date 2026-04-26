# D95 — Object creation protocols

**Question:** What is the complete creation protocol for each structural kernel
object (Observer, Field, Pulsar)? Where does each piece of structural backing
come from?

**Rests on:** D32 (type conversion — Space consumed entirely), D35 (minimal
create, composable setup, inert state), D43 (Observer schema), D57 (reserved
slots: 0=handler, 1=reply, 2=self), D13 (bounded queue), D44 (Pulsar), D62
(armed at creation), D72 (relative nanoseconds), D83 (per-core deadline array),
D89 (L1 per-Observer), D92 (page table memory accounting).

**Status:** settled.

---

## Settles

### 1. Observer structural backing from consumed Space

D32 says object creation is type conversion: a Space is consumed entirely and
becomes the object's functional backing. D35 says the consumed Space becomes
structural backing (register save area, cap table pages, page table root). D32
also says per-object kernel metadata is allocated from the kernel's root Space.

This produces a clean split:

| Allocation               | Source          | Size                       |
| ------------------------ | --------------- | -------------------------- |
| Observer metadata struct | root pool (D32) | 96 bytes (arena slab)      |
| RegisterState save area  | consumed Space  | 816 bytes                  |
| Cap table pages          | consumed Space  | initial allocation         |
| L1 page table root       | consumed Space  | 16 KiB (one page, D89/D92) |

No split sourcing within structural backing. The consumed Space funds all three
structural pieces (RegisterState, cap table, L1 root). The Observer struct
itself is metadata — small, bounded, from the root pool. D32's "bounded per
object, small relative to functional backing" describes this exactly.

### 2. CreateObserver protocol

```text
create_observer(space_cap, handler_field_cap, badge) -> observer_cap
```

D35 settles the signature and the inert-state contract. The protocol is:

1. **Consume Space.** The Space cap is consumed entirely (D32 type conversion).
   The physical pages become structural backing.
2. **Allocate Observer struct.** From the kernel's root pool (D32 metadata).
   Arena slab allocation (D70).
3. **Allocate RegisterState.** From the consumed Space's backing. 816 bytes (31
   GPRs + SP + PC + PSTATE + TPIDR + 32 FP/SIMD regs + FPCR + FPSR). The
   Observer struct receives a `RegisterStateHandle` pointing into this region.
4. **Allocate cap table pages.** From the consumed Space's backing. Initial
   capacity determined by remaining Space size after RegisterState and L1 root.
5. **Allocate L1 page table root.** From the consumed Space's backing. One 16
   KiB page (D89 per-Observer L1, D92 charged to Observer).
6. **Populate reserved slots.**
   - Slot 0 (SLOT_FAULT_HANDLER): `handler_field_cap` with `badge` (D21).
   - Slot 1 (SLOT_REPLY_FIELD): empty initially. The kernel creates the reply
     Field on first Call (D16 — reply field pre-allocated per Observer, but the
     cap-table slot is prepared at Observer creation).
   - Slot 2 (SLOT_SELF): self-reference cap with full Observer rights (D57).
7. **Set inert state.** `PrimaryState::Inert`, not scheduled (D35).
8. **Return observer_cap.** Single cap handle to the caller.

The caller then configures via composable operations (D35):

- `ObserverInstallCap` — install Space/Time/Field caps into the Observer's
  table.
- `ObserverWriteRegisters` — set PC, SP, initial register values.
- `ObserverResume` — transition from Inert to Runnable (D14).

### 3. CreateField protocol

```text
create_field(space_cap) -> field_cap
```

Space consumed for queue backing (D32). The queue capacity is derived from the
Space size:

```text
queue_capacity = floor(space_bytes / size_of::<Message>())
```

No explicit capacity argument. The Space size IS the capacity specification. The
caller controls capacity by choosing the Space size at split time. This is D32's
type conversion applied to Fields: the Space determines the object's functional
capacity, not a separate parameter.

`DEFAULT_QUEUE_CAPACITY` in config.rs (currently 16) is for Fields created
without explicit sizing — kernel-created Fields at boot (e.g., the root
Observer's fault handler Field, interrupt delivery Fields). These use
kernel-internal Space allocation from the root pool.

The protocol:

1. **Consume Space.** Type conversion (D32).
2. **Allocate Field struct.** From root pool (D32 metadata). Arena slab.
3. **Allocate queue buffer.** From consumed Space's backing. Contiguous array of
   `Message` slots, capacity derived from Space size.
4. **Initialize.** Empty queue (head=0, length=0), no waiters, no routing table,
   no pending list, badge_tracking per creator specification.
5. **Return field_cap.** Full Field rights.

### 4. CreatePulsar protocol

```text
create_pulsar(space_cap, field_cap, badge, duration_ns, period_ns) -> pulsar_cap
```

D62 settles that Pulsars are armed at creation — no separate arm operation. D44
settles that Pulsars have no structural gap between creation and arming. D72
settles that duration is relative nanoseconds.

The protocol:

1. **Consume Space.** Type conversion (D32). Space backs the Pulsar metadata.
   Pulsars are small objects — the Space is mostly overhead, but D32's uniform
   type-conversion model applies regardless of object size.
2. **Allocate Pulsar struct.** From root pool (D32 metadata). Arena slab.
3. **Convert duration to deadline.** `duration_ns` converted to absolute ticks
   using CNTFRQ_EL0 (D72: kernel absorbs ns-to-ticks conversion, A5).
   `next_deadline_ticks = now_ticks + ns_to_ticks(duration_ns, counter_freq)`.
4. **Install deadline.** The Pulsar's deadline is installed in the creating
   Observer's current core's deadline array (D83: per-core, max 32 entries). If
   the deadline array is full (32 active deadlines on this core), reject with
   error. The caller must retry after existing Pulsars expire or are destroyed.
5. **Record delivery target.** `delivery_field = field_cap`'s ObjectId, `badge`
   stored for message injection (D17, D63).
6. **Set period.** `period_ns = 0` for one-shot. `period_ns > 0` for repeating
   with kernel-managed re-arm and drift compensation (D44:
   `next = scheduled + period`, not `next = now + period`).
7. **Return pulsar_cap.** Destroy + clone rights (D52).

### 5. L1 page table root from consumed Space

D92 already settled this: "L1 root — charged to the Observer." D35 says the
consumed Space becomes structural backing. D89 says L1 is per-Observer. The L1
root is one 16 KiB page allocated from the consumed Space at Observer creation.

This is not from the kernel root pool. The L1 root is part of the Observer's
structural existence — same category as the RegisterState save area and cap
table pages. The root pool funds L2 tables (D92: on-demand, per-region
connectivity) and the Observer metadata struct, but not the L1 root.

---

## Rejected alternatives

### Split structural backing between consumed Space and root pool

D32 says "consumed entirely." If the consumed Space funded only some structural
pieces (say, RegisterState) and the root pool funded others (say, L1 root), the
Space would not be consumed entirely — some capacity would remain in the Space
after partial allocation. The type-conversion model requires complete
consumption. All structural backing from the consumed Space; all metadata from
the root pool.

### Explicit queue capacity argument for CreateField

Redundant with Space size. Adding a capacity parameter creates two ways to
express the same thing, and the kernel must validate consistency between them
(capacity _ message_size <= space_bytes). The Space size alone is sufficient and
unambiguous. If the caller wants 32 message slots, they create a Space of 32 _
size_of::\<Message\>() bytes and call create_field.

### Separate Pulsar arm operation

D62 explicitly forecloses this: "Pulsars have no structural gap between creation
and arming." A two-step create-then-arm model would introduce a state where the
Pulsar exists but is not armed — requiring an inert state, a separate arm
syscall, and rights checking on the arm call. The Observer's composable setup
pattern (D35) does not apply because Pulsars have no configuration that must
happen between creation and activation. Cancel = destroy(pulsar_cap). Modify =
destroy + create.

### L1 root from root pool

D35 and D92 settle this as structural backing from the consumed Space. The L1
root is part of the Observer's structural existence (D43 lists it alongside
RegisterState and cap table pages). The root pool funds per-object metadata
(small, bounded) and L2 tables (on-demand connectivity), not structural backing.

---

## Does NOT settle

- Cap table initial capacity calculation (how many slots fit in the remaining
  Space after RegisterState and L1 root — implementation arithmetic).
- Reply Field creation timing (at Observer creation vs. lazy on first Call — D16
  compatible with either; D35's "empty initially" for slot 1 suggests lazy).
- Space minimum size for Observer creation (must fit 816 bytes RegisterState +
  16 KiB L1 root + at least 3 cap table entries for reserved slots — exact
  minimum is implementation detail).
- Pulsar deadline array full error type (kernel error variant for "deadline
  array at capacity" — deferred to syscall error enumeration).
- Boot-time Field creation path (kernel-created Fields use
  DEFAULT_QUEUE_CAPACITY with root pool Space — protocol mechanics identical,
  Space source differs).
