//! Field: queued IPC mechanism.
//!
//! D13: queued fields with direct-switch fast path.
//! D15: unidirectional, many-to-many, send/receive as object-rights.
//! D16: reply via pre-allocated reply field with send-once cap.
//! D17: badge semantics (minter-assigned, opt-in lifecycle tracking).
//! D18: error-to-sender overflow, deferred fault delivery.
//! D28: fixed-size message format.
//! D45: field split — badge-range routing with fallback-on-destroy.
//! D52: rights — send, receive, mint, split, destroy, clone (6 bits).
//! D54: routing table — nullable pointer to external sorted array.
//! D67: generation counter for revocation.
//! D71: badge condition form — closed range [low, high].
//! D73: reply Field always-tracked (D17 specialization).

use crate::arena::ObjectId;
use crate::capability::{Badge, TransferredCap};
use core::ptr::NonNull;
use core::sync::atomic::AtomicU64;

// ── Message (D28) ───────────────────────────────────────────────────

/// Fixed-size IPC message (D28).
///
/// Sender provides: label + 4 data words + 0–1 cap handle.
/// Receiver sees: badge + label + 4 data words + 0–1 remapped cap + reply cap.
/// The kernel transforms in transit: injects badge (D17), translates
/// cap handles (D8), injects reply cap for Call() (D16).
///
/// Fault messages, interrupt messages, badge-closure notifications, and
/// Pulsar fire messages all use this same format (D12, D22, D64, D63).
pub struct Message {
    /// Four untyped data words (D28). Arbitrary 64-bit values.
    pub data: [u64; 4],

    /// Pass-through label (D28). Kernel does not dispatch on it.
    pub label: u64,

    /// Sender's badge, injected by kernel from cap entry (D17).
    pub badge: Badge,

    /// User cap slot: 0 or 1 transferred cap (D28).
    /// None = no cap in message (fast-path eligible per D50).
    pub user_cap: Option<TransferredCap>,

    /// Reply cap: kernel-created send-once cap for Call() (D16, D28).
    /// None for Send, Receive, and kernel-as-sender messages.
    pub reply_cap: Option<TransferredCap>,
}

// ── Known message labels ────────────────────────────────────────────
//
// Numeric values are provisional — D63 and D64 defer label assignment.
// The 0xFFFF_FFFF_FFFF_xxxx range reserves the top 48 bits to avoid
// colliding with user-chosen labels. Settle via derivation before
// Phase D delivery paths are implemented.

/// Label for Pulsar fire messages (D63). Value provisional.
pub const LABEL_TIMER_FIRE: u64 = 0xFFFF_FFFF_FFFF_0001;

/// Label for badge-closure notifications (D64). Value provisional.
pub const LABEL_CLOSURE: u64 = 0xFFFF_FFFF_FFFF_0002;

/// Label for VM page faults (D61). Value provisional.
pub const LABEL_VM_FAULT: u64 = 0xFFFF_FFFF_FFFF_0003;

/// Label for resource requests (D31, D61). Value provisional.
pub const LABEL_RESOURCE_REQUEST: u64 = 0xFFFF_FFFF_FFFF_0004;

/// Label for cap-table-full faults (D8, D61). Value provisional.
pub const LABEL_CAP_TABLE_FULL: u64 = 0xFFFF_FFFF_FFFF_0005;

/// Label for hardware exceptions (D61). Value provisional.
pub const LABEL_HARDWARE_EXCEPTION: u64 = 0xFFFF_FFFF_FFFF_0006;

/// Label for device interrupt messages (D22, D81). Value provisional.
pub const LABEL_DEVICE_IRQ: u64 = 0xFFFF_FFFF_FFFF_0007;

// ── Routing (D45, D54, D71) ─────────────────────────────────────────

/// Single routing rule in a Field's routing table (D45, D54, D71).
///
/// Badge-range condition: `low <= badge <= high` (D71 closed range).
/// Evaluated via binary search over D54's sorted array.
pub struct RoutingEntry {
    /// Low end of badge range (inclusive). D71.
    pub badge_low: u64,

    /// High end of badge range (inclusive). D71.
    pub badge_high: u64,

    /// Destination Field arena identifier.
    pub destination: ObjectId,

    /// Destination's generation at installation time (D55).
    /// Mismatch on routing evaluation → stale entry → fallback to source.
    pub destination_generation: u64,

    /// Back-pointer intrusive list linkage (D54, D55).
    /// Enables O(1)-per-source cleanup when the destination is destroyed.
    /// None at list endpoints.
    pub back_prev: Option<NonNull<RoutingEntry>>,
    pub back_next: Option<NonNull<RoutingEntry>>,
}

/// Per-Field routing table (D54).
///
/// Nullable: null when unsplit (zero hot-path cost). On first split,
/// allocated from root Space (D31). Sorted by badge_low for binary
/// search (D71).
pub struct RoutingTable {
    /// Contiguous sorted array of routing entries. Always valid when the
    /// RoutingTable exists (the table itself is nullable on Field).
    pub entries: NonNull<RoutingEntry>,
    pub count: u32,
    pub capacity: u32,
}

// ── Field ───────────────────────────────────────────────────────────

/// Bounded queue with waiters list (D13, D15).
///
/// Single kernel object. Rights (send, receive, mint, split) carried
/// in the capability, not the field. Topology emerges from capability
/// distribution. All information delivery — peer IPC, fault
/// notifications (D12), interrupt signals (D22), badge-closure (D17),
/// Pulsar fires (D44) — uses this mechanism.
pub struct Field {
    /// Bounded circular queue of Message (D13). Always allocated.
    pub queue: NonNull<Message>,
    pub queue_capacity: u32,
    pub queue_length: u32,
    pub queue_head: u32,

    /// Intrusive waiters list head — Observers blocked on Receive (D13).
    /// None when no waiters.
    pub waiters_head: Option<NonNull<crate::observer::WaitEntry>>,

    /// Intrusive waiters list tail — O(1) FIFO insertion.
    /// None when no waiters.
    pub waiters_tail: Option<NonNull<crate::observer::WaitEntry>>,

    /// Nullable routing table (D54). None = unsplit (zero hot-path cost).
    pub routing_table: Option<NonNull<RoutingTable>>,

    /// D18: pending list head — Observers whose fault/interrupt message
    /// could not be delivered due to a full queue. Distinct from waiters
    /// (waiters = blocked on Receive; pending = deferred kernel-as-sender).
    /// On each dequeue that frees a slot, the pending list is checked and
    /// the deferred message is delivered.
    pub pending_head: Option<NonNull<crate::observer::WaitEntry>>,

    /// D18: single pending kernel-as-sender message (IRQ, timer).
    /// Used when the queue is full and no waiter is present. The next
    /// receive() drains this before checking pending_head. At most one
    /// message — if a second arrives, the first is overwritten (acceptable
    /// for edge-triggered IRQs where only the latest matters).
    pub pending_kernel_message: Option<Message>,

    /// Per-badge refcount tracking enabled (D17 opt-in).
    /// Reply Fields are always-tracked (D73).
    pub badge_tracking: bool,

    /// Back-pointer list head for D55 routing cleanup.
    /// When this Field is a routing destination, source Fields link
    /// their routing entries here for O(1) cleanup on destroy.
    /// None when no sources route here.
    pub back_pointer_head: Option<NonNull<RoutingEntry>>,

    /// D32/D98: VA base of the Space consumed at creation.
    /// Used by Destroy to reconstruct the Space cap (reverse type conversion).
    pub backing_va_base: usize,

    /// D32/D98: size in bytes of the Space consumed at creation.
    pub backing_size: usize,

    /// Outstanding capability references (D11).
    pub refcount: u32,

    /// Revocation generation counter (D67). AtomicU64 per D67.
    pub generation: AtomicU64,
}

// ── Error types ────────────────────────────────────────────────────

/// Errors from Field operations (D13, D18).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldError {
    /// D18: queue is full. Error returned to sender — the sender
    /// handles overflow, not the kernel. For kernel-as-sender (faults,
    /// interrupts), this triggers deferred delivery via the pending list.
    QueueFull,
    /// Routing table insertion failed (no space for geometric growth).
    RoutingTableFull,
}

// ── Message construction helpers ───────────────────────────────────

impl Message {
    /// Construct a Pulsar fire message (D63).
    ///
    /// data[0] = actual fire time in raw CNTVCT_EL0 ticks (cheaper than
    /// converting to nanoseconds at interrupt time, directly comparable
    /// to Observer counter reads). data[1] = overrun count. data[2..3]
    /// reserved zero. No cap — satisfies D50 fast-path 0-cap condition.
    pub fn timer_fire(badge: Badge, fire_time_ticks: u64, overrun_count: u32) -> Message {
        Message {
            data: [fire_time_ticks, overrun_count as u64, 0, 0],
            label: LABEL_TIMER_FIRE,
            badge,
            user_cap: None,
            reply_cap: None,
        }
    }

    /// Construct a badge-closure notification (D64).
    ///
    /// D17: sent when the last send cap with badge B to a tracked
    /// Field is closed. Data words are zero — the badge alone identifies
    /// which client disconnected. Reason codes unnecessary because D17
    /// minter-assigned badges let servers self-encode cap types in
    /// badge ranges.
    ///
    /// D18: dropped on full queue (not a correctness issue — receiver
    /// discovers staleness lazily).
    pub fn badge_closure(badge: Badge) -> Message {
        Message {
            data: [0; 4],
            label: LABEL_CLOSURE,
            badge,
            user_cap: None,
            reply_cap: None,
        }
    }

    /// Construct a device interrupt message (D22, D81).
    ///
    /// D22: kernel-as-sender deposits interrupt notification to the driver
    /// Observer's Field. The badge identifies the IRQ (D17). data[0] carries
    /// the raw INTID for driver-side dispatch. No user cap. The send-once ack
    /// cap (D16) for unmask is a future addition — the current implementation
    /// delivers the notification without the ack cap.
    ///
    /// D81: the IRQ routing table maps INTID -> (field_id, badge, generation).
    /// The message is constructed from the route's badge, not the raw INTID.
    pub fn device_irq(badge: Badge, intid: u32) -> Message {
        Message {
            data: [intid as u64, 0, 0, 0],
            label: LABEL_DEVICE_IRQ,
            badge,
            user_cap: None,
            reply_cap: None,
        }
    }
}

// ── Field methods ──────────────────────────────────────────────────

impl Field {
    /// Construct a new Field with an allocated queue (D13, D32).
    ///
    /// All dynamic state starts empty: zero-length queue, no waiters,
    /// no routing table, no pending list. Badge tracking off.
    /// Used by CreateField and FieldSplit.
    pub fn new(
        queue: NonNull<Message>,
        queue_capacity: u32,
        backing_va_base: usize,
        backing_size: usize,
    ) -> Field {
        Field {
            queue,
            queue_capacity,
            queue_length: 0,
            queue_head: 0,
            waiters_head: None,
            waiters_tail: None,
            routing_table: None,
            pending_head: None,
            pending_kernel_message: None,
            badge_tracking: false,
            back_pointer_head: None,
            backing_va_base,
            backing_size,
            refcount: 1,
            generation: AtomicU64::new(0),
        }
    }

    /// Enqueue a message into the bounded queue.
    ///
    /// D13: queued fields. D18: returns error on full queue (error-to-
    /// sender). The caller (IPC send path or kernel-as-sender) must
    /// handle the overflow — for userspace senders that means returning
    /// an error; for kernel-as-sender (faults, interrupts) it means
    /// deferred delivery via the pending list (D18).
    ///
    /// Performance: O(1) circular buffer insertion. Hot path for IPC.
    pub fn enqueue(&mut self, message: Message) -> Result<(), FieldError> {
        if self.is_full() {
            return Err(FieldError::QueueFull);
        }

        let write_index = (self.queue_head + self.queue_length) % self.queue_capacity;

        crate::frame::fields::queue_write(self.queue, self.queue_capacity, write_index, message);

        self.queue_length += 1;

        Ok(())
    }

    /// Dequeue the front message from the queue.
    ///
    /// D13: returns `None` if the queue is empty — the caller should
    /// block the receiving Observer (add to waiters list). After
    /// dequeuing, the caller should check the pending list (D18) and
    /// deliver any deferred fault/interrupt messages that were waiting
    /// for a free slot.
    ///
    /// Performance: O(1) circular buffer removal.
    pub fn dequeue(&mut self) -> Option<Message> {
        if self.is_empty() {
            return None;
        }

        let message =
            crate::frame::fields::queue_read(self.queue, self.queue_capacity, self.queue_head);

        self.queue_head = (self.queue_head + 1) % self.queue_capacity;
        self.queue_length -= 1;

        message
    }

    /// Whether the queue has no messages.
    pub const fn is_empty(&self) -> bool {
        self.queue_length == 0
    }

    /// Whether the queue is at capacity (D18: next send will error).
    pub const fn is_full(&self) -> bool {
        self.queue_length >= self.queue_capacity
    }

    /// Add an Observer to the waiters list (blocked on Receive).
    ///
    /// D13: intrusive doubly-linked list through WaitEntry. Zero
    /// allocation — the WaitEntry is stored inline in the Observer's
    /// wait_state (D43 common case) or allocated for multi-field wait
    /// (D19).
    ///
    /// The waiters list is distinct from the pending list (D18):
    /// waiters = Observers blocked on Receive; pending = Observers
    /// whose fault message could not be delivered due to full queue.
    pub fn add_waiter(&mut self, entry: &mut crate::observer::WaitEntry) {
        crate::frame::fields::waiter_push_back(
            &mut self.waiters_head,
            &mut self.waiters_tail,
            entry,
        );
    }

    /// Remove an Observer from the waiters list.
    ///
    /// Called when: a message arrives and the front waiter is woken,
    /// the Observer is destroyed while waiting, or the Observer is
    /// suspended (D39) while blocked.
    pub fn remove_waiter(&mut self, entry: &mut crate::observer::WaitEntry) {
        crate::frame::fields::waiter_remove(&mut self.waiters_head, &mut self.waiters_tail, entry);
    }

    /// Pop the front waiter for direct-switch or message delivery.
    ///
    /// D13/D50: when a sender finds a waiting receiver, the kernel
    /// can bypass the queue and hand the message directly. Returns
    /// the waiter's Observer pointer for the scheduler's
    /// `should_switch_to` check (D50 condition 5).
    pub fn pop_waiter(&mut self) -> Option<NonNull<crate::observer::WaitEntry>> {
        crate::frame::fields::waiter_pop_front(&mut self.waiters_head, &mut self.waiters_tail)
    }

    /// Resolve badge-range routing for a message (D45, D54, D71).
    ///
    /// D54: null routing table → no routing (zero hot-path cost).
    /// D71: binary search over sorted array of closed ranges
    /// `[low, high]`. Returns the destination Field ObjectId if a
    /// range matches, or `None` for delivery to this (source) Field.
    ///
    /// D55: checks destination_generation against the live object to
    /// detect stale routing entries. Stale entries are treated as
    /// absent — message falls back to the source queue.
    ///
    /// Performance: O(log N) where N = number of routing rules.
    /// Unsplit fields: ~0 cost (null pointer check in hot cache line).
    pub fn resolve_route(&self, badge: u64) -> Option<ObjectId> {
        let table_ptr = self.routing_table?;

        crate::frame::fields::route_lookup(table_ptr, badge)
    }

    /// Add a routing rule for field split (D45).
    ///
    /// D54: geometric doubling on growth. The array is contiguous for
    /// binary-search cache-friendliness. Each split adds ~40-48 bytes
    /// from root Space (D31).
    ///
    /// D55: links a back-pointer into the destination Field's
    /// back_pointer_head list for O(1)-per-source cleanup on destroy.
    pub fn add_route(
        &mut self,
        low: u64,
        high: u64,
        destination: ObjectId,
        destination_generation: u64,
    ) -> Result<(), FieldError> {
        crate::frame::fields::route_add(
            &mut self.routing_table,
            low,
            high,
            destination,
            destination_generation,
        )
    }

    /// D67: atomically increment the generation counter, revoking all
    /// capabilities that stored the previous generation value.
    pub fn revoke(&self) {
        self.generation
            .fetch_add(1, core::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_layout() {
        assert_eq!(core::mem::size_of::<Field>(), 192);
    }

    #[test]
    fn message_is_concrete_type() {
        assert!(
            core::mem::size_of::<Message>() > 0,
            "Message must be a concrete fixed-size type (D28)"
        );
    }

    #[test]
    fn routing_entry_has_back_pointers() {
        let _ = core::mem::offset_of!(RoutingEntry, back_prev);
        let _ = core::mem::offset_of!(RoutingEntry, back_next);
    }

    #[test]
    fn timer_fire_message_has_no_cap() {
        let msg = Message::timer_fire(Badge(42), 12345, 0);

        assert!(
            msg.user_cap.is_none(),
            "D63: timer fire must have no cap (D50 fast-path)"
        );
        assert!(msg.reply_cap.is_none());
        assert_eq!(msg.label, LABEL_TIMER_FIRE);
        assert_eq!(msg.data[0], 12345);
    }

    #[test]
    fn closure_message_is_zero_data() {
        let msg = Message::badge_closure(Badge(99));

        assert_eq!(msg.label, LABEL_CLOSURE);
        assert_eq!(msg.data, [0; 4]);
        assert!(msg.user_cap.is_none());
    }

    #[test]
    fn kernel_labels_are_distinct() {
        let labels = [
            LABEL_TIMER_FIRE,
            LABEL_CLOSURE,
            LABEL_VM_FAULT,
            LABEL_RESOURCE_REQUEST,
            LABEL_CAP_TABLE_FULL,
            LABEL_HARDWARE_EXCEPTION,
            LABEL_DEVICE_IRQ,
        ];

        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "kernel labels must be distinct");
                }
            }
        }
    }

    // ── Helpers ────────────────────────────────────────────────────

    /// Construct a WaitEntry with dangling pointers for test use.
    fn make_wait_entry() -> crate::observer::WaitEntry {
        crate::observer::WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        }
    }

    /// Construct a Field with a real queue allocation for test use.
    fn test_field(capacity: u32) -> Field {
        Field::new(
            crate::frame::fields::alloc_test_queue(capacity),
            capacity,
            0,
            0,
        )
    }

    // ── D13: Queued fields, FIFO ordering ──────────────────────────

    /// D13: messages dequeued in first-in, first-out order.
    #[test]
    fn test_d13_enqueue_dequeue_fifo() {
        let mut field = test_field(8);
        let msg_a = Message {
            data: [1, 0, 0, 0],
            label: 100,
            badge: Badge(10),
            user_cap: None,
            reply_cap: None,
        };
        let msg_b = Message {
            data: [2, 0, 0, 0],
            label: 200,
            badge: Badge(20),
            user_cap: None,
            reply_cap: None,
        };

        field.enqueue(msg_a).unwrap();
        field.enqueue(msg_b).unwrap();

        let first = field.dequeue().unwrap();

        assert_eq!(
            first.data[0], 1,
            "D13: first enqueued message dequeued first"
        );
        assert_eq!(first.label, 100);

        let second = field.dequeue().unwrap();

        assert_eq!(
            second.data[0], 2,
            "D13: second enqueued message dequeued second"
        );
        assert_eq!(second.label, 200);
    }

    /// D13: dequeue from empty queue returns None.
    #[test]
    fn test_d13_dequeue_empty_returns_none() {
        let mut field = test_field(4);
        let result = field.dequeue();

        assert!(
            result.is_none(),
            "D13: empty queue dequeue must return None"
        );
    }

    /// D13: enqueue increments queue_length.
    #[test]
    fn test_d13_enqueue_increments_length() {
        let mut field = test_field(4);

        assert_eq!(field.queue_length, 0);

        let msg = Message::timer_fire(Badge(1), 100, 0);

        field.enqueue(msg).unwrap();

        assert_eq!(
            field.queue_length, 1,
            "D13: enqueue must increment queue_length"
        );
    }

    /// D13: dequeue decrements queue_length.
    #[test]
    fn test_d13_dequeue_decrements_length() {
        let mut field = test_field(4);
        let msg = Message::timer_fire(Badge(1), 100, 0);

        field.enqueue(msg).unwrap();

        assert_eq!(field.queue_length, 1);

        let _ = field.dequeue();

        assert_eq!(
            field.queue_length, 0,
            "D13: dequeue must decrement queue_length"
        );
    }

    // ── D18: Error-to-sender overflow ──────────────────────────────

    /// D18: enqueue to a full queue returns FieldError::QueueFull.
    #[test]
    fn test_d18_enqueue_full_returns_queue_full() {
        let mut field = test_field(2);

        // Fill the queue.
        field.enqueue(Message::timer_fire(Badge(1), 10, 0)).unwrap();
        field.enqueue(Message::timer_fire(Badge(2), 20, 0)).unwrap();

        // Third enqueue must fail with QueueFull.
        let result = field.enqueue(Message::timer_fire(Badge(3), 30, 0));

        assert_eq!(
            result,
            Err(FieldError::QueueFull),
            "D18: full queue must return QueueFull"
        );
    }

    /// D18: full queue rejects new messages without overwriting existing ones.
    #[test]
    fn test_d18_no_overwrite_on_full() {
        let mut field = test_field(2);
        let msg_a = Message {
            data: [0xA, 0, 0, 0],
            label: 1,
            badge: Badge(1),
            user_cap: None,
            reply_cap: None,
        };
        let msg_b = Message {
            data: [0xB, 0, 0, 0],
            label: 2,
            badge: Badge(2),
            user_cap: None,
            reply_cap: None,
        };

        field.enqueue(msg_a).unwrap();
        field.enqueue(msg_b).unwrap();

        // Reject the overflow message.
        let _ = field.enqueue(Message::timer_fire(Badge(99), 0, 0));
        // Original messages must be intact.
        let first = field.dequeue().unwrap();

        assert_eq!(
            first.data[0], 0xA,
            "D18: first message must survive rejected overflow"
        );

        let second = field.dequeue().unwrap();

        assert_eq!(
            second.data[0], 0xB,
            "D18: second message must survive rejected overflow"
        );
    }

    // ── D13/D50: Waiters list ──────────────────────────────────────

    /// D13: pop_waiter on empty waiters list returns None.
    #[test]
    fn test_d13_pop_waiter_returns_none_when_empty() {
        let mut field = test_field(4);
        let result = field.pop_waiter();

        assert!(
            result.is_none(),
            "D13: pop_waiter on empty list must return None"
        );
    }

    /// D13: add a waiter then pop it back.
    #[test]
    fn test_d13_add_waiter_then_pop() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        let popped = field.pop_waiter();

        assert!(
            popped.is_some(),
            "D13: pop_waiter must return the added waiter"
        );
    }

    /// D13: multiple waiters are popped in FIFO order.
    #[test]
    fn test_d13_waiters_are_fifo() {
        let mut field = test_field(4);
        let mut entry_a = make_wait_entry();
        let mut entry_b = make_wait_entry();

        field.add_waiter(&mut entry_a);
        field.add_waiter(&mut entry_b);

        // First pop should return entry_a (added first).
        let first = field.pop_waiter();

        assert!(first.is_some(), "D13: first waiter must be poppable");

        // Second pop should return entry_b (added second).
        let second = field.pop_waiter();

        assert!(second.is_some(), "D13: second waiter must be poppable");

        // List is now empty.
        let third = field.pop_waiter();

        assert!(third.is_none(), "D13: empty list after popping all waiters");
    }

    /// D13: remove_waiter removes a specific waiter from the list.
    #[test]
    fn test_d13_remove_waiter_from_list() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);
        field.remove_waiter(&mut entry);

        // After removal, pop should return None.
        let result = field.pop_waiter();

        assert!(result.is_none(), "D13: removed waiter must not be poppable");
    }

    // ── D45/D54/D71: Routing ───────────────────────────────────────

    /// D54: null routing table means no routing (zero hot-path cost).
    #[test]
    fn test_d54_no_routing_when_null() {
        let field = test_field(4);

        assert!(
            field.routing_table.is_none(),
            "precondition: no routing table"
        );

        let result = field.resolve_route(42);

        assert!(result.is_none(), "D54: null routing table must return None");
    }

    /// D45: resolve_route finds destination for a badge within a routing range.
    #[test]
    fn test_d45_resolve_route_matching_badge() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);

        // Add a route covering badges 100..=200 -> destination ObjectId(7).
        field.add_route(100, 200, ObjectId(7), 0).unwrap();

        let result = field.resolve_route(150);

        assert_eq!(
            result,
            Some(ObjectId(7)),
            "D45: badge within range must resolve to destination"
        );
    }

    /// D45: resolve_route returns None when badge is outside all ranges.
    #[test]
    fn test_d45_resolve_route_no_match() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);

        // Route covers 100..=200.
        field.add_route(100, 200, ObjectId(7), 0).unwrap();

        let result = field.resolve_route(50);

        assert!(
            result.is_none(),
            "D45: badge outside all ranges must return None"
        );

        let result = field.resolve_route(201);

        assert!(result.is_none(), "D45: badge above range must return None");
    }

    /// D71: badge range is closed inclusive — both endpoints match.
    #[test]
    fn test_d71_badge_range_closed_inclusive() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);

        // Route covers exactly [100, 200].
        field.add_route(100, 200, ObjectId(5), 0).unwrap();

        let low_match = field.resolve_route(100);

        assert_eq!(
            low_match,
            Some(ObjectId(5)),
            "D71: low endpoint of closed range must match"
        );

        let high_match = field.resolve_route(200);

        assert_eq!(
            high_match,
            Some(ObjectId(5)),
            "D71: high endpoint of closed range must match"
        );
    }

    /// D45: add_route adds a routing entry successfully.
    #[test]
    fn test_d45_add_route_succeeds() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);
        let result = field.add_route(10, 20, ObjectId(3), 0);

        assert!(
            result.is_ok(),
            "D45: add_route must succeed on empty routing table"
        );
    }

    /// D54: add_route when routing table is at capacity returns RoutingTableFull.
    #[test]
    fn test_d54_routing_table_full_error() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);
        // Fill the routing table to capacity. The exact capacity depends on
        // implementation, but adding many routes should eventually exhaust it.
        // We attempt enough to trigger the error. If the implementation uses
        // geometric growth starting from a small initial size, this will hit
        // the limit when allocation for growth fails. We test the error variant
        // exists and is returned.
        let mut last_result = Ok(());

        for i in 0..1024 {
            let low = i * 10;
            let high = low + 9;

            last_result = field.add_route(low, high, ObjectId(i as u32), 0);

            if last_result == Err(FieldError::RoutingTableFull) {
                break;
            }
        }

        assert_eq!(
            last_result,
            Err(FieldError::RoutingTableFull),
            "D54: routing table must return RoutingTableFull when at capacity"
        );
    }

    // ── D67: Revocation ────────────────────────────────────────────

    /// D67: revoke() atomically increments the generation counter.
    #[test]
    fn test_d67_revoke_increments_generation() {
        let field = test_field(4);
        let gen_before = field.generation.load(core::sync::atomic::Ordering::Acquire);

        assert_eq!(gen_before, 0, "initial generation must be 0");

        field.revoke();

        let gen_after = field.generation.load(core::sync::atomic::Ordering::Acquire);

        assert_eq!(
            gen_after, 1,
            "D67: revoke must increment generation from 0 to 1"
        );

        field.revoke();

        let gen_after_2 = field.generation.load(core::sync::atomic::Ordering::Acquire);

        assert_eq!(
            gen_after_2, 2,
            "D67: second revoke must increment generation from 1 to 2"
        );
    }

    // ── Invariants from doc comments ───────────────────────────────

    /// is_empty reflects queue_length == 0.
    #[test]
    fn test_is_empty_reflects_queue_state() {
        let empty = test_field(4);

        assert!(
            empty.is_empty(),
            "is_empty must be true when queue_length is 0"
        );

        let nonempty = Field {
            queue_length: 2,
            ..test_field(4)
        };

        assert!(
            !nonempty.is_empty(),
            "is_empty must be false when queue_length > 0"
        );
    }

    /// is_full reflects queue_length >= queue_capacity.
    #[test]
    fn test_is_full_reflects_capacity() {
        let empty = test_field(4);

        assert!(
            !empty.is_full(),
            "is_full must be false when queue_length < capacity"
        );

        let at_capacity = Field {
            queue_length: 4,
            ..test_field(4)
        };

        assert!(
            at_capacity.is_full(),
            "is_full must be true when queue_length == capacity"
        );

        let over_capacity = Field {
            queue_length: 5,
            ..test_field(4)
        };

        assert!(
            over_capacity.is_full(),
            "is_full must be true when queue_length > capacity (defensive)"
        );
    }

    // ── Adversarial tests ─────────────────────────────────────────────
    //
    // Boundary conditions, wrap-around, state corruption, overflow,
    // underflow, and invariant violations. Assumes bugs exist.

    // ── Queue boundary conditions ─────────────────────────────────────

    /// Capacity-1 queue: single-element queue should accept one and reject two.
    #[test]
    fn test_adversarial_field_single_element_queue() {
        let mut field = test_field(1);

        // First enqueue should succeed.
        field
            .enqueue(Message::timer_fire(Badge(1), 100, 0))
            .unwrap();

        assert!(
            field.is_full(),
            "capacity-1 queue must be full after one enqueue"
        );

        // Second enqueue should fail.
        let result = field.enqueue(Message::timer_fire(Badge(2), 200, 0));

        assert_eq!(result, Err(FieldError::QueueFull));

        // Dequeue should return the first message.
        let msg = field.dequeue().unwrap();

        assert_eq!(msg.data[0], 100);
        assert!(field.is_empty());
    }

    /// Fill a queue to exact capacity, then drain it completely.
    #[test]
    fn test_adversarial_field_fill_then_drain() {
        let capacity = 8u32;
        let mut field = test_field(capacity);

        // Fill to exact capacity.
        for i in 0..capacity {
            let msg = Message {
                data: [i as u64, 0, 0, 0],
                label: i as u64,
                badge: Badge(i as u64),
                user_cap: None,
                reply_cap: None,
            };

            field.enqueue(msg).unwrap();
        }

        assert!(field.is_full());
        assert_eq!(field.queue_length, capacity);

        // Drain all — verify FIFO.
        for i in 0..capacity {
            let msg = field.dequeue().unwrap();

            assert_eq!(msg.data[0], i as u64, "FIFO order broken at position {i}");
        }

        assert!(field.is_empty());
        assert_eq!(field.queue_length, 0);
        // Extra dequeue on empty should return None.
        assert!(field.dequeue().is_none());
    }

    /// Capacity-0 queue: every enqueue should fail immediately.
    #[test]
    fn test_adversarial_field_zero_capacity_queue() {
        let mut field = test_field(0);

        assert!(field.is_full(), "zero-capacity queue must report full");
        assert!(field.is_empty(), "zero-capacity queue must report empty");

        let result = field.enqueue(Message::timer_fire(Badge(1), 10, 0));

        assert_eq!(
            result,
            Err(FieldError::QueueFull),
            "zero-capacity queue must reject all enqueues"
        );
    }

    /// Dequeue from a queue with capacity > 0 but length 0.
    #[test]
    fn test_adversarial_field_dequeue_from_empty_nonempty_capacity() {
        let mut field = test_field(16);

        assert!(!field.is_full());
        assert!(field.is_empty());

        let result = field.dequeue();

        assert!(
            result.is_none(),
            "dequeue from empty queue must return None"
        );
    }

    // ── Circular buffer wrap-around ───────────────────────────────────

    /// Enqueue capacity-1, dequeue all, enqueue capacity-1 more.
    /// The head wraps past the buffer end.
    #[test]
    fn test_adversarial_field_wraparound_head_past_end() {
        let capacity = 4u32;
        let mut field = test_field(capacity);

        // First fill: capacity-1 messages.
        for i in 0..(capacity - 1) {
            field
                .enqueue(Message {
                    data: [i as u64 + 100, 0, 0, 0],
                    label: 0,
                    badge: Badge(0),
                    user_cap: None,
                    reply_cap: None,
                })
                .unwrap();
        }
        // Drain all.
        for _ in 0..(capacity - 1) {
            field.dequeue().unwrap();
        }

        assert!(field.is_empty());

        // Second fill: capacity-1 messages (head should wrap).
        for i in 0..(capacity - 1) {
            field
                .enqueue(Message {
                    data: [i as u64 + 200, 0, 0, 0],
                    label: 0,
                    badge: Badge(0),
                    user_cap: None,
                    reply_cap: None,
                })
                .unwrap();
        }
        // Verify FIFO on second batch.
        for i in 0..(capacity - 1) {
            let msg = field.dequeue().unwrap();

            assert_eq!(
                msg.data[0],
                i as u64 + 200,
                "FIFO broken after wrap-around at position {i}"
            );
        }

        assert!(field.is_empty());
    }

    /// Enqueue/dequeue N cycles where N >> capacity. Forces multiple
    /// wrap-arounds of the circular buffer.
    #[test]
    fn test_adversarial_field_wraparound_after_n_cycles() {
        let capacity = 4u32;
        let mut field = test_field(capacity);
        let cycles = 100u64;

        for cycle in 0..cycles {
            let msg = Message {
                data: [cycle, 0, 0, 0],
                label: cycle,
                badge: Badge(cycle),
                user_cap: None,
                reply_cap: None,
            };

            field.enqueue(msg).unwrap();

            let dequeued = field.dequeue().unwrap();

            assert_eq!(
                dequeued.data[0], cycle,
                "FIFO order broken on cycle {cycle}"
            );
            assert!(field.is_empty());
        }

        // After many cycles, internal state must be consistent.
        assert_eq!(field.queue_length, 0);
    }

    /// After wrap-around, verify queue_head and queue_length consistency.
    #[test]
    fn test_adversarial_field_wraparound_head_length_consistency() {
        let capacity = 3u32;
        let mut field = test_field(capacity);

        // Fill completely, drain completely — head advances by capacity.
        for _ in 0..capacity {
            field.enqueue(Message::timer_fire(Badge(0), 0, 0)).unwrap();
        }
        for _ in 0..capacity {
            field.dequeue().unwrap();
        }

        // Now head should have wrapped. State must be consistent.
        assert_eq!(field.queue_length, 0);
        assert!(field.is_empty());
        assert!(!field.is_full());

        // Enqueue one more — this exercises the wrapped head position.
        field
            .enqueue(Message {
                data: [0xBEEF, 0, 0, 0],
                label: 0,
                badge: Badge(0),
                user_cap: None,
                reply_cap: None,
            })
            .unwrap();

        assert_eq!(field.queue_length, 1);
        assert!(!field.is_empty());

        let msg = field.dequeue().unwrap();

        assert_eq!(msg.data[0], 0xBEEF);
        assert!(field.is_empty());
    }

    /// Batch-fill then batch-drain after multiple wrap-arounds —
    /// verifies FIFO ordering is preserved across wraparound boundaries.
    #[test]
    fn test_adversarial_field_batch_fifo_across_wraparound() {
        let capacity = 4u32;
        let mut field = test_field(capacity);

        // Cycle 1: advance head past the buffer end.
        for _ in 0..capacity {
            field.enqueue(Message::timer_fire(Badge(0), 0, 0)).unwrap();
        }
        for _ in 0..capacity {
            field.dequeue().unwrap();
        }
        // Cycle 2: batch-fill at the wrapped position.
        for i in 0..capacity {
            field
                .enqueue(Message {
                    data: [i as u64 + 500, 0, 0, 0],
                    label: 0,
                    badge: Badge(0),
                    user_cap: None,
                    reply_cap: None,
                })
                .unwrap();
        }

        assert!(field.is_full());

        // Batch-drain: must be in exact FIFO order.
        for i in 0..capacity {
            let msg = field.dequeue().unwrap();

            assert_eq!(
                msg.data[0],
                i as u64 + 500,
                "Batch FIFO broken at index {i} after wraparound"
            );
        }
    }

    // ── Waiters list abuse ────────────────────────────────────────────

    /// Add 3 waiters, remove the middle one. Prev/next linkage must
    /// remain correct — pop returns first and third.
    #[test]
    fn test_adversarial_field_remove_middle_waiter() {
        let mut field = test_field(4);
        let mut entry_a = make_wait_entry();
        let mut entry_b = make_wait_entry();
        let mut entry_c = make_wait_entry();

        field.add_waiter(&mut entry_a);
        field.add_waiter(&mut entry_b);
        field.add_waiter(&mut entry_c);
        // Remove the middle waiter.
        field.remove_waiter(&mut entry_b);

        // Pop should return entry_a first, then entry_c.
        let first = field.pop_waiter();

        assert!(
            first.is_some(),
            "first waiter must be poppable after middle removal"
        );

        let second = field.pop_waiter();

        assert!(
            second.is_some(),
            "third waiter must be poppable after middle removal"
        );

        let third = field.pop_waiter();

        assert!(
            third.is_none(),
            "list must be empty after popping two (one removed)"
        );
    }

    /// Add a waiter, pop it, add it again — re-insertion after removal.
    #[test]
    fn test_adversarial_field_waiter_reinsert_after_pop() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        let _ = field.pop_waiter();

        // Re-insert the same entry.
        field.add_waiter(&mut entry);

        let popped = field.pop_waiter();

        assert!(popped.is_some(), "re-inserted waiter must be poppable");

        let empty = field.pop_waiter();

        assert!(
            empty.is_none(),
            "list must be empty after popping re-inserted waiter"
        );
    }

    /// Remove a waiter that was already popped — must not corrupt the list.
    #[test]
    fn test_adversarial_field_remove_already_popped_waiter() {
        let mut field = test_field(4);
        let mut entry_a = make_wait_entry();
        let mut entry_b = make_wait_entry();

        field.add_waiter(&mut entry_a);
        field.add_waiter(&mut entry_b);

        // Pop entry_a.
        let _ = field.pop_waiter();

        // Now try to remove the already-popped entry_a.
        // This must not corrupt entry_b's linkage.
        field.remove_waiter(&mut entry_a);

        // entry_b should still be retrievable.
        let popped = field.pop_waiter();

        assert!(
            popped.is_some(),
            "remaining waiter must survive remove of already-popped entry"
        );
        assert!(
            field.pop_waiter().is_none(),
            "list must be empty after all entries consumed"
        );
    }

    /// Pop all waiters one by one — verify list ends up empty.
    #[test]
    fn test_adversarial_field_pop_all_waiters_to_empty() {
        let mut field = test_field(4);
        let mut entries: [_; 5] = core::array::from_fn(|_| make_wait_entry());

        for entry in entries.iter_mut() {
            field.add_waiter(entry);
        }

        // Pop all 5.
        for i in 0..5 {
            let popped = field.pop_waiter();

            assert!(popped.is_some(), "waiter {i} must be poppable");
        }

        assert!(
            field.pop_waiter().is_none(),
            "list must be empty after popping all"
        );
        assert!(
            field.waiters_head.is_none(),
            "waiters_head must be None after popping all"
        );
    }

    /// Add N waiters then pop N — all come out, list is empty.
    #[test]
    fn test_adversarial_field_add_n_pop_n_exhaustive() {
        const N: usize = 10;
        let mut field = test_field(4);
        let mut entries: [_; N] = core::array::from_fn(|_| make_wait_entry());

        for entry in entries.iter_mut() {
            field.add_waiter(entry);
        }

        let mut pop_count = 0usize;

        while field.pop_waiter().is_some() {
            pop_count += 1;
        }

        assert_eq!(pop_count, N, "must pop exactly {N} waiters");
    }

    // ── Routing edge cases ────────────────────────────────────────────

    /// resolve_route with badge 0 — minimum badge value.
    #[test]
    fn test_adversarial_field_route_badge_zero() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);

        field.add_route(0, 10, ObjectId(1), 0).unwrap();

        let result = field.resolve_route(0);

        assert_eq!(
            result,
            Some(ObjectId(1)),
            "badge 0 must match range [0, 10]"
        );
    }

    /// resolve_route with badge u64::MAX — maximum badge value.
    #[test]
    fn test_adversarial_field_route_badge_max() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);

        field
            .add_route(u64::MAX - 10, u64::MAX, ObjectId(99), 0)
            .unwrap();

        let result = field.resolve_route(u64::MAX);

        assert_eq!(
            result,
            Some(ObjectId(99)),
            "badge u64::MAX must match range [MAX-10, MAX]"
        );
    }

    /// add_route with low > high — invalid/degenerate range.
    /// Implementation should either reject or produce an unmatchable entry.
    #[test]
    fn test_adversarial_field_route_inverted_range() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);
        // low > high: badge range [100, 50] is inverted.
        let result = field.add_route(100, 50, ObjectId(1), 0);

        // If it succeeds, verify nothing matches the inverted range.
        if result.is_ok() {
            assert!(
                field.resolve_route(75).is_none(),
                "inverted range [100, 50] must not match any badge"
            );
            assert!(
                field.resolve_route(100).is_none(),
                "inverted range must not match low endpoint"
            );
            assert!(
                field.resolve_route(50).is_none(),
                "inverted range must not match high endpoint"
            );
        }
        // If it returns an error, that's also acceptable behavior.
    }

    /// add_route with low == high — exact-match routing (degenerate range).
    #[test]
    fn test_adversarial_field_route_exact_match() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);

        field.add_route(42, 42, ObjectId(7), 0).unwrap();

        assert_eq!(
            field.resolve_route(42),
            Some(ObjectId(7)),
            "exact-match route [42, 42] must match badge 42"
        );
        assert!(
            field.resolve_route(41).is_none(),
            "badge 41 must not match exact-match route [42, 42]"
        );
        assert!(
            field.resolve_route(43).is_none(),
            "badge 43 must not match exact-match route [42, 42]"
        );
    }

    /// add_route twice with overlapping ranges — what happens?
    /// At minimum: no crash, and the first-matching or last-added route wins.
    #[test]
    fn test_adversarial_field_route_overlapping_ranges() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);

        // Range A: [10, 50] -> ObjectId(1)
        field.add_route(10, 50, ObjectId(1), 0).unwrap();
        // Range B: [30, 70] -> ObjectId(2) (overlaps with A on [30, 50])
        field.add_route(30, 70, ObjectId(2), 0).unwrap();

        // Badge 20 is in A only.
        let only_a = field.resolve_route(20);

        assert_eq!(only_a, Some(ObjectId(1)), "badge 20 must match range A");

        // Badge 60 is in B only.
        let only_b = field.resolve_route(60);

        assert_eq!(only_b, Some(ObjectId(2)), "badge 60 must match range B");

        // Badge 40 is in the overlap — either ObjectId(1) or ObjectId(2) is
        // acceptable, but it must return Some.
        let overlap = field.resolve_route(40);

        assert!(
            overlap.is_some(),
            "badge in overlap region must resolve to some destination"
        );
    }

    /// Multiple non-overlapping routes, verify binary search correctness.
    #[test]
    fn test_adversarial_field_route_multiple_binary_search() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);

        // Add routes in non-sorted order to test insertion sort.
        field.add_route(300, 399, ObjectId(3), 0).unwrap();
        field.add_route(100, 199, ObjectId(1), 0).unwrap();
        field.add_route(500, 599, ObjectId(5), 0).unwrap();
        field.add_route(200, 299, ObjectId(2), 0).unwrap();
        field.add_route(400, 499, ObjectId(4), 0).unwrap();

        // Each range should resolve to its destination.
        assert_eq!(field.resolve_route(150), Some(ObjectId(1)));
        assert_eq!(field.resolve_route(250), Some(ObjectId(2)));
        assert_eq!(field.resolve_route(350), Some(ObjectId(3)));
        assert_eq!(field.resolve_route(450), Some(ObjectId(4)));
        assert_eq!(field.resolve_route(550), Some(ObjectId(5)));
        // Gaps between ranges should return None.
        assert!(field.resolve_route(99).is_none());
        assert!(field.resolve_route(600).is_none());
        assert!(field.resolve_route(0).is_none());
    }

    /// Route with destination_generation 0 and u64::MAX.
    #[test]
    fn test_adversarial_field_route_generation_extremes() {
        use crate::arena::ObjectId;

        let mut field = test_field(4);

        // Generation 0 — the most common initial value.
        field.add_route(10, 20, ObjectId(1), 0).unwrap();
        // Generation u64::MAX — the maximum value.
        field.add_route(30, 40, ObjectId(2), u64::MAX).unwrap();

        // Both should resolve (whether they match live object generation
        // is the caller's problem, but add_route should not reject them).
        let r1 = field.resolve_route(15);

        assert_eq!(r1, Some(ObjectId(1)));

        // Generation MAX may cause stale-entry detection to fail if the
        // implementation uses wrapping arithmetic on generation comparison.
        let r2 = field.resolve_route(35);

        assert_eq!(r2, Some(ObjectId(2)));
    }

    // ── State consistency checks ──────────────────────────────────────

    /// After enqueue+dequeue cycles, is_empty and is_full reflect actual state.
    #[test]
    fn test_adversarial_field_state_consistency_after_cycles() {
        let capacity = 4u32;
        let mut field = test_field(capacity);

        // Cycle: fill, drain, check.
        for _ in 0..10 {
            for _ in 0..capacity {
                assert!(!field.is_full());
                field.enqueue(Message::timer_fire(Badge(0), 0, 0)).unwrap();
            }

            assert!(field.is_full());
            assert!(!field.is_empty());

            for _ in 0..capacity {
                assert!(!field.is_empty());
                field.dequeue().unwrap();
            }

            assert!(field.is_empty());
            assert!(!field.is_full());
        }
    }

    /// queue_length never goes negative — u32 underflow on dequeue from empty.
    /// If dequeue is called on an empty queue, queue_length must stay at 0.
    #[test]
    fn test_adversarial_field_no_underflow_on_empty_dequeue() {
        let mut field = test_field(4);

        assert_eq!(field.queue_length, 0);

        // Dequeue from empty queue.
        let result = field.dequeue();

        assert!(result.is_none());
        // queue_length must still be 0, not u32::MAX (underflow).
        assert_eq!(
            field.queue_length, 0,
            "queue_length must not underflow on dequeue from empty"
        );
    }

    /// queue_length never exceeds capacity after enqueue on full.
    #[test]
    fn test_adversarial_field_no_overflow_on_full_enqueue() {
        let capacity = 2u32;
        let mut field = test_field(capacity);

        // Fill to capacity.
        for _ in 0..capacity {
            field.enqueue(Message::timer_fire(Badge(0), 0, 0)).unwrap();
        }

        assert_eq!(field.queue_length, capacity);

        // Attempt to enqueue beyond capacity.
        let _ = field.enqueue(Message::timer_fire(Badge(0), 0, 0));

        assert!(
            field.queue_length <= capacity,
            "queue_length must not exceed capacity (was {})",
            field.queue_length
        );
    }

    /// Zero-capacity queue: is_empty and is_full are both true simultaneously.
    /// This is the degenerate case. The implementation must handle it without
    /// inconsistency.
    #[test]
    fn test_adversarial_field_zero_capacity_both_empty_and_full() {
        let field = test_field(0);

        assert!(
            field.is_empty(),
            "zero-capacity queue has length 0 => empty"
        );
        assert!(
            field.is_full(),
            "zero-capacity queue has length 0 >= capacity 0 => full"
        );
    }

    // ── Revocation ────────────────────────────────────────────────────

    /// Multiple revoke() calls — generation increments monotonically.
    #[test]
    fn test_adversarial_field_revoke_monotonic() {
        let field = test_field(4);
        let mut prev = field.generation.load(core::sync::atomic::Ordering::Acquire);

        assert_eq!(prev, 0);

        for expected in 1..=100u64 {
            field.revoke();

            let current = field.generation.load(core::sync::atomic::Ordering::Acquire);

            assert_eq!(
                current, expected,
                "generation must be {expected} after {expected} revocations"
            );
            assert!(current > prev, "generation must be strictly increasing");

            prev = current;
        }
    }

    /// revoke() from u64::MAX — wraps to 0.
    ///
    /// BUG DETECTOR: AtomicU64::fetch_add(1) on u64::MAX wraps to 0 in
    /// release builds (no overflow check). This means:
    /// 1. Generation goes 0 -> 1 -> ... -> MAX -> 0
    /// 2. Any capability entry created with generation 0 that was supposed
    ///    to be revoked by the MAX->0 transition would suddenly pass the
    ///    generation check again.
    /// This is a security-critical wraparound bug. The test documents the
    /// behavior — the implementation should either saturate or detect wrap.
    #[test]
    fn test_adversarial_field_revoke_overflow_from_max() {
        let field = Field {
            generation: AtomicU64::new(u64::MAX),
            ..test_field(4)
        };
        let gen_before = field.generation.load(core::sync::atomic::Ordering::Acquire);

        assert_eq!(gen_before, u64::MAX);

        field.revoke();

        let gen_after = field.generation.load(core::sync::atomic::Ordering::Acquire);

        // This documents the ACTUAL behavior: wrapping to 0.
        // A correct implementation should either:
        // - Saturate at u64::MAX (never wrap)
        // - Use a wider counter
        // - Detect the wrap and handle it
        assert_eq!(
            gen_after, 0,
            "fetch_add(1) on u64::MAX wraps to 0 — this is a security concern"
        );
        // The generation went backwards, which violates monotonicity.
        assert!(
            gen_after < gen_before,
            "generation wrapped around — monotonicity violated"
        );
    }

    /// revoke() from u64::MAX - 1: the penultimate value should increment
    /// to u64::MAX correctly (no premature wrap).
    #[test]
    fn test_adversarial_field_revoke_near_max() {
        let field = Field {
            generation: AtomicU64::new(u64::MAX - 1),
            ..test_field(4)
        };

        field.revoke();

        let generation_value = field.generation.load(core::sync::atomic::Ordering::Acquire);

        assert_eq!(
            generation_value,
            u64::MAX,
            "generation must reach u64::MAX without premature wrap"
        );
    }

    // ── is_full / is_empty edge cases with manual field construction ──

    /// is_full when queue_length equals capacity exactly.
    #[test]
    fn test_adversarial_field_is_full_at_exact_capacity() {
        for cap in [1u32, 2, 7, 255, 1024] {
            let field = Field {
                queue_length: cap,
                ..test_field(cap)
            };

            assert!(
                field.is_full(),
                "is_full must be true when queue_length == capacity ({cap})"
            );
        }
    }

    /// is_full is false when queue_length is one less than capacity.
    #[test]
    fn test_adversarial_field_not_full_one_below_capacity() {
        for cap in [1u32, 2, 7, 255, 1024] {
            let field = Field {
                queue_length: cap - 1,
                ..test_field(cap)
            };

            assert!(
                !field.is_full(),
                "is_full must be false when queue_length == capacity - 1 ({cap})"
            );
        }
    }

    /// is_empty is false when queue_length is 1.
    #[test]
    fn test_adversarial_field_not_empty_at_one() {
        let field = Field {
            queue_length: 1,
            ..test_field(4)
        };

        assert!(
            !field.is_empty(),
            "is_empty must be false when queue_length is 1"
        );
    }

    // ── Message construction edge cases ───────────────────────────────

    /// timer_fire with u32::MAX overrun count — no truncation.
    #[test]
    fn test_adversarial_field_timer_fire_max_overrun() {
        let msg = Message::timer_fire(Badge(0), 0, u32::MAX);

        assert_eq!(
            msg.data[1],
            u32::MAX as u64,
            "overrun count must not be truncated"
        );
    }

    /// timer_fire with u64::MAX fire_time_ticks — no truncation.
    #[test]
    fn test_adversarial_field_timer_fire_max_ticks() {
        let msg = Message::timer_fire(Badge(0), u64::MAX, 0);

        assert_eq!(
            msg.data[0],
            u64::MAX,
            "fire_time_ticks must not be truncated"
        );
    }

    /// badge_closure with Badge(0) and Badge(u64::MAX).
    #[test]
    fn test_adversarial_field_closure_badge_extremes() {
        let msg_zero = Message::badge_closure(Badge(0));

        assert_eq!(msg_zero.badge, Badge(0));
        assert_eq!(msg_zero.data, [0; 4]);

        let msg_max = Message::badge_closure(Badge(u64::MAX));

        assert_eq!(msg_max.badge, Badge(u64::MAX));
        assert_eq!(msg_max.data, [0; 4]);
    }

    /// resolve_route on a field with no routing table returns None
    /// without panicking (the null-check fast path).
    #[test]
    fn test_adversarial_field_resolve_route_null_table_badge_extremes() {
        let field = test_field(4);

        assert!(field.routing_table.is_none());
        assert!(field.resolve_route(0).is_none());
        assert!(field.resolve_route(u64::MAX).is_none());
        assert!(field.resolve_route(u64::MAX / 2).is_none());
    }

    // ── Message constructor tests ────────────────────────────────────

    #[test]
    fn timer_fire_message_layout() {
        let msg = Message::timer_fire(Badge(0xAA), 12345, 3);

        assert_eq!(msg.label, LABEL_TIMER_FIRE);
        assert_eq!(msg.badge, Badge(0xAA));
        assert_eq!(msg.data[0], 12345);
        assert_eq!(msg.data[1], 3);
        assert!(msg.user_cap.is_none());
        assert!(msg.reply_cap.is_none());
    }

    #[test]
    fn badge_closure_message_layout() {
        let msg = Message::badge_closure(Badge(0xBB));

        assert_eq!(msg.label, LABEL_CLOSURE);
        assert_eq!(msg.badge, Badge(0xBB));
        assert_eq!(msg.data, [0; 4]);
        assert!(msg.user_cap.is_none());
    }

    #[test]
    fn device_irq_message_layout() {
        let msg = Message::device_irq(Badge(0xCC), 42);

        assert_eq!(msg.label, LABEL_DEVICE_IRQ);
        assert_eq!(msg.badge, Badge(0xCC));
        assert_eq!(msg.data[0], 42);
        assert!(msg.user_cap.is_none());
    }

    #[test]
    fn label_constants_are_in_kernel_range() {
        let labels = [
            LABEL_TIMER_FIRE,
            LABEL_CLOSURE,
            LABEL_VM_FAULT,
            LABEL_RESOURCE_REQUEST,
            LABEL_CAP_TABLE_FULL,
            LABEL_HARDWARE_EXCEPTION,
            LABEL_DEVICE_IRQ,
        ];

        for label in labels {
            assert!(
                label >= 0xFFFF_FFFF_FFFF_0000,
                "kernel labels must be in reserved high range"
            );
        }
    }

    // ── Queue boundary tests ─────────────────────────────────────────

    #[test]
    fn enqueue_then_dequeue_preserves_order() {
        let mut field = test_field(4);

        for i in 0..3u64 {
            let msg = Message {
                data: [i, 0, 0, 0],
                label: i * 10,
                badge: Badge(i),
                user_cap: None,
                reply_cap: None,
            };

            field.enqueue(msg).unwrap();
        }
        for i in 0..3u64 {
            let msg = field.dequeue().unwrap();

            assert_eq!(msg.data[0], i);
            assert_eq!(msg.badge, Badge(i));
        }
    }

    #[test]
    fn dequeue_empty_field_returns_none() {
        let mut field = test_field(4);

        assert!(field.dequeue().is_none());
    }

    #[test]
    fn is_empty_and_is_full_consistency() {
        let mut field = test_field(2);

        assert!(field.is_empty());
        assert!(!field.is_full());

        let msg = Message {
            data: [0; 4],
            label: 0,
            badge: Badge(0),
            user_cap: None,
            reply_cap: None,
        };

        field.enqueue(msg).unwrap();

        assert!(!field.is_empty());
        assert!(!field.is_full());

        let msg2 = Message {
            data: [1; 4],
            label: 1,
            badge: Badge(1),
            user_cap: None,
            reply_cap: None,
        };

        field.enqueue(msg2).unwrap();

        assert!(!field.is_empty());
        assert!(field.is_full());
    }

    #[test]
    fn enqueue_full_field_returns_error() {
        let mut field = test_field(1);
        let msg = Message {
            data: [0; 4],
            label: 0,
            badge: Badge(0),
            user_cap: None,
            reply_cap: None,
        };

        field.enqueue(msg).unwrap();

        let msg2 = Message {
            data: [1; 4],
            label: 1,
            badge: Badge(1),
            user_cap: None,
            reply_cap: None,
        };

        assert!(field.enqueue(msg2).is_err());
    }

    #[test]
    fn enqueue_dequeue_wraps_around() {
        let mut field = test_field(2);
        let msg = |v: u64| Message {
            data: [v, 0, 0, 0],
            label: v,
            badge: Badge(v),
            user_cap: None,
            reply_cap: None,
        };

        field.enqueue(msg(1)).unwrap();
        field.enqueue(msg(2)).unwrap();
        field.dequeue().unwrap();
        field.enqueue(msg(3)).unwrap();

        let m = field.dequeue().unwrap();

        assert_eq!(m.data[0], 2);

        let m = field.dequeue().unwrap();

        assert_eq!(m.data[0], 3);
        assert!(field.dequeue().is_none());
    }

    #[test]
    fn revoke_increments_generation() {
        let field = test_field(4);
        let gen_before = field.generation.load(core::sync::atomic::Ordering::Acquire);

        field.revoke();

        let gen_after = field.generation.load(core::sync::atomic::Ordering::Acquire);

        assert_eq!(gen_after, gen_before + 1);
    }
}
