//! Field unsafe operations — circular queue, intrusive linked list, routing table.
//!
//! All `unsafe` for Field methods lives here. The safe `field.rs` module
//! delegates pointer-dereference operations to these functions, keeping
//! the framekernel boundary (journal 023) intact.
//!
//! Three categories:
//! - **Circular queue**: read/write Message slots via NonNull<Message>.
//! - **Intrusive linked list**: WaitEntry push/pop/remove via NonNull<WaitEntry>.
//! - **Routing table**: binary search and sorted insertion via NonNull<RoutingTable>.

use crate::arena::ObjectId;
use crate::field::{FieldError, Message, RoutingEntry, RoutingTable};
use crate::observer::WaitEntry;
use core::ptr::NonNull;

// ── A. Circular queue operations ──────────────────────────────────────

/// Write a Message to the queue at the given index.
///
/// The caller (Field::enqueue) has already computed the circular buffer
/// index and verified the queue is not full. This function performs the
/// raw pointer write.
#[inline]
pub fn queue_write(queue: NonNull<Message>, capacity: u32, index: u32, message: Message) {
    assert!(
        index < capacity,
        "queue_write: index ({index}) must be < capacity ({capacity})"
    );

    // SAFETY: Field construction guarantees `queue` points to a valid
    // contiguous array of at least `capacity` Message slots. The caller
    // verified index < capacity, so `queue.as_ptr().add(index)` is within
    // bounds. We write a fully-initialized Message value. No aliased
    // mutable references exist — Field::enqueue holds &mut self.
    unsafe {
        core::ptr::write(queue.as_ptr().add(index as usize), message);
    }
}

/// Read and move a Message from the queue at the given index.
///
/// The caller (Field::dequeue) has already verified the queue is non-empty
/// and computed the head index.
#[inline]
pub fn queue_read(queue: NonNull<Message>, capacity: u32, index: u32) -> Option<Message> {
    assert!(
        index < capacity,
        "queue_read: index ({index}) must be < capacity ({capacity})"
    );

    // SAFETY: Field construction guarantees `queue` points to a valid
    // contiguous array of at least `capacity` Message slots. The index
    // has been bounds-checked above. `ptr::read` moves the value out
    // without dropping the source — the slot becomes logically empty
    // (the caller advances queue_head past it). No aliased references
    // exist — Field::dequeue holds &mut self.
    let message = unsafe { core::ptr::read(queue.as_ptr().add(index as usize)) };

    Some(message)
}

/// Allocate a test queue array (test-only).
///
/// Returns a NonNull<Message> pointing to a heap-allocated array of
/// `capacity` Message slots. The caller is responsible for the lifetime
/// (in tests, the allocation leaks — acceptable for test-only code).
#[cfg(test)]
pub fn alloc_test_queue(capacity: u32) -> NonNull<Message> {
    if capacity == 0 {
        // Zero-capacity queues never dereference the pointer — enqueue
        // returns QueueFull immediately, dequeue returns None immediately.
        // Return dangling to satisfy NonNull without allocating.
        return NonNull::dangling();
    }

    extern crate alloc;

    use alloc::vec::Vec;

    // Zero-initialize all slots so Option discriminants are valid (None).
    // Message contains Option<TransferredCap> fields whose discriminant
    // bits must be deterministic — uninitialized memory would be UB if
    // ptr::read ever executed on an unwritten slot.
    let mut vec: Vec<Message> = Vec::with_capacity(capacity as usize);

    for _ in 0..capacity {
        // SAFETY: Message is a plain data struct with no padding-dependent
        // invariants. All-zero bytes produce: data=[0;4], label=0,
        // badge=Badge(0), user_cap=None, reply_cap=None. Every field
        // is valid at zero (u64, Option<T> with discriminant 0 = None).
        vec.push(unsafe { core::mem::zeroed() });
    }

    let ptr = vec.as_mut_ptr();

    core::mem::forget(vec);

    // SAFETY: Vec::as_mut_ptr() on a non-empty Vec returns a non-null
    // pointer. We just verified capacity > 0.
    unsafe { NonNull::new_unchecked(ptr) }
}

/// Extract the Observer pointer from a WaitEntry pointer.
///
/// Safe wrapper around the unsafe dereference of NonNull<WaitEntry>.
/// Used by the IPC send/call paths (communication.rs) which live outside
/// the framekernel boundary and cannot use unsafe directly.
#[inline]
pub fn waiter_observer(entry: NonNull<WaitEntry>) -> NonNull<crate::observer::Observer> {
    // SAFETY: entry was returned by waiter_pop_front, which only returns
    // pointers that were previously inserted via waiter_push_back from a
    // valid &mut WaitEntry reference. The WaitEntry is still alive (the
    // caller holds the &mut Field that owns the list, guaranteeing no
    // concurrent mutation). We read the observer field without moving it.
    unsafe { (*entry.as_ptr()).observer }
}

/// Read the next pointer from a WaitEntry pointer.
///
/// Safe wrapper for consuming pending list entries (D18). The pending
/// list uses the same WaitEntry next-pointer linkage as the waiters list.
#[inline]
pub fn waiter_next(entry: NonNull<WaitEntry>) -> Option<NonNull<WaitEntry>> {
    // SAFETY: entry is sourced from field.pending_head, which is set by the
    // kernel-as-sender fault/interrupt path from a live WaitEntry reference.
    // The caller holds &mut Field, preventing concurrent mutation. We read
    // the next field without moving it — the pointer remains valid for the
    // duration of the borrow.
    unsafe { (*entry.as_ptr()).next }
}

// ── B. Intrusive linked list operations (WaitEntry) ───────────────────

/// Insert entry at the tail of the list (FIFO).
///
/// Updates entry.prev, entry.next, and the current tail's next pointer.
/// If the list is empty, the entry becomes the sole element (head).
#[inline]
pub fn waiter_push_back(
    head: &mut Option<NonNull<WaitEntry>>,
    tail: &mut Option<NonNull<WaitEntry>>,
    entry: &mut WaitEntry,
) {
    let entry_ptr = NonNull::from(&*entry);
    entry.next = None;

    match *tail {
        None => {
            entry.prev = None;
            *head = Some(entry_ptr);
            *tail = Some(entry_ptr);
        }
        Some(mut tail_ptr) => {
            entry.prev = Some(tail_ptr);

            // SAFETY: tail_ptr was set by a prior push_back or is the sole
            // element. The caller holds &mut on the Field, ensuring exclusive
            // access. We link the new entry after the current tail.
            unsafe {
                tail_ptr.as_mut().next = Some(entry_ptr);
            }

            *tail = Some(entry_ptr);
        }
    }
}

/// Remove a specific entry from the list.
///
/// Updates adjacent entries' prev/next and potentially the head.
/// Safe to call on an entry that is not in the list (prev and next
/// are both None and entry is not the head) — this is a no-op.
pub fn waiter_remove(
    head: &mut Option<NonNull<WaitEntry>>,
    tail: &mut Option<NonNull<WaitEntry>>,
    entry: &mut WaitEntry,
) {
    let entry_ptr = NonNull::from(&*entry);
    let is_head = head.is_some_and(|h| h == entry_ptr);
    let is_tail = tail.is_some_and(|t| t == entry_ptr);

    if !is_head && !is_tail && entry.prev.is_none() && entry.next.is_none() {
        return;
    }

    if let Some(mut prev_ptr) = entry.prev {
        // SAFETY: prev_ptr was set by a prior push_back and points to a
        // valid WaitEntry in this list. The caller holds &mut on the Field.
        unsafe {
            prev_ptr.as_mut().next = entry.next;
        }
    }
    if let Some(mut next_ptr) = entry.next {
        // SAFETY: next_ptr was set by a prior push_back and points to a
        // valid WaitEntry in this list. The caller holds &mut on the Field.
        unsafe {
            next_ptr.as_mut().prev = entry.prev;
        }
    }

    if is_head {
        *head = entry.next;
    }
    if is_tail {
        *tail = entry.prev;
    }

    entry.prev = None;
    entry.next = None;
}

/// Pop the front entry from the list.
///
/// Returns the NonNull to the popped entry, or None if the list is empty.
/// The popped entry's prev/next are cleared.
#[inline]
pub fn waiter_pop_front(
    head: &mut Option<NonNull<WaitEntry>>,
    tail: &mut Option<NonNull<WaitEntry>>,
) -> Option<NonNull<WaitEntry>> {
    let head_ptr = (*head)?;

    // SAFETY: head_ptr was set by a prior push_back and points to a valid
    // WaitEntry. The caller holds &mut on the Field, ensuring exclusive
    // access to the list.
    unsafe {
        let head_ref = head_ptr.as_ptr();
        let next = (*head_ref).next;

        *head = next;

        if let Some(mut next_ptr) = next {
            next_ptr.as_mut().prev = None;
        } else {
            *tail = None;
        }

        (*head_ref).prev = None;
        (*head_ref).next = None;
    }

    Some(head_ptr)
}

// ── C. Routing table operations ───────────────────────────────────────

/// Binary search the routing table for a badge match.
///
/// Returns the destination ObjectId if a range [low, high] contains
/// the badge. The entries array is sorted by badge_low, so we use
/// binary search for O(log N) lookup.
pub fn route_lookup(table_ptr: NonNull<RoutingTable>, badge: u64) -> Option<ObjectId> {
    // SAFETY: table_ptr was set by route_add and points to a valid
    // RoutingTable. The RoutingTable's entries pointer is valid for
    // `count` elements. The caller holds &self on the Field.
    unsafe {
        let table = table_ptr.as_ref();
        let count = table.count as usize;

        if count == 0 {
            return None;
        }

        let entries = table.entries.as_ptr();
        // Binary search: find the rightmost entry whose badge_low <= badge.
        let mut lo: usize = 0;
        let mut hi: usize = count;

        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            // SAFETY: mid is in [lo, hi) which is within [0, count).
            // entries points to a valid array of `count` RoutingEntry.
            let entry = &*entries.add(mid);

            if entry.badge_low <= badge {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }

        // lo is now the index past the last entry with badge_low <= badge.
        // Check the candidate at lo-1: the entry with the largest badge_low
        // that is still <= badge. If the badge also falls within its
        // badge_high, we have a match.
        if lo > 0 {
            let candidate = &*entries.add(lo - 1);

            if badge >= candidate.badge_low && badge <= candidate.badge_high {
                return Some(candidate.destination);
            }
        }

        None
    }
}

/// Add a routing entry to the sorted array.
///
/// Handles initial allocation and geometric growth. Maintains sorted
/// order by badge_low via insertion into the correct position.
///
/// Test builds allocate via the global allocator. Bare-metal builds
/// will allocate from root Space (D31) — currently stubbed.
pub fn route_add(
    table_ptr: &mut Option<NonNull<RoutingTable>>,
    low: u64,
    high: u64,
    destination: ObjectId,
    destination_generation: u64,
) -> Result<(), FieldError> {
    // Initial capacity for a new routing table.
    const INITIAL_CAPACITY: u32 = 4;
    // Maximum capacity to prevent unbounded growth in tests.
    const MAX_CAPACITY: u32 = 256;

    match *table_ptr {
        None => {
            // First route: allocate the routing table and its entries array.
            let table = allocate_routing_table(INITIAL_CAPACITY)?;

            // SAFETY: We just allocated this table. It is valid and has
            // capacity >= 1 with count == 0. We write the first entry
            // at index 0.
            unsafe {
                let table_ref = &mut *table.as_ptr();
                let entry_ptr = table_ref.entries.as_ptr();

                core::ptr::write(
                    entry_ptr,
                    RoutingEntry {
                        badge_low: low,
                        badge_high: high,
                        destination,
                        destination_generation,
                        back_prev: None,
                        back_next: None,
                    },
                );

                table_ref.count = 1;
            }

            *table_ptr = Some(table);

            Ok(())
        }
        Some(table) => {
            // SAFETY: table was set by a prior route_add call and points
            // to a valid RoutingTable. The caller holds &mut self on the
            // Field, ensuring exclusive access.
            unsafe {
                let table_ref = &mut *table.as_ptr();

                // Check if we need to grow.
                if table_ref.count >= table_ref.capacity {
                    // Geometric doubling.
                    let new_capacity = table_ref
                        .capacity
                        .checked_mul(2)
                        .ok_or(FieldError::RoutingTableFull)?;

                    if new_capacity > MAX_CAPACITY {
                        return Err(FieldError::RoutingTableFull);
                    }

                    grow_routing_table(table_ref, new_capacity)?;
                }

                let count = table_ref.count as usize;
                let entries = table_ref.entries.as_ptr();
                // Find insertion point to maintain sorted order by badge_low.
                let mut insert_at = count;

                for i in 0..count {
                    // SAFETY: i < count, entries is valid for count elements.
                    if (*entries.add(i)).badge_low > low {
                        insert_at = i;

                        break;
                    }
                }

                // D45: reject overlapping badge ranges.
                // Check left neighbor: does the previous entry's range
                // overlap with [low, high]?
                if insert_at > 0 {
                    let prev = &*entries.add(insert_at - 1);

                    if prev.badge_high >= low {
                        return Err(FieldError::RoutingTableFull);
                    }
                }
                // Check right neighbor: does the next entry's range
                // overlap with [low, high]?
                if insert_at < count {
                    let next = &*entries.add(insert_at);

                    if high >= next.badge_low {
                        return Err(FieldError::RoutingTableFull);
                    }
                }

                // Shift entries after insert_at to make room.
                if insert_at < count {
                    // SAFETY: We are shifting entries within the valid
                    // capacity of the array. count < capacity (checked
                    // above). The source and destination ranges may overlap
                    // so we use copy (memmove semantics).
                    core::ptr::copy(
                        entries.add(insert_at),
                        entries.add(insert_at + 1),
                        count - insert_at,
                    );
                }

                // Write the new entry at the insertion point.
                // SAFETY: insert_at <= count < capacity. The slot is
                // either freshly shifted or at the end of the array.
                core::ptr::write(
                    entries.add(insert_at),
                    RoutingEntry {
                        badge_low: low,
                        badge_high: high,
                        destination,
                        destination_generation,
                        back_prev: None,
                        back_next: None,
                    },
                );

                table_ref.count += 1;
            }

            Ok(())
        }
    }
}

/// Remove all routing entries targeting a specific destination (D55).
///
/// When a split Field is destroyed, source Fields that have routing entries
/// pointing to it must have those entries removed to prevent use-after-free.
/// This function scans the routing table and compacts it, removing any entry
/// whose `destination` matches `dest_id`.
///
/// Returns the number of entries removed.
pub fn remove_routes_to_destination(
    table_ptr: &mut Option<NonNull<RoutingTable>>,
    dest_id: ObjectId,
) -> u32 {
    let Some(table) = *table_ptr else {
        return 0;
    };

    // SAFETY: table was set by a prior route_add and points to a valid
    // RoutingTable. The caller holds &mut on the Field, ensuring exclusive
    // access to the routing table.
    unsafe {
        let table_ref = &mut *table.as_ptr();
        let count = table_ref.count as usize;
        let entries = table_ref.entries.as_ptr();
        let mut write_pos: usize = 0;
        let mut removed: u32 = 0;

        // Compact: copy entries that do NOT target dest_id.
        for read_pos in 0..count {
            // SAFETY: read_pos < count, entries is valid for count elements.
            let entry = &*entries.add(read_pos);

            if entry.destination == dest_id {
                removed += 1;
            } else {
                if write_pos != read_pos {
                    // SAFETY: write_pos < read_pos < count, both within the
                    // valid entries array. The source and destination do not
                    // overlap (write_pos < read_pos after at least one removal).
                    core::ptr::copy_nonoverlapping(
                        entries.add(read_pos),
                        entries.add(write_pos),
                        1,
                    );
                }

                write_pos += 1;
            }
        }

        table_ref.count = write_pos as u32;

        removed
    }
}

// ── Internal allocation helpers ───────────────────────────────────────

/// Allocate a new RoutingTable with the given entry capacity.
///
/// Test builds use the global allocator. Bare-metal builds will use
/// root Space pages (D31).
#[cfg(test)]
fn allocate_routing_table(capacity: u32) -> Result<NonNull<RoutingTable>, FieldError> {
    extern crate alloc;

    use alloc::alloc::{Layout, alloc};

    // Allocate the entries array.
    let entry_layout = Layout::array::<RoutingEntry>(capacity as usize)
        .map_err(|_| FieldError::RoutingTableFull)?;
    // SAFETY: entry_layout has non-zero size (capacity > 0, RoutingEntry
    // is non-ZST). alloc returns a valid pointer on success or null on
    // failure.
    let entries_raw = unsafe { alloc(entry_layout) };

    if entries_raw.is_null() {
        return Err(FieldError::RoutingTableFull);
    }

    // SAFETY: We just verified entries_raw is non-null.
    let entries_nn = unsafe { NonNull::new_unchecked(entries_raw as *mut RoutingEntry) };
    // Allocate the RoutingTable header.
    let table_layout = Layout::new::<RoutingTable>();
    // SAFETY: table_layout has non-zero size (RoutingTable is non-ZST).
    let table_raw = unsafe { alloc(table_layout) };

    if table_raw.is_null() {
        // SAFETY: entries_raw was allocated with entry_layout and is non-null.
        unsafe {
            alloc::alloc::dealloc(entries_raw, entry_layout);
        }

        return Err(FieldError::RoutingTableFull);
    }

    let table_ptr = table_raw as *mut RoutingTable;

    // SAFETY: table_ptr is non-null, properly aligned (from Layout::new),
    // and has room for one RoutingTable. We write a fully initialized value.
    unsafe {
        core::ptr::write(
            table_ptr,
            RoutingTable {
                entries: entries_nn,
                count: 0,
                capacity,
            },
        );

        Ok(NonNull::new_unchecked(table_ptr))
    }
}

/// Stub for bare-metal builds — routing table allocation requires
/// root Space pages (D31), not yet wired.
#[cfg(not(test))]
fn allocate_routing_table(_capacity: u32) -> Result<NonNull<RoutingTable>, FieldError> {
    Err(FieldError::RoutingTableFull)
}

/// Grow the routing table's entries array to new_capacity.
///
/// Copies existing entries to the new array, deallocates the old one.
#[cfg(test)]
fn grow_routing_table(table: &mut RoutingTable, new_capacity: u32) -> Result<(), FieldError> {
    extern crate alloc;

    use alloc::alloc::{Layout, alloc, dealloc};

    let old_count = table.count as usize;
    // Allocate new entries array.
    let new_layout = Layout::array::<RoutingEntry>(new_capacity as usize)
        .map_err(|_| FieldError::RoutingTableFull)?;
    // SAFETY: new_layout has non-zero size (new_capacity > 0).
    let new_raw = unsafe { alloc(new_layout) };

    if new_raw.is_null() {
        return Err(FieldError::RoutingTableFull);
    }

    let new_ptr = new_raw as *mut RoutingEntry;

    // Copy existing entries to the new array.
    if old_count > 0 {
        // SAFETY: table.entries is valid for old_count elements (maintained
        // by prior route_add calls). new_ptr is freshly allocated with room
        // for new_capacity >= old_count elements. The regions do not overlap
        // (distinct allocations).
        unsafe {
            core::ptr::copy_nonoverlapping(table.entries.as_ptr(), new_ptr, old_count);
        }
    }

    // Deallocate old entries array.
    let old_layout = Layout::array::<RoutingEntry>(table.capacity as usize)
        .expect("old layout must be valid since it was previously allocated");

    // SAFETY: table.entries was allocated with old_layout by a prior
    // allocate_routing_table or grow_routing_table call.
    unsafe {
        dealloc(table.entries.as_ptr() as *mut u8, old_layout);
    }

    // SAFETY: new_ptr is non-null (checked above).
    table.entries = unsafe { NonNull::new_unchecked(new_ptr) };
    table.capacity = new_capacity;

    Ok(())
}

/// Bare-metal stub for grow.
#[cfg(not(test))]
fn grow_routing_table(_table: &mut RoutingTable, _new_capacity: u32) -> Result<(), FieldError> {
    Err(FieldError::RoutingTableFull)
}

// ── D. Queue allocation for object creation (D95, D32) ──────────────

/// Allocate queue backing for a new Field (D95, D32).
///
/// Queue pages logically come from the consumed Space's structural backing.
/// Test builds use the heap allocator; bare-metal builds allocate zeroed
/// pages from the SpaceManager root pool (identity-mapped PA = VA).
#[cfg(any(target_os = "none", test))]
pub fn allocate_field_queue(capacity: u32) -> Option<NonNull<Message>> {
    if capacity == 0 {
        return None;
    }

    #[cfg(test)]
    {
        Some(alloc_test_queue(capacity))
    }
    #[cfg(not(test))]
    {
        let total_bytes = (capacity as usize) * core::mem::size_of::<Message>();
        let page_count = total_bytes.div_ceil(crate::frame::arch::mmu::page_size());
        let ks = crate::frame::kernel_state();
        let pa = crate::frame::boot::alloc_zeroed_pages(ks, page_count).ok()?;

        NonNull::new(crate::frame::phys_to_virt(pa) as *mut Message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::Badge;

    // ── Circular queue operations ─────────────────────────────────

    #[test]
    fn queue_write_and_read_roundtrip() {
        let capacity = 4u32;
        let queue = alloc_test_queue(capacity);
        let msg = Message {
            data: [0x1111, 0x2222, 0x3333, 0x4444],
            label: 0xABCD,
            badge: Badge(42),
            user_cap: None,
            reply_cap: None,
        };

        queue_write(queue, capacity, 0, msg);

        let read_back = queue_read(queue, capacity, 0).unwrap();

        assert_eq!(read_back.data, [0x1111, 0x2222, 0x3333, 0x4444]);
        assert_eq!(read_back.label, 0xABCD);
        assert_eq!(read_back.badge, Badge(42));
    }

    #[test]
    fn queue_write_all_slots() {
        let capacity = 4u32;
        let queue = alloc_test_queue(capacity);

        for i in 0..capacity {
            let msg = Message {
                data: [i as u64, 0, 0, 0],
                label: i as u64 * 10,
                badge: Badge(i as u64),
                user_cap: None,
                reply_cap: None,
            };

            queue_write(queue, capacity, i, msg);
        }

        for i in 0..capacity {
            let read_back = queue_read(queue, capacity, i).unwrap();

            assert_eq!(read_back.data[0], i as u64);
            assert_eq!(read_back.label, i as u64 * 10);
            assert_eq!(read_back.badge, Badge(i as u64));
        }
    }

    #[test]
    #[should_panic(expected = "queue_write: index (4) must be < capacity (4)")]
    fn queue_write_out_of_bounds_panics() {
        let capacity = 4u32;
        let queue = alloc_test_queue(capacity);
        let msg = Message {
            data: [0; 4],
            label: 0,
            badge: Badge(0),
            user_cap: None,
            reply_cap: None,
        };

        queue_write(queue, capacity, 4, msg);
    }

    #[test]
    #[should_panic(expected = "queue_read: index (4) must be < capacity (4)")]
    fn queue_read_out_of_bounds_panics() {
        let capacity = 4u32;
        let queue = alloc_test_queue(capacity);

        queue_read(queue, capacity, 4);
    }

    // ── Intrusive linked list operations ──────────────────────────

    #[test]
    fn waiter_push_back_single_element() {
        let mut head: Option<NonNull<WaitEntry>> = None;
        let mut tail: Option<NonNull<WaitEntry>> = None;
        let mut entry = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };

        waiter_push_back(&mut head, &mut tail, &mut entry);

        assert!(head.is_some());
        assert!(tail.is_some());
        assert_eq!(head, tail, "single element: head == tail");
    }

    #[test]
    fn waiter_push_back_two_elements_fifo_order() {
        let mut head: Option<NonNull<WaitEntry>> = None;
        let mut tail: Option<NonNull<WaitEntry>> = None;
        let mut entry_a = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };
        let mut entry_b = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };

        waiter_push_back(&mut head, &mut tail, &mut entry_a);
        waiter_push_back(&mut head, &mut tail, &mut entry_b);

        // Head should be entry_a, tail should be entry_b.
        assert_eq!(head, Some(NonNull::from(&entry_a)));
        assert_eq!(tail, Some(NonNull::from(&entry_b)));
    }

    #[test]
    fn waiter_pop_front_returns_fifo() {
        let mut head: Option<NonNull<WaitEntry>> = None;
        let mut tail: Option<NonNull<WaitEntry>> = None;
        let mut entry_a = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };
        let mut entry_b = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };

        waiter_push_back(&mut head, &mut tail, &mut entry_a);
        waiter_push_back(&mut head, &mut tail, &mut entry_b);

        let popped = waiter_pop_front(&mut head, &mut tail);

        assert_eq!(popped, Some(NonNull::from(&entry_a)));
        assert_eq!(head, Some(NonNull::from(&entry_b)));
    }

    #[test]
    fn waiter_pop_front_empty_returns_none() {
        let mut head: Option<NonNull<WaitEntry>> = None;
        let mut tail: Option<NonNull<WaitEntry>> = None;

        assert!(waiter_pop_front(&mut head, &mut tail).is_none());
    }

    #[test]
    fn waiter_pop_front_last_element_clears_tail() {
        let mut head: Option<NonNull<WaitEntry>> = None;
        let mut tail: Option<NonNull<WaitEntry>> = None;
        let mut entry = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };

        waiter_push_back(&mut head, &mut tail, &mut entry);

        let popped = waiter_pop_front(&mut head, &mut tail);

        assert!(popped.is_some());
        assert!(head.is_none(), "head must be None after popping last");
        assert!(tail.is_none(), "tail must be None after popping last");
    }

    #[test]
    fn waiter_remove_middle_element() {
        let mut head: Option<NonNull<WaitEntry>> = None;
        let mut tail: Option<NonNull<WaitEntry>> = None;
        let mut entry_a = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };
        let mut entry_b = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };
        let mut entry_c = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };

        waiter_push_back(&mut head, &mut tail, &mut entry_a);
        waiter_push_back(&mut head, &mut tail, &mut entry_b);
        waiter_push_back(&mut head, &mut tail, &mut entry_c);
        waiter_remove(&mut head, &mut tail, &mut entry_b);

        assert_eq!(head, Some(NonNull::from(&entry_a)));
        assert_eq!(tail, Some(NonNull::from(&entry_c)));
        // Verify linkage: a.next should be c, c.prev should be a.
        assert_eq!(entry_a.next, Some(NonNull::from(&entry_c)));
        assert_eq!(entry_c.prev, Some(NonNull::from(&entry_a)));
    }

    #[test]
    fn waiter_remove_not_in_list_is_noop() {
        let mut head: Option<NonNull<WaitEntry>> = None;
        let mut tail: Option<NonNull<WaitEntry>> = None;
        let mut stray = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };

        // Removing an element that was never inserted should not panic.
        waiter_remove(&mut head, &mut tail, &mut stray);

        assert!(head.is_none());
        assert!(tail.is_none());
    }

    // ── Routing table operations ─────────────────────────────────

    #[test]
    fn route_add_creates_table_on_first_insert() {
        let mut table_ptr: Option<NonNull<RoutingTable>> = None;
        let dest = ObjectId(7);
        let result = route_add(&mut table_ptr, 100, 200, dest, 1);

        assert!(result.is_ok());
        assert!(table_ptr.is_some());
    }

    #[test]
    fn route_lookup_finds_exact_badge() {
        let mut table_ptr: Option<NonNull<RoutingTable>> = None;
        let dest = ObjectId(7);

        route_add(&mut table_ptr, 100, 200, dest, 1).unwrap();

        let found = route_lookup(table_ptr.unwrap(), 150);

        assert_eq!(found, Some(dest));
    }

    #[test]
    fn route_lookup_boundary_values() {
        let mut table_ptr: Option<NonNull<RoutingTable>> = None;
        let dest = ObjectId(3);

        route_add(&mut table_ptr, 10, 20, dest, 1).unwrap();

        // Exact low boundary.
        assert_eq!(route_lookup(table_ptr.unwrap(), 10), Some(dest));
        // Exact high boundary.
        assert_eq!(route_lookup(table_ptr.unwrap(), 20), Some(dest));
        // Just below low.
        assert_eq!(route_lookup(table_ptr.unwrap(), 9), None);
        // Just above high.
        assert_eq!(route_lookup(table_ptr.unwrap(), 21), None);
    }

    #[test]
    fn route_lookup_empty_table_returns_none() {
        let mut table_ptr: Option<NonNull<RoutingTable>> = None;

        route_add(&mut table_ptr, 100, 200, ObjectId(1), 1).unwrap();

        // Badge outside any range.
        assert_eq!(route_lookup(table_ptr.unwrap(), 50), None);
        assert_eq!(route_lookup(table_ptr.unwrap(), 300), None);
    }

    #[test]
    fn route_add_maintains_sorted_order() {
        let mut table_ptr: Option<NonNull<RoutingTable>> = None;

        // Insert out of order.
        route_add(&mut table_ptr, 300, 400, ObjectId(3), 1).unwrap();
        route_add(&mut table_ptr, 100, 200, ObjectId(1), 1).unwrap();
        route_add(&mut table_ptr, 500, 600, ObjectId(5), 1).unwrap();

        // Each range should resolve to the correct destination.
        assert_eq!(route_lookup(table_ptr.unwrap(), 150), Some(ObjectId(1)));
        assert_eq!(route_lookup(table_ptr.unwrap(), 350), Some(ObjectId(3)));
        assert_eq!(route_lookup(table_ptr.unwrap(), 550), Some(ObjectId(5)));
    }

    #[test]
    fn route_add_triggers_growth() {
        let mut table_ptr: Option<NonNull<RoutingTable>> = None;

        // Initial capacity is 4 (INITIAL_CAPACITY). Insert 5 to force growth.
        for i in 0..5u64 {
            let low = i * 100;
            let high = low + 50;

            route_add(&mut table_ptr, low, high, ObjectId(i as u32), 1).unwrap();
        }
        // Verify all routes are findable after growth.
        for i in 0..5u64 {
            let badge = i * 100 + 25;

            assert_eq!(
                route_lookup(table_ptr.unwrap(), badge),
                Some(ObjectId(i as u32)),
            );
        }
    }

    // ── Queue allocation ─────────────────────────────────────────

    #[test]
    fn allocate_field_queue_zero_capacity_returns_none() {
        assert!(allocate_field_queue(0).is_none());
    }

    #[test]
    fn allocate_field_queue_nonzero_returns_some() {
        let result = allocate_field_queue(8);

        assert!(result.is_some());
    }

    #[test]
    fn alloc_test_queue_zero_capacity_returns_dangling() {
        let ptr = alloc_test_queue(0);

        // NonNull::dangling() is valid but should not be dereferenced.
        assert_eq!(ptr, NonNull::dangling());
    }
}
