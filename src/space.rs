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

// ── Error types ────────────────────────────────────────────────────

/// Errors from Space topology operations (D41, D60).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceError {
    /// D60: size = 0 is an error. A zero-size Space is meaningless.
    ZeroSize,
    /// Split requests more bytes than this Space contains (minus one
    /// page — a Space cannot be emptied; destroy is the way to
    /// eliminate a Space).
    InsufficientSpace,
    /// D41: merge requires adjacent VA space. The kernel's internal
    /// VA layout has no room to extend the target upward.
    NotAdjacent,
}

// ── Space methods ──────────────────────────────────────────────────

impl Space {
    /// Split this Space, extracting `size` bytes into a new Space.
    ///
    /// D41: split is one of two topology-changing operations (with merge).
    /// D60: `size` is in bytes; the kernel rounds up to `page_size`.
    /// The new Space receives a kernel-assigned VA base (D26). This
    /// Space's VA range contracts — holders may lose access to the
    /// extracted portion (parallels D11 destroy visibility).
    ///
    /// Returns `(new_va_base, rounded_size)` for the new Space. The
    /// caller (frame/) allocates an arena slot, constructs the new Space,
    /// and handles page table subtree splitting.
    ///
    /// **Ordering constraint:** this method mutates `self.size` immediately.
    /// The caller must ensure the subsequent arena allocation succeeds. On
    /// allocation failure, restore via `self.size += rounded_size`.
    ///
    /// D32 conservation: pages change membership, not quantity.
    /// Performance: cold path (D1). Requires cross-core TLB invalidation
    /// for shared Spaces (O2).
    /// Security: SPLIT right (D52) checked at the cap layer before this.
    pub fn split(&mut self, size: usize, page_size: usize) -> Result<(usize, usize), SpaceError> {
        let rounded = (size + page_size - 1) & !(page_size - 1);

        if rounded == 0 {
            return Err(SpaceError::ZeroSize);
        }
        if rounded >= self.size {
            return Err(SpaceError::InsufficientSpace);
        }

        let new_va = self.va_base + self.size - rounded;

        self.size -= rounded;

        Ok((new_va, rounded))
    }

    /// Merge a source Space into this Space.
    ///
    /// D41: the source is absorbed — it ceases to exist as an independent
    /// Space. This Space's VA range extends upward from its stable base
    /// (D26). All holders see the extended range immediately (D24).
    ///
    /// D41 resolves D40's demand-paging gap: a pager handling an OOB
    /// fault merges a source Space into the faulting Space to cover the
    /// offset, then resumes the Observer.
    ///
    /// D32 conservation: total pages unchanged. The source's physical
    /// pages and page table subtree memory are absorbed.
    ///
    /// Security: MERGE right (D52) checked at the cap layer before this.
    pub fn merge(&mut self, source: &Space) -> Result<(), SpaceError> {
        let expected_va = self.va_base + self.size;

        if source.va_base != expected_va {
            return Err(SpaceError::NotAdjacent);
        }

        self.size += source.size;

        Ok(())
    }

    /// Check whether a byte offset falls within this Space.
    ///
    /// D26: Observers access Spaces via (cap, offset) pairs. This
    /// validates that the offset is within bounds before the kernel
    /// translates it to a virtual address.
    pub const fn contains_offset(&self, offset: usize) -> bool {
        offset < self.size
    }

    /// Number of pages backing this Space (D25).
    pub const fn page_count(&self, page_size: usize) -> usize {
        self.size / page_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_layout() {
        assert_eq!(core::mem::size_of::<Space>(), 32);
    }

    #[test]
    fn split_rounds_up_to_page_size() {
        let mut space = Space {
            va_base: 0x1000,
            size: 0x4000,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let (new_va, rounded) = space.split(100, 4096).unwrap();

        assert_eq!(rounded, 4096);
        assert_eq!(space.size, 0x3000);
        assert_eq!(new_va, 0x1000 + 0x3000);
    }

    #[test]
    fn split_rejects_zero_size() {
        let mut space = Space {
            va_base: 0,
            size: 4096,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(space.split(0, 4096), Err(SpaceError::ZeroSize));
    }

    #[test]
    fn split_rejects_oversized() {
        let mut space = Space {
            va_base: 0,
            size: 4096,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(space.split(4096, 4096), Err(SpaceError::InsufficientSpace));
    }

    #[test]
    fn merge_requires_adjacency() {
        let mut target = Space {
            va_base: 0x1000,
            size: 0x2000,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let adjacent = Space {
            va_base: 0x3000,
            size: 0x1000,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let nonadjacent = Space {
            va_base: 0x5000,
            size: 0x1000,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert!(target.merge(&adjacent).is_ok());
        assert_eq!(target.size, 0x3000);
        assert_eq!(target.merge(&nonadjacent), Err(SpaceError::NotAdjacent));
    }

    #[test]
    fn contains_offset_boundary() {
        let space = Space {
            va_base: 0,
            size: 4096,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert!(space.contains_offset(0));
        assert!(space.contains_offset(4095));
        assert!(!space.contains_offset(4096));
    }
}
