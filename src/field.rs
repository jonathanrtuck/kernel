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

    /// Nullable routing table (D54). None = unsplit (zero hot-path cost).
    pub routing_table: Option<NonNull<RoutingTable>>,

    /// D18: pending list head — Observers whose fault/interrupt message
    /// could not be delivered due to a full queue. Distinct from waiters
    /// (waiters = blocked on Receive; pending = deferred kernel-as-sender).
    /// On each dequeue that frees a slot, the pending list is checked and
    /// the deferred message is delivered.
    pub pending_head: Option<NonNull<crate::observer::WaitEntry>>,

    /// Per-badge refcount tracking enabled (D17 opt-in).
    /// Reply Fields are always-tracked (D73).
    pub badge_tracking: bool,

    /// Back-pointer list head for D55 routing cleanup.
    /// When this Field is a routing destination, source Fields link
    /// their routing entries here for O(1) cleanup on destroy.
    /// None when no sources route here.
    pub back_pointer_head: Option<NonNull<RoutingEntry>>,

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
}

// ── Field methods ──────────────────────────────────────────────────

impl Field {
    /// Enqueue a message into the bounded queue.
    ///
    /// D13: queued fields. D18: returns error on full queue (error-to-
    /// sender). The caller (IPC send path or kernel-as-sender) must
    /// handle the overflow — for userspace senders that means returning
    /// an error; for kernel-as-sender (faults, interrupts) it means
    /// deferred delivery via the pending list (D18).
    ///
    /// Performance: O(1) circular buffer insertion. Hot path for IPC.
    pub fn enqueue(&mut self, _message: Message) -> Result<(), FieldError> {
        todo!()
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
        todo!()
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
    pub fn add_waiter(&mut self, _entry: &mut crate::observer::WaitEntry) {
        todo!()
    }

    /// Remove an Observer from the waiters list.
    ///
    /// Called when: a message arrives and the front waiter is woken,
    /// the Observer is destroyed while waiting, or the Observer is
    /// suspended (D39) while blocked.
    pub fn remove_waiter(&mut self, _entry: &mut crate::observer::WaitEntry) {
        todo!()
    }

    /// Pop the front waiter for direct-switch or message delivery.
    ///
    /// D13/D50: when a sender finds a waiting receiver, the kernel
    /// can bypass the queue and hand the message directly. Returns
    /// the waiter's Observer pointer for the scheduler's
    /// `should_switch_to` check (D50 condition 5).
    pub fn pop_waiter(&mut self) -> Option<NonNull<crate::observer::WaitEntry>> {
        todo!()
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
    pub fn resolve_route(&self, _badge: u64) -> Option<ObjectId> {
        todo!()
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
        _low: u64,
        _high: u64,
        _destination: ObjectId,
        _destination_generation: u64,
    ) -> Result<(), FieldError> {
        todo!()
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
        assert_eq!(core::mem::size_of::<Field>(), 72);
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
        ];

        for (i, a) in labels.iter().enumerate() {
            for (j, b) in labels.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b, "kernel labels must be distinct");
                }
            }
        }
    }
}
