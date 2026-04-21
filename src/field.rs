//! Field: queued IPC mechanism.
//!
//! D13: queued fields with direct-switch fast path.
//! D15: unidirectional, many-to-many, send/receive as object-rights.
//! D16: reply via pre-allocated reply field with send-once cap.
//! D17: badge semantics (minter-assigned, opt-in lifecycle tracking).
//! D18: error-to-sender overflow, deferred fault delivery.
//! D28: fixed-size message format.

/// Bounded queue with waiters list.
///
/// Single kernel object. Rights (send, receive, mint) carried in the
/// capability, not the field. Topology emerges from capability distribution.
///
/// All information delivery — peer IPC, fault notifications (D12), interrupt
/// signals (D22), badge-closure (D17) — uses this mechanism.
pub struct Field {
    // Kernel-internal: bounded queue, waiters list.
    // Optional: per-badge refcount map (creation-time opt-in, D17).
    // Split/combine semantics open (D22 downstream).
}

/// Fixed-size IPC message (D28).
///
/// 4 untyped data words + 1 user cap slot + label (header) + badge
/// (kernel-injected) + reply cap (kernel-injected, Call only).
/// Data words and cap slots are structurally separate.
pub struct Message {
    // Layout open.
    // Fault messages use same format (D12 + D28).
}
