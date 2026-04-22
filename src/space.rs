//! Space: kernel-managed memory object.
//!
//! D9:  variable-size, capability-designated.
//! D26: accessed via (cap, offset) pairs — holding the cap is sufficient.
//! D27: flat cardinality — Observers hold multiple independent Space caps.
//! D25: page size exposed (minimum Space size = page size).
//! D32: created by type conversion (Space consumed → becomes object backing).
//! D41: merge (two → one) and split (one → two) change Space boundaries.

/// A claim to a portion of the system's bounded memory resource.
///
/// Each Space has a kernel-assigned VA base (per-Space, same for all holders).
/// Physical backing is kernel-internal. Sharing is through capability transfer
/// Multiple Observers hold caps to the same Space.
pub struct Space {
    // Kernel-internal: size, physical backing, VA base, refcount.
    // D3: physical allocation through the Space manager.
    // D5: page table subtrees shared across holders of same Space.
}
