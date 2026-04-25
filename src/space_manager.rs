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
}

impl SpaceManager {
    /// Allocate pages from the root pool for a new Space or arena slab.
    ///
    /// D31: draws from unallocated physical memory. Returns an error
    /// if the pool is exhausted.
    ///
    /// D70: arena slab pages use this same path. When all slots on a
    /// slab page are freed, the page returns here via `return_pages`.
    pub fn allocate_pages(&mut self, _count: usize) -> Result<usize, AllocError> {
        todo!()
    }

    /// Return pages to the root pool.
    ///
    /// D70: freed slab pages return here. D33: cascade-freed structural
    /// backing returns here (not to the caller — only top-level destroy
    /// returns Space to the destroyer).
    pub fn return_pages(&mut self, _base: usize, _count: usize) {
        todo!()
    }

    /// Assign a VA base for a new Space (D26).
    ///
    /// D26: kernel-assigned, stable for the Space's lifetime. The
    /// policy for choosing VA bases is kernel-internal — this method
    /// encapsulates it.
    ///
    /// D41: merge may fail if no adjacent VA space is available. The
    /// assignment policy should minimize this by leaving headroom.
    pub fn assign_va(&mut self, _size: usize) -> Result<VaAssignment, AllocError> {
        todo!()
    }

    /// Compute the overhead of type conversion for a given Space size.
    ///
    /// D32: at split time, the parent shrinks by `child_size + overhead`.
    /// Overhead covers the page table subtree entries needed to map the
    /// new Space. First holder populates from reserved capacity;
    /// subsequent holders increment the reference count.
    pub fn type_conversion_overhead(&self, _space_size: usize) -> usize {
        todo!()
    }
}
