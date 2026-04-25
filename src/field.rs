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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn field_layout() {
        assert_eq!(core::mem::size_of::<Field>(), 64);
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
}
