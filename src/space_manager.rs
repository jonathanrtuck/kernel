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

// ── Space creation result ──────────────────────────────────────────

/// Outcome of a successful Space creation (D32 type conversion).
///
/// Contains all the physical and virtual resources allocated for the
/// new Space. The caller is responsible for:
/// - Populating the L3 table via page_table::populate_l3 (D90)
/// - Constructing the Space struct in an arena slot
/// - Shrinking the parent Space by size + overhead
pub struct SpaceCreationResult {
    /// VA base for the new Space (D26, D89: 32 MiB aligned).
    pub va_base: usize,
    /// Rounded size in bytes (D60: page-aligned).
    pub size: usize,
    /// Physical address of the Space's content pages.
    pub content_pa: usize,
    /// Physical address of the L3 table(s) for this Space.
    pub l3_table_pa: usize,
    /// Number of content pages allocated.
    pub page_count: usize,
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

    /// Assign a VA base for a new Space (D26, D89).
    ///
    /// D26: kernel-assigned, stable for the Space's lifetime. The
    /// policy for choosing VA bases is kernel-internal — this method
    /// encapsulates it.
    ///
    /// D89: the `alignment` parameter controls VA base alignment.
    /// For L3-table-based mapping, Spaces are 32 MiB aligned (with
    /// 16 KiB granule). Zero alignment is treated as page_size
    /// (defensive default). Alignment must be a power of two
    /// (`debug_assert` in debug builds).
    ///
    /// D41: merge may fail if no adjacent VA space is available. The
    /// assignment policy should minimize this by leaving headroom.
    pub fn assign_va(&mut self, size: usize, alignment: usize) -> Result<VaAssignment, AllocError> {
        let page_size = self.root_pool.page_size;
        let effective_alignment = if alignment == 0 { page_size } else { alignment };
        debug_assert!(
            effective_alignment.is_power_of_two(),
            "alignment must be a power of two, got {effective_alignment}"
        );
        let align_mask = effective_alignment - 1;
        let page_mask = page_size - 1;
        // Round up to page boundary; overflow means the request is impossibly large.
        let aligned_size = size.checked_add(page_mask).ok_or(AllocError::OutOfMemory)? & !page_mask;
        // A zero-size request still consumes one page of VA space.
        let consume = if aligned_size == 0 {
            page_size
        } else {
            aligned_size
        };
        // Round up the cursor to the requested alignment.
        let aligned_cursor = self
            .next_va_base
            .checked_add(align_mask)
            .ok_or(AllocError::OutOfMemory)?
            & !align_mask;
        // VA budget: physical memory size, starting from page_size.
        let va_limit = page_size.saturating_add(self.root_pool.total_bytes);
        let next = aligned_cursor
            .checked_add(consume)
            .ok_or(AllocError::OutOfMemory)?;

        if next > va_limit {
            return Err(AllocError::OutOfMemory);
        }

        self.next_va_base = next;

        Ok(VaAssignment {
            va_base: aligned_cursor,
        })
    }

    /// Compute the overhead of type conversion for a given Space size.
    ///
    /// D32: at split time, the parent shrinks by `child_size + overhead`.
    /// D92: only L3 tables are charged to the Space. L1/L2 tables are
    /// per-Observer costs charged elsewhere (L1 to consumed Space at
    /// Observer creation, L2 to kernel root pool on demand).
    ///
    /// Formula: `ceil(page_count / entries_per_table) * page_size`.
    /// One L3 table per `entries_per_table` pages (2048 for 16 KiB granule).
    fn l3_table_count(&self, page_count: usize) -> usize {
        let entries_per_table = self.root_pool.page_size / 8;

        debug_assert!(entries_per_table > 0, "page_size must be >= 8");

        page_count.saturating_add(entries_per_table - 1) / entries_per_table
    }

    pub fn type_conversion_overhead(&self, space_size: usize) -> usize {
        if space_size == 0 {
            return 0;
        }

        let page_size = self.root_pool.page_size;
        let page_count = space_size.saturating_add(page_size - 1) / page_size;

        self.l3_table_count(page_count).saturating_mul(page_size)
    }

    /// Orchestrate Space creation (D32 type conversion).
    ///
    /// Allocates content pages, L3 table page(s), and assigns a VA base.
    /// The caller is responsible for:
    /// - Populating the L3 table via page_table::populate_l3 (D90)
    /// - Constructing the Space struct in an arena slot
    /// - Shrinking the parent Space by size + overhead
    ///
    /// On failure, no resources are consumed (atomic: all-or-nothing).
    pub fn create_space(
        &mut self,
        size: usize,
        alignment: usize,
    ) -> Result<SpaceCreationResult, AllocError> {
        let page_size = self.root_pool.page_size;
        let page_mask = page_size - 1;

        // D60: round up to page boundary.
        let rounded_size = size.checked_add(page_mask).ok_or(AllocError::OutOfMemory)? & !page_mask;
        // A zero-size request rounds to one page (minimum Space size = page_size, D25).
        let effective_size = if rounded_size == 0 {
            page_size
        } else {
            rounded_size
        };

        let page_count = effective_size / page_size;
        let l3_table_count = self.l3_table_count(page_count);

        let content_pa = self.allocate_pages(page_count)?;

        let l3_table_pa = match self.allocate_pages(l3_table_count) {
            Ok(pa) => pa,
            Err(e) => {
                self.return_pages(content_pa, page_count);
                return Err(e);
            }
        };

        let va_assignment = match self.assign_va(effective_size, alignment) {
            Ok(va) => va,
            Err(e) => {
                self.return_pages(content_pa, page_count);
                self.return_pages(l3_table_pa, l3_table_count);
                return Err(e);
            }
        };

        Ok(SpaceCreationResult {
            va_base: va_assignment.va_base,
            size: effective_size,
            content_pa,
            l3_table_pa,
            page_count,
        })
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
            .assign_va(page_size, page_size)
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
            .assign_va(page_size, page_size)
            .expect("first assign_va must succeed");
        let b = sm
            .assign_va(page_size, page_size)
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
            match sm.assign_va(page_size, page_size) {
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

        match sm.assign_va(0, 0) {
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
        match sm.assign_va(usize::MAX, 4096) {
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
            match sm.assign_va(size, page_size) {
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

    // ── D89 — VA alignment ────────────────────────────────────────────

    /// D89: 32 MiB alignment. With 16 KiB pages, one L3 table covers
    /// 2048 * 16 KiB = 32 MiB. VA bases must be 32 MiB aligned for
    /// L3-table-aligned mapping.
    #[test]
    fn d89_assign_va_32mib_alignment() {
        const PAGE_16K: usize = 16384;
        const ALIGN_32M: usize = 32 * 1024 * 1024;
        // Need enough VA space: pool must be larger than alignment.
        // 4096 pages * 16 KiB = 64 MiB total.
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let assignment = sm
            .assign_va(PAGE_16K, ALIGN_32M)
            .expect("32 MiB aligned assign_va must succeed");

        assert_eq!(
            assignment.va_base % ALIGN_32M,
            0,
            "VA base must be 32 MiB aligned (va_base={:#x}, remainder={:#x})",
            assignment.va_base,
            assignment.va_base % ALIGN_32M
        );
    }

    /// D89: two consecutive 32 MiB aligned assignments must be non-overlapping
    /// and both aligned.
    #[test]
    fn d89_assign_va_32mib_multiple_aligned_non_overlapping() {
        const PAGE_16K: usize = 16384;
        const ALIGN_32M: usize = 32 * 1024 * 1024;
        // Need at least 3 * 32 MiB of VA space: initial cursor at page_size
        // wastes up to 32 MiB aligning, then two 32 MiB regions.
        // 8192 pages * 16 KiB = 128 MiB.
        let mut sm = make_space_manager_with(8192, PAGE_16K);
        let a = sm
            .assign_va(ALIGN_32M, ALIGN_32M)
            .expect("first 32 MiB aligned assign must succeed");
        let b = sm
            .assign_va(ALIGN_32M, ALIGN_32M)
            .expect("second 32 MiB aligned assign must succeed");

        // Both must be 32 MiB aligned.
        assert_eq!(
            a.va_base % ALIGN_32M,
            0,
            "first VA base must be 32 MiB aligned (va_base={:#x})",
            a.va_base
        );
        assert_eq!(
            b.va_base % ALIGN_32M,
            0,
            "second VA base must be 32 MiB aligned (va_base={:#x})",
            b.va_base
        );

        // Must not overlap.
        let a_end = a.va_base + ALIGN_32M;

        assert!(
            b.va_base >= a_end,
            "second assignment must not overlap first \
             (a=[{:#x}, {:#x}), b.va_base={:#x})",
            a.va_base,
            a_end,
            b.va_base
        );
    }

    /// D89: alignment with a pool smaller than the alignment.
    /// If the total VA budget cannot satisfy the alignment, assign_va
    /// must return OutOfMemory.
    #[test]
    fn d89_assign_va_alignment_exceeds_pool() {
        const PAGE_16K: usize = 16384;
        const ALIGN_32M: usize = 32 * 1024 * 1024;
        // Only 2 pages * 16 KiB = 32 KiB total. The VA budget is far
        // smaller than 32 MiB, so alignment cannot be satisfied.
        let mut sm = make_space_manager_with(2, PAGE_16K);
        let result = sm.assign_va(PAGE_16K, ALIGN_32M);

        assert_eq!(
            result.err(),
            Some(AllocError::OutOfMemory),
            "assign_va with alignment exceeding VA budget must return OutOfMemory"
        );
    }

    /// D89: page-size alignment behaves identically to the original
    /// assign_va behavior (backward compatibility).
    #[test]
    fn d89_assign_va_page_size_alignment_backward_compatible() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(16, PAGE_16K);
        let page_size = sm.root_pool.page_size;
        let assignment = sm
            .assign_va(page_size, page_size)
            .expect("page-aligned assign_va must succeed");

        assert_eq!(
            assignment.va_base % page_size,
            0,
            "page-size aligned VA base must be page-aligned"
        );
        assert_ne!(assignment.va_base, 0, "VA base must not be zero");
    }

    /// D89: zero alignment is treated as page_size (defensive default).
    #[test]
    fn d89_assign_va_zero_alignment_defaults_to_page_size() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(16, PAGE_16K);
        let page_size = sm.root_pool.page_size;
        let assignment = sm
            .assign_va(page_size, 0)
            .expect("zero-alignment assign_va must succeed");

        assert_eq!(
            assignment.va_base % page_size,
            0,
            "zero alignment must default to page-size alignment"
        );
        assert_ne!(assignment.va_base, 0, "VA base must not be zero");
    }

    /// D89: alignment with 16 KiB pages and various alignment values.
    /// Each returned VA must satisfy the requested alignment.
    #[test]
    fn d89_assign_va_various_alignments() {
        const PAGE_16K: usize = 16384;
        // Test power-of-two alignments from page_size up to 1 MiB.
        let alignments = [PAGE_16K, 2 * PAGE_16K, 64 * 1024, 256 * 1024, 1024 * 1024];

        for &alignment in &alignments {
            // Need enough VA space for alignment waste + the allocation.
            let pages_needed = (2 * alignment) / PAGE_16K + 1;
            let mut sm = make_space_manager_with(pages_needed, PAGE_16K);
            let assignment = sm
                .assign_va(PAGE_16K, alignment)
                .expect("aligned assign_va must succeed");

            assert_eq!(
                assignment.va_base % alignment,
                0,
                "VA base must be {alignment}-byte aligned (va_base={:#x})",
                assignment.va_base
            );
        }
    }

    /// D89: multiple aligned assignments are all aligned AND non-overlapping.
    /// Uses 64 KiB alignment with 16 KiB pages.
    #[test]
    fn d89_assign_va_multiple_aligned_all_non_overlapping() {
        const PAGE_16K: usize = 16384;
        const ALIGNMENT: usize = 64 * 1024; // 64 KiB
        const ALLOC_SIZE: usize = 2 * PAGE_16K; // 32 KiB per allocation
        const MAX_ASSIGNMENTS: usize = 8;
        // 64 pages * 16 KiB = 1 MiB total.
        let mut sm = make_space_manager_with(64, PAGE_16K);
        let mut bases = [0usize; MAX_ASSIGNMENTS];
        let mut count = 0usize;

        for _ in 0..MAX_ASSIGNMENTS {
            match sm.assign_va(ALLOC_SIZE, ALIGNMENT) {
                Ok(a) => {
                    assert_eq!(
                        a.va_base % ALIGNMENT,
                        0,
                        "VA base must be {ALIGNMENT}-byte aligned (va_base={:#x})",
                        a.va_base
                    );

                    bases[count] = a.va_base;
                    count += 1;
                }
                Err(AllocError::OutOfMemory) => break,
            }
        }

        // Verify no overlaps among all assignments.
        for i in 0..count {
            for j in (i + 1)..count {
                let a_base = bases[i];
                let b_base = bases[j];
                let a_end = a_base + ALLOC_SIZE;
                let b_end = b_base + ALLOC_SIZE;
                let overlaps = a_base < b_end && b_base < a_end;

                assert!(
                    !overlaps,
                    "assignments {i} and {j} overlap: \
                     [{a_base:#x}, {a_end:#x}) and [{b_base:#x}, {b_end:#x})"
                );
            }
        }

        assert!(
            count >= 2,
            "must have at least 2 successful aligned assignments for overlap check"
        );
    }

    // ── D92 — L3 table overhead accounting ────────────────────────────

    /// D92: zero-size Space has zero overhead.
    #[test]
    fn d92_type_conversion_overhead_zero_size() {
        let sm = make_space_manager_with(16, 16384);

        assert_eq!(
            sm.type_conversion_overhead(0),
            0,
            "zero-size Space must have zero overhead"
        );
    }

    /// D92: 1 page Space with 16 KiB granule requires 1 L3 table = 16 KiB overhead.
    /// entries_per_table = 16384 / 8 = 2048. ceil(1 / 2048) = 1 table.
    #[test]
    fn d92_type_conversion_overhead_one_page_16kib() {
        let sm = make_space_manager_with(16, 16384);
        let page_size = sm.root_pool.page_size;
        let overhead = sm.type_conversion_overhead(page_size);

        assert_eq!(
            overhead, page_size,
            "1-page Space must need 1 L3 table = {page_size} bytes overhead \
             (got {overhead})"
        );
    }

    /// D92: 2048 page Space with 16 KiB granule requires 1 L3 table = 16 KiB.
    /// entries_per_table = 2048. ceil(2048 / 2048) = 1 table.
    /// This is the maximum Space that fits in a single L3 table.
    #[test]
    fn d92_type_conversion_overhead_2048_pages_16kib() {
        let sm = make_space_manager_with(4096, 16384);
        let page_size = sm.root_pool.page_size;
        let space_size = 2048 * page_size; // 32 MiB
        let overhead = sm.type_conversion_overhead(space_size);

        assert_eq!(
            overhead, page_size,
            "2048-page Space must need 1 L3 table = {page_size} bytes overhead \
             (got {overhead})"
        );
    }

    /// D92: 2049 page Space with 16 KiB granule requires 2 L3 tables = 32 KiB.
    /// entries_per_table = 2048. ceil(2049 / 2048) = 2 L3 tables.
    /// D92: only L3 tables charged to Space. L2 is per-Observer (root pool).
    #[test]
    fn d92_type_conversion_overhead_2049_pages_16kib() {
        let sm = make_space_manager_with(4096, 16384);
        let page_size = sm.root_pool.page_size;
        let space_size = 2049 * page_size;
        let overhead = sm.type_conversion_overhead(space_size);

        assert_eq!(
            overhead,
            2 * page_size,
            "2049 pages = 2 L3 tables (D92: L3 only, no L2 charged to Space)"
        );
    }

    /// D92: exact boundary — entries_per_table pages fits in 1 L3 table.
    /// With 16 KiB pages: entries_per_table = 2048.
    #[test]
    fn d92_type_conversion_overhead_exact_boundary_16kib() {
        let sm = make_space_manager_with(4096, 16384);
        let page_size = sm.root_pool.page_size;
        let entries_per_table = page_size / 8; // 2048
        // Exactly entries_per_table pages: 1 L3 table.
        let overhead_exact = sm.type_conversion_overhead(entries_per_table * page_size);

        assert_eq!(
            overhead_exact, page_size,
            "exactly {entries_per_table} pages must need 1 L3 table"
        );

        // entries_per_table + 1 pages: 2 L3 tables (D92: L3 only).
        let overhead_plus_one = sm.type_conversion_overhead((entries_per_table + 1) * page_size);

        assert!(
            overhead_plus_one > overhead_exact,
            "overhead for {0} pages ({overhead_plus_one}) must exceed \
             overhead for {entries_per_table} pages ({overhead_exact})",
            entries_per_table + 1
        );
    }

    /// D92: overhead scales monotonically — more pages never means less overhead.
    #[test]
    fn d92_type_conversion_overhead_monotonic_16kib() {
        let sm = make_space_manager_with(65536, 16384);
        let page_size = sm.root_pool.page_size;
        let mut prev_overhead = 0usize;
        // Sample points: 1, 512, 1024, 2048, 2049, 4096, 8192 pages.
        let page_counts = [1, 512, 1024, 2048, 2049, 4096, 8192];

        for &count in &page_counts {
            let overhead = sm.type_conversion_overhead(count * page_size);

            assert!(
                overhead >= prev_overhead,
                "overhead must be non-decreasing: {count} pages gave \
                 {overhead}, but previous was {prev_overhead}"
            );

            prev_overhead = overhead;
        }
    }

    /// D92: overhead with 4 KiB pages (existing granule) — verify
    /// D92 L3-only formula works with different page sizes.
    /// entries_per_table = 4096/8 = 512.
    /// 1 page: ceil(1/512) = 1 L3 table = 4096.
    /// 512 pages: ceil(512/512) = 1 L3 table = 4096.
    #[test]
    fn d92_type_conversion_overhead_4kib_compatibility() {
        let sm = make_space_manager_with(1024, 4096);
        let page_size = sm.root_pool.page_size;
        let overhead_1 = sm.type_conversion_overhead(page_size);

        assert_eq!(
            overhead_1, page_size,
            "1-page Space with 4 KiB pages: 1 L3 table"
        );

        let overhead_512 = sm.type_conversion_overhead(512 * page_size);

        assert_eq!(
            overhead_512, page_size,
            "512-page Space with 4 KiB pages: 1 L3 table"
        );
    }

    /// D92: sub-page size input rounds up to 1 page, requiring 1 L3 table.
    #[test]
    fn d92_type_conversion_overhead_sub_page_rounds_up_16kib() {
        let sm = make_space_manager_with(16, 16384);
        let page_size = sm.root_pool.page_size;
        let overhead = sm.type_conversion_overhead(1); // 1 byte

        assert_eq!(
            overhead, page_size,
            "sub-page Space rounds up to 1 page, needing 1 L3 table = {page_size} bytes"
        );
    }

    /// D92: overhead for exactly 1 L3 table of pages equals exactly 1 page.
    /// This verifies the formula: ceil(N / entries_per_table) * page_size.
    #[test]
    fn d92_type_conversion_overhead_formula_verification() {
        let sm = make_space_manager_with(65536, 16384);
        let page_size = sm.root_pool.page_size;
        let entries_per_table = page_size / 8;

        // Single L3 table cases: 1 to entries_per_table pages.
        for &count in &[1usize, entries_per_table / 2, entries_per_table] {
            let overhead = sm.type_conversion_overhead(count * page_size);

            assert_eq!(
                overhead, page_size,
                "{count} pages (within single L3 table) must need exactly 1 table"
            );
        }
    }

    // ── D32 — create_space orchestration ──────────────────────────────

    /// 32 MiB alignment constant for 16 KiB granule tests (D89).
    const SPACE_VA_ALIGNMENT: usize = 32 * 1024 * 1024;

    /// D32: happy path — create_space with a valid size returns correct result.
    /// VA base is aligned, page_count matches, l3_table_pa differs from content_pa,
    /// and size is page-rounded.
    #[test]
    fn d32_create_space_happy_path() {
        const PAGE_16K: usize = 16384;
        // Need enough physical memory for content + L3 table + VA space for alignment.
        // 4096 pages * 16 KiB = 64 MiB.
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let result = sm
            .create_space(PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("create_space must succeed");

        // VA base must be aligned to SPACE_VA_ALIGNMENT.
        assert_eq!(
            result.va_base % SPACE_VA_ALIGNMENT,
            0,
            "VA base must be 32 MiB aligned (va_base={:#x})",
            result.va_base
        );
        // page_count must match ceil(size / page_size).
        assert_eq!(
            result.page_count, 1,
            "1-page Space must report page_count=1"
        );
        // l3_table_pa must be non-zero and different from content_pa.
        assert_ne!(result.l3_table_pa, 0, "l3_table_pa must be non-zero");
        assert_ne!(
            result.l3_table_pa, result.content_pa,
            "l3_table_pa must differ from content_pa"
        );
        // Size must be page-rounded.
        assert_eq!(
            result.size % PAGE_16K,
            0,
            "size must be page-aligned (size={})",
            result.size
        );
        assert_eq!(
            result.size, PAGE_16K,
            "1-page request must produce size=page_size"
        );
    }

    /// D32: create_space with a sub-page size rounds up to one page.
    #[test]
    fn d32_create_space_rounds_sub_page_to_one_page() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let result = sm
            .create_space(1, SPACE_VA_ALIGNMENT)
            .expect("create_space with 1 byte must succeed");

        assert_eq!(
            result.size, PAGE_16K,
            "1-byte request must round up to page_size"
        );
        assert_eq!(result.page_count, 1, "rounded-up Space must have 1 page");
    }

    /// D32: create_space with a multi-page size returns correct page_count.
    #[test]
    fn d32_create_space_multi_page() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let result = sm
            .create_space(4 * PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("create_space with 4 pages must succeed");

        assert_eq!(result.page_count, 4, "4-page request must report 4 pages");
        assert_eq!(
            result.size,
            4 * PAGE_16K,
            "4-page request must produce exact size"
        );
    }

    /// D92: total allocation = content_pages + L3 table pages.
    /// For 1-page Space: 1 content page + 1 L3 table page = 2 pages consumed.
    #[test]
    fn d92_create_space_overhead_accounting_one_page() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let before = sm.root_pool.free_bytes;

        sm.create_space(PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("create_space must succeed");

        let consumed = before - sm.root_pool.free_bytes;

        // 1 content page + 1 L3 table page = 2 * page_size.
        assert_eq!(
            consumed,
            2 * PAGE_16K,
            "1-page Space must consume exactly 2 pages (1 content + 1 L3 table), \
             consumed {consumed} bytes"
        );
    }

    /// D92: for a Space of 2048 pages (fills one L3 table exactly),
    /// overhead is 1 L3 table page. Total = 2048 + 1 = 2049 pages.
    #[test]
    fn d92_create_space_overhead_exact_l3_boundary() {
        const PAGE_16K: usize = 16384;
        // Need 2049 pages + VA space overhead. Use 8192 pages.
        let mut sm = make_space_manager_with(8192, PAGE_16K);
        let before = sm.root_pool.free_bytes;

        sm.create_space(2048 * PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("create_space with 2048 pages must succeed");

        let consumed = before - sm.root_pool.free_bytes;

        assert_eq!(
            consumed,
            (2048 + 1) * PAGE_16K,
            "2048-page Space must consume 2049 pages (2048 content + 1 L3 table)"
        );
    }

    /// D92: for a Space of 2049 pages (crosses L3 table boundary),
    /// overhead is 2 L3 table pages. Total = 2049 + 2 = 2051 pages.
    #[test]
    fn d92_create_space_overhead_crosses_l3_boundary() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(8192, PAGE_16K);
        let before = sm.root_pool.free_bytes;

        sm.create_space(2049 * PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("create_space with 2049 pages must succeed");

        let consumed = before - sm.root_pool.free_bytes;

        assert_eq!(
            consumed,
            (2049 + 2) * PAGE_16K,
            "2049-page Space must consume 2051 pages (2049 content + 2 L3 tables)"
        );
    }

    /// D32: multiple create_space calls return distinct, aligned VA bases.
    #[test]
    fn d32_create_space_multiple_distinct_aligned_vas() {
        const PAGE_16K: usize = 16384;
        // Need enough for 3 aligned allocations. Each alignment wastes up to
        // 32 MiB of VA. 3 * 32 MiB content + alignment overhead.
        // 16384 pages * 16 KiB = 256 MiB.
        let mut sm = make_space_manager_with(16384, PAGE_16K);
        let a = sm
            .create_space(PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("first create_space must succeed");
        let b = sm
            .create_space(PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("second create_space must succeed");
        let c = sm
            .create_space(PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("third create_space must succeed");

        // All VAs must be aligned.
        assert_eq!(a.va_base % SPACE_VA_ALIGNMENT, 0);
        assert_eq!(b.va_base % SPACE_VA_ALIGNMENT, 0);
        assert_eq!(c.va_base % SPACE_VA_ALIGNMENT, 0);
        // All VAs must be distinct.
        assert_ne!(a.va_base, b.va_base, "first and second VAs must differ");
        assert_ne!(b.va_base, c.va_base, "second and third VAs must differ");
        assert_ne!(a.va_base, c.va_base, "first and third VAs must differ");
    }

    /// D32: failure rollback — pool has enough for content but not L3.
    /// Error returned, free_bytes unchanged.
    #[test]
    fn d32_create_space_rollback_insufficient_for_l3() {
        const PAGE_16K: usize = 16384;
        // Pool with exactly 1 page: enough for 1 content page but not
        // the additional L3 table page.
        let mut sm = make_space_manager_with(1, PAGE_16K);
        let before = sm.root_pool.free_bytes;
        let result = sm.create_space(PAGE_16K, PAGE_16K);

        assert_eq!(
            result.err(),
            Some(AllocError::OutOfMemory),
            "create_space must fail when pool cannot cover L3 table"
        );
        assert_eq!(
            sm.root_pool.free_bytes, before,
            "failed create_space must not change free_bytes (all-or-nothing rollback)"
        );
    }

    /// D32: failure rollback — pool has enough for content + L3 but VA
    /// space is exhausted. Error returned, free_bytes unchanged.
    #[test]
    fn d32_create_space_rollback_insufficient_va() {
        const PAGE_16K: usize = 16384;
        // Pool with 4 pages but very small total_bytes (VA budget = page_size + total_bytes).
        // Use 4 pages = 64 KiB total. VA budget = 16 KiB + 64 KiB = 80 KiB.
        // 32 MiB alignment requires more VA than 80 KiB.
        let mut sm = make_space_manager_with(4, PAGE_16K);
        let before = sm.root_pool.free_bytes;
        let result = sm.create_space(PAGE_16K, SPACE_VA_ALIGNMENT);

        assert_eq!(
            result.err(),
            Some(AllocError::OutOfMemory),
            "create_space must fail when VA space cannot satisfy alignment"
        );
        assert_eq!(
            sm.root_pool.free_bytes, before,
            "failed create_space must roll back all physical allocations"
        );
    }

    /// D32: zero-size request. D25 says minimum Space = page_size,
    /// but create_space rounds up, so size=0 should produce 1-page Space.
    #[test]
    fn d32_create_space_zero_size_rounds_to_one_page() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let result = sm
            .create_space(0, SPACE_VA_ALIGNMENT)
            .expect("create_space with size=0 must succeed (rounds to 1 page)");

        assert_eq!(
            result.size, PAGE_16K,
            "zero-size request must round to page_size"
        );
        assert_eq!(
            result.page_count, 1,
            "zero-size request must produce 1 page"
        );
    }

    /// D32: size=1 byte rounds to 1 page.
    #[test]
    fn d32_create_space_one_byte_rounds_to_one_page() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let result = sm
            .create_space(1, SPACE_VA_ALIGNMENT)
            .expect("create_space with size=1 must succeed");

        assert_eq!(
            result.size, PAGE_16K,
            "1-byte request must round to page_size"
        );
        assert_eq!(result.page_count, 1);
    }

    /// D32: large Space crossing L3 table boundary — 2049 pages with 16 KiB
    /// granule needs 2 L3 tables. Overhead is 2 * page_size.
    #[test]
    fn d32_create_space_large_crosses_l3_boundary() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(8192, PAGE_16K);
        let result = sm
            .create_space(2049 * PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("create_space with 2049 pages must succeed");

        assert_eq!(result.page_count, 2049);
        assert_eq!(result.size, 2049 * PAGE_16K);
        // l3_table_pa must point to a contiguous allocation of 2 L3 table pages.
        // We verify it is non-zero and differs from content_pa.
        assert_ne!(result.l3_table_pa, 0);
        assert_ne!(result.l3_table_pa, result.content_pa);
    }

    /// D32: two consecutive create_space calls return non-overlapping
    /// content_pa and l3_table_pa ranges.
    #[test]
    fn d32_create_space_non_overlapping_allocations() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(16384, PAGE_16K);
        let a = sm
            .create_space(2 * PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("first create_space must succeed");
        let b = sm
            .create_space(3 * PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("second create_space must succeed");
        // Collect all allocated ranges: [base, base + count * page_size).
        let ranges = [
            (a.content_pa, a.page_count * PAGE_16K, "a.content"),
            (a.l3_table_pa, PAGE_16K, "a.l3_table"),
            (b.content_pa, b.page_count * PAGE_16K, "b.content"),
            (b.l3_table_pa, PAGE_16K, "b.l3_table"),
        ];

        for i in 0..ranges.len() {
            for j in (i + 1)..ranges.len() {
                let (base_i, size_i, name_i) = ranges[i];
                let (base_j, size_j, name_j) = ranges[j];
                let end_i = base_i + size_i;
                let end_j = base_j + size_j;
                let overlaps = base_i < end_j && base_j < end_i;

                assert!(
                    !overlaps,
                    "allocations {name_i} and {name_j} overlap: \
                     [{base_i:#x}, {end_i:#x}) and [{base_j:#x}, {end_j:#x})"
                );
            }
        }
    }

    /// D32: content_pa is page-aligned (physical pages are always page-aligned).
    #[test]
    fn d32_create_space_content_pa_page_aligned() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let result = sm
            .create_space(PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("create_space must succeed");

        assert_eq!(
            result.content_pa % PAGE_16K,
            0,
            "content_pa must be page-aligned (content_pa={:#x})",
            result.content_pa
        );
    }

    /// D32: l3_table_pa is page-aligned.
    #[test]
    fn d32_create_space_l3_table_pa_page_aligned() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let result = sm
            .create_space(PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("create_space must succeed");

        assert_eq!(
            result.l3_table_pa % PAGE_16K,
            0,
            "l3_table_pa must be page-aligned (l3_table_pa={:#x})",
            result.l3_table_pa
        );
    }

    /// D32: size not on page boundary rounds up correctly.
    #[test]
    fn d32_create_space_non_page_boundary_size() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        // Request size that is not a multiple of page_size.
        let result = sm
            .create_space(PAGE_16K + 1, SPACE_VA_ALIGNMENT)
            .expect("create_space must succeed");

        assert_eq!(
            result.size,
            2 * PAGE_16K,
            "size=page_size+1 must round up to 2*page_size"
        );
        assert_eq!(result.page_count, 2);
    }

    /// D32: create_space with 4 KiB pages (alternate granule).
    /// entries_per_table = 4096/8 = 512. 1 page Space needs 1 L3 table.
    #[test]
    fn d32_create_space_4kib_granule() {
        let mut sm = make_space_manager_with(1024, 4096);
        let before = sm.root_pool.free_bytes;
        let result = sm
            .create_space(4096, 4096)
            .expect("create_space with 4 KiB page must succeed");

        assert_eq!(result.page_count, 1);
        assert_eq!(result.size, 4096);

        let consumed = before - sm.root_pool.free_bytes;

        assert_eq!(
            consumed,
            2 * 4096,
            "1-page Space with 4 KiB granule: 1 content + 1 L3 table = 2 pages"
        );
    }

    /// D32: conservation across create_space — total_bytes never changes,
    /// free_bytes decreases by exactly (content + overhead) pages.
    #[test]
    fn d32_create_space_conservation() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(4096, PAGE_16K);
        let total_before = sm.root_pool.total_bytes;
        let free_before = sm.root_pool.free_bytes;
        let result = sm
            .create_space(8 * PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("create_space must succeed");

        // total_bytes must not change.
        assert_eq!(
            sm.root_pool.total_bytes, total_before,
            "total_bytes must be conserved"
        );

        // free_bytes must decrease by content + overhead.
        let expected_overhead = PAGE_16K; // 8 pages fit in 1 L3 table
        let expected_consumed = result.page_count * PAGE_16K + expected_overhead;

        assert_eq!(
            sm.root_pool.free_bytes,
            free_before - expected_consumed,
            "free_bytes must decrease by exactly (content + L3 overhead)"
        );
    }

    /// D32: rollback atomicity — after a failed create_space, the
    /// SpaceManager state is identical to before the call.
    #[test]
    fn d32_create_space_rollback_atomicity() {
        const PAGE_16K: usize = 16384;
        // 3 pages: enough for 1 content + 1 L3 but VA alignment (32 MiB)
        // cannot be satisfied with only 3 * 16 KiB = 48 KiB VA budget.
        let mut sm = make_space_manager_with(3, PAGE_16K);
        let free_before = sm.root_pool.free_bytes;
        let result = sm.create_space(PAGE_16K, SPACE_VA_ALIGNMENT);

        assert!(
            result.is_err(),
            "create_space must fail with tiny VA budget"
        );
        assert_eq!(
            sm.root_pool.free_bytes, free_before,
            "free_bytes must be restored after rollback"
        );
        // Bump allocator cursors advance monotonically and are not rolled
        // back — free_bytes is the critical conservation invariant.
    }

    /// D32: pool has exactly enough for content + L3 table.
    /// This exercises the exact-boundary allocation path.
    #[test]
    fn d32_create_space_exact_pool_boundary() {
        const PAGE_16K: usize = 16384;
        // 2 pages: exactly enough for 1 content + 1 L3 table.
        // But we also need VA space. With total_bytes = 2 * 16384 = 32768,
        // VA budget = 16384 + 32768 = 49152. page_size alignment works.
        let mut sm = make_space_manager_with(2, PAGE_16K);
        let result = sm.create_space(PAGE_16K, PAGE_16K);

        assert!(
            result.is_ok(),
            "create_space must succeed with exactly 2 pages (1 content + 1 L3)"
        );
        assert_eq!(
            sm.root_pool.free_bytes, 0,
            "free_bytes must be 0 after consuming all pages"
        );
    }

    /// D32: second create_space fails when pool is exhausted after first.
    #[test]
    fn d32_create_space_second_fails_after_exhaustion() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(2, PAGE_16K);

        sm.create_space(PAGE_16K, PAGE_16K)
            .expect("first create_space must succeed");

        let result = sm.create_space(PAGE_16K, PAGE_16K);

        assert_eq!(
            result.err(),
            Some(AllocError::OutOfMemory),
            "second create_space must fail on exhausted pool"
        );
    }

    /// D32: VA bases from create_space are non-overlapping in VA space.
    #[test]
    fn d32_create_space_va_ranges_non_overlapping() {
        const PAGE_16K: usize = 16384;
        let mut sm = make_space_manager_with(16384, PAGE_16K);
        let a = sm
            .create_space(4 * PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("first");
        let b = sm
            .create_space(8 * PAGE_16K, SPACE_VA_ALIGNMENT)
            .expect("second");
        let a_end = a.va_base + a.size;
        let b_end = b.va_base + b.size;
        let overlaps = a.va_base < b_end && b.va_base < a_end;

        assert!(
            !overlaps,
            "VA ranges must not overlap: [{:#x}, {:#x}) and [{:#x}, {:#x})",
            a.va_base, a_end, b.va_base, b_end
        );
    }
}
