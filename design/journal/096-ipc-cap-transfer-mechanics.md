# D96 — IPC cap transfer mechanics

**Question:** How do capabilities move between Observers during IPC? What
happens to the reply cap, the user cap, and slot allocation in the receiver's
table? What happens when the fast path is denied?

**Rests on:** D16 (reply mechanism — send-once cap), D28 (message format — 0-1
transferred caps), D30 (multi-Time — over-allocation invariant forces move), D37
(Time donation — explicit move statement), D43 (reply field at reserved slot 1),
D47 (register layout — x6 user cap, x7 reply cap), D50 (fast-path conditions —
no user cap for DirectSwitch), D51 (send-once flag on Entry, not a rights bit),
D74 (register pass-through on fast path), D78 (message ownership — WokeReceiver
returns Message), D8 (kernel-managed cap table), D40 (fault delegation for table
full).

**Status:** settled.

---

## Settles

### 1. Reply cap minting on Call

When an Observer performs Call (SVC #3, D48), the kernel creates a send-once
Entry pointing at the caller's reply Field. The mechanics:

1. **Identify reply Field.** The caller's cap table slot 1 (SLOT_REPLY_FIELD,
   D43) holds the reply Field cap. The kernel reads the ObjectId and generation
   from this entry.

2. **Construct send-once Entry.** The Entry has:
   - `object`: the reply Field's `(ObjectType::Field, object_id)`.
   - `rights`: full Field rights minus RECEIVE (the server should send a reply,
     not receive from the reply Field). Specifically: SEND is replaced by the
     send-once mechanism — the `send_once` flag substitutes for normal SEND.
   - `send_once`: true (D51 — flag on Entry, not a rights bit).
   - `badge`: the caller's `reply_badge` (D65 — caller-supplied, identifies
     which outstanding RPC is being answered).
   - `stored_generation`: the reply Field's current generation (D67).

3. **Allocate slot in receiver's table.** The kernel calls
   `Table::allocate_slot()` on the receiver's cap table. This picks the next
   free slot from the freelist head. If the table is full (no free slots),
   delivery cannot proceed — see "Cap slot allocation" below.

4. **Install Entry.** The send-once Entry is installed at the allocated slot.
   The receiver sees the reply cap handle (encoded as slot index + slot tag) in
   register x7 of the delivered message (D47).

The reply cap is ephemeral: consumed after one Send, removed from the receiver's
table (D51). The reply Field persists across RPCs (D16).

### 2. User cap transfer during IPC: move semantics

D28 settles that a message carries 0 or 1 user caps. When a cap is present (x6
!= CAP_ABSENT), the transfer uses move semantics: the cap is removed from the
sender's table and installed in the receiver's table.

Move semantics are forced by D30's over-allocation invariant. Consider Time
caps: if both sender and receiver held references to the same Time object after
a copy, the kernel's per-Observer compute aggregates would double-count the
Time's compute units. The scheduler would allocate more CPU time than physically
exists. D30's aggregate model requires that at any moment, each Time cap is held
by exactly one Observer. Copy semantics would violate this. Move is the only
correct transfer mode for Time, and D28's single cap-slot design applies
uniformly to all object types — no per-type transfer mode.

The protocol:

1. **Validate sender's cap.** Resolve the handle from x6 in the sender's
   registers. Bounds check, slot tag check (D11), generation check (D67), rights
   check.

2. **Remove from sender's table.** The Entry is removed from the sender's table
   (slot freed, freelist updated). The cap information is captured as a
   `TransferredCap` — the intermediate representation for a cap between tables.

3. **Allocate slot in receiver's table.** `Table::allocate_slot()` on the
   receiver's cap table. If full, the message cannot be delivered (see below).

4. **Install in receiver's table.** The `TransferredCap` fields are written into
   a new Entry at the allocated slot. The receiver sees the user cap handle in
   register x6 (D47).

The sender loses the cap regardless of delivery outcome — this is a move, not a
conditional transfer. If the receiver's table is full, the cap is not lost
silently; the fault mechanism handles the failure (see below).

### 3. Cap slot allocation for transfers

The kernel picks slots via `Table::allocate_slot()` — next free from the
freelist head. D8 settles that the kernel manages slot allocation; userspace has
no control over which slot receives an incoming cap. This is consistent with
D8's "handles are opaque integers; the kernel handles slot allocation."

When the receiver's table is full (no free slots), the message cannot be
delivered. The kernel cannot silently drop a message containing a cap (the cap
was already removed from the sender's table — dropping it would leak the
object). Instead:

1. **Fault the receiver.** D40's pager fault resolution protocol applies. The
   kernel generates a cap-table-full fault (LABEL_CAP_TABLE_FULL, D61) to the
   receiver's fault handler Field (slot 0).

2. **Handler provides Space.** The fault handler provides more Space for table
   growth (D8 table-full fault protocol, D32 — Space consumed as cap table
   backing).

3. **Kernel retries delivery.** After the table grows, the kernel retries slot
   allocation and message delivery.

This preserves the invariant that no cap is silently lost during IPC. The
sender's cap was removed; it must arrive at the receiver or be recoverable
through the fault path.

### 4. DirectSwitch denial fallback

D50 condition 5: the per-core scheduler's `should_switch_to(receiver)` callback
must approve the direct switch. When the scheduler denies the switch, the kernel
falls back to the slow path. The question is whether `CallOutcome` needs a new
variant for this case.

No enum change is needed. `CallOutcome::DirectSwitch` stays as-is (carries only
a `NonNull<Observer>`, no Message). When the scheduler denies:

1. **Data is in sender's RegisterState.** The sender's registers were saved into
   RegisterState at SVC entry (D74: EL0 exception entry saves directly into
   RegisterState). The data words (x0-x3), label (x4), badge (x5), and cap
   handles (x6, x7) are all in the sender's saved state.

2. **Dispatch constructs Message from saved registers.** The denial point in the
   dispatch path reads the sender's RegisterState via frame/ helpers and
   constructs a `Message` struct. This is a read-then-construct at the denial
   point, not a value carried through the CallOutcome enum.

3. **Slow-path delivery.** The constructed Message is enqueued into the Field's
   queue (the waiter was already popped, so the receiver is already unblocked).
   The scheduler's `enqueue(receiver)` and `pick_next()` determine who runs next
   (D79 row 6 denial path).

This avoids two alternatives:

- Carrying `Option<Message>` in DirectSwitch: wastes Message construction on
  every fast-path attempt, even successful ones. D78 explicitly chose not to
  carry a Message in DirectSwitch because D74 guarantees register pass-through.
- Adding a three-variant CallOutcome (DirectSwitch, Denied, Enqueued): the
  Denied variant would be structurally identical to WokeReceiverSlowPath. The
  denial path can reuse the existing slow-path delivery code with a Message
  constructed from saved registers.

---

## Rejected alternatives

### Copy semantics for cap transfer

Violates D30's over-allocation invariant for Time caps. If both sender and
receiver hold Time caps to the same object, the per-Observer compute aggregates
double-count. The scheduler would over-allocate CPU time. Move semantics are
forced for Time, and applying move uniformly to all types is simpler than
per-type transfer modes.

D37 explicitly states "explicit cap transfer" for Time donation on IPC — the
word "transfer" denotes move, not copy. The IPC path handles all five object
types identically through `TransferredCap`.

### Receiver pre-designates slot for incoming caps

D8 says the kernel manages slot allocation. Letting the receiver choose a slot
would expose table structure to userspace, require validation (is the slot
empty? is it within bounds?), and create a new class of errors (designated slot
occupied). The kernel picks the next free slot — O(1) from the freelist,
consistent with D8's opacity principle.

### DirectSwitch carries Option\<Message\>

D78 says "DirectSwitch carries only the observer pointer" and "no Message struct
needed because x0-x3 pass through in physical registers (D74)." Adding an
Option\<Message\> would construct the Message on the fast path even when the
scheduler approves — wasting cycles on the path that must be cheapest. The
denial case is rare (the scheduler usually approves same-core direct switch);
constructing the Message only at the denial point is the correct tradeoff.

### Three-variant CallOutcome with explicit Denied

Unnecessary complexity. The denied path needs: (a) the receiver pointer (already
popped from the waiters list), and (b) a Message constructed from the sender's
saved registers. This is exactly what WokeReceiverSlowPath handles — a woken
receiver with a Message for slow-path delivery. The dispatch code at the denial
point constructs the Message and proceeds through the slow-path delivery code,
which already handles enqueue + scheduler interaction (D79 row 6).

---

## Does NOT settle

- Cap installation during slow-path delivery (the receiver's Entry construction
  from TransferredCap — D8 downstream implementation).
- Reply cap creation timing within the Call dispatch path (before or after
  waiter check — ordering relative to the 0-cap gate, since the reply cap is
  kernel-created and does not count against D50 condition 4).
- Cross-core cap transfer (message enqueued on one core, received on another —
  the cap is in the queue's Message struct as a TransferredCap; installation
  happens on the receiving core at dequeue time).
- Cap transfer failure rollback (if receiver table allocation fails after sender
  cap removal — the TransferredCap must be held in kernel state during the fault
  resolution; exact mechanism deferred).
- Send-once cap consumption protocol (the kernel removes the send-once Entry
  from the sender's table after delivery — exact ordering relative to message
  enqueue deferred).
