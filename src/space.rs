//! Space: kernel-managed memory object.
//!
//! D9:  variable-size, capability-designated.
//! D25: page size exposed (minimum Space size = page size).
//! D26: accessed via (cap, offset) pairs — holding the cap is sufficient.
//!      Kernel assigns a VA base per Space; all holders see the same VA.
//! D27: flat cardinality — Observers hold multiple independent Space caps.
//! D32: created by type conversion (Space consumed → becomes object backing).
//! D41: merge (two → one) and split (one → two) change Space boundaries.
//! D52: rights — split, merge, destroy, clone (4 bits).
//! D60: byte-addressed inputs; kernel rounds to PAGE_SIZE internally.
//! D67: generation counter for revocation.

use core::sync::atomic::AtomicU64;

/// A claim to a portion of the system's bounded memory resource.
///
/// Each Space has a kernel-assigned VA base (D26), stable for the
/// Space's lifetime. Physical backing and page table subtrees are
/// kernel-internal concerns (D9, D5).
///
/// Operations: split (D41), merge (D41), destroy (D11/D33).
/// Creation: type conversion from another Space (D32).
pub struct Space {
    /// Kernel-assigned virtual address base (D26).
    /// Stable — all holders see the same VA range.
    pub va_base: usize,

    /// Size in bytes, page-aligned (D25, D60).
    pub size: usize,

    /// Number of capability references to this Space (D11).
    pub refcount: u32,

    /// Revocation generation counter (D67). Bumped atomically on
    /// explicit revocation; capability entries store the value at
    /// creation. AtomicU64 per D67: hot-path cap checks may read
    /// this without holding the arena lock.
    pub generation: AtomicU64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_layout() {
        assert_eq!(core::mem::size_of::<Space>(), 32);
    }
}
