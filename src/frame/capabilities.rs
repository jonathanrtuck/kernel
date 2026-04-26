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

    // Spectre v1: the branch above may be speculatively bypassed with a
    // mispredicted index, allowing out-of-bounds reads. SB ensures the
    // branch resolves before the dependent pointer dereference executes.
    #[cfg(any(target_os = "none", test))]
    crate::frame::arch::speculation::speculation_barrier();

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

    // Spectre v1: same barrier as entry_ref — prevent speculative
    // dereference past the bounds check with a mispredicted index.
    #[cfg(any(target_os = "none", test))]
    crate::frame::arch::speculation::speculation_barrier();

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

/// Write a capability entry at the given index via raw pointer (D95, D32).
///
/// Writes directly through the raw pointer without creating a `&mut Entry`,
/// avoiding aliasing with outstanding `&Entry` references from `entry_ref`.
/// Used by type-conversion operations where the entry was previously read
/// via `entry_ref` and must now be overwritten.
pub fn write_entry(entries: NonNull<Entry>, capacity: u32, index: u32, new_entry: Entry) -> bool {
    if index >= capacity {
        return false;
    }

    #[cfg(any(target_os = "none", test))]
    crate::frame::arch::speculation::speculation_barrier();

    // SAFETY: index < capacity (checked above). entries points to a valid
    // array of at least capacity Entry elements (structural invariant of
    // the owning cap table). We write through a raw pointer rather than
    // creating a &mut Entry reference to avoid aliasing with any outstanding
    // &Entry from entry_ref in the same dispatch path.
    unsafe {
        core::ptr::write(entries.as_ptr().add(index as usize), new_entry);
    }

    true
}

/// Allocate cap table entries for a new Observer (D95, D32).
///
/// Cap table pages come from the consumed Space's structural backing (D95).
/// Test builds use the heap allocator. Bare-metal builds will use Space
/// pages once wired.
#[cfg(any(target_os = "none", test))]
pub fn allocate_cap_table(capacity: u32) -> Option<NonNull<Entry>> {
    if capacity == 0 {
        return None;
    }

    #[cfg(test)]
    {
        let entries = alloc_test_entries(capacity);

        init_freelist(entries, capacity, crate::capability::SLOT_USER_START);

        Some(entries)
    }
    #[cfg(not(test))]
    {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::ObjectId;
    use crate::capability::{Badge, ObjectType, Rights};

    // ── entry_ref and entry_mut ───────────────────────────────────

    #[test]
    fn entry_ref_returns_empty_entry_at_valid_index() {
        let capacity = 8u32;
        let entries = alloc_test_entries(capacity);
        let entry = entry_ref(entries, capacity, 0).unwrap();

        assert!(
            !entry.is_occupied(),
            "freshly allocated entry must be empty"
        );
    }

    #[test]
    fn entry_ref_out_of_bounds_returns_none() {
        let capacity = 8u32;
        let entries = alloc_test_entries(capacity);

        assert!(entry_ref(entries, capacity, 8).is_none());
        assert!(entry_ref(entries, capacity, 100).is_none());
    }

    #[test]
    fn entry_mut_allows_writing_and_reading_back() {
        let capacity = 8u32;
        let entries = alloc_test_entries(capacity);
        let entry = entry_mut(entries, capacity, 3).unwrap();

        entry.object = Some((ObjectType::Field, ObjectId(42)));
        entry.rights = Rights::FIELD_ALL;
        entry.badge = Badge(0xDEAD);
        entry.stored_generation = 5;

        let read_back = entry_ref(entries, capacity, 3).unwrap();

        assert!(read_back.is_occupied());
        assert_eq!(read_back.badge, Badge(0xDEAD));
        assert!(read_back.check_rights(Rights::SEND));
        assert!(read_back.check_generation(5));
        assert!(read_back.check_type(ObjectType::Field));
    }

    #[test]
    fn entry_mut_out_of_bounds_returns_none() {
        let capacity = 4u32;
        let entries = alloc_test_entries(capacity);

        assert!(entry_mut(entries, capacity, 4).is_none());
    }

    // ── write_entry ──────────────────────────────────────────────

    #[test]
    fn write_entry_overwrites_slot() {
        let capacity = 8u32;
        let entries = alloc_test_entries(capacity);
        let new_entry = Entry {
            object: Some((ObjectType::Observer, ObjectId(10))),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(99),
            slot_tag: SlotTag(1),
            send_once: false,
            stored_generation: 7,
        };
        let ok = write_entry(entries, capacity, 2, new_entry);

        assert!(ok);

        let read_back = entry_ref(entries, capacity, 2).unwrap();

        assert!(read_back.is_occupied());
        assert!(read_back.check_type(ObjectType::Observer));
        assert_eq!(read_back.badge, Badge(99));
        assert_eq!(read_back.stored_generation, 7);
    }

    #[test]
    fn write_entry_out_of_bounds_returns_false() {
        let capacity = 4u32;
        let entries = alloc_test_entries(capacity);
        let entry = Entry::empty(SlotTag(0));

        assert!(!write_entry(entries, capacity, 4, entry));
        assert!(!write_entry(entries, capacity, 100, entry));
    }

    #[test]
    fn write_entry_can_clear_occupied_slot() {
        let capacity = 4u32;
        let entries = alloc_test_entries(capacity);
        // First, occupy the slot.
        let occupied = Entry {
            object: Some((ObjectType::Space, ObjectId(1))),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 1,
        };

        write_entry(entries, capacity, 0, occupied);

        assert!(entry_ref(entries, capacity, 0).unwrap().is_occupied());

        // Now clear it.
        write_entry(entries, capacity, 0, Entry::empty(SlotTag(1)));

        let cleared = entry_ref(entries, capacity, 0).unwrap();

        assert!(!cleared.is_occupied());
        assert_eq!(cleared.slot_tag, SlotTag(1));
    }

    // ── allocate_cap_table ───────────────────────────────────────

    #[test]
    fn allocate_cap_table_zero_returns_none() {
        assert!(allocate_cap_table(0).is_none());
    }

    #[test]
    fn allocate_cap_table_returns_valid_entries() {
        let capacity = 16u32;
        let entries_ptr = allocate_cap_table(capacity).unwrap();

        // All entries should be readable without panic.
        for i in 0..capacity {
            let entry = entry_ref(entries_ptr, capacity, i).unwrap();

            assert!(!entry.is_occupied());
        }
    }

    #[test]
    fn allocate_cap_table_freelist_initialized() {
        let capacity = 8u32;
        let entries_ptr = allocate_cap_table(capacity).unwrap();
        // init_freelist links slots from SLOT_USER_START to capacity-1.
        // Each empty entry's stored_generation stores the next-free index;
        // the last stores u64::MAX.
        let start = crate::capability::SLOT_USER_START;

        for i in start..capacity {
            let entry = entry_ref(entries_ptr, capacity, i).unwrap();

            if i + 1 < capacity {
                assert_eq!(
                    entry.stored_generation,
                    (i + 1) as u64,
                    "slot {i} should point to next free slot {}",
                    i + 1,
                );
            } else {
                assert_eq!(
                    entry.stored_generation,
                    u64::MAX,
                    "last slot must have sentinel value"
                );
            }
        }
    }

    // ── alloc_test_entries ───────────────────────────────────────

    #[test]
    fn alloc_test_entries_zero_capacity_returns_dangling() {
        let ptr = alloc_test_entries(0);

        assert_eq!(ptr, NonNull::dangling());
    }
}
