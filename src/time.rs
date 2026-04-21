//! Time: capability-held compute allocation.
//!
//! D29: capability-held kernel object type.
//! D30: one or more per Observer, in regular cap-table slots.
//! D31: abstract — core assignment is kernel-internal.
//! D36: carries normalized compute units (Space = bytes, Time = compute units).

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
    // Clonability open (D23 settled others; uniformity suggests clonable).
    // Donation on IPC open (explicit cap transfer vs. kernel-internal).
}
