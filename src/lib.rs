//! Microkernel library.
//!
//! Module root for the kernel crate. Declares the module tree used by both
//! the bare-metal binary (`main.rs`) and host-side test builds
//! (`cargo test --target aarch64-apple-darwin`).

#![no_std]
#![deny(unsafe_code)]

#[allow(unsafe_code)]
pub mod frame;

pub mod arena;
pub mod capability;
pub mod communication;
pub mod config;
pub mod core_manager;
pub mod fault;
pub mod field;
pub mod observer;
#[cfg(any(target_os = "none", test))]
pub mod print;
pub mod pulsar;
pub mod space;
pub mod space_manager;
pub mod syscall;
pub mod time;
pub mod time_manager;

// ── Wave 1 integration tests ──────────────────────────────────────
//
// Cross-module tests verifying Arena, Capability Table, and Field
// compose correctly. These exercise the interfaces that Wave 2
// (Communication, SpaceManager) will depend on.

#[cfg(test)]
mod integration_tests {
    use crate::arena::{Arena, ObjectId};
    use crate::capability::{
        Badge, CapError, CloseResult, Entry, Handle, ObjectType, Rights, SLOT_USER_START, SlotTag,
        Table,
    };
    use crate::field::{Field, Message};
    use crate::observer::WaitEntry;
    use core::ptr::NonNull;
    use core::sync::atomic::{AtomicU64, Ordering};

    // ── Test helpers ──────────────────────────────────────────────────

    /// Minimal kernel object stand-in with generation counter (D67).
    #[derive(Debug)]
    struct TestObject {
        value: u32,
        generation: AtomicU64,
    }

    fn make_arena<T>() -> Arena<T> {
        Arena {
            store: crate::frame::slab::SlabStore::new(),
        }
    }

    fn test_table(capacity: u32) -> Table {
        let entries = crate::frame::cap_ops::alloc_test_entries(capacity);

        crate::frame::cap_ops::init_freelist(entries, capacity, SLOT_USER_START);

        Table {
            entries,
            capacity,
            count: 0,
            free_head: if capacity > SLOT_USER_START {
                Some(SLOT_USER_START)
            } else {
                None
            },
        }
    }

    fn test_field(capacity: u32) -> Field {
        Field {
            queue: crate::frame::field_ops::alloc_test_queue(capacity),
            queue_capacity: capacity,
            queue_length: 0,
            queue_head: 0,
            waiters_head: None,
            waiters_tail: None,
            routing_table: None,
            pending_head: None,
            badge_tracking: false,
            back_pointer_head: None,
            refcount: 1,
            generation: AtomicU64::new(0),
        }
    }

    fn simple_message(label: u64) -> Message {
        Message {
            data: [label, 0, 0, 0],
            label,
            badge: Badge(0),
            user_cap: None,
            reply_cap: None,
        }
    }

    // ── Scenario 1: Arena + Capability — generation-based revocation ──
    //
    // Cross-module interaction: Arena stores the object with its generation
    // counter, while the Capability Table stores the generation at entry
    // creation time. Revocation bumps the arena object's generation,
    // causing the cap entry's stored generation to become stale. This
    // verifies the D67 revocation mechanism works across the two modules.

    #[test]
    fn test_integration_arena_cap_generation_revocation() {
        // 1. Create an Arena<TestObject>.
        let mut arena: Arena<TestObject> = make_arena();
        // 2. Allocate a TestObject, get ObjectId, set generation to 0.
        let (object_id, obj) = arena.allocate().expect("arena allocate");

        obj.value = 42;
        obj.generation.store(0, Ordering::Relaxed);

        // 3. Create a Table.
        let mut table = test_table(16);
        // 4. Install a cap entry pointing to the arena object.
        let slot_index = table.allocate_slot().expect("allocate slot");
        let entry = Entry {
            object: Some((ObjectType::Field, object_id)),
            rights: Rights::FIELD_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        table.install_at(slot_index, entry);

        // 5. Resolve the cap — check_generation(0) should be true.
        let handle = Handle {
            index: slot_index,
            slot_tag: SlotTag(0),
        };
        let resolved = table.resolve(handle).expect("resolve must succeed");

        assert!(
            resolved.check_generation(0),
            "cap entry generation must match object's live generation (both 0)"
        );

        // 6. Bump the TestObject's generation via the arena (simulating revocation).
        let obj_mut = arena
            .get_mut(object_id)
            .expect("object must still be in arena");

        obj_mut.generation.fetch_add(1, Ordering::Release);

        let new_generation = obj_mut.generation.load(Ordering::Acquire);

        assert_eq!(new_generation, 1, "generation must have been bumped to 1");

        // 7. Resolve the cap again — the entry's stored_generation (0) no
        //    longer matches the object's live generation (1).
        let resolved_again = table
            .resolve(handle)
            .expect("resolve still succeeds (slot valid)");

        assert!(
            !resolved_again.check_generation(new_generation),
            "cap entry must report stale: stored generation 0 != live generation 1"
        );
        assert!(
            resolved_again.check_generation(0),
            "cap entry still stores generation 0 (not updated by resolve)"
        );
    }

    // ── Scenario 2: Field queue — FIFO across capacity boundary ───────
    //
    // Cross-module interaction: Field's circular queue must preserve FIFO
    // ordering even when the queue fills completely and then partially
    // drains. This exercises the wrap-around logic that Wave 2 IPC paths
    // will depend on for message ordering guarantees.

    #[test]
    fn test_integration_field_fifo_across_full_boundary() {
        // 1. Create a Field with capacity 4.
        let mut field = test_field(4);

        // 2. Enqueue 4 messages with labels 1, 2, 3, 4 (fills queue).
        for label in 1..=4u64 {
            field
                .enqueue(simple_message(label))
                .expect("enqueue must succeed before full");
        }

        assert!(field.is_full(), "queue must be full after 4 enqueues");

        // 3. Dequeue 1 message — should be label 1 (FIFO).
        let first = field.dequeue().expect("dequeue from full queue");

        assert_eq!(
            first.label, 1,
            "first dequeued message must have label 1 (FIFO)"
        );

        // 4. Enqueue 1 more message with label 5 (fills the slot just freed).
        field
            .enqueue(simple_message(5))
            .expect("enqueue into freed slot must succeed");

        assert!(
            field.is_full(),
            "queue must be full again after re-filling freed slot"
        );

        // 5. Dequeue remaining 4 — should be labels 2, 3, 4, 5 (FIFO preserved).
        let expected_labels = [2u64, 3, 4, 5];

        for &expected in &expected_labels {
            let msg = field.dequeue().expect("dequeue must succeed");

            assert_eq!(
                msg.label, expected,
                "FIFO order violated: expected label {expected}, got {}",
                msg.label
            );
        }

        assert!(
            field.is_empty(),
            "queue must be empty after draining all messages"
        );
    }

    // ── Scenario 3: Full Table lifecycle ──────────────────────────────
    //
    // Cross-module interaction: Table allocate_slot, install, close, and
    // cascade_step must maintain consistent count tracking across a
    // complete lifecycle. Wave 2 IPC depends on slot allocation for cap
    // transfer and cascade for Observer destruction cleanup.

    #[test]
    fn test_integration_table_full_lifecycle() {
        // 1. Create a Table with capacity 16.
        let mut table = test_table(16);
        // 2-3. Allocate 5 user slots and install entries at each.
        //      allocate_slot finds the next free slot but does not mark it
        //      occupied, so we must install immediately before the next allocate.
        let types_and_badges: [(ObjectType, u64); 5] = [
            (ObjectType::Field, 100),
            (ObjectType::Observer, 200),
            (ObjectType::Space, 300),
            (ObjectType::Time, 400),
            (ObjectType::Pulsar, 500),
        ];
        let mut slots = [0u32; 5];

        for (i, &(obj_type, badge_val)) in types_and_badges.iter().enumerate() {
            let idx = table.allocate_slot().expect("allocate_slot");

            assert!(
                idx >= SLOT_USER_START,
                "user slot index must be >= {SLOT_USER_START}, got {idx}"
            );

            let rights = match obj_type {
                ObjectType::Field => Rights::FIELD_ALL,
                ObjectType::Observer => Rights::OBSERVER_ALL,
                ObjectType::Space => Rights::SPACE_ALL,
                ObjectType::Time => Rights::TIME_ALL,
                ObjectType::Pulsar => Rights::PULSAR_ALL,
            };
            let entry = Entry {
                object: Some((obj_type, ObjectId(i as u32))),
                rights,
                badge: Badge(badge_val),
                slot_tag: SlotTag(0),
                send_once: false,
                stored_generation: 0,
            };

            table.install_at(idx, entry);

            slots[i] = idx;
        }

        // 4. Verify count is 5.
        assert_eq!(table.count, 5, "count must be 5 after installing 5 entries");

        // 5. Close 2 entries, verify CloseResult::Closed with correct object_type/id.
        let close_result_0 = table.close(slots[0]);

        match close_result_0 {
            CloseResult::Closed {
                object_type,
                object_id,
                ..
            } => {
                assert_eq!(object_type, ObjectType::Field);
                assert_eq!(object_id, ObjectId(0));
            }
            _ => panic!("expected CloseResult::Closed for slot 0"),
        }

        let close_result_1 = table.close(slots[1]);

        match close_result_1 {
            CloseResult::Closed {
                object_type,
                object_id,
                ..
            } => {
                assert_eq!(object_type, ObjectType::Observer);
                assert_eq!(object_id, ObjectId(1));
            }
            _ => panic!("expected CloseResult::Closed for slot 1"),
        }

        // 6. Verify count decreased to 3.
        assert_eq!(
            table.count, 3,
            "count must be 3 after closing 2 of 5 entries"
        );

        // 7. Allocate 2 more slots — should get back freed slots (or other free ones).
        //    Must install immediately after each allocate_slot, since
        //    allocate_slot scans for empty slots without marking them occupied.
        let realloc_a = table.allocate_slot().expect("re-allocate slot a");

        assert!(
            realloc_a >= SLOT_USER_START,
            "re-allocated slot a must be in user range"
        );

        table.install_at(
            realloc_a,
            Entry {
                object: Some((ObjectType::Field, ObjectId(10))),
                rights: Rights::FIELD_ALL,
                badge: Badge(0),
                slot_tag: SlotTag(0),
                send_once: false,
                stored_generation: 0,
            },
        );

        let realloc_b = table.allocate_slot().expect("re-allocate slot b");

        assert!(
            realloc_b >= SLOT_USER_START,
            "re-allocated slot b must be in user range"
        );

        table.install_at(
            realloc_b,
            Entry {
                object: Some((ObjectType::Field, ObjectId(11))),
                rights: Rights::FIELD_ALL,
                badge: Badge(0),
                slot_tag: SlotTag(0),
                send_once: false,
                stored_generation: 0,
            },
        );

        assert_ne!(realloc_a, realloc_b, "re-allocated slots must be distinct");
        assert_eq!(
            table.count, 5,
            "count must be 5 again after re-installing 2 entries"
        );

        // 8. Begin cascade, run cascade_step to completion.
        let mut state = table.begin_cascade();
        let mut steps = 0u32;

        loop {
            let done = table.cascade_step(&mut state);

            steps += 1;

            if done {
                break;
            }

            assert!(
                steps < 100,
                "cascade must terminate within a reasonable number of steps"
            );
        }

        // 9. Verify count is 0 and cascade state is complete.
        assert_eq!(table.count, 0, "count must be 0 after cascade completes");
        assert!(state.complete, "cascade state must be complete");
    }

    // ── Scenario 4: Field queue + waiters interaction ─────────────────
    //
    // Cross-module interaction: Field's waiter list and queue interact
    // during IPC — when a message arrives for a Field with a waiting
    // receiver, the D50 fast path pops the waiter instead of enqueuing.
    // This verifies the waiter mechanism works correctly for Wave 2's
    // send/receive paths.

    #[test]
    fn test_integration_field_waiter_then_message() {
        // 1. Create a Field with capacity 4.
        let mut field = test_field(4);
        // 2. Create a WaitEntry (simulating a blocked receiver).
        let mut wait_entry = WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        };

        // 3. Add the waiter to the field.
        field.add_waiter(&mut wait_entry);

        assert!(
            field.waiters_head.is_some(),
            "waiters_head must be non-None after adding a waiter"
        );

        // 4. Pop the waiter (simulating send finding a waiting receiver — D50 fast path).
        let popped = field.pop_waiter();

        // 5. Verify the waiter was returned.
        assert!(
            popped.is_some(),
            "pop_waiter must return the waiting receiver"
        );
        // 6. Verify waiters list is now empty.
        assert!(
            field.waiters_head.is_none(),
            "waiters_head must be None after popping the only waiter"
        );
        assert!(
            field.pop_waiter().is_none(),
            "second pop_waiter must return None (list empty)"
        );
    }

    // ── Scenario 5: Cap table ABA defense end-to-end ──────────────────
    //
    // Cross-module interaction: Handle contains both index and slot_tag.
    // When a slot is freed and reused, the old slot_tag becomes invalid.
    // This prevents stale handles from aliasing newly installed entries —
    // critical for Wave 2's cap transfer during IPC where handles transit
    // between Observers.

    #[test]
    fn test_integration_cap_aba_defense() {
        // 1. Create a Table.
        let mut table = test_table(16);
        // 2. Allocate a slot, install an entry, save the Handle.
        let slot_index = table.allocate_slot().expect("allocate_slot");
        let original_entry = Entry {
            object: Some((ObjectType::Field, ObjectId(42))),
            rights: Rights::FIELD_ALL,
            badge: Badge(100),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        table.install_at(slot_index, original_entry);

        let original_handle = Handle {
            index: slot_index,
            slot_tag: SlotTag(0),
        };

        // Verify the original handle resolves.
        let resolved = table
            .resolve(original_handle)
            .expect("original handle resolves");

        assert_eq!(resolved.badge, Badge(100));

        // 3. Close the slot (slot_tag bumps internally via close).
        let close_result = table.close(slot_index);

        assert!(
            matches!(close_result, CloseResult::Closed { .. }),
            "close must return Closed"
        );

        // 4. Try to resolve with the original Handle — should get SlotTagMismatch
        //    or InvalidHandle (slot is now empty with bumped tag).
        let stale_result = table.resolve(original_handle);

        assert!(
            matches!(
                stale_result,
                Err(CapError::SlotTagMismatch) | Err(CapError::InvalidHandle)
            ),
            "stale handle after close must fail with SlotTagMismatch or InvalidHandle"
        );

        // 5. Allocate a new slot at the same index — should work (reused slot).
        //    The allocate_slot scanner will find this slot since it is now empty.
        let new_slot = table.allocate_slot().expect("re-allocate slot");
        // 6. Install a new entry with the NEW slot_tag (bumped by close).
        //    We need to read the current slot_tag from the raw entry.
        //    After close, the slot_tag was bumped. The new entry must use that tag.
        let new_tag = SlotTag(1); // close bumps 0 -> 1
        let new_entry = Entry {
            object: Some((ObjectType::Observer, ObjectId(99))),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(200),
            slot_tag: new_tag,
            send_once: false,
            stored_generation: 0,
        };

        table.install_at(new_slot, new_entry);

        // 7. Resolve with the NEW handle — should succeed.
        let new_handle = Handle {
            index: new_slot,
            slot_tag: new_tag,
        };
        let new_resolved = table.resolve(new_handle).expect("new handle must resolve");

        assert_eq!(new_resolved.badge, Badge(200));
        assert!(new_resolved.check_type(ObjectType::Observer));

        // The original handle still fails — ABA defense holds.
        let still_stale = table.resolve(original_handle);

        assert!(
            still_stale.is_err(),
            "original handle must still fail after slot reuse"
        );
    }

    // ── Scenario 6: Cascade progression (D33) ─────────────────────────
    //
    // Cross-module interaction: Cascade iterates over Table entries in
    // bounded steps, closing each one. This tests that the cascade
    // processes all entries, does so in multiple steps (bounded per-step),
    // and leaves the table fully cleaned up. Wave 2's Observer destruction
    // depends on this for preemptible cleanup.

    #[test]
    fn test_integration_cascade_bounded_and_completes() {
        // 1. Create a Table with capacity 64.
        let mut table = test_table(64);
        // 2. Install entries at all 61 user slots (64 - 3 reserved).
        let user_slot_count = 64 - SLOT_USER_START;

        for i in 0..user_slot_count {
            let slot_index = SLOT_USER_START + i;
            let entry = Entry {
                object: Some((ObjectType::Field, ObjectId(i))),
                rights: Rights::FIELD_ALL,
                badge: Badge(i as u64),
                slot_tag: SlotTag(0),
                send_once: false,
                stored_generation: 0,
            };

            table.install_at(slot_index, entry);
        }

        assert_eq!(
            table.count, user_slot_count,
            "count must equal the number of installed user entries"
        );

        // 3. Begin cascade.
        let mut state = table.begin_cascade();

        assert!(
            !state.complete,
            "cascade must not be complete before any steps"
        );

        // 4. Run cascade_step in a loop, counting steps.
        let mut steps = 0u32;

        loop {
            let done = table.cascade_step(&mut state);

            steps += 1;

            if done {
                break;
            }

            assert!(
                steps < 1000,
                "cascade did not terminate after {steps} steps"
            );
        }

        // 5. Verify cascade completes.
        assert!(state.complete, "cascade state must be complete");
        // 6. Verify it took more than 1 step (bounded per-step processing).
        //    With 64 capacity and CASCADE_STEP_SIZE of 16, the cascade should
        //    take ceil(64/16) = 4 steps.
        assert!(
            steps > 1,
            "cascade must take more than 1 step for 64 entries (got {steps})"
        );
        // 7. Verify all entries are closed (count == 0).
        assert_eq!(
            table.count, 0,
            "all entries must be closed after cascade completes"
        );
    }
}
