# 081 — Hardware event protocol

**Date:** 2026-04-25

**Question:** How do timer interrupts and device IRQs flow through the dispatch
path?

**Rests on:** D2 (preemption), D22 (IRQ routing), D44 (Pulsar), D62/D63 (timer
fire message), D75 (KernelState), D76 (dispatch entry contract), D82 (global
state organization), D83 (per-core deadline data).

---

## Exploration

### The question

Two hardware event paths need concrete protocol definitions:

1. **Timer interrupts:** frame/ calls `core.handle_timer(current_ticks)` (D76).
   The core manager must check per-core Pulsar deadlines (D83), fire expired
   ones, rearm repeating Pulsars, remove one-shots, then do the usual
   on_preempt + schedule_next (D2).

2. **Device IRQs:** frame/ calls `core.handle_irq(intid)` (D76). The core
   manager must look up the INTID in an IRQ routing table, construct a message,
   deliver it to the target Field, then schedule_next.

Both paths compose existing primitives (Message, Field.enqueue, Pulsar
fire_message/rearm). The derivation settles the data structures that connect
them and the exact flow.

### IRQ routing: table structure

**Constraint from D22:** The kernel maintains an internal IRQ-to-Field routing
table. No separate IRQ object type. The interrupt namespace maps onto the Field
namespace.

**Constraint from implementation plan (Task 1.3):** Max IRQ count = 1024.
Direct-indexed by INTID. 16 KiB static array covers the full GICv3 SPI range.

**Route contents:** Each route needs:

- `field_id: ObjectId` — delivery target
- `badge: Badge` — per-IRQ identifier injected into message (D17)
- `generation: u64` — for stale-route detection (D67)

No ack cap in the route. D22 specifies a send-once ack cap per interrupt
message, but the ack cap is constructed per-delivery (like D16 reply caps), not
stored in the route.

**Location:** KernelState (D82). The IRQ routing table is kernel-wide shared
cold-path state. It does not participate in the Field-Observer-Pulsar lock
ordering chain (D53) — it gets its own unordered LockOrder::IrqRouting.

**Table type:**

```rust
pub struct IrqRoute {
    pub field_id: ObjectId,
    pub badge: Badge,
    pub generation: u64,
}

pub struct IrqRoutingTable {
    pub routes: [Option<IrqRoute>; 1024],
}
```

Direct-indexed: `routes[intid as usize]`. O(1) lookup. Simple, bounded,
deterministic. The hardware already constrains the space.

### IRQ message format

D22 says interrupt messages carry a badge and a send-once ack cap. The message
label needs a kernel-reserved value. Added `LABEL_DEVICE_IRQ` (provisional
0xFFFF_FFFF_FFFF_0007) alongside the existing D61/D63/D64 labels.

Message constructor: `Message::device_irq(badge, intid)` with data[0] = INTID.
The INTID in data[0] allows the driver to identify which specific interrupt
fired when multiple INTIDs route to the same Field with the same badge.

### handle_irq flow

```text
handle_irq(irq: u32, kernel_state: &KernelState) -> DispatchResult
  1. Acquire irq_routes lock.
  2. Lookup routes[irq]. If None: drop lock, return schedule_next().
  3. Copy route fields (field_id, badge, generation). Drop irq_routes lock.
  4. Acquire fields arena lock.
  5. Look up target Field by field_id. If freed: drop, schedule_next().
  6. Check live generation vs route generation (D67). If mismatch: stale, skip.
  7. Construct Message::device_irq(badge, irq).
  8. Enqueue. If full (D18): message dropped. IRQ stays masked.
  9. Drop fields lock.
  10. Return schedule_next().
```

Key design decisions:

- **Drop irq_routes lock before acquiring fields lock.** The two locks are both
  unordered, but holding both simultaneously is unnecessary. Copy the route data
  out (16 bytes), release, then acquire fields. Minimizes lock hold time on the
  interrupt path.

- **D18: dropped on full queue.** IRQ messages are edge-triggered notifications.
  The interrupt stays masked (D22: ack cap consumed to unmask). No pending list
  needed — unlike faults (D80), there is no Observer to link.

- **Stale route: silently ignored.** If the target Field was revoked, the route
  generation won't match. The interrupt is effectively orphaned. This is correct
  — the Field holder is responsible for maintaining their interrupt routes.

### Timer/Pulsar: deadline checking protocol

**Constraint from D83:** DeadlineEntry carries field_id, pulsar_id, badge,
deadline_ticks. The per-core deadline array is dense-packed, max 32 entries.
Scanned on every timer interrupt.

**Constraint from D44:** Pulsar.fire_message(actual_fire_ticks) constructs the
D63 message. Pulsar.rearm(counter_freq) does drift-compensated next deadline.
Pulsar.record_overrun() increments the overrun counter for full-queue cases.

### handle_timer flow

```text
handle_timer(current_ticks: u64, kernel_state: &KernelState, counter_freq: u64)
  1. For each deadline entry where deadline_ticks <= current_ticks:
     a. Acquire pulsars lock. Look up Pulsar by pulsar_id.
     b. If Pulsar freed: remove deadline (swap-remove), continue.
     c. Call pulsar.fire_message(current_ticks) -> Message.
     d. Acquire fields lock. Look up Field by field_id.
     e. Enqueue message. If full: pulsar.record_overrun() (D44).
     f. Drop fields lock.
     g. If repeating: pulsar.rearm(counter_freq). Update deadline entry
        with new next_deadline_ticks. Advance to next entry.
     h. If one-shot: drop pulsars lock. swap_remove_deadline. Don't advance
        (swapped entry needs checking).
  2. scheduler.on_preempt() (D2).
  3. Return schedule_next().
```

Key design decisions:

- **counter_freq as parameter.** Same rationale as current_ticks (D76): it's
  hardware state that should be pushed for testability, not pulled via frame/
  helper. The value is constant per boot (CNTFRQ_EL0), but passing it keeps
  handle_timer pure.

- **Overrun handling (D44).** When the delivery Field is full, the Pulsar's
  overrun_count is incremented. The fire message includes the overrun count. The
  next successful delivery reports accumulated overruns. This matches D44's
  "kernel stops re-arming" on overflow — but for repeating Pulsars, we still
  rearm to the next deadline. The overrun count tells the receiver how many
  fires were missed due to queue pressure.

- **Swap-remove during iteration.** When removing a one-shot deadline at index
  i, the last entry is swapped in and deadline_count decremented. The loop
  doesn't increment i, so the swapped entry is checked next. This maintains the
  D83 dense-packing invariant.

- **Lock acquisition order.** Within each deadline check: pulsars lock first (to
  get fire_message and check is_repeating), then fields lock (to enqueue). Both
  are unordered in D53, but the consistent order prevents potential issues.

### Signature changes

Both `handle_timer` and `handle_irq` now take `&KernelState` as a parameter.
`handle_timer` also takes `counter_freq: u64`.

This is an interface change from the original signatures:

- Old: `handle_timer(&mut self, current_ticks: u64) -> DispatchResult`
- New:
  `handle_timer(&mut self, current_ticks: u64, kernel_state: &KernelState, counter_freq: u64) -> DispatchResult`
- Old: `handle_irq(&mut self, irq: u32) -> DispatchResult`
- New:
  `handle_irq(&mut self, irq: u32, kernel_state: &KernelState) -> DispatchResult`

The change is forced: both methods need access to global arenas (fields for
message delivery, pulsars for rearm/overrun). D82 settled that arenas are
accessed via `kernel_state()` global function call, not CoreState fields. For
testability, the parameter is explicit rather than calling the global accessor
directly.

D76 already established that `current_ticks` is pushed as a parameter for
testability. `kernel_state` and `counter_freq` follow the same pattern.

---

## Settles

1. **IrqRoute struct:** `{field_id: ObjectId, badge: Badge, generation: u64}`.
   In `kernel_state.rs`.

2. **IrqRoutingTable:** `routes: [Option<IrqRoute>; 1024]`, direct-indexed by
   INTID. Methods: `lookup(intid) -> Option<&IrqRoute>`,
   `install(intid, route) -> Option<bool>`, `remove(intid) -> Option<IrqRoute>`.

3. **KernelState.irq_routes:** `Lock<IrqRoutingTable>` with
   `LockOrder::IrqRouting` (unordered).

4. **LABEL_DEVICE_IRQ:** Provisional `0xFFFF_FFFF_FFFF_0007`.

5. **Message::device_irq(badge, intid):** data[0] = INTID, no cap.

6. **handle_irq flow:** route lookup -> generation check -> message construction
   -> enqueue -> schedule_next.

7. **handle_timer flow:** deadline scan -> fire expired -> rearm repeating ->
   remove one-shots -> on_preempt -> schedule_next.

8. **swap_remove_deadline:** O(1) removal maintaining D83 dense-packing
   invariant.

9. **Signature changes:** Both handle_timer and handle_irq take &KernelState.
   handle_timer also takes counter_freq: u64. Forced by D82 (arenas in global
   state) and testability.

## Rejected alternatives

**IRQ routing in CoreState (per-core).** D22 says "kernel maintains an internal
IRQ→field routing table" — singular. IRQ routing is kernel-wide, not per-core.
An IRQ can be routed to any Field regardless of which core the driver Observer
runs on. A per-core table would require replication or indirection.

**Sorted deadline array / min-heap.** D83 already rejected this: with n <= 32,
linear scan is fast enough. A sorted structure complicates swap-remove for
negligible gain.

**Generation check in IrqRoutingTable.lookup().** Considered having the table
itself check generation against a callback, but the table doesn't have access to
the field arena. The caller (handle_irq) does the generation check after
acquiring the fields lock. Separation of concerns.

**handle_timer calling frame/ for counter_freq.** counter_freq is constant per
boot (CNTFRQ_EL0). Pushing it as a parameter keeps handle_timer pure and
testable, consistent with D76's rationale for current_ticks. frame/ reads it
once at exception entry and passes it down.

---

## Interface changes

- `handle_timer` signature: added `kernel_state: &KernelState` and
  `counter_freq: u64` parameters. Forced by D82 access pattern and D44 rearm.
- `handle_irq` signature: added `kernel_state: &KernelState` parameter. Forced
  by D82 access pattern.
- `KernelState` struct: added `irq_routes: Lock<IrqRoutingTable>` field. Extends
  D82 as anticipated (D82 journal: "Revisit if: IRQ routing table is added").
- `LockOrder` enum: added `IrqRouting = 6` variant. Unordered.
- `field.rs`: added `LABEL_DEVICE_IRQ` constant and `Message::device_irq`
  constructor.

No changes to settled interfaces D1-D83 beyond the additions above.

---

## Test

25 tests covering:

**IRQ routing table (7):** empty table, max IRQs constant, install + lookup,
overwrite, remove, out-of-range bounds, boundary INTIDs (0, 1023).

**handle_irq (3):** delivery to routed Field (message label + badge + INTID
verified), unrouted INTID ignored, generation mismatch skips delivery.

**handle_timer (4):** expired one-shot fires and removes, repeating Pulsar
rearms, non-expired deadline untouched, multiple expired fire all.

**swap_remove_deadline (2):** middle element removal with swap, last element
removal.

**KernelState integration (2):** irq_routes field accessible, lock order is
unordered.

**kernel_state.rs tests (6):** irq_routes acquirable, lock order correct,
roundtrip, MAX_IRQS, route size, acquirable alongside field arena lock.

## Reference check

Matches `.claude/implementation-plan-layers-1-3.md`:

- Task 1.3: IrqRoute struct, IrqRoutingTable, MAX_IRQS = 1024, direct-indexed,
  added to KernelState with unordered Lock. Implemented as specified.
- Task 1.7: handle_irq with route lookup, generation check, message
  construction, enqueue. Implemented as specified.
- Task 1.8: handle_timer with deadline checking, fire, rearm, remove.
  Implemented as specified. counter_freq parameter added (not in plan, forced by
  D44 rearm needing ns-to-ticks conversion).

## Status

**Settled.** The hardware event protocol composes D22 (IRQ routing), D44 (Pulsar
fire/rearm), D63 (fire message format), D82 (KernelState access), and D83
(per-core deadlines) into concrete runtime flows. No forks — all decisions are
forced by the constraint chain.

Revisit if: D22 is revised (changes interrupt delegation model), D44 is revised
(changes Pulsar fire/rearm semantics), D82 is revised (changes KernelState
access pattern), or if GICv4 virtual interrupt support changes the routing
model.
