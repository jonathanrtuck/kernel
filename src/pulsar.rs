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
use core::sync::atomic::AtomicU64;

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

    /// Revocation generation counter (D67). AtomicU64 per D67.
    pub generation: AtomicU64,
}

// ── Pulsar methods ─────────────────────────────────────────────────

impl Pulsar {
    /// Create a new Pulsar, armed immediately (D62).
    ///
    /// D62: single-call, armed-at-creation. No separate arm, configure,
    /// or modify call. D35's composable pattern does not apply — Pulsars
    /// have no structural gap requiring an inert state.
    /// Cancel = `destroy(pulsar_cap)`. Modify = destroy + create.
    ///
    /// D72: `duration_ns` is a relative duration in nanoseconds. The
    /// kernel converts to absolute ticks using `counter_freq` (CNTFRQ_EL0).
    /// A5: the kernel absorbs the trivial ns→ticks conversion so callers
    /// express intent in human-meaningful units.
    ///
    /// D44: `period_ns = 0` means one-shot. `period_ns > 0` means
    /// repeating with kernel-managed re-arm and drift compensation.
    pub fn new(
        delivery_field: ObjectId,
        badge: Badge,
        duration_ns: u64,
        period_ns: u64,
        counter_freq: u64,
        now_ticks: u64,
    ) -> Pulsar {
        let duration_ticks = ns_to_ticks(duration_ns, counter_freq);

        Pulsar {
            delivery_field,
            badge,
            duration_ns,
            period_ns,
            next_deadline_ticks: now_ticks + duration_ticks,
            overrun_count: 0,
            refcount: 1,
            generation: AtomicU64::new(0),
        }
    }

    /// Whether this Pulsar auto-repeats (D44).
    pub const fn is_repeating(&self) -> bool {
        self.period_ns > 0
    }

    /// Construct the fire message for delivery to the Field (D63).
    ///
    /// D63: badge + LABEL_TIMER_FIRE + fire_time (raw CNTVCT_EL0 ticks
    /// at interrupt entry) + overrun_count. Empty cap slot — satisfies
    /// D50 fast-path 0-cap condition.
    ///
    /// Fire time in raw ticks, not nanoseconds: cheaper at interrupt
    /// time and directly comparable to Observer counter reads. No
    /// surveyed system includes a firing timestamp — D44 deliberately
    /// departs from consensus.
    pub fn fire_message(&self, actual_fire_ticks: u64) -> crate::field::Message {
        crate::field::Message::timer_fire(self.badge, actual_fire_ticks, self.overrun_count)
    }

    /// Re-arm for the next period (D44).
    ///
    /// D44: drift-compensated — `next = scheduled + period`, not
    /// `next = now + period`. This prevents systematic drift from
    /// interrupt latency accumulation.
    ///
    /// The caller resets `overrun_count` to zero after constructing
    /// the fire message. The counter_freq is needed to convert
    /// period_ns to ticks.
    pub fn rearm(&mut self, counter_freq: u64) {
        let period_ticks = ns_to_ticks(self.period_ns, counter_freq);

        self.next_deadline_ticks += period_ticks;
        self.overrun_count = 0;
    }

    /// Record a missed delivery due to full queue (D44).
    ///
    /// D44: when the delivery Field is full, the kernel stops
    /// re-arming. The overrun count accumulates until a slot opens.
    /// On the next receive that frees a slot, the kernel re-arms and
    /// includes the overrun count in the fire message (D63).
    pub fn record_overrun(&mut self) {
        self.overrun_count += 1;
    }

    /// D67: atomically increment the generation counter.
    pub fn revoke(&self) {
        self.generation
            .fetch_add(1, core::sync::atomic::Ordering::Release);
    }
}

/// Convert nanoseconds to counter ticks using the hardware frequency.
///
/// D72: the kernel absorbs this conversion (A5). Uses integer
/// arithmetic to avoid floating point in the kernel.
fn ns_to_ticks(ns: u64, counter_freq: u64) -> u64 {
    (ns as u128 * counter_freq as u128 / 1_000_000_000) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ns_to_ticks_conversion() {
        let freq = 24_000_000; // 24 MHz (typical ARM timer)

        assert_eq!(ns_to_ticks(1_000_000_000, freq), 24_000_000);
        assert_eq!(ns_to_ticks(1_000_000, freq), 24_000);
        assert_eq!(ns_to_ticks(0, freq), 0);
    }

    #[test]
    fn one_shot_is_not_repeating() {
        let p = Pulsar::new(ObjectId(0), Badge(1), 1_000_000, 0, 24_000_000, 0);

        assert!(!p.is_repeating());
    }

    #[test]
    fn repeating_pulsar() {
        let p = Pulsar::new(ObjectId(0), Badge(1), 1_000_000, 10_000_000, 24_000_000, 0);

        assert!(p.is_repeating());
    }

    #[test]
    fn rearm_uses_scheduled_not_actual() {
        let mut p = Pulsar::new(
            ObjectId(0),
            Badge(1),
            1_000_000,
            10_000_000,
            24_000_000,
            100,
        );
        let first_deadline = p.next_deadline_ticks;

        p.rearm(24_000_000);

        let period_ticks = ns_to_ticks(10_000_000, 24_000_000);

        assert_eq!(p.next_deadline_ticks, first_deadline + period_ticks);
    }

    #[test]
    fn fire_message_has_no_cap() {
        let p = Pulsar::new(ObjectId(0), Badge(42), 1_000_000, 0, 24_000_000, 0);
        let msg = p.fire_message(12345);

        assert!(msg.user_cap.is_none());
        assert!(msg.reply_cap.is_none());
    }
}
