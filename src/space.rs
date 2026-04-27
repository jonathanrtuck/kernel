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

    /// Physical address of the Space's content pages.
    /// Set at Space creation (D32 type conversion), immutable
    /// thereafter. Needed for reclamation when the Space is destroyed.
    pub content_pa: u64,

    /// Physical address of the L3 page table for this Space (D89/D92).
    /// One L3 table per 32 MiB of content (with 16 KiB granule: 2048
    /// entries * 16 KiB = 32 MiB coverage). Set at Space creation,
    /// immutable thereafter. Split does not alter the parent's
    /// l3_table_pa — the child gets its own L3 table allocated from
    /// the conversion overhead budget (D92).
    pub l3_table_pa: u64,

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
        assert_eq!(core::mem::size_of::<Space>(), 48);
    }

    #[test]
    fn split_rounds_up_to_page_size() {
        let mut space = Space {
            va_base: 0x1000,
            size: 0x4000,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0xDEAD_0000,
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
            content_pa: 0,
            l3_table_pa: 0,
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
            content_pa: 0,
            l3_table_pa: 0,
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
            content_pa: 0,
            l3_table_pa: 0xAAAA_0000,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let adjacent = Space {
            va_base: 0x3000,
            size: 0x1000,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0xBBBB_0000,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let nonadjacent = Space {
            va_base: 0x5000,
            size: 0x1000,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0xCCCC_0000,
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
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert!(space.contains_offset(0));
        assert!(space.contains_offset(4095));
        assert!(!space.contains_offset(4096));
    }

    // ── D89 — L3 table PA preserved through topology operations ───────

    /// D89: split does not alter the parent Space's l3_table_pa.
    /// The L3 table PA is set at creation and immutable for the Space's
    /// lifetime. Split creates a new Space (with its own L3 table); the
    /// parent keeps its original.
    #[test]
    fn d89_split_preserves_parent_l3_table_pa() {
        let original_l3_pa: u64 = 0xDEAD_BEEF_0000;
        let mut space = Space {
            va_base: 0x1000,
            size: 4 * 16384, // 4 pages of 16 KiB
            refcount: 1,
            content_pa: 0,
            l3_table_pa: original_l3_pa,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let _result = space.split(16384, 16384).expect("split must succeed");

        assert_eq!(
            space.l3_table_pa, original_l3_pa,
            "split must not alter parent's l3_table_pa \
             (expected {original_l3_pa:#x}, got {:#x})",
            space.l3_table_pa
        );
    }

    /// D89: multiple sequential splits preserve l3_table_pa every time.
    #[test]
    fn d89_multiple_splits_preserve_l3_table_pa() {
        let original_l3_pa: u64 = 0xCAFE_0000;
        let mut space = Space {
            va_base: 0x0,
            size: 8 * 4096, // 8 pages of 4 KiB
            refcount: 1,
            content_pa: 0,
            l3_table_pa: original_l3_pa,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        // Split off three times; l3_table_pa must remain unchanged each time.
        for i in 0..3 {
            let _result = space.split(4096, 4096).expect("split must succeed");

            assert_eq!(
                space.l3_table_pa, original_l3_pa,
                "split #{i}: parent l3_table_pa must be unchanged"
            );
        }
    }

    /// D89: merge does not alter the target Space's l3_table_pa.
    /// When absorbing a source Space, the target keeps its own L3 table
    /// PA. The source's L3 table is freed separately (by the caller).
    #[test]
    fn d89_merge_preserves_target_l3_table_pa() {
        let target_l3_pa: u64 = 0xAAAA_0000;
        let source_l3_pa: u64 = 0xBBBB_0000;
        let mut target = Space {
            va_base: 0x1000,
            size: 0x2000,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: target_l3_pa,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let source = Space {
            va_base: 0x3000,
            size: 0x1000,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: source_l3_pa,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        target.merge(&source).expect("merge must succeed");

        assert_eq!(
            target.l3_table_pa, target_l3_pa,
            "merge must not alter target's l3_table_pa \
             (expected {target_l3_pa:#x}, got {:#x})",
            target.l3_table_pa
        );
    }

    /// D89: l3_table_pa is independent between target and source.
    /// After merge, target's l3_table_pa is still its own, not the source's.
    #[test]
    fn d89_merge_does_not_adopt_source_l3_table_pa() {
        let mut target = Space {
            va_base: 0x0,
            size: 0x4000,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0x1111_0000,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let source = Space {
            va_base: 0x4000,
            size: 0x4000,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0x2222_0000,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        target.merge(&source).expect("merge must succeed");

        assert_ne!(
            target.l3_table_pa, source.l3_table_pa,
            "target must keep its own l3_table_pa, not adopt source's"
        );
        assert_eq!(target.l3_table_pa, 0x1111_0000);
    }

    // ── D90 — page_count with real hardware granule ───────────────────

    /// D90: page_count with 16 KiB pages (ARM64 hardware granule).
    /// A Space of exactly N * 16 KiB must report N pages.
    #[test]
    fn d90_page_count_16kib_exact_pages() {
        let page_size = 16384;
        let space = Space {
            va_base: 0,
            size: 4 * page_size,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(
            space.page_count(page_size),
            4,
            "4 * 16 KiB Space must report 4 pages"
        );
    }

    /// D90: page_count with 16 KiB pages — single page (minimum Space).
    #[test]
    fn d90_page_count_16kib_single_page() {
        let page_size = 16384;
        let space = Space {
            va_base: 0,
            size: page_size,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(
            space.page_count(page_size),
            1,
            "single 16 KiB page Space must report 1 page"
        );
    }

    /// D90: page_count with 16 KiB pages — large Space (2048 pages = 32 MiB).
    /// This is the coverage of one L3 table with 16 KiB granule.
    #[test]
    fn d90_page_count_16kib_one_l3_table_coverage() {
        let page_size = 16384;
        let space = Space {
            va_base: 0,
            size: 2048 * page_size, // 32 MiB
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(
            space.page_count(page_size),
            2048,
            "32 MiB Space with 16 KiB pages must report 2048 pages"
        );
    }

    /// D90: page_count with 16 KiB pages — just over one L3 table.
    #[test]
    fn d90_page_count_16kib_over_one_l3_table() {
        let page_size = 16384;
        let space = Space {
            va_base: 0,
            size: 2049 * page_size,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(
            space.page_count(page_size),
            2049,
            "2049 * 16 KiB Space must report 2049 pages"
        );
    }

    /// D90: page_count with 4 KiB pages (alternate granule).
    #[test]
    fn d90_page_count_4kib_pages() {
        let page_size = 4096;
        let space = Space {
            va_base: 0,
            size: 10 * page_size,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(
            space.page_count(page_size),
            10,
            "10 * 4 KiB Space must report 10 pages"
        );
    }

    /// D90: page_count of zero-size Space is zero.
    #[test]
    fn d90_page_count_zero_size() {
        let space = Space {
            va_base: 0,
            size: 0,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(
            space.page_count(16384),
            0,
            "zero-size Space must report 0 pages"
        );
    }

    #[test]
    fn split_conservation_total_size() {
        let mut space = Space {
            va_base: 0x0,
            size: 8 * 4096,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let original_size = space.size;
        let (_, split_size) = space.split(4096, 4096).unwrap();

        assert_eq!(space.size + split_size, original_size);
    }

    #[test]
    fn split_new_va_is_at_end_of_parent() {
        let mut space = Space {
            va_base: 0x10000,
            size: 4 * 4096,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let (new_va, new_size) = space.split(4096, 4096).unwrap();

        assert_eq!(new_va, space.va_base + space.size);
        assert_eq!(new_va + new_size, 0x10000 + 4 * 4096);
    }

    #[test]
    fn split_exact_page_no_rounding() {
        let mut space = Space {
            va_base: 0,
            size: 2 * 16384,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let (_, rounded) = space.split(16384, 16384).unwrap();

        assert_eq!(rounded, 16384);
    }

    #[test]
    fn split_leaves_minimum_one_page() {
        let mut space = Space {
            va_base: 0,
            size: 2 * 4096,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        space.split(4096, 4096).unwrap();

        assert_eq!(space.size, 4096);
        assert!(space.split(4096, 4096).is_err());
    }

    #[test]
    fn merge_extends_size() {
        let mut target = Space {
            va_base: 0,
            size: 4096,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let source = Space {
            va_base: 4096,
            size: 4096,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        target.merge(&source).unwrap();

        assert_eq!(target.size, 8192);
        assert_eq!(target.va_base, 0);
    }

    #[test]
    fn merge_rejects_gap() {
        let mut target = Space {
            va_base: 0,
            size: 4096,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let source = Space {
            va_base: 8192,
            size: 4096,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(target.merge(&source), Err(SpaceError::NotAdjacent));
    }

    #[test]
    fn merge_rejects_overlap() {
        let mut target = Space {
            va_base: 0,
            size: 8192,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let source = Space {
            va_base: 4096,
            size: 4096,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert_eq!(target.merge(&source), Err(SpaceError::NotAdjacent));
    }

    #[test]
    fn contains_offset_zero_size_space() {
        let space = Space {
            va_base: 0,
            size: 0,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        assert!(!space.contains_offset(0));
    }

    #[test]
    fn revoke_increments_generation() {
        let space = Space {
            va_base: 0,
            size: 4096,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        space
            .generation
            .fetch_add(1, core::sync::atomic::Ordering::Release);

        assert_eq!(
            space.generation.load(core::sync::atomic::Ordering::Acquire),
            1
        );
    }

    #[test]
    fn split_then_merge_restores_original_size() {
        let page_size = 4096;
        let mut space = Space {
            va_base: 0,
            size: 4 * page_size,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let original_size = space.size;
        let (new_va, new_size) = space.split(page_size, page_size).unwrap();
        let split_off = Space {
            va_base: new_va,
            size: new_size,
            refcount: 1,
            content_pa: 0,
            l3_table_pa: 0,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        space.merge(&split_off).unwrap();

        assert_eq!(space.size, original_size);
    }
}
