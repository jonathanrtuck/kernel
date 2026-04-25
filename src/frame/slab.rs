//! Slab allocator internals — the unsafe boundary for arena page management.
//!
//! D70: per-type slab allocator with page return. The slab page layout,
//! freelist threading, and page allocation/deallocation live here.
//! Safe arena operations in `arena.rs` delegate to these primitives.
//!
//! Test builds use a `Vec`-backed implementation (needs `alloc`).
//! Bare-metal builds provide a stub that compiles but panics if called —
//! arena allocation is not yet wired into the boot path.

#[cfg(test)]
extern crate alloc;

use crate::arena::{AllocError, ObjectId};

// ── Test build: Vec-backed slab ───────────────────────────────────

/// Vec-backed slab store for host-side tests.
///
/// Uses `Vec<Option<T>>` for slot storage (Some = occupied, None = free)
/// and a separate freelist vector for O(1) allocation from freed slots.
/// Maximum capacity simulates finite slab pages.
///
/// The real slab allocator will use hardware pages with an intrusive
/// freelist. This test stand-in provides the same interface using heap
/// allocation, allowing arena tests to run on the host.
#[cfg(test)]
pub struct SlabStore<T> {
    /// Slot storage: `Some` = occupied, `None` = free.
    slots: alloc::vec::Vec<Option<T>>,
    /// Indices of free slots (LIFO freelist).
    freelist: alloc::vec::Vec<u32>,
    /// Maximum number of slots (simulates finite slab pages).
    max_slots: u32,
}

#[cfg(test)]
impl<T> SlabStore<T> {
    /// Default test capacity — 256 slots, enough for adversarial tests
    /// that exhaust the arena while fitting in a fixed-size tracking buffer.
    const DEFAULT_MAX_SLOTS: u32 = 256;

    /// Create a new empty slab store.
    ///
    /// Slots are allocated on demand up to `DEFAULT_MAX_SLOTS`. The freelist
    /// starts empty because no slots have been allocated-then-freed yet.
    pub fn new() -> SlabStore<T> {
        SlabStore {
            slots: alloc::vec::Vec::new(),
            freelist: alloc::vec::Vec::new(),
            max_slots: Self::DEFAULT_MAX_SLOTS,
        }
    }

    /// Allocate a slot: reuse from freelist or grow, zero-initialize,
    /// and return (index, &mut T).
    ///
    /// Returns `Err(OutOfMemory)` when the freelist is empty and the
    /// slot vector has reached `max_slots`.
    pub fn allocate(&mut self) -> Result<(ObjectId, &mut T), AllocError> {
        if let Some(index) = self.freelist.pop() {
            // Reuse a previously freed slot.
            // SAFETY: Creating a zeroed T for the slot. For all test types
            // (u32, AtomicU64 fields), zero is a valid bit pattern. The caller
            // overwrites fields through the returned &mut T before reading.
            // For kernel object types with NonNull fields, the caller must
            // initialize all fields before use.
            let zeroed: T = unsafe { core::mem::zeroed() };

            self.slots[index as usize] = Some(zeroed);

            let value = self.slots[index as usize].as_mut().unwrap();

            Ok((ObjectId(index), value))
        } else if (self.slots.len() as u32) < self.max_slots {
            // Grow the slot vector.
            let index = self.slots.len() as u32;
            // SAFETY: Creating a zeroed T for the new slot. Same safety
            // argument as the freelist path above — zero is valid for test
            // types, and the caller initializes before reading.
            let zeroed: T = unsafe { core::mem::zeroed() };

            self.slots.push(Some(zeroed));

            let value = self.slots.last_mut().unwrap().as_mut().unwrap();

            Ok((ObjectId(index), value))
        } else {
            Err(AllocError::OutOfMemory)
        }
    }

    /// Look up an object by index.
    ///
    /// Returns `None` if the index is out of bounds or the slot is free.
    pub fn get(&self, id: ObjectId) -> Option<&T> {
        self.slots.get(id.0 as usize).and_then(|slot| slot.as_ref())
    }

    /// Mutable lookup by index.
    ///
    /// Returns `None` if the index is out of bounds or the slot is free.
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut T> {
        self.slots
            .get_mut(id.0 as usize)
            .and_then(|slot| slot.as_mut())
    }

    /// Return a slot to the freelist.
    ///
    /// Silently ignores out-of-bounds indices and already-free slots
    /// (double-free protection). The double-free guard prevents freelist
    /// corruption — without it, freeing an already-free slot would push
    /// its index onto the freelist a second time, causing two allocates
    /// to return the same slot (aliased &mut T).
    pub fn free(&mut self, id: ObjectId) {
        let index = id.0 as usize;

        if index < self.slots.len() && self.slots[index].is_some() {
            self.slots[index] = None;

            self.freelist.push(id.0);
        }
    }
}

// ── Bare-metal build: compile-time stub ───────────────────────────

/// Stub slab store for bare-metal builds.
///
/// Arena allocation is not yet wired into the boot path. This stub
/// allows the crate to compile without `alloc`. All methods panic
/// with an explicit message if called.
#[cfg(not(test))]
pub struct SlabStore<T> {
    _marker: core::marker::PhantomData<T>,
}

#[cfg(not(test))]
impl<T> Default for SlabStore<T> {
    fn default() -> SlabStore<T> {
        SlabStore::new()
    }
}

#[cfg(not(test))]
impl<T> SlabStore<T> {
    pub const fn new() -> SlabStore<T> {
        SlabStore {
            _marker: core::marker::PhantomData,
        }
    }

    pub fn allocate(&mut self) -> Result<(ObjectId, &mut T), AllocError> {
        unimplemented!("slab allocator requires alloc (not available in bare-metal builds yet)")
    }

    pub fn get(&self, _id: ObjectId) -> Option<&T> {
        unimplemented!("slab allocator requires alloc (not available in bare-metal builds yet)")
    }

    pub fn get_mut(&mut self, _id: ObjectId) -> Option<&mut T> {
        unimplemented!("slab allocator requires alloc (not available in bare-metal builds yet)")
    }

    pub fn free(&mut self, _id: ObjectId) {
        unimplemented!("slab allocator requires alloc (not available in bare-metal builds yet)")
    }
}
