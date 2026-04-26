//! Time: capability-held compute allocation.
//!
//! D29: capability-held kernel object type.
//! D30: one or more per Observer, in regular cap-table slots.
//! D31: abstract — core assignment is kernel-internal.
//! D36: carries normalized compute units (Space = bytes, Time = compute units).
//! D37: donation via explicit cap transfer in user cap slot on Call().
//! D38: non-clonable (linear). Authority delegation via split, not clone.
//! D52: rights — split, destroy (2 bits). No clone.
//! D67: generation counter for revocation.

use core::sync::atomic::AtomicU64;

/// A claim to a portion of the system's compute capacity, denominated
/// in normalized compute units (D36).
///
/// The unit is calibrated to hardware core capacity factors so that a
/// given quantity represents approximately the same work on any core.
/// The kernel translates to per-core scheduling time internally.
///
/// Linear (D38): at most one capability reference per Time object.
/// Clone structurally forbidden — D30's cached aggregate would
/// double-count. Authority delegation uses split (new object with a
/// portion of the original's quantity).
pub struct Time {
    /// Normalized compute units (D36).
    pub compute_units: u32,

    /// Outstanding capability references (D11). Always 0 or 1 under
    /// D38 linearity — stored for cross-type uniformity with Space,
    /// Field, Observer, and Pulsar.
    pub refcount: u32,

    /// Revocation generation counter (D67). AtomicU64 per D67.
    pub generation: AtomicU64,
}

// ── Error types ────────────────────────────────────────────────────

/// Errors from Time operations (D38).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeError {
    /// Cannot split zero compute units.
    ZeroAmount,
    /// Split requests more compute units than this Time holds.
    InsufficientUnits,
}

// ── Time methods ───────────────────────────────────────────────────

impl Time {
    /// Split compute units from this Time into a new Time object.
    ///
    /// D38: Time caps are linear (non-clonable). Authority delegation
    /// uses split — a new object with a portion of the original's
    /// quantity. The original shrinks; the new object is created by
    /// the caller in a fresh arena slot.
    ///
    /// Returns the compute units for the new Time. The caller (frame/)
    /// allocates an arena slot and constructs the new Time object.
    ///
    /// D36 conservation: `sum(all_time.compute_units) + kernel_pool
    /// <= total_system_capacity`. Split preserves this — units transfer,
    /// not multiply.
    ///
    /// Security: SPLIT right (D52) checked at the cap layer before this.
    /// Performance: cold path (D1). Observer's compute_aggregate (D30)
    /// must be updated by the caller after the new Time cap is installed.
    pub fn split(&mut self, amount: u32) -> Result<u32, TimeError> {
        if amount == 0 {
            return Err(TimeError::ZeroAmount);
        }
        if amount > self.compute_units {
            return Err(TimeError::InsufficientUnits);
        }

        self.compute_units -= amount;

        Ok(amount)
    }

    /// D67: atomically increment the generation counter, revoking all
    /// capabilities that stored the previous generation value.
    ///
    /// O(1) revocation. Stale caps are detected lazily on next use
    /// (Coyotos pattern). No IPI needed — ARM64 cache coherence (O2)
    /// ensures the bump is eventually visible on all cores.
    pub fn revoke(&self) {
        self.generation
            .fetch_add(1, core::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_transfers_units() {
        let mut time = Time {
            compute_units: 100,
            refcount: 1,
            generation: AtomicU64::new(0),
        };
        let new_units = time.split(40).unwrap();

        assert_eq!(new_units, 40);
        assert_eq!(time.compute_units, 60);
    }

    #[test]
    fn split_rejects_zero() {
        let mut time = Time {
            compute_units: 100,
            refcount: 1,
            generation: AtomicU64::new(0),
        };

        assert_eq!(time.split(0), Err(TimeError::ZeroAmount));
    }

    #[test]
    fn split_allows_exhausting() {
        let mut time = Time {
            compute_units: 100,
            refcount: 1,
            generation: AtomicU64::new(0),
        };
        let new_units = time.split(100).unwrap();

        assert_eq!(new_units, 100);
        assert_eq!(time.compute_units, 0);
    }

    #[test]
    fn split_rejects_oversized() {
        let mut time = Time {
            compute_units: 100,
            refcount: 1,
            generation: AtomicU64::new(0),
        };

        assert_eq!(time.split(101), Err(TimeError::InsufficientUnits));
        assert_eq!(time.split(200), Err(TimeError::InsufficientUnits));
    }

    #[test]
    fn revoke_bumps_generation() {
        let time = Time {
            compute_units: 50,
            refcount: 1,
            generation: AtomicU64::new(0),
        };

        time.revoke();

        assert_eq!(
            time.generation.load(core::sync::atomic::Ordering::Acquire),
            1
        );
    }

    #[test]
    fn split_conservation_across_multiple_splits() {
        let mut time = Time {
            compute_units: 100,
            refcount: 1,
            generation: AtomicU64::new(0),
        };
        let a = time.split(30).unwrap();
        let b = time.split(20).unwrap();
        let c = time.split(50).unwrap();

        assert_eq!(a + b + c + time.compute_units, 100);
        assert_eq!(time.compute_units, 0);
    }

    #[test]
    fn split_after_exhaust_fails() {
        let mut time = Time {
            compute_units: 100,
            refcount: 1,
            generation: AtomicU64::new(0),
        };

        time.split(100).unwrap();

        assert_eq!(time.split(1), Err(TimeError::InsufficientUnits));
    }

    #[test]
    fn split_does_not_modify_on_error() {
        let mut time = Time {
            compute_units: 50,
            refcount: 1,
            generation: AtomicU64::new(0),
        };
        let _ = time.split(51);

        assert_eq!(time.compute_units, 50);
    }

    #[test]
    fn split_one_unit() {
        let mut time = Time {
            compute_units: 1,
            refcount: 1,
            generation: AtomicU64::new(0),
        };

        assert_eq!(time.split(1).unwrap(), 1);
        assert_eq!(time.compute_units, 0);
    }

    #[test]
    fn revoke_is_cumulative() {
        let time = Time {
            compute_units: 10,
            refcount: 1,
            generation: AtomicU64::new(0),
        };

        time.revoke();
        time.revoke();
        time.revoke();

        assert_eq!(
            time.generation.load(core::sync::atomic::Ordering::Acquire),
            3
        );
    }
}
