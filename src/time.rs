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

    /// Revocation generation counter (D67).
    pub generation: u64,
}
