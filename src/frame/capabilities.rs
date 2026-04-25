//! Capability table pointer operations — the unsafe boundary for Table.
//!
//! Safe public functions that wrap unsafe pointer dereferences on Table's
//! `NonNull<Entry>` array. The algorithmic logic (which slot to pick,
//! what to return) lives in `capability.rs`; the pointer access lives here.
//!
//! D8: Table is a kernel-managed flat array backed by typed memory.
//! The pointer validity invariant is structural — upheld by Table
//! construction, not verified at each call.

#[cfg(test)]
extern crate alloc;

use crate::capability::Entry;
#[cfg(test)]
use crate::capability::SlotTag;
use core::ptr::NonNull;

/// Get a shared reference to the entry at `index`.
///
/// Returns `None` if `index >= capacity` (bounds check).
///
/// # Safety (structural invariant, not caller obligation)
///
/// `entries` must point to a valid array of at least `capacity` `Entry`
/// elements. This is upheld by `Table` construction, not verified here.
/// The returned lifetime is unconstrained in this function; the caller
/// constrains it through lifetime elision on `&self` methods.
pub fn entry_ref<'a>(entries: NonNull<Entry>, capacity: u32, index: u32) -> Option<&'a Entry> {
    if index >= capacity {
        return None;
    }

    // SAFETY: index < capacity; pointer validity is a structural invariant
    // of the Table that owns this entries array. The Table was constructed
    // with a valid allocation of at least `capacity` Entry elements.
    // Violation would mean Table was constructed with an invalid pointer
    // or insufficient allocation, which is a bug in Table creation.
    unsafe { Some(&*entries.as_ptr().add(index as usize)) }
}

/// Get a mutable reference to the entry at `index`.
///
/// Returns `None` if `index >= capacity` (bounds check).
///
/// # Safety (structural invariant, not caller obligation)
///
/// `entries` must point to a valid array of at least `capacity` `Entry`
/// elements. This is upheld by `Table` construction, not verified here.
/// The returned lifetime is unconstrained in this function; the caller
/// constrains it through lifetime elision on `&mut self` methods.
pub fn entry_mut<'a>(entries: NonNull<Entry>, capacity: u32, index: u32) -> Option<&'a mut Entry> {
    if index >= capacity {
        return None;
    }

    // SAFETY: index < capacity; pointer validity is a structural invariant
    // of the Table that owns this entries array. The Table was constructed
    // with a valid allocation of at least `capacity` Entry elements.
    // Mutable access is sound because the caller holds `&mut Table`,
    // ensuring exclusive access to the entire entries array.
    unsafe { Some(&mut *entries.as_ptr().add(index as usize)) }
}

/// Allocate a test `Entry` array (test-only).
///
/// Returns `NonNull` pointing to `capacity` `Entry::empty(SlotTag(0))` entries.
/// The allocation uses `Vec` and leaks it — acceptable for test code only.
#[cfg(test)]
pub fn alloc_test_entries(capacity: u32) -> NonNull<Entry> {
    let mut entries: alloc::vec::Vec<Entry> = alloc::vec::Vec::with_capacity(capacity as usize);

    for _ in 0..capacity {
        entries.push(Entry::empty(SlotTag(0)));
    }

    if capacity == 0 {
        // Zero-capacity tables never dereference the pointer — resolve
        // returns InvalidHandle, allocate_slot returns TableFull. Return
        // dangling to satisfy NonNull without allocating.
        return NonNull::dangling();
    }

    let ptr = entries.as_mut_ptr();

    core::mem::forget(entries);

    // SAFETY: ptr is non-null because Vec with non-zero capacity always
    // allocates, and as_mut_ptr returns the allocation base.
    unsafe { NonNull::new_unchecked(ptr) }
}

/// Initialize the intrusive freelist through empty entries.
///
/// Links slots from `start` to `capacity - 1`. Each empty entry's
/// `stored_generation` stores the next-free index; the last stores
/// `u64::MAX` (freelist end sentinel).
#[cfg(test)]
pub fn init_freelist(entries: NonNull<Entry>, capacity: u32, start: u32) {
    for i in start..capacity {
        if let Some(e) = entry_mut(entries, capacity, i) {
            e.stored_generation = if i + 1 < capacity {
                (i + 1) as u64
            } else {
                u64::MAX
            };
        }
    }
}
