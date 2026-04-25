//! Pulsar: capability-held timer object with kernel-managed delivery.
//!
//! D44: fifth kernel object type. Created from Space (D32), delivers to
//!      a Field (D13/D17). Kernel manages re-arm, drift compensation,
//!      overflow. Period is EDF admission input (D42).
//! D52: rights — destroy, clone (2 bits).
//! D62: creation API — single-call, armed-at-creation. Cancel = destroy.
//! D63: message layout — badge + LABEL_TIMER_FIRE + fire_time + overrun_count.
//! D72: duration parameter is relative nanoseconds. Kernel converts to
//!      absolute ticks internally.
//! D67: generation counter for revocation.

use crate::arena::ObjectId;
use crate::capability::Badge;

/// A timer that the kernel programs on behalf of an Observer and
/// delivers as a Field message when it fires (D44).
///
/// Armed on creation (D62). No separate arm, configure, or modify call.
/// Cancel = `destroy(pulsar_cap)`. Modify = destroy + create.
/// One-shot loop is the manual-control escape hatch for adaptive timing.
pub struct Pulsar {
    /// Delivery target Field (D44). Kernel enqueues a message here on fire.
    pub delivery_field: ObjectId,

    /// Badge injected into the fire message (D17, D63).
    pub badge: Badge,

    /// Duration in nanoseconds (D72). The kernel converts to absolute
    /// ticks using CNTFRQ_EL0 at creation time.
    pub duration_ns: u64,

    /// Period in nanoseconds (D44). 0 = one-shot; >0 = repeating with
    /// kernel-managed re-arm and drift compensation.
    pub period_ns: u64,

    /// Next absolute deadline in counter ticks (kernel-internal).
    /// Computed from duration_ns at creation; updated as
    /// `next = scheduled + period` for repeating Pulsars (D44).
    pub next_deadline_ticks: u64,

    /// Accumulated overrun count (D44, D63). Incremented when the
    /// delivery Field is full and re-arm is deferred.
    pub overrun_count: u32,

    /// Outstanding capability references (D11).
    pub refcount: u32,

    /// Revocation generation counter (D67).
    pub generation: u64,
}
