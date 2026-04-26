# D99 — Hardware event wiring

**Date:** 2026-04-25

**Question:** How does IRQ authority flow from boot to drivers, how does the GIC
acknowledgment protocol work, and where do Pulsar deadlines live at creation
time?

**Rests on:** D22 (IRQ routing to Fields), D44 (Pulsar — kernel-managed timer),
D45 (FieldSplit — badge-range routing), D62 (armed at creation), D63 (timer fire
message), D67 (generation counter — stale route detection), D72 (relative
nanoseconds, drift compensation), D81 (hardware event protocol — IRQ routing
table), D83 (per-core deadline array, max 32), A5 (kernel absorbs complexity —
GIC protocol handled internally).

**Status:** settled.

---

## Exploration

### Three open questions

D81 settled the runtime flows (`handle_irq`, `handle_timer`) and D83 settled the
per-core data layout. What remains are three wiring questions that connect those
flows to the rest of the system:

1. **How does IRQ authority reach driver Observers?** D22 says the kernel
   maintains an IRQ-to-Field routing table and that delegation happens through
   Field split. But what populates the table at boot, and what updates it when
   splits happen?

2. **When does the kernel acknowledge the GIC?** D22 says the kernel does the
   GIC protocol (mask, signal, EOI). The exact placement of IAR read and EOI
   write relative to message construction matters for correctness.

3. **Where does a Pulsar's deadline entry go at creation?** D83 defines the
   per-core deadline array. D62 says Pulsars are armed at creation. The
   connection between CreatePulsar and the deadline array is the gap.

### Decision 1: IRQ authority delegation

D22 says: "At boot, device interrupts route to a root interrupt field. The
initial Observer receives this field. To delegate, the holder splits the field
by IRQ range."

Working through the concrete mechanism:

**Boot:** The kernel populates the `IrqRoutingTable` (D81) with all discovered
device INTIDs. Each route points to the root interrupt Field with
`badge = INTID`. This is the initial state — every device interrupt routes to
one Field, and the badge distinguishes which device fired. The root Observer
holds the receive cap for this Field.

**Delegation via FieldSplit (D45):** When the root Observer (or any intermediate
supervisor) splits the interrupt Field by badge range, the kernel must update
the routing table. The split creates a new Field for the badge range. The kernel
walks the affected `IrqRoutingTable` entries (those whose badge falls within the
split range) and repoints them from the source Field to the new destination
Field.

This is the key insight: the `IrqRoutingTable` is the kernel-internal
materialization of the Field-based authority model. D45 routing rules are the
capability-level abstraction; the routing table is the mechanism that makes
`handle_irq` fast (O(1) INTID lookup instead of walking a routing chain).
Parallel to D24 (page tables as materialized views of capability state).

**Stale route detection:** D67 generation counters handle the race where a
destination Field is destroyed between route installation and interrupt
delivery. `handle_irq` (D81) already checks `route.generation` against the live
Field's generation. When FieldSplit executes, the new route is installed with
the destination Field's current generation. When the destination is destroyed
and the routing rule is removed (D45 fallback-on-destroy), the routing table
entry reverts to the parent Field.

### Decision 2: IRQ acknowledgment protocol

The GIC interrupt lifecycle is: pending → active → inactive. The kernel must
read `ICC_IAR1_EL1` (acknowledge, transitions pending→active, returns INTID) and
write `ICC_EOIR1_EL1` (end-of-interrupt, transitions active→inactive).

Consulting the ARM Architecture Reference Manual (GICv3): IAR read is the
acknowledgment. Until IAR is read, the interrupt stays pending and can be
preempted by a higher-priority interrupt. After IAR, the interrupt is active and
the INTID is known.

The question is when to write EOI relative to message construction:

**Option A: EOI immediately after IAR.** The interrupt becomes inactive before
message construction. If message construction or enqueue fails, the interrupt
can fire again — but for edge-triggered interrupts, there is no re-trigger until
the next hardware edge. For level-triggered, the still-asserted line would
immediately re-trigger, creating a loop.

**Option B: EOI after message construction, before return to userspace.** The
interrupt stays active during message construction (preventing re-trigger of the
same INTID) and becomes inactive only after the message is safely constructed
and enqueued (or dropped on full queue). This is correct for both edge-triggered
and level-triggered: the driver receives the message and can act on it before
the interrupt can fire again.

Option B chosen. The flow:

1. GIC signals IRQ → kernel takes exception
2. Read `ICC_IAR1_EL1` (acknowledge, get INTID) — `gic::acknowledge()`
3. Look up `IrqRoutingTable` → get target Field, badge, generation
4. Construct `Message::device_irq(badge, intid)`
5. Enqueue to target Field (or drop on full queue)
6. Write `ICC_EOIR1_EL1` (end of interrupt) — `gic::end_of_interrupt(intid)`
7. Return to scheduling decision

The driver Observer does NOT need to explicitly acknowledge — the kernel handles
the entire GIC protocol. This is consistent with A5 (kernel absorbs complexity).
The driver sees an ordinary Field message.

**Level-triggered re-enable:** For level-triggered interrupts, EOI alone is not
sufficient if the device is still asserting the line — the interrupt will
immediately re-trigger. The driver needs a way to tell the kernel "I've serviced
the device, re-enable the interrupt." D22 envisions a send-once ack cap for
this. For the initial implementation (edge-triggered only), kernel EOI is
sufficient. Level-triggered re-enable is a future addition — the mechanism
(send-once cap that unmasks via GIC redistributor) does not require changes to
the routing table or message format.

### Decision 3: Pulsar deadline installation

D62 says CreatePulsar is single-call, armed-at-creation. D83 says per-core
deadline arrays with max 32 entries. The connection:

When `CreatePulsar` executes:

1. The Pulsar object is created in the arena (D70) with `Pulsar::new()` (D72:
   converts duration_ns to absolute ticks).
2. The kernel installs a `DeadlineEntry` in the creating Observer's current
   core's deadline array (`CoreState.deadlines`, D83).

"Current core" is unambiguous: D56 (placement) is kernel-internal. Each Observer
runs on exactly one core at a time. The creating Observer is running on this
core (it issued the syscall), so the deadline goes in this core's array.

The deadline array has a hard cap of `MAX_DEADLINES_PER_CORE = 32` (D83). If the
array is full, `CreatePulsar` fails with an error. This is the resource
exhaustion path — 32 per-core timers is generous for typical workloads (D83
journal: "2-5 timers per application").

**When a Pulsar fires:** `handle_timer` (D81) scans the deadline array, finds
expired entries, and calls `pulsar.fire_message()` + enqueues to the target
Field. For repeating Pulsars, the kernel calls `pulsar.rearm()` (D72: drift
compensation, `next = scheduled + period`) and updates the `deadline_ticks` in
the same array slot. For one-shot Pulsars, the kernel removes the entry via
swap-remove (D83 dense-packing invariant).

**When a Pulsar is destroyed:** The kernel scans the deadline array for the
matching `pulsar_id` and removes it via swap-remove. This is O(32) in the worst
case — acceptable for a cold-path destroy operation.

**Observer migration:** If the Observer migrates to a different core (D56
rebalancing), the Pulsar's deadline entry could migrate too. This is a future
optimization, not required for the initial implementation. The Pulsar fires on
whatever core owns the deadline entry — the fire message goes to the Field, not
directly to the Observer, so it reaches the Observer regardless of which core it
is currently running on.

---

## Settles

### IRQ authority delegation (#23)

At boot, the kernel populates the `IrqRoutingTable` with all discovered device
INTIDs routing to the root interrupt Field (with `badge = INTID`). The root
Observer holds the receive cap for this Field. IRQ delegation happens through
FieldSplit (D45): the root Observer splits the interrupt Field by badge range to
create sub-Fields for specific IRQ ranges. When FieldSplit executes, the kernel
updates the `IrqRoutingTable` entries for the affected badge range to point to
the new sub-Field. The routing table is the kernel-internal mechanism that
implements the Field-based authority model. Stale routes detected via generation
counter (D67).

### IRQ acknowledgment protocol (#24)

Kernel acknowledges the interrupt immediately. Flow: GIC signals IRQ → kernel
reads IAR (acknowledge, gets INTID) → kernel looks up `IrqRoutingTable` →
constructs `Message::device_irq` → enqueues to target Field → writes EOI to
`ICC_EOIR1_EL1`. EOI happens after message construction but before returning to
userspace. The driver Observer does NOT need to explicitly ack — the kernel
handles the GIC protocol entirely. This is consistent with A5 (kernel absorbs
complexity). If the driver needs to re-enable a level-triggered interrupt, it
uses a separate mechanism (future — not needed for initial implementation with
edge-triggered only).

### Pulsar deadline installation (#25)

When `CreatePulsar` executes, the kernel installs the deadline in the creating
Observer's current core's deadline array (`CoreState.deadlines`, D83). "Current
core" is unambiguous because each Observer runs on exactly one core at a time
(D56 placement). The deadline array has a hard cap of
`MAX_DEADLINES_PER_CORE = 32` (D83). If the array is full, `CreatePulsar` fails
with an error. When a Pulsar fires, the kernel re-arms it (D72 drift
compensation for repeating Pulsars) by updating the `deadline_ticks` in the same
array slot. When a Pulsar is destroyed, the kernel removes it from the array.

---

## Does NOT settle

- Boot sequence for root interrupt Field creation (part of the unsettled boot
  distribution protocol referenced in D22).
- Level-triggered interrupt re-enable mechanism (D22 envisions send-once ack
  cap; deferred until level-triggered device support is needed).
- Pulsar deadline migration on Observer migration (future optimization; the fire
  message reaches the Observer regardless of core via Field delivery).
- FieldSplit routing table update atomicity (the IrqRoutingTable update and the
  D45 routing rule installation are separate operations; ordering between them
  is a future concern for concurrent FieldSplit).

---

## Rejected alternatives

**Userspace IRQ acknowledgment syscall.** Over-engineering for edge-triggered
interrupts. Kernel EOI is sufficient. Level-triggered re-enable can be added
later behind a separate interface (send-once ack cap per D22) without changing
the core acknowledgment flow.

**EOI before message construction (Option A).** For level-triggered interrupts,
the still-asserted line would immediately re-trigger, creating a storm. EOI
after construction (Option B) keeps the interrupt active during the window where
the kernel is building the message, preventing re-trigger.

**Pulsar deadline on a different core than the creator.** D56 placement is
kernel-internal; the kernel places the Observer, and the Pulsar fires on
whatever core the Observer is running. If the Observer migrates, Pulsar
deadlines could migrate too (future optimization, not initial implementation).

**Dynamic deadline array growth.** D83 settled a hard cap at 32. A bounded array
avoids allocation on the timer interrupt path. CreatePulsar failure on a full
array is the correct resource-exhaustion signal.

**Eager routing table update on FieldSplit.** Considered deferring the
IrqRoutingTable update until the next interrupt delivery (lazy update on cache
miss). Rejected: the routing table is small (1024 entries) and the update is
infrequent (cold-path FieldSplit). Eager update avoids a window where interrupts
route to the old Field after the split has logically taken effect.
