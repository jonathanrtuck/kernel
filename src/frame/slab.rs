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
            // SAFETY: MaybeUninit::zeroed() produces all-zero bytes without
            // triggering Rust's debug-mode validity checks (which would reject
            // zero for types containing NonNull). assume_init() is sound here
            // because the caller MUST initialize all fields through the returned
            // &mut T before reading. The &mut T is the only live reference to
            // this slot (exclusive arena lock, LIFO freelist ensures no alias).
            let zeroed: T = unsafe { core::mem::MaybeUninit::<T>::zeroed().assume_init() };

            self.slots[index as usize] = Some(zeroed);

            let value = self.slots[index as usize].as_mut().unwrap();

            Ok((ObjectId(index), value))
        } else if (self.slots.len() as u32) < self.max_slots {
            // Grow the slot vector.
            let index = self.slots.len() as u32;
            // SAFETY: Same as the freelist path above — MaybeUninit::zeroed()
            // bypasses validity checks; assume_init() is sound because the
            // caller initializes all fields before reading.
            let zeroed: T = unsafe { core::mem::MaybeUninit::<T>::zeroed().assume_init() };

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

// ── Bare-metal build: page-backed slab (D70, D93) ────────────────
//
// D93: slab page source — pages drawn from SpaceManager root pool via
// frame::kernel_state(). Lazy initialization: the first allocate()
// triggers a page request. Identity mapping (PA = VA) is assumed for
// kernel pages.
//
// Single-page slab: sufficient for Phase E boot (a handful of objects
// per type). Returns OutOfMemory when the page is full. Multi-page
// growth can be added behind this interface later.
//
// Occupancy tracked with a bitmap (512 bits = 8 u64 words in the struct).
// Free slots thread an intrusive u32 freelist through slot memory.

/// Sentinel value for an empty freelist.
#[cfg(not(test))]
const FREELIST_NONE: u32 = u32::MAX;

/// Maximum slots per slab page. Determines bitmap size.
///
/// With 16 KiB pages and 32-byte minimum slot stride (u32 freelist +
/// alignment), worst case is 16384/32 = 512 slots. Kernel objects are
/// all larger than 32 bytes, so real capacities are well below this.
#[cfg(not(test))]
const MAX_SLOTS_PER_PAGE: usize = 512;

/// Bitmap words: 512 bits / 64 bits per u64 = 8 words.
#[cfg(not(test))]
const BITMAP_U64S: usize = MAX_SLOTS_PER_PAGE / 64;

/// Page-backed slab store for bare-metal builds (D70, D93).
///
/// Each `SlabStore<T>` owns a single physical page, carved into
/// fixed-size slots of `stride >= size_of::<T>()`. Free slots form
/// an intrusive LIFO freelist (u32 next-pointer in slot memory).
/// A bitmap tracks which slots are occupied for O(1) `get`/`get_mut`
/// validity checks.
///
/// The page is requested from SpaceManager on first `allocate()` —
/// construction (`new()`) is allocation-free and `const`.
#[cfg(not(test))]
pub struct SlabStore<T> {
    /// Base address of the slab page (null until initialized).
    base: *mut u8,
    /// Byte distance between consecutive slots.
    /// `max(size_of::<T>(), size_of::<u32>())` rounded up to `align_of::<T>()`.
    slot_stride: usize,
    /// Number of slots that fit in one page.
    capacity: u32,
    /// Index of the first free slot, or `FREELIST_NONE` if all occupied.
    free_head: u32,
    /// Bitmap: bit N set ↔ slot N is occupied. Indexed as `[word][bit]`.
    occupied: [u64; BITMAP_U64S],
    _marker: core::marker::PhantomData<T>,
}

// SAFETY: SlabStore is accessed exclusively through Arena<T>, which is
// wrapped in Lock<Arena<T>> (D53). The lock guarantees no concurrent
// access. The raw pointer `base` points to a page exclusively owned by
// this slab — no other code holds or produces pointers into it.
#[cfg(not(test))]
unsafe impl<T: Send> Send for SlabStore<T> {}

#[cfg(not(test))]
impl<T> Default for SlabStore<T> {
    fn default() -> SlabStore<T> {
        SlabStore::new()
    }
}

#[cfg(not(test))]
impl<T> SlabStore<T> {
    /// Create an empty slab store. No page is allocated until the first
    /// `allocate()` call.
    pub const fn new() -> SlabStore<T> {
        SlabStore {
            base: core::ptr::null_mut(),
            slot_stride: 0,
            capacity: 0,
            free_head: FREELIST_NONE,
            occupied: [0u64; BITMAP_U64S],
            _marker: core::marker::PhantomData,
        }
    }

    /// Compute the byte stride between consecutive slots.
    ///
    /// Each slot must be large enough for both T (when occupied) and a
    /// u32 freelist pointer (when free). The stride is rounded up to
    /// `align_of::<T>()` so that every slot is properly aligned.
    fn slot_stride() -> usize {
        let min_size = core::mem::size_of::<T>().max(core::mem::size_of::<u32>());

        min_size.next_multiple_of(core::mem::align_of::<T>())
    }

    /// Allocate a page from SpaceManager and initialize the freelist.
    ///
    /// Called once, on the first `allocate()`. Acquires the SpaceManager
    /// lock (D53: SpaceManager is unordered, safe to acquire inside an
    /// arena lock).
    fn init_page(&mut self) -> Result<(), AllocError> {
        debug_assert!(
            core::mem::size_of::<T>() > 0,
            "SlabStore does not support zero-sized types"
        );

        let ks = crate::frame::kernel_state();
        let mut sm = ks.space_manager.acquire();
        let page_size = sm.root_pool.page_size;
        let pa = sm.allocate_pages(1)?;

        drop(sm);

        let stride = Self::slot_stride();
        let capacity = page_size / stride;

        debug_assert!(capacity > 0, "page cannot fit even one slot of this type");
        debug_assert!(
            capacity <= MAX_SLOTS_PER_PAGE,
            "slot count {capacity} exceeds bitmap capacity {MAX_SLOTS_PER_PAGE}"
        );

        // SAFETY: `pa` is a valid, page-aligned physical address returned
        // by SpaceManager::allocate_pages. Under identity mapping (kernel
        // VA = PA for kernel-owned pages), this is a valid writable pointer.
        // The page is exclusively ours — SpaceManager removed it from the
        // free pool.
        let base = pa as *mut u8;

        // Thread the intrusive freelist: each free slot stores the index
        // of the next free slot. The last slot stores FREELIST_NONE.
        for i in 0..capacity {
            let next = if i + 1 < capacity {
                (i + 1) as u32
            } else {
                FREELIST_NONE
            };

            // SAFETY: base + i * stride is within the page (i < capacity,
            // capacity * stride <= page_size). stride >= size_of::<u32>(),
            // so writing a u32 at the slot start is in-bounds. The page
            // base is page-aligned, so (base + i * stride) is at least
            // u32-aligned (stride is a multiple of align_of::<T>() which
            // is >= 1, and stride >= 4).
            unsafe {
                let slot = base.add(i * stride) as *mut u32;

                slot.write(next);
            }
        }

        self.base = base;
        self.slot_stride = stride;
        self.capacity = capacity as u32;
        self.free_head = 0;

        Ok(())
    }

    /// Pointer to the start of slot `index`. Caller must verify
    /// `index < capacity`.
    fn slot_ptr(&self, index: u32) -> *mut u8 {
        debug_assert!(!self.base.is_null());
        debug_assert!((index as usize) < self.capacity as usize);

        // SAFETY: index < capacity (debug-asserted), and
        // capacity * slot_stride <= page_size, so the computed address
        // is within the allocated page.
        unsafe { self.base.add(index as usize * self.slot_stride) }
    }

    fn is_occupied(&self, index: u32) -> bool {
        let word = index as usize / 64;
        let bit = index as usize % 64;

        self.occupied[word] & (1u64 << bit) != 0
    }

    fn mark_occupied(&mut self, index: u32) {
        let word = index as usize / 64;
        let bit = index as usize % 64;

        self.occupied[word] |= 1u64 << bit;
    }

    fn mark_free(&mut self, index: u32) {
        let word = index as usize / 64;
        let bit = index as usize % 64;

        self.occupied[word] &= !(1u64 << bit);
    }

    /// Allocate a slot: pop from freelist, zero-initialize, return
    /// `(ObjectId, &mut T)`.
    ///
    /// On first call, requests a page from SpaceManager (D93). Returns
    /// `Err(OutOfMemory)` when the page is full and no more pages are
    /// available.
    pub fn allocate(&mut self) -> Result<(ObjectId, &mut T), AllocError> {
        if self.base.is_null() {
            self.init_page()?;
        }

        if self.free_head == FREELIST_NONE {
            return Err(AllocError::OutOfMemory);
        }

        let index = self.free_head;
        let ptr = self.slot_ptr(index);
        // SAFETY: the slot is free (on the freelist, bitmap clear), so its
        // first 4 bytes contain a u32 next-pointer. Reading it is safe
        // because the slot is within the page and properly aligned.
        let next_free = unsafe { (ptr as *const u32).read() };

        self.free_head = next_free;

        // SAFETY: ptr points to a valid slot within the page. size_of::<T>()
        // bytes starting at ptr are within bounds (slot_stride >= size_of::<T>()).
        // Zeroing before returning ensures the caller gets a clean slate
        // (same contract as the test build's MaybeUninit::zeroed path).
        unsafe {
            core::ptr::write_bytes(ptr, 0, core::mem::size_of::<T>());
        }

        self.mark_occupied(index);

        // SAFETY: ptr is aligned to align_of::<T>() (page base is page-aligned,
        // slot_stride is a multiple of align_of::<T>()). The slot contains
        // size_of::<T>() zero bytes — the caller initializes all fields through
        // the returned &mut T before reading (same contract as test build).
        // Exclusive access: arena lock (D53) + freshly removed from freelist.
        let reference = unsafe { &mut *(ptr as *mut T) };

        Ok((ObjectId(index), reference))
    }

    /// Look up an object by index.
    ///
    /// Returns `None` if the slab is uninitialized, the index is out of
    /// bounds, or the slot is free.
    pub fn get(&self, id: ObjectId) -> Option<&T> {
        if self.base.is_null() || id.0 >= self.capacity || !self.is_occupied(id.0) {
            return None;
        }

        let ptr = self.slot_ptr(id.0);

        // SAFETY: bounds-checked (id.0 < capacity), occupancy-checked
        // (bitmap bit set), properly aligned (same as allocate). The T was
        // initialized by the allocate caller.
        Some(unsafe { &*(ptr as *const T) })
    }

    /// Mutable lookup by index.
    ///
    /// Returns `None` if the slab is uninitialized, the index is out of
    /// bounds, or the slot is free.
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut T> {
        if self.base.is_null() || id.0 >= self.capacity || !self.is_occupied(id.0) {
            return None;
        }

        let ptr = self.slot_ptr(id.0);

        // SAFETY: same as get(), plus &mut self ensures no aliasing.
        Some(unsafe { &mut *(ptr as *mut T) })
    }

    /// Return a slot to the freelist.
    ///
    /// Silently ignores: uninitialized slab, out-of-bounds index,
    /// already-free slot (double-free protection via bitmap check).
    pub fn free(&mut self, id: ObjectId) {
        let index = id.0;

        if self.base.is_null() || index >= self.capacity || !self.is_occupied(index) {
            return;
        }

        let ptr = self.slot_ptr(index);

        // SAFETY: index < capacity (checked), slot is occupied (bitmap
        // checked). Writing a u32 freelist pointer into the slot start is
        // safe because slot_stride >= size_of::<u32>() and the slot is
        // within the page.
        unsafe {
            (ptr as *mut u32).write(self.free_head);
        }

        self.mark_free(index);
        self.free_head = index;
    }
}
