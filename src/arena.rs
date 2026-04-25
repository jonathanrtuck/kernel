//! Per-type object arena with generation counters.
//!
//! D53: global-arena concurrency model — one SpinLock per Arena<T>.
//! D67: every kernel object carries a generation counter for revocation.
//! D70: per-type slab allocator with page return.

/// Kernel-internal object identifier.
///
/// Index into a per-type Arena<T> slab. The object's own `generation`
/// field (D67) is the revocation counter — checked against the stored
/// generation in each capability entry on use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectId(pub u32);

/// Allocation failure from a per-type arena (D70, D31).
///
/// Occurs when the slab freelist is empty and no pages can be drawn
/// from the root Space pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocError {
    /// Root Space pool exhausted — no pages available for slab growth.
    OutOfMemory,
}

/// Per-type kernel object arena (D53, D70).
///
/// Five arenas total: one per kernel object type. Lock ordering (D53):
/// `Arena<Field>` < `Arena<Observer>` < `Arena<Pulsar>`.
/// `Arena<Space>` and `Arena<Time>` are unordered (no cross-arena ops).
///
/// Internal structure (D70): hardware pages divided into N fixed-size
/// slots, intrusive freelist through freed slots. When all slots on a
/// page are free, the page returns to the root Space pool.
///
/// All unsafe slab internals live inside frame/ (journal 023). This
/// module defines the interface; frame/ provides the implementation.
pub struct Arena<T> {
    _marker: core::marker::PhantomData<T>,
}

impl<T> Arena<T> {
    /// Allocate a slot for a new object.
    ///
    /// D70: draws from the intrusive freelist within slab pages. When
    /// the freelist is empty, requests a new page from the root Space
    /// pool (D31). Object addresses are stable for the object's lifetime
    /// — no compaction (D70, D4: pointer = capability reference).
    ///
    /// **Caller must hold this arena's lock (D53).**
    ///
    /// Performance: amortized O(1). Page acquisition is cold path (D1).
    pub fn allocate(&mut self) -> Result<(ObjectId, &mut T), AllocError> {
        todo!()
    }

    /// Look up an object by identifier.
    ///
    /// Returns `None` if `id` is out of bounds or the slot is free.
    /// Callers should check the object's `generation` field (D67)
    /// against the capability entry's stored generation before use —
    /// a successful lookup does not imply the cap is still valid.
    ///
    /// Performance: O(1) — direct index into the slab page array.
    pub fn get(&self, _id: ObjectId) -> Option<&T> {
        todo!()
    }

    /// Mutable lookup by identifier.
    ///
    /// **Caller must hold this arena's lock (D53).**
    pub fn get_mut(&mut self, _id: ObjectId) -> Option<&mut T> {
        todo!()
    }

    /// Return a slot to the freelist.
    ///
    /// D70: when all slots on a page become free, the page returns to
    /// the root Space pool (D31). Ensures memory usage is proportional
    /// to steady-state allocation, not peak — grows-never-shrinks
    /// rejected under A3 (generic kernel cannot absorb permanent waste
    /// from transient allocation peaks).
    ///
    /// **Caller must hold this arena's lock (D53).**
    pub fn free(&mut self, _id: ObjectId) {
        todo!()
    }
}
