//! Time: capability-held compute allocation.
//!
//! D29: capability-held kernel object type.
//! D30: one or more per Observer, in regular cap-table slots.
//! D31: abstract — core assignment is kernel-internal.
//! D36: carries normalized compute units (Space = bytes, Time = compute units).
//! D37: donation via explicit cap transfer in user cap slot on Call().

/// A claim to a portion of the system's compute capacity, denominated in
/// normalized compute units.
///
/// The unit is calibrated to hardware core capacity factors so that a given
/// quantity represents approximately the same work on any core. The kernel
/// translates to per-core scheduling time internally.
///
/// Multiple Time caps per Observer are additive — kernel maintains a cached
/// aggregate on the Observer struct (D30).
pub struct Time {
    compute_units: u32,
    // Clonability open (D23 uniformity vs. D30 aggregate double-counting).
    // Donation settled (D37): explicit cap transfer via user cap slot on Call().
    // Transfer is move-only (D30 aggregate correctness).
}
