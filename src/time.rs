//! Time: capability-held scheduling allocation.
//!
//! D29: capability-held kernel object type.
//! D30: one or more per Observer, in regular cap-table slots.
//! D31: abstract — core assignment is kernel-internal.

/// A claim to a portion of the system's scheduling capacity.
///
/// Multiple Time caps per Observer are additive — kernel maintains a cached
/// scheduling aggregate on the Observer struct (D30).
/// Core assignment, migration, and algorithm selection are kernel-internal
/// (D31).
pub struct Time {
    // Parameters open (budget/period vs. fraction vs. claim-to-participate).
    // Clonability open (D23 settled others; uniformity suggests clonable).
    // Donation on IPC open (explicit cap transfer vs. kernel-internal).
}
