//! Space manager: physical memory allocation and VA assignment.
//!
//! Named after the graph.d2 component. Single logical interface (D3).
//! Internal strategy (partitioning, NUMA-awareness, per-core caches,
//! cross-node fallback) is a leaf-node concern below this layer.
//!
//! D3:  one logical Space manager for the system.
//! D31: retains unallocated physical memory as kernel-internal root Space.
//!      Allocates to Observers via pager chain on request.
//! D32: type conversion — Space consumed becomes object backing.
//!      Page table subtree cost baked in at split time.
//! D70: arena slab pages drawn from and returned to the root pool.

use crate::arena::AllocError;

// ── Page size (D25) ────────────────────────────────────────────────
//
// Page size is architecture-specific (ARM64: 4K/16K/64K granule,
// selected at boot from hardware and DTB). The authoritative value
// lives in frame/arch/ — this module receives it as a parameter
// where needed.
//
// D60: Space operations accept byte inputs; the kernel rounds up to
// the page size internally. The page size is queryable (D25) but not
// configurable at runtime.

// ── Root pool ──────────────────────────────────────────────────────

/// The kernel's root physical memory pool (D31).
///
/// All unallocated physical memory starts here. Space creation (D32
/// type conversion) draws from this pool. Object destruction returns
/// structural backing here. Arena slab pages (D70) are drawn from
/// and returned to this pool.
///
/// D31: the root pool is the terminus of the pager chain. When a
/// resource request escalates through handler → handler → ... →
/// kernel, the kernel allocates from this pool or denies.
///
/// Conservation: total physical memory = root pool + sum of all
/// allocated Spaces + kernel metadata overhead. Nothing is created
/// or destroyed — only moved.
pub struct RootPool {
    /// Total physical memory available at boot, in bytes.
    pub total_bytes: usize,
    /// Currently unallocated bytes in the pool.
    pub free_bytes: usize,
    /// Page size discovered at boot from frame/arch/ (D25).
    pub page_size: usize,
}

// ── VA assignment ──────────────────────────────────────────────────

/// VA assignment outcome for a new Space (D26).
///
/// D26: the kernel assigns a VA base per Space. All holders see the
/// same VA. The Observer never chooses or manages virtual addresses.
/// VA layout is a kernel-internal policy concern.
pub struct VaAssignment {
    /// Kernel-assigned virtual address base for the new Space.
    pub va_base: usize,
}

// ── SpaceManager ───────────────────────────────────────────────────

/// The kernel's single Space management interface (D3).
///
/// Owns the root pool, handles VA assignment, and mediates type
/// conversion (D32). Internal strategy (partitioning, NUMA awareness,
/// per-core page caches) is behind this interface — a leaf node
/// (philosophy: push complexity to the leaves).
pub struct SpaceManager {
    pub root_pool: RootPool,
    /// Bump allocator cursor for physical page allocation.
    /// Initialized to page_size to avoid returning address 0.
    pub next_physical_base: usize,
    /// Bump allocator cursor for VA assignment.
    /// Initialized to page_size to avoid returning address 0.
    pub next_va_base: usize,
}

impl SpaceManager {
    /// Allocate pages from the root pool for a new Space or arena slab.
    ///
    /// D31: draws from unallocated physical memory. Returns an error
    /// if the pool is exhausted.
    ///
    /// D70: arena slab pages use this same path. When all slots on a
    /// slab page are freed, the page returns here via `return_pages`.
    pub fn allocate_pages(&mut self, count: usize) -> Result<usize, AllocError> {
        if count == 0 {
            return Ok(self.next_physical_base);
        }

        let bytes_needed = count
            .checked_mul(self.root_pool.page_size)
            .ok_or(AllocError::OutOfMemory)?;

        if bytes_needed > self.root_pool.free_bytes {
            return Err(AllocError::OutOfMemory);
        }

        self.root_pool.free_bytes -= bytes_needed;

        let base = self.next_physical_base;

        self.next_physical_base += bytes_needed;

        Ok(base)
    }

    /// Return pages to the root pool.
    ///
    /// D70: freed slab pages return here. D33: cascade-freed structural
    /// backing returns here (not to the caller — only top-level destroy
    /// returns Space to the destroyer).
    pub fn return_pages(&mut self, _base: usize, count: usize) {
        if count == 0 {
            return;
        }

        let bytes_returned = count
            .checked_mul(self.root_pool.page_size)
            .expect("return_pages: count * page_size overflow");

        self.root_pool.free_bytes = self
            .root_pool
            .free_bytes
            .saturating_add(bytes_returned)
            .min(self.root_pool.total_bytes);
    }

    /// Assign a VA base for a new Space (D26).
    ///
    /// D26: kernel-assigned, stable for the Space's lifetime. The
    /// policy for choosing VA bases is kernel-internal — this method
    /// encapsulates it.
    ///
    /// D41: merge may fail if no adjacent VA space is available. The
    /// assignment policy should minimize this by leaving headroom.
    pub fn assign_va(&mut self, size: usize) -> Result<VaAssignment, AllocError> {
        let page_mask = self.root_pool.page_size - 1;
        // Round up to page boundary; overflow means the request is impossibly large.
        let aligned_size = size.checked_add(page_mask).ok_or(AllocError::OutOfMemory)? & !page_mask;
        // A zero-size request still consumes one page of VA space.
        let consume = if aligned_size == 0 {
            self.root_pool.page_size
        } else {
            aligned_size
        };
        // VA budget: physical memory size, starting from page_size.
        let va_limit = self
            .root_pool
            .page_size
            .saturating_add(self.root_pool.total_bytes);
        let next = self
            .next_va_base
            .checked_add(consume)
            .ok_or(AllocError::OutOfMemory)?;

        if next > va_limit {
            return Err(AllocError::OutOfMemory);
        }

        let va_base = self.next_va_base;

        self.next_va_base = next;

        Ok(VaAssignment { va_base })
    }

    /// Compute the overhead of type conversion for a given Space size.
    ///
    /// D32: at split time, the parent shrinks by `child_size + overhead`.
    /// Overhead covers the page table subtree entries needed to map the
    /// new Space. First holder populates from reserved capacity;
    /// subsequent holders increment the reference count.
    pub fn type_conversion_overhead(&self, space_size: usize) -> usize {
        if space_size == 0 {
            return 0;
        }

        let page_size = self.root_pool.page_size;
        let page_count = space_size.saturating_add(page_size - 1) / page_size;
        // Each page table entry is 8 bytes; entries per table page = page_size / 8.
        let entries_per_table = page_size / 8;

        debug_assert!(entries_per_table > 0, "page_size must be >= 8");

        let mut tables: usize = 0;
        let mut remaining = page_count;

        while remaining > 0 {
            let level_tables = remaining.saturating_add(entries_per_table - 1) / entries_per_table;

            tables = tables.saturating_add(level_tables);
            if level_tables <= 1 {
                break;
            }

            remaining = level_tables;
        }

        tables.saturating_mul(page_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::AllocError;

    // ── Test helper ───────────────────────────────────────────────────

    /// Create a SpaceManager with 16 pages of 4096 bytes (65536 total bytes).
    ///
    /// Using a small, deterministic configuration makes all assertions
    /// about free_bytes, page counts, and exhaustion predictable.
    fn make_space_manager() -> SpaceManager {
        SpaceManager {
            root_pool: RootPool {
                total_bytes: 16 * 4096, // 65536
                free_bytes: 16 * 4096,
                page_size: 4096,
            },
            next_physical_base: 4096,
            next_va_base: 4096,
        }
    }

    /// Create a SpaceManager with a custom page count and page size.
    fn make_space_manager_with(page_count: usize, page_size: usize) -> SpaceManager {
        SpaceManager {
            root_pool: RootPool {
                total_bytes: page_count * page_size,
                free_bytes: page_count * page_size,
                page_size,
            },
            next_physical_base: page_size,
            next_va_base: page_size,
        }
    }

    // ── D31 — Root pool allocation ─────────────────────────────────────

    /// D31: allocate_pages draws from the root pool. After allocating N
    /// pages, free_bytes must decrease by N * page_size.
    #[test]
    fn test_d31_allocate_pages_decreases_free_bytes() {
        let mut sm = make_space_manager();
        let before = sm.root_pool.free_bytes;
        let page_size = sm.root_pool.page_size;

        sm.allocate_pages(3).expect("allocate 3 pages must succeed");

        assert_eq!(
            sm.root_pool.free_bytes,
            before - 3 * page_size,
            "free_bytes must decrease by count * page_size after allocation"
        );
    }

    /// D31: when the root pool has insufficient pages, allocate_pages
    /// returns AllocError::OutOfMemory.
    #[test]
    fn test_d31_allocate_pages_returns_error_when_exhausted() {
        let mut sm = make_space_manager();
        let total_pages = sm.root_pool.total_bytes / sm.root_pool.page_size;

        // Exhaust the pool completely.
        sm.allocate_pages(total_pages)
            .expect("full allocation must succeed");

        // One more page must fail.
        let result = sm.allocate_pages(1);

        assert_eq!(
            result,
            Err(AllocError::OutOfMemory),
            "allocate_pages on exhausted pool must return OutOfMemory"
        );
    }

    /// D31: allocate_pages returns a base address. The address must be
    /// non-zero and page-aligned (the kernel never hands out address 0
    /// as usable physical memory, and all physical pages are page-aligned).
    #[test]
    fn test_d31_allocate_pages_returns_base_address() {
        let mut sm = make_space_manager();
        let page_size = sm.root_pool.page_size;
        let base = sm
            .allocate_pages(1)
            .expect("allocate_pages must succeed on a fresh pool");

        // The returned address must be page-aligned.
        assert_eq!(
            base % page_size,
            0,
            "returned base address must be page-aligned (base={base:#x})"
        );

        // The returned address must be non-zero (physical address 0 is
        // never valid memory in this kernel).
        assert_ne!(base, 0, "returned base address must not be zero");
    }

    /// D31: allocating 0 pages is a degenerate but defined request.
    /// A correct implementation must handle it without panic — either
    /// succeeding (returning any base, free_bytes unchanged) or returning
    /// an explicit error. It must not silently corrupt state.
    #[test]
    fn test_d31_allocate_pages_zero_count() {
        let mut sm = make_space_manager();
        let before = sm.root_pool.free_bytes;

        match sm.allocate_pages(0) {
            Ok(_base) => {
                // Success path: free_bytes must not change for a zero-count
                // allocation.
                assert_eq!(
                    sm.root_pool.free_bytes, before,
                    "allocating 0 pages must not decrease free_bytes"
                );
            }
            Err(AllocError::OutOfMemory) => {
                // Explicit error is also acceptable. free_bytes must be
                // unchanged since nothing was allocated.
                assert_eq!(
                    sm.root_pool.free_bytes, before,
                    "failed 0-page allocation must not change free_bytes"
                );
            }
        }
    }

    // ── D70 — Arena slab page return ───────────────────────────────────

    /// D70: return_pages increases free_bytes by count * page_size.
    /// This is the mirror of allocate_pages.
    #[test]
    fn test_d70_return_pages_increases_free_bytes() {
        let mut sm = make_space_manager();
        let page_size = sm.root_pool.page_size;
        // Allocate first so we have a valid base to return.
        let base = sm.allocate_pages(4).expect("allocate 4 pages must succeed");
        let after_alloc = sm.root_pool.free_bytes;

        sm.return_pages(base, 4);

        assert_eq!(
            sm.root_pool.free_bytes,
            after_alloc + 4 * page_size,
            "return_pages must increase free_bytes by count * page_size"
        );
    }

    // ── D32 — Type conversion overhead ────────────────────────────────

    /// D32: type_conversion_overhead is a pure function. The same inputs
    /// must always produce the same output — no internal state mutation,
    /// no randomness.
    #[test]
    fn test_d32_type_conversion_overhead_deterministic() {
        let sm = make_space_manager();
        let size = 4 * 4096; // 4 pages
        let first = sm.type_conversion_overhead(size);
        let second = sm.type_conversion_overhead(size);
        let third = sm.type_conversion_overhead(size);

        assert_eq!(
            first, second,
            "type_conversion_overhead must be deterministic (same inputs, same output)"
        );
        assert_eq!(
            second, third,
            "type_conversion_overhead must be deterministic on repeated calls"
        );
    }

    /// D32: overhead is a function of space_size and page_size. Two
    /// different Space sizes must produce results consistent with being
    /// a function of size (larger spaces may need more page table entries,
    /// so overhead should be non-decreasing with size).
    #[test]
    fn test_d32_type_conversion_overhead_is_function_of_size_and_page_size() {
        let sm_4k = make_space_manager_with(16, 4096);
        let sm_16k = make_space_manager_with(16, 16384);
        let size_small = 4096;
        let size_large = 8 * 4096;
        let overhead_small = sm_4k.type_conversion_overhead(size_small);
        let overhead_large = sm_4k.type_conversion_overhead(size_large);

        // Overhead for a larger Space must be >= overhead for a smaller Space.
        // (A larger region requires at least as many page table entries.)
        assert!(
            overhead_large >= overhead_small,
            "overhead for a larger Space ({overhead_large}) must be \
             >= overhead for a smaller Space ({overhead_small})"
        );

        // A different page_size should produce a consistent (possibly different)
        // result from the same logical size — the overhead depends on both
        // space_size and page_size.
        let overhead_4k_page = sm_4k.type_conversion_overhead(size_small);
        let overhead_16k_page = sm_16k.type_conversion_overhead(size_small);
        // Both must be non-negative (they are usize, so always >= 0), and
        // they are allowed to differ since page size affects table structure.
        let _ = overhead_4k_page;
        let _ = overhead_16k_page;
        // Just verifying neither panics and both are accessible.
    }

    /// D32: overhead for a zero-size Space is a defined edge case.
    /// Must not panic. The result must be a usize (>= 0 by type).
    #[test]
    fn test_d32_type_conversion_overhead_zero_size() {
        let sm = make_space_manager();
        // Must not panic. The exact value is implementation-defined, but
        // it must be expressible as a usize.
        let overhead = sm.type_conversion_overhead(0);
        let _ = overhead;
    }

    /// D32: overhead for a single page. Must not panic and must be a
    /// valid usize (>= 0). A single page requires at minimum one leaf
    /// page table entry, so overhead >= 0.
    #[test]
    fn test_d32_type_conversion_overhead_one_page() {
        let sm = make_space_manager();
        let page_size = sm.root_pool.page_size;
        let overhead = sm.type_conversion_overhead(page_size);
        let _ = overhead;
        // Must not panic; overhead is a valid usize.
    }

    /// D32: overhead should scale with Space size. A Space that spans
    /// two page-table levels needs more backing entries than one that
    /// fits in a single level. At minimum, overhead must be
    /// non-decreasing as space_size grows.
    #[test]
    fn test_d32_type_conversion_overhead_scales_with_size() {
        let sm = make_space_manager();
        let page_size = sm.root_pool.page_size;
        let overhead_1_page = sm.type_conversion_overhead(page_size);
        let overhead_8_pages = sm.type_conversion_overhead(8 * page_size);
        let overhead_64_pages = sm.type_conversion_overhead(64 * page_size);

        assert!(
            overhead_8_pages >= overhead_1_page,
            "overhead for 8 pages ({overhead_8_pages}) must be >= overhead \
             for 1 page ({overhead_1_page})"
        );
        assert!(
            overhead_64_pages >= overhead_8_pages,
            "overhead for 64 pages ({overhead_64_pages}) must be >= overhead \
             for 8 pages ({overhead_8_pages})"
        );
    }

    // ── D26 — VA assignment ────────────────────────────────────────────

    /// D26: assign_va returns a VaAssignment with a va_base. The returned
    /// va_base must be non-zero (address 0 is reserved) and page-aligned.
    #[test]
    fn test_d26_assign_va_returns_valid_assignment() {
        let mut sm = make_space_manager();
        let page_size = sm.root_pool.page_size;
        let assignment = sm
            .assign_va(page_size)
            .expect("assign_va must succeed on a fresh SpaceManager");

        assert_ne!(
            assignment.va_base, 0,
            "assigned VA base must not be zero (address 0 is reserved)"
        );
        assert_eq!(
            assignment.va_base % page_size,
            0,
            "assigned VA base must be page-aligned (va_base={:#x})",
            assignment.va_base
        );
    }

    /// D26: multiple assign_va calls must return non-overlapping VA ranges.
    /// Each Space gets its own kernel-assigned VA base; two Spaces must not
    /// share the same base.
    #[test]
    fn test_d26_assign_va_returns_different_bases() {
        let mut sm = make_space_manager();
        let page_size = sm.root_pool.page_size;
        let a = sm
            .assign_va(page_size)
            .expect("first assign_va must succeed");
        let b = sm
            .assign_va(page_size)
            .expect("second assign_va must succeed");

        assert_ne!(
            a.va_base, b.va_base,
            "consecutive assign_va calls must return distinct VA bases \
             (both returned {:#x})",
            a.va_base
        );
    }

    /// D26: when VA address space is exhausted, assign_va returns
    /// AllocError::OutOfMemory. Use a SpaceManager whose VA budget is
    /// exactly two pages, then request three assignments.
    ///
    /// The VA space available to the kernel is finite. The SpaceManager
    /// tracks which VA ranges have been handed out and must refuse once
    /// all VA space is consumed.
    #[test]
    fn test_d26_assign_va_returns_error_when_no_va_space() {
        // Use a minimal SpaceManager: 2 pages physical, 2 pages VA budget.
        // The implementation must track VA assignments and eventually
        // refuse when the VA budget is exhausted.
        //
        // We drive it to exhaustion by requesting page-size assignments
        // in a loop. The loop terminates when OutOfMemory is returned
        // or after a large number of iterations (guarding against an
        // implementation that never exhausts — which would be wrong).
        let mut sm = make_space_manager_with(2, 4096);
        let page_size = sm.root_pool.page_size;
        let mut assigned = 0u32;
        let mut got_error = false;

        for _ in 0..10_000 {
            match sm.assign_va(page_size) {
                Ok(_) => {
                    assigned += 1;
                }
                Err(AllocError::OutOfMemory) => {
                    got_error = true;
                    break;
                }
            }
        }

        assert!(
            got_error,
            "assign_va must eventually return OutOfMemory when VA space is \
             exhausted (assigned {assigned} ranges without error)"
        );
    }

    // ── Conservation invariants ────────────────────────────────────────

    /// Conservation: allocate then return must leave free_bytes at its
    /// original value. Pages change membership, not quantity.
    #[test]
    fn test_conservation_allocate_return_roundtrip() {
        let mut sm = make_space_manager();
        let original_free = sm.root_pool.free_bytes;
        let base = sm.allocate_pages(5).expect("allocate 5 pages must succeed");

        sm.return_pages(base, 5);

        assert_eq!(
            sm.root_pool.free_bytes, original_free,
            "free_bytes must return to original value after allocate+return \
             roundtrip (expected {original_free}, got {})",
            sm.root_pool.free_bytes
        );
    }

    /// Conservation: after returning pages, free_bytes must never exceed
    /// total_bytes. Returning more than was allocated would violate the
    /// conservation invariant.
    #[test]
    fn test_conservation_free_bytes_never_exceeds_total() {
        let mut sm = make_space_manager();
        let total = sm.root_pool.total_bytes;
        let page_size = sm.root_pool.page_size;
        // Allocate 4 pages.
        let base = sm.allocate_pages(4).expect("allocate 4 pages must succeed");

        // Return exactly what was allocated.
        sm.return_pages(base, 4);

        assert!(
            sm.root_pool.free_bytes <= total,
            "free_bytes ({}) must never exceed total_bytes ({total}) after return",
            sm.root_pool.free_bytes
        );

        // Perform a single extra allocation and verify the invariant again.
        sm.allocate_pages(1)
            .expect("allocate 1 page after roundtrip must succeed");
        // Return 1 page.
        sm.return_pages(base, 1);

        assert!(
            sm.root_pool.free_bytes <= total,
            "free_bytes ({}) must never exceed total_bytes ({total}) after \
             second return",
            sm.root_pool.free_bytes
        );

        // Also verify free_bytes is representable as usize (always true)
        // and that it is <= total after a partial allocation.
        let _ = page_size;
    }

    // ── Adversarial tests ─────────────────────────────────────────────
    //
    // These tests assume the implementation has bugs. They stress
    // boundary conditions, off-by-one errors, integer overflow/underflow,
    // and conservation invariants across sequences of operations.

    /// Requesting one more page than the pool holds must return
    /// OutOfMemory and must not corrupt free_bytes.
    #[test]
    fn test_adversarial_sm_allocate_exceeds_pool_by_one() {
        let mut sm = make_space_manager();
        let total_pages = sm.root_pool.total_bytes / sm.root_pool.page_size;
        let before = sm.root_pool.free_bytes;
        let result = sm.allocate_pages(total_pages + 1);

        assert_eq!(
            result,
            Err(AllocError::OutOfMemory),
            "allocating total_pages+1 must return OutOfMemory"
        );
        assert_eq!(
            sm.root_pool.free_bytes, before,
            "a failed allocation must not change free_bytes"
        );
    }

    /// Allocating exactly all available pages must succeed.
    /// This is the off-by-one boundary: total_pages must be in-bounds.
    #[test]
    fn test_adversarial_sm_allocate_exactly_all_pages_succeeds() {
        let mut sm = make_space_manager();
        let total_pages = sm.root_pool.total_bytes / sm.root_pool.page_size;
        let result = sm.allocate_pages(total_pages);

        assert!(
            result.is_ok(),
            "allocating exactly total_pages must succeed, got {result:?}"
        );
        assert_eq!(
            sm.root_pool.free_bytes, 0,
            "free_bytes must be 0 after allocating all pages"
        );
    }

    /// return_pages with a count of 0 must be a no-op: free_bytes unchanged.
    #[test]
    fn test_adversarial_sm_return_zero_pages_is_noop() {
        let mut sm = make_space_manager();
        let base = sm.allocate_pages(4).expect("allocate 4 pages must succeed");
        let before = sm.root_pool.free_bytes;

        sm.return_pages(base, 0);

        assert_eq!(
            sm.root_pool.free_bytes, before,
            "return_pages with count=0 must not change free_bytes"
        );
    }

    /// Returning more pages than were allocated would push free_bytes above
    /// total_bytes — a conservation violation. The implementation must
    /// clamp or reject; it must never let free_bytes > total_bytes.
    #[test]
    fn test_adversarial_sm_return_more_than_allocated_does_not_exceed_total() {
        let mut sm = make_space_manager();
        let total = sm.root_pool.total_bytes;
        let page_size = sm.root_pool.page_size;
        // Allocate 4 pages, then try to return 8.
        let base = sm.allocate_pages(4).expect("allocate 4 pages must succeed");

        sm.return_pages(base, 8);

        assert!(
            sm.root_pool.free_bytes <= total,
            "free_bytes ({}) must never exceed total_bytes ({total}) after \
             over-return (returned 8 pages but only 4 were allocated)",
            sm.root_pool.free_bytes
        );

        let _ = page_size;
    }

    /// allocate → return → allocate: the pool must be usable again.
    /// Tests that the implementation does not mark freed pages as
    /// permanently unavailable.
    #[test]
    fn test_adversarial_sm_allocate_after_full_return_reuses_pool() {
        let mut sm = make_space_manager();
        let total_pages = sm.root_pool.total_bytes / sm.root_pool.page_size;
        let original_free = sm.root_pool.free_bytes;
        let base = sm
            .allocate_pages(total_pages)
            .expect("first full allocation must succeed");

        assert_eq!(
            sm.root_pool.free_bytes, 0,
            "free_bytes must be 0 after full allocation"
        );

        sm.return_pages(base, total_pages);

        assert_eq!(
            sm.root_pool.free_bytes, original_free,
            "free_bytes must be restored after full return"
        );

        let result = sm.allocate_pages(total_pages);

        assert!(
            result.is_ok(),
            "second full allocation after return must succeed, got {result:?}"
        );
    }

    /// Partial allocate then partial return then re-allocate the freed
    /// portion.  Tests fragmentation handling and re-use of partial ranges.
    #[test]
    fn test_adversarial_sm_partial_allocate_return_reallocate() {
        let mut sm = make_space_manager(); // 16 pages
        let page_size = sm.root_pool.page_size;
        // Allocate 10 pages.
        let base = sm
            .allocate_pages(10)
            .expect("allocate 10 pages must succeed");

        assert_eq!(
            sm.root_pool.free_bytes,
            6 * page_size,
            "6 pages must remain after allocating 10"
        );

        // Return 5 of the 10 allocated pages.
        sm.return_pages(base, 5);

        assert_eq!(
            sm.root_pool.free_bytes,
            11 * page_size,
            "11 pages must be free after returning 5"
        );

        // Allocate the 5 that were just returned.
        let result = sm.allocate_pages(5);

        assert!(
            result.is_ok(),
            "allocating 5 pages after partial return must succeed, got {result:?}"
        );
        assert_eq!(
            sm.root_pool.free_bytes,
            6 * page_size,
            "6 pages must remain after re-allocating the 5 returned pages"
        );
    }

    /// type_conversion_overhead with usize::MAX must not panic or overflow.
    /// ARM64 VA space is at most 48/52 bits; the function must handle any
    /// usize input without undefined behaviour.
    #[test]
    fn test_adversarial_sm_type_conversion_overhead_usize_max_no_panic() {
        let sm = make_space_manager();
        // Must not panic. Return value is implementation-defined.
        let overhead = sm.type_conversion_overhead(usize::MAX);
        let _ = overhead;
    }

    /// type_conversion_overhead with page_size - 1 (a sub-page size).
    /// A sub-page request is smaller than one page but must still produce
    /// a valid usize without panicking. The implementation might round up
    /// or treat it as a single page — either is acceptable; panic is not.
    #[test]
    fn test_adversarial_sm_type_conversion_overhead_sub_page_size() {
        let sm = make_space_manager();
        let page_size = sm.root_pool.page_size;
        // page_size is always >= 1 (4096), so page_size - 1 is valid.
        let overhead = sm.type_conversion_overhead(page_size - 1);
        let _ = overhead;
    }

    /// assign_va with size 0 must not panic. Either an Ok with some
    /// page-aligned base or Err(OutOfMemory) is acceptable; a panic is not.
    #[test]
    fn test_adversarial_sm_assign_va_zero_size_no_panic() {
        let mut sm = make_space_manager();

        match sm.assign_va(0) {
            Ok(assignment) => {
                // If a zero-size VA is accepted, the base must still be
                // page-aligned (alignment is unconditional).
                let page_size = sm.root_pool.page_size;

                assert_eq!(
                    assignment.va_base % page_size,
                    0,
                    "VA base returned for 0-size must be page-aligned \
                     (va_base={:#x})",
                    assignment.va_base
                );
            }
            Err(AllocError::OutOfMemory) => {
                // Explicit rejection is also acceptable.
            }
        }
    }

    /// assign_va with usize::MAX must not panic or overflow. The kernel's
    /// VA space is smaller than usize::MAX; the implementation must reject
    /// without crashing.
    #[test]
    fn test_adversarial_sm_assign_va_usize_max_no_panic() {
        let mut sm = make_space_manager();

        // Expect OutOfMemory (no VA range of size usize::MAX exists), but
        // must not panic regardless.
        match sm.assign_va(usize::MAX) {
            Ok(_) => {
                // Unexpected success is a logic error in the implementation,
                // but the test records it without panicking so the failure
                // message is clear.
                panic!(
                    "assign_va(usize::MAX) returned Ok — implementation \
                        did not reject an impossible VA request"
                );
            }
            Err(AllocError::OutOfMemory) => {
                // Correct: no VA range of size usize::MAX is available.
            }
        }
    }

    /// Two consecutive allocate_pages calls must return non-overlapping
    /// physical ranges.  The second base must be >= first_base + count * page_size.
    #[test]
    fn test_adversarial_sm_two_allocations_are_non_overlapping() {
        let mut sm = make_space_manager();
        let page_size = sm.root_pool.page_size;
        let count = 4usize;
        let base_a = sm
            .allocate_pages(count)
            .expect("first allocate_pages must succeed");
        let base_b = sm
            .allocate_pages(count)
            .expect("second allocate_pages must succeed");
        // The ranges [base_a, base_a + count*page_size) and
        // [base_b, base_b + count*page_size) must not overlap.
        // Since both are page-aligned ranges from the same pool, one must
        // start at or after the other ends.
        let end_a = base_a + count * page_size;
        let end_b = base_b + count * page_size;
        let overlaps = base_a < end_b && base_b < end_a;

        assert!(
            !overlaps,
            "two consecutive allocations overlap: \
             [{base_a:#x}, {end_a:#x}) and [{base_b:#x}, {end_b:#x})"
        );
    }

    /// Allocate all pages, return all pages, then allocate all pages again.
    /// The pool must function correctly across a full allocate/return/allocate cycle.
    #[test]
    fn test_adversarial_sm_full_cycle_allocate_return_allocate() {
        let mut sm = make_space_manager();
        let total_pages = sm.root_pool.total_bytes / sm.root_pool.page_size;
        let base = sm
            .allocate_pages(total_pages)
            .expect("first full allocation must succeed");

        sm.return_pages(base, total_pages);

        let result = sm.allocate_pages(total_pages);

        assert!(
            result.is_ok(),
            "second full allocation after full return must succeed, got {result:?}"
        );
        assert_eq!(
            sm.root_pool.free_bytes, 0,
            "free_bytes must be 0 after second full allocation"
        );
    }

    /// Arithmetic overflow guard: count * page_size overflows usize.
    /// For page_size = 4096 (2^12), usize::MAX / 4096 + 1 overflows.
    /// The implementation must not panic or silently wrap around and
    /// succeed with a bogus allocation.
    #[test]
    fn test_adversarial_sm_allocate_count_times_page_size_overflow() {
        let mut sm = make_space_manager();
        let page_size = sm.root_pool.page_size; // 4096
        // This count, when multiplied by page_size, overflows a 64-bit usize.
        let overflowing_count = usize::MAX / page_size + 1;
        let result = sm.allocate_pages(overflowing_count);

        // The only acceptable outcomes are OutOfMemory or a checked-overflow
        // rejection. It must NEVER succeed (that would mean the pool somehow
        // has more bytes than a usize can represent).
        assert_eq!(
            result,
            Err(AllocError::OutOfMemory),
            "allocate_pages with an overflowing count must return OutOfMemory, \
             got {result:?}"
        );
        // free_bytes must not have changed.
        assert_eq!(
            sm.root_pool.free_bytes, sm.root_pool.total_bytes,
            "a rejected overflow allocation must not change free_bytes"
        );
    }

    /// Single-page pool: allocate the one page, return it, allocate again.
    /// Exercises the degenerate minimum-pool case and verifies the pool
    /// remains usable after a round-trip.
    #[test]
    fn test_adversarial_sm_single_page_pool_round_trip() {
        let mut sm = make_space_manager_with(1, 4096);
        let page_size = sm.root_pool.page_size;
        // Allocate the single page.
        let base = sm
            .allocate_pages(1)
            .expect("allocate 1 page from single-page pool must succeed");

        assert_eq!(
            sm.root_pool.free_bytes, 0,
            "free_bytes must be 0 after allocating the only page"
        );
        // Requesting any more pages must fail.
        assert_eq!(
            sm.allocate_pages(1),
            Err(AllocError::OutOfMemory),
            "second allocation on exhausted single-page pool must return OutOfMemory"
        );

        // Return the page.
        sm.return_pages(base, 1);

        assert_eq!(
            sm.root_pool.free_bytes, page_size,
            "free_bytes must be page_size after returning the single page"
        );

        // Allocate again — pool must be reusable.
        let result = sm.allocate_pages(1);

        assert!(
            result.is_ok(),
            "re-allocation from single-page pool after return must succeed, \
             got {result:?}"
        );
    }

    /// Large page size (64 KiB granule): pool with 64K pages.
    /// Verifies the implementation uses the configured page_size rather
    /// than a hard-coded 4096.
    #[test]
    fn test_adversarial_sm_large_page_size_64k() {
        const PAGE_64K: usize = 65536;
        let mut sm = make_space_manager_with(8, PAGE_64K);
        let total_pages = sm.root_pool.total_bytes / sm.root_pool.page_size;

        assert_eq!(total_pages, 8, "pool must have 8 pages of 64K each");

        let base = sm
            .allocate_pages(4)
            .expect("allocate 4 pages (64K each) must succeed");

        // Base must be aligned to 64K.
        assert_eq!(
            base % PAGE_64K,
            0,
            "allocated base must be 64K-aligned for a 64K page pool \
             (base={base:#x})"
        );
        assert_eq!(
            sm.root_pool.free_bytes,
            4 * PAGE_64K,
            "4 pages * 64K must remain free after allocating 4 pages"
        );

        sm.return_pages(base, 4);

        assert_eq!(
            sm.root_pool.free_bytes,
            8 * PAGE_64K,
            "all 8 pages must be free after returning 4"
        );
    }

    /// Every VA returned by assign_va must be page-aligned.
    /// Tests multiple calls and verifies all returned bases have
    /// zero remainder modulo page_size.
    #[test]
    fn test_adversarial_sm_assign_va_page_alignment_all_results() {
        let mut sm = make_space_manager();
        let page_size = sm.root_pool.page_size;
        // Make 8 assignments of varying sizes; all must be page-aligned.
        let sizes = [
            page_size,
            2 * page_size,
            3 * page_size,
            page_size,
            4 * page_size,
            page_size,
            2 * page_size,
            page_size,
        ];

        for (i, &size) in sizes.iter().enumerate() {
            match sm.assign_va(size) {
                Ok(assignment) => {
                    assert_eq!(
                        assignment.va_base % page_size,
                        0,
                        "assign_va call #{i} returned unaligned VA base \
                         ({:#x}) for size {size} (page_size={page_size})",
                        assignment.va_base
                    );
                    assert_ne!(
                        assignment.va_base, 0,
                        "assign_va call #{i} returned zero VA base for \
                         size {size}"
                    );
                }
                Err(AllocError::OutOfMemory) => {
                    // VA space exhaustion is acceptable after enough calls.
                    break;
                }
            }
        }
    }
}
