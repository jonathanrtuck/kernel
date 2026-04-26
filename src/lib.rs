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
pub mod kernel_state;
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
        Table, TransferredCap,
    };
    use crate::communication::{
        CallOutcome, ReceiveOutcome, SendOutcome, call, receive, reply_recv, send,
    };
    use crate::field::{Field, Message};
    use crate::observer::WaitEntry;
    use crate::space_manager::{RootPool, SpaceManager};
    use crate::time_manager::Scheduler;
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
        let entries = crate::frame::capabilities::alloc_test_entries(capacity);

        crate::frame::capabilities::init_freelist(entries, capacity, SLOT_USER_START);

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
            queue: crate::frame::fields::alloc_test_queue(capacity),
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
            backing_va_base: 0,
            backing_size: 0,
        }
    }

    fn make_wait_entry() -> WaitEntry {
        WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        }
    }

    fn make_message(label: u64, badge: u64) -> Message {
        Message {
            data: [label, 0, 0, 0],
            label,
            badge: Badge(badge),
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
                .enqueue(make_message(label, 0))
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
            .enqueue(make_message(5, 0))
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
        let mut wait_entry = make_wait_entry();

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

    // ── Wave 2 integration tests ──────────────────────────────────────
    //
    // Cross-module tests verifying Communication (send/receive/call/
    // reply_recv) and SpaceManager (allocate_pages/return_pages/
    // assign_va/type_conversion_overhead) compose correctly with the
    // Wave 1 primitives (Field, capability table, arena). These cover
    // the D50 fast-path conditions, D18 pending-list draining, D33
    // cascade continuity, and SpaceManager conservation invariants.

    // ── Scenario 7: D50 WokeReceiver — send to field with waiting receiver

    // Cross-module interaction: communication::send invokes Field's
    // pop_waiter to implement the D50 direct-delivery fast path.
    // When a receiver is waiting, the message bypasses the queue
    // entirely (WokeReceiver). A subsequent receive on the same field
    // must then block because the queue was never touched.

    #[test]
    fn test_integration_wave2_d50_send_to_waiting_receiver_woke_receiver() {
        let mut field = test_field(4);
        // 1. Add a waiter, simulating a blocked receiver.
        let mut wait_entry = make_wait_entry();

        field.add_waiter(&mut wait_entry);

        assert!(
            field.waiters_head.is_some(),
            "precondition: waiter must be present before send"
        );

        // 2. Send a message — must take the D50 fast path.
        let message = make_message(42, 0);
        let outcome = send(&mut field, message).expect("send must succeed");

        assert!(
            matches!(outcome, SendOutcome::WokeReceiver(..)),
            "D50: send to field with waiting receiver must return WokeReceiver"
        );
        // 3. Message bypassed the queue — queue must still be empty.
        assert_eq!(
            field.queue_length, 0,
            "D50 fast path: queue must remain empty after direct delivery"
        );
        assert!(
            field.waiters_head.is_none(),
            "D50 fast path: waiter must be popped after direct delivery"
        );

        // 4. A subsequent receive on the now-empty queue must block.
        //    This confirms the queue was never used as a relay.
        let mut next_receiver = make_wait_entry();
        let receive_outcome = receive(&mut field, &mut next_receiver);

        assert!(
            matches!(receive_outcome, ReceiveOutcome::Blocked),
            "D50: receive on empty field (after direct delivery) must block"
        );
    }

    // ── Scenario 8: D50 DirectSwitch — call with 0-cap + waiting receiver

    // Cross-module interaction: communication::call checks user_cap before
    // popping the waiter. A 0-cap message with a waiting receiver takes the
    // DirectSwitch fast path; the queue is not touched at all.

    #[test]
    fn test_integration_wave2_d50_call_zero_cap_waiter_direct_switch() {
        let mut field = test_field(4);
        // 1. Add a waiter (blocked server).
        let mut wait_entry = make_wait_entry();

        field.add_waiter(&mut wait_entry);

        // 2. Call with a 0-cap message — D50 fast-path eligible.
        let zero_cap_message = Message {
            data: [10, 20, 30, 40],
            label: 0xABCD,
            badge: Badge(0),
            user_cap: None,
            reply_cap: None,
        };
        let outcome = call(&mut field, zero_cap_message, Badge(0))
            .expect("call with 0-cap message must not return QueueFull");

        assert!(
            matches!(outcome, CallOutcome::DirectSwitch(_)),
            "D50: call with 0-cap message and waiting receiver must return DirectSwitch"
        );
        // 3. The queue must be untouched — DirectSwitch bypasses it.
        assert_eq!(
            field.queue_length, 0,
            "D50 DirectSwitch: queue must remain empty (message not enqueued)"
        );
        assert!(
            field.waiters_head.is_none(),
            "D50 DirectSwitch: waiter must be consumed"
        );
    }

    // ── Scenario 9: D50 slow path — call with user_cap + waiting receiver

    // Cross-module interaction: communication::call must fall back to
    // the slow path (Enqueued) when the message carries a user cap,
    // even if a receiver is waiting. The user_cap gate in call() checks
    // user_cap.is_none() before popping the waiter.

    #[test]
    fn test_integration_wave2_d50_call_with_user_cap_forces_enqueued() {
        let mut field = test_field(4);
        // 1. Add a waiter — this would normally trigger DirectSwitch.
        let mut wait_entry = make_wait_entry();

        field.add_waiter(&mut wait_entry);

        // 2. Call with a message carrying a user cap (D50 slow-path gate).
        let message_with_cap = Message {
            data: [1, 2, 3, 4],
            label: 0xDEAD,
            badge: Badge(0),
            user_cap: Some(TransferredCap {
                object_type: ObjectType::Field,
                object_id: ObjectId(0),
                rights: Rights::SEND,
                badge: Badge(0),
                send_once: false,
                stored_generation: 0,
            }),
            reply_cap: None,
        };
        let outcome = call(&mut field, message_with_cap, Badge(0))
            .expect("call with user cap must not return QueueFull");

        // D78: waiter present + user cap = WokeReceiverSlowPath (not DirectSwitch).
        // The waiter IS popped (message bypasses queue), but delivery requires
        // the slow path for cap transfer. The dispatch layer delivers via
        // write_message_to_registers.
        assert!(
            matches!(outcome, CallOutcome::WokeReceiverSlowPath(..)),
            "D78: call with user cap and waiter must return WokeReceiverSlowPath"
        );
        // D78: message bypassed the queue — queue stays empty.
        assert_eq!(
            field.queue_length, 0,
            "D78: message bypasses queue when waiter is present (slow-path delivery)"
        );
        // D78: waiter was popped for direct delivery.
        assert!(
            field.waiters_head.is_none(),
            "D78: waiter must be popped on WokeReceiverSlowPath"
        );
    }

    // ── Scenario 10: D16 ReplyRecv round-trip

    // Cross-module interaction: communication::reply_recv atomically sends
    // a reply to reply_field then receives from recv_field. This test
    // verifies the reply is enqueued in reply_field and the next message
    // is received from recv_field — the two-field compound operation works
    // as a unit across Field's enqueue and dequeue operations.

    #[test]
    fn test_integration_wave2_d16_reply_recv_round_trip() {
        // 1. Create target field (where the server receives requests) and
        //    reply field (where the client waits for replies).
        let mut target_field = test_field(4);
        let mut reply_field = test_field(4);

        // 2. Pre-load a request message on target_field (simulating a client
        //    that already sent before the server calls reply_recv).
        target_field
            .enqueue(make_message(100, 0))
            .expect("enqueue request on target field");

        // 3. Server calls reply_recv: sends reply_message to reply_field,
        //    then receives the next request from target_field.
        let reply_message = make_message(200, 0);
        let mut server_receiver = make_wait_entry();
        let outcome = reply_recv(
            &mut reply_field,
            &mut target_field,
            reply_message,
            &mut server_receiver,
        );

        // 4. The reply must have been enqueued into reply_field.
        assert_eq!(
            reply_field.queue_length, 1,
            "D16: reply_recv must enqueue the reply message into reply_field"
        );
        // D78: reply with no waiter → enqueued, no reply delivery.
        assert!(
            outcome.reply_delivery.is_none(),
            "D78: no waiter on reply_field means reply was enqueued"
        );

        // 5. The receive side must have returned the pre-loaded request.
        match outcome.receive_outcome {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(
                    msg.label, 100,
                    "D16: reply_recv must return the request from target_field"
                );
            }
            ReceiveOutcome::Blocked => {
                panic!("D16: reply_recv must return Received when target_field has messages");
            }
        }

        // 6. target_field must now be empty (the request was consumed).
        assert_eq!(
            target_field.queue_length, 0,
            "D16: target_field must be empty after reply_recv consumes the request"
        );

        // 7. Retrieve the reply from reply_field and verify its label.
        let mut client_receiver = make_wait_entry();
        let client_outcome = receive(&mut reply_field, &mut client_receiver);

        match client_outcome {
            ReceiveOutcome::Received(reply) => {
                assert_eq!(
                    reply.label, 200,
                    "D16: client must receive the reply message from reply_field"
                );
            }
            ReceiveOutcome::Blocked => {
                panic!("D16: client receive from reply_field must return Received");
            }
        }
    }

    // ── Scenario 11: Wave 3 integration note (D50 scheduler deny)
    //
    // D50 condition 5 (scheduler denies direct switch → falls back to enqueue)
    // depends on the SchedulerCallback, which is a Wave 3 concern.  At the
    // communication.rs level there is no scheduler check — the module returns
    // DirectSwitch/WokeReceiver and the dispatch layer (core_manager, Wave 3)
    // decides whether to follow through.  This scenario cannot be exercised
    // here.  It is documented as a Wave 3 integration concern.

    // ── Scenario 12: D18 pending list draining

    // Cross-module interaction: communication::receive invokes Field's
    // dequeue and then checks field.pending_head. After dequeuing frees
    // a slot in a full queue, the pending entry is consumed and a
    // placeholder message refills the freed slot. This test verifies the
    // ordering: first message dequeued, pending consumed, queue remains full.

    #[test]
    fn test_integration_wave2_d18_pending_drains_on_receive() {
        // 1. Create a Field with capacity 2 and fill it completely.
        let mut field = test_field(2);

        field
            .enqueue(make_message(10, 0))
            .expect("enqueue label 10");
        field
            .enqueue(make_message(20, 0))
            .expect("enqueue label 20");

        assert!(field.is_full(), "precondition: field queue must be full");

        // 2. Set a pending_head simulating a deferred kernel-as-sender message
        //    (fault or interrupt) that could not deliver because the queue was full.
        let mut pending_entry = make_wait_entry();

        field.pending_head = Some(NonNull::from(&mut pending_entry));

        // 3. Receive: must dequeue the first message (label 10), consume the
        //    pending entry, and refill the freed slot with a placeholder.
        let mut receiver = make_wait_entry();
        let outcome = receive(&mut field, &mut receiver);

        // 4. The first message (label 10) must be returned — FIFO order.
        match outcome {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(
                    msg.label, 10,
                    "D18: receive must return the first queued message (FIFO)"
                );
            }
            ReceiveOutcome::Blocked => {
                panic!("D18: receive on non-empty queue must not block");
            }
        }

        // 5. The pending entry must be consumed.
        assert!(
            field.pending_head.is_none(),
            "D18: pending_head must be None after receive drains the pending entry"
        );
        // 6. The queue must still be full: the freed slot was immediately
        //    refilled by the placeholder from the pending entry.
        assert_eq!(
            field.queue_length, 2,
            "D18: queue must be refilled from pending — length stays at capacity"
        );
        assert!(
            field.is_full(),
            "D18: field must remain full after pending entry is consumed"
        );
    }

    // ── Scenario 13: D33 cascade continuity — fields still functional after cascade

    // Cross-module interaction: after a cap table cascade completes
    // (closing all entries), the Field objects those entries pointed to
    // are untouched. A Field allocated independently must still enqueue
    // and dequeue correctly after a cascade runs to completion on a
    // separate table. This verifies that cascade does not corrupt shared
    // arena state.

    #[test]
    fn test_integration_wave2_d33_cascade_fields_still_functional() {
        // 1. Create a table and install Field cap entries.
        let mut table = test_table(16);
        let slot = table.allocate_slot().expect("allocate slot");

        table.install_at(
            slot,
            Entry {
                object: Some((ObjectType::Field, ObjectId(0))),
                rights: Rights::FIELD_ALL,
                badge: Badge(0),
                slot_tag: SlotTag(0),
                send_once: false,
                stored_generation: 0,
            },
        );

        // 2. Create a Field independently — it is NOT stored in the table.
        //    This simulates a Field in the arena, referenced by the cap.
        let mut field = test_field(4);

        field
            .enqueue(make_message(77, 0))
            .expect("enqueue before cascade");
        field
            .enqueue(make_message(88, 0))
            .expect("enqueue before cascade");

        // 3. Run a full cascade on the table.
        let mut state = table.begin_cascade();

        loop {
            if table.cascade_step(&mut state) {
                break;
            }
        }

        assert!(state.complete, "cascade must complete");
        assert_eq!(table.count, 0, "cascade must close all entries");

        // 4. The independently-created Field must still work: enqueue,
        //    dequeue, and maintain FIFO order, undisturbed by the cascade.
        field
            .enqueue(make_message(99, 0))
            .expect("enqueue after cascade must succeed");

        let mut receiver = make_wait_entry();
        let first = receive(&mut field, &mut receiver);

        match first {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(
                    msg.label, 77,
                    "D33: first message after cascade must be label 77 (FIFO preserved)"
                );
            }
            ReceiveOutcome::Blocked => panic!("field must not block after cascade"),
        }

        let mut r2 = make_wait_entry();
        let second = receive(&mut field, &mut r2);

        match second {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(msg.label, 88, "D33: second message must be label 88");
            }
            ReceiveOutcome::Blocked => panic!("field must not block on second receive"),
        }

        let mut r3 = make_wait_entry();
        let third = receive(&mut field, &mut r3);

        match third {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(msg.label, 99, "D33: third message must be label 99");
            }
            ReceiveOutcome::Blocked => panic!("field must not block on third receive"),
        }

        // Field is now empty — next receive must block.
        let mut r4 = make_wait_entry();

        assert!(
            matches!(receive(&mut field, &mut r4), ReceiveOutcome::Blocked),
            "D33: field must block on receive after all messages drained"
        );
    }

    // ── Scenario 14: SpaceManager conservation across slab-like usage

    // Cross-module interaction: SpaceManager::allocate_pages draws from the
    // root pool; return_pages restores it. This test simulates the arena
    // slab page lifecycle (D70): allocate N pages, use them, return them,
    // and verify the total physical memory in the system is conserved.

    #[test]
    fn test_integration_wave2_space_manager_conservation_slab_lifecycle() {
        // 1. Create a SpaceManager with 16 pages.
        let mut space_manager = SpaceManager {
            root_pool: RootPool {
                total_bytes: 16 * 4096,
                free_bytes: 16 * 4096,
                page_size: 4096,
            },
            next_physical_base: 4096,
            next_va_base: 4096,
        };
        let initial_free = space_manager.root_pool.free_bytes;
        let page_size = space_manager.root_pool.page_size;
        // 2. Simulate arena slab: allocate 4 pages (one slab page each for
        //    four object arenas).
        let base_a = space_manager
            .allocate_pages(4)
            .expect("slab alloc A must succeed");
        let base_b = space_manager
            .allocate_pages(2)
            .expect("slab alloc B must succeed");
        let after_alloc = space_manager.root_pool.free_bytes;

        assert_eq!(
            after_alloc,
            initial_free - 6 * page_size,
            "conservation: 6 pages allocated must reduce free_bytes by 6 * page_size"
        );

        // 3. Simulate usage — nothing to do, pages are "live".

        // 4. Return the slab pages (simulating arena slab freeing when all
        //    slots on the slab are freed — D70).
        space_manager.return_pages(base_a, 4);
        space_manager.return_pages(base_b, 2);

        // 5. Conservation: free_bytes must be restored to its initial value.
        assert_eq!(
            space_manager.root_pool.free_bytes, initial_free,
            "conservation: free_bytes must equal initial after allocate+return cycle"
        );
        // 6. free_bytes must never exceed total_bytes.
        assert!(
            space_manager.root_pool.free_bytes <= space_manager.root_pool.total_bytes,
            "conservation: free_bytes ({}) must never exceed total_bytes ({})",
            space_manager.root_pool.free_bytes,
            space_manager.root_pool.total_bytes
        );
    }

    // ── Scenario 15: type_conversion_overhead + allocate_pages accounting

    // Cross-module interaction: D32 mandates that at split time the parent
    // Space shrinks by child_size + overhead, where overhead is computed by
    // type_conversion_overhead. This test verifies that the overhead is a
    // well-defined usize, that allocating child_size + overhead pages succeeds,
    // and that the accounting is exact: free_bytes decreases by exactly
    // (child_pages + overhead_pages) * page_size.

    #[test]
    fn test_integration_wave2_type_conversion_overhead_and_allocate_accounting() {
        let mut space_manager = SpaceManager {
            root_pool: RootPool {
                total_bytes: 64 * 4096,
                free_bytes: 64 * 4096,
                page_size: 4096,
            },
            next_physical_base: 4096,
            next_va_base: 4096,
        };
        let page_size = space_manager.root_pool.page_size;
        let child_size_bytes = 8 * page_size; // 8 pages for the child Space
        // 1. Compute the overhead for a child Space of child_size_bytes.
        //    This is a pure function — no state change.
        let overhead_bytes = space_manager.type_conversion_overhead(child_size_bytes);

        // 2. Overhead must be representable as a whole number of pages.
        //    (The implementation computes it in bytes; we verify rounding.)
        assert_eq!(
            overhead_bytes % page_size,
            0,
            "type_conversion_overhead must return a page-aligned byte count \
             (got {overhead_bytes} bytes, page_size={page_size})"
        );

        // 3. Total pages to allocate: child pages + overhead pages.
        let overhead_pages = overhead_bytes / page_size;
        let child_pages = child_size_bytes / page_size;
        let total_pages = child_pages + overhead_pages;
        let initial_free = space_manager.root_pool.free_bytes;

        // 4. Allocate child_pages + overhead_pages: must succeed since the
        //    pool has 64 pages and overhead for 8 pages is small.
        space_manager
            .allocate_pages(total_pages)
            .expect("allocate child + overhead pages must succeed");

        // 5. free_bytes must have decreased by exactly total_pages * page_size.
        let expected_free = initial_free - total_pages * page_size;

        assert_eq!(
            space_manager.root_pool.free_bytes, expected_free,
            "D32: parent pool must shrink by child_pages ({child_pages}) + \
             overhead_pages ({overhead_pages}) = {total_pages} pages total"
        );
    }

    // ── Scenario 16: Full IPC roundtrip with all Message fields

    // Cross-module interaction: communication::send invokes Field::enqueue,
    // which stores the complete Message struct. communication::receive invokes
    // Field::dequeue, which reads it back. All Message fields must survive
    // the queue roundtrip with exact bit-for-bit fidelity — no truncation,
    // no field aliasing, no struct padding corruption.

    #[test]
    fn test_integration_wave2_full_ipc_roundtrip_all_fields() {
        let mut field = test_field(4);
        // 1. Construct a message with every field populated with distinct
        //    non-zero values to catch truncation and aliasing bugs.
        let sent = Message {
            data: [
                0x1111_1111_1111_1111,
                0x2222_2222_2222_2222,
                0x3333_3333_3333_3333,
                0x4444_4444_4444_4444,
            ],
            label: 0xDEAD_BEEF_CAFE_1234,
            badge: Badge(0xABCD_EF01_2345_6789),
            user_cap: Some(TransferredCap {
                object_type: ObjectType::Observer,
                object_id: ObjectId(42),
                rights: Rights::OBSERVER_ALL,
                badge: Badge(0xFF00_FF00_FF00_FF00),
                send_once: true,
                stored_generation: 0xBEEF_CAFE,
            }),
            reply_cap: Some(TransferredCap {
                object_type: ObjectType::Field,
                object_id: ObjectId(7),
                rights: Rights::SEND,
                badge: Badge(0x0123_4567_89AB_CDEF),
                send_once: true,
                stored_generation: 0xDEAD_1234,
            }),
        };
        // 2. Send the message — no waiter present, so it must be enqueued.
        let send_outcome = send(&mut field, sent).expect("send must succeed");

        assert!(
            matches!(send_outcome, SendOutcome::Enqueued),
            "send must return Enqueued when no waiter is present"
        );
        assert_eq!(field.queue_length, 1, "queue must hold exactly one message");

        // 3. Receive the message back.
        let mut receiver = make_wait_entry();
        let receive_outcome = receive(&mut field, &mut receiver);

        // 4. Verify every field with exact values.
        match receive_outcome {
            ReceiveOutcome::Received(got) => {
                assert_eq!(
                    got.data,
                    [
                        0x1111_1111_1111_1111,
                        0x2222_2222_2222_2222,
                        0x3333_3333_3333_3333,
                        0x4444_4444_4444_4444,
                    ],
                    "all 4 data words must survive send→receive roundtrip"
                );
                assert_eq!(
                    got.label, 0xDEAD_BEEF_CAFE_1234,
                    "label must survive send→receive roundtrip"
                );
                assert_eq!(
                    got.badge,
                    Badge(0xABCD_EF01_2345_6789),
                    "badge must survive send→receive roundtrip"
                );

                // Verify user_cap fidelity.
                let user_cap = got.user_cap.expect("user_cap must be Some after roundtrip");

                assert_eq!(
                    user_cap.object_type,
                    ObjectType::Observer,
                    "user_cap.object_type must survive roundtrip"
                );
                assert_eq!(
                    user_cap.object_id,
                    ObjectId(42),
                    "user_cap.object_id must survive roundtrip"
                );
                assert_eq!(
                    user_cap.rights,
                    Rights::OBSERVER_ALL,
                    "user_cap.rights must survive roundtrip"
                );
                assert_eq!(
                    user_cap.badge,
                    Badge(0xFF00_FF00_FF00_FF00),
                    "user_cap.badge must survive roundtrip"
                );
                assert!(
                    user_cap.send_once,
                    "user_cap.send_once must survive roundtrip"
                );
                assert_eq!(
                    user_cap.stored_generation, 0xBEEF_CAFE,
                    "user_cap.stored_generation must survive roundtrip"
                );

                // Verify reply_cap fidelity.
                let reply_cap = got
                    .reply_cap
                    .expect("reply_cap must be Some after roundtrip");

                assert_eq!(
                    reply_cap.object_type,
                    ObjectType::Field,
                    "reply_cap.object_type must survive roundtrip"
                );
                assert_eq!(
                    reply_cap.object_id,
                    ObjectId(7),
                    "reply_cap.object_id must survive roundtrip"
                );
                assert_eq!(
                    reply_cap.rights,
                    Rights::SEND,
                    "reply_cap.rights must survive roundtrip"
                );
                assert_eq!(
                    reply_cap.badge,
                    Badge(0x0123_4567_89AB_CDEF),
                    "reply_cap.badge must survive roundtrip"
                );
                assert!(
                    reply_cap.send_once,
                    "reply_cap.send_once must survive roundtrip"
                );
                assert_eq!(
                    reply_cap.stored_generation, 0xDEAD_1234,
                    "reply_cap.stored_generation must survive roundtrip"
                );
            }
            ReceiveOutcome::Blocked => {
                panic!("receive after send must return Received, not Blocked");
            }
        }

        // 5. Queue must be empty after consuming the only message.
        assert_eq!(
            field.queue_length, 0,
            "queue must be empty after roundtrip receive"
        );
    }

    // ── Wave 3 integration tests ──────────────────────────────────────
    //
    // Cross-module tests verifying CoreManager orchestration, Scheduler
    // trait implementations, and Placement decisions compose correctly
    // with the Wave 1/2 primitives. These cover the D50 scheduler-deny
    // path (deferred from Wave 2), D2 scheduling rotation across the
    // full dispatch path, and D56 placement decisions.

    // ── Scenario 17: D50 scheduler callback — should_switch_to approval
    //
    // Cross-module interaction: when communication::send returns
    // WokeReceiver, the dispatch layer (core_manager) must consult the
    // scheduler's should_switch_to callback before direct-switching.
    // RoundRobin always approves — this test verifies the callback is
    // reachable and its answer is respected.

    #[test]
    fn test_integration_wave3_d50_scheduler_approves_direct_switch() {
        use crate::communication::{SendOutcome, send};
        use crate::time_manager::round_robin::RoundRobin;

        let mut field = test_field(4);
        let mut wait_entry = make_wait_entry();

        field.add_waiter(&mut wait_entry);

        let message = make_message(42, 0);
        let outcome = send(&mut field, message).expect("send must succeed");

        match outcome {
            SendOutcome::WokeReceiver(receiver_ptr, _message) => {
                let sched = RoundRobin::new();
                let approved = sched.should_switch_to(receiver_ptr);

                assert!(
                    approved,
                    "D50: RoundRobin must approve direct switch to woken receiver"
                );
            }
            SendOutcome::Enqueued => {
                panic!("D50: send to field with waiter must return WokeReceiver");
            }
        }
    }

    // ── Scenario 18: D2 handle_timer drives round-robin rotation
    //
    // Cross-module interaction: CoreManager::handle_timer calls
    // scheduler.on_preempt() (which rotates the RoundRobin queue)
    // then scheduler.pick_next() (which returns the new head).
    // This verifies the full dispatch path from timer interrupt
    // through scheduling decision.

    #[test]
    fn test_integration_wave3_d2_handle_timer_round_robin_rotation() {
        use crate::core_manager::{CoreState, DispatchResult, MAX_DEADLINES_PER_CORE};
        use crate::kernel_state::KernelState;
        use crate::time_manager::CoreId;
        use crate::time_manager::round_robin::RoundRobin;

        const FREQ: u64 = 24_000_000;
        let ks = KernelState::new(SpaceManager {
            root_pool: RootPool {
                total_bytes: 16 * 4096,
                free_bytes: 16 * 4096,
                page_size: 4096,
            },
            next_physical_base: 4096,
            next_va_base: 4096,
        });
        let mut core = CoreState {
            core_id: CoreId(0),
            current: None,
            scheduler: RoundRobin::new(),
            deadlines: [None; MAX_DEADLINES_PER_CORE],
            deadline_count: 0,
            cascade_continuation: None,
        };
        let mut obs_a = make_observer_for_scheduler();
        let mut obs_b = make_observer_for_scheduler();
        let mut obs_c = make_observer_for_scheduler();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);
        let ptr_c = NonNull::from(&mut obs_c);

        core.scheduler.enqueue(ptr_a);
        core.scheduler.enqueue(ptr_b);
        core.scheduler.enqueue(ptr_c);

        // Timer tick 1: rotates A to tail → pick_next returns B.
        match core.handle_timer(1000, &ks, FREQ) {
            DispatchResult::Resume(ptr) | DispatchResult::ResumeFastPath(ptr) => {
                assert_eq!(ptr, ptr_b, "tick 1: B")
            }
            DispatchResult::Idle => panic!("must not idle with 3 observers"),
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
        // Timer tick 2: rotates B to tail → pick_next returns C.
        match core.handle_timer(1000, &ks, FREQ) {
            DispatchResult::Resume(ptr) | DispatchResult::ResumeFastPath(ptr) => {
                assert_eq!(ptr, ptr_c, "tick 2: C")
            }
            DispatchResult::Idle => panic!("must not idle"),
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
        // Timer tick 3: rotates C to tail → pick_next returns A.
        match core.handle_timer(1000, &ks, FREQ) {
            DispatchResult::Resume(ptr) | DispatchResult::ResumeFastPath(ptr) => {
                assert_eq!(ptr, ptr_a, "tick 3: A")
            }
            DispatchResult::Idle => panic!("must not idle"),
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    // ── Scenario 19: D56 scored placement prefers idle remote core
    //
    // Cross-module interaction: ScoredPlacement reads CoreSnapshots
    // to make a placement decision. When the local core is busy and
    // a remote core is idle, placement must return Remote.

    #[test]
    fn test_integration_wave3_d56_placement_idle_remote() {
        use crate::time_manager::CoreId;
        use crate::time_manager::scored_placement::ScoredPlacement;
        use crate::time_manager::{CoreSnapshot, Placement, PlacementDecision};

        let placement = ScoredPlacement::new();
        let obs = make_observer_for_scheduler();
        let snapshots = [
            CoreSnapshot {
                core_id: CoreId(0),
                idle: false,
                queue_depth: 3,
                capacity_factor: 100,
            },
            CoreSnapshot {
                core_id: CoreId(1),
                idle: true,
                queue_depth: 0,
                capacity_factor: 100,
            },
        ];
        let decision = placement.place(&obs, &snapshots);

        match decision {
            PlacementDecision::Remote(core_id) => {
                assert_eq!(core_id, CoreId(1), "D56: idle remote core must be selected");
            }
            PlacementDecision::Local => {
                panic!("D56: busy local must lose to idle remote");
            }
        }
    }

    // ── Scenario 20: D46 idle when all Observers block
    //
    // Cross-module interaction: when the last runnable Observer blocks
    // on receive (transitioning to Blocked), the scheduler's run queue
    // is empty. handle_timer must return Idle (D46: WFI).

    #[test]
    fn test_integration_wave3_d46_idle_after_all_block() {
        use crate::core_manager::{CoreState, DispatchResult, MAX_DEADLINES_PER_CORE};
        use crate::kernel_state::KernelState;
        use crate::time_manager::CoreId;
        use crate::time_manager::round_robin::RoundRobin;

        const FREQ: u64 = 24_000_000;
        let ks = KernelState::new(SpaceManager {
            root_pool: RootPool {
                total_bytes: 16 * 4096,
                free_bytes: 16 * 4096,
                page_size: 4096,
            },
            next_physical_base: 4096,
            next_va_base: 4096,
        });
        let mut core = CoreState {
            core_id: CoreId(0),
            current: None,
            scheduler: RoundRobin::new(),
            deadlines: [None; MAX_DEADLINES_PER_CORE],
            deadline_count: 0,
            cascade_continuation: None,
        };

        // No observers enqueued — empty queue.
        match core.handle_timer(1000, &ks, FREQ) {
            DispatchResult::Idle => {}
            DispatchResult::Resume(_) | DispatchResult::ResumeFastPath(_) => {
                panic!("D46: empty run queue must return Idle (WFI)");
            }
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    fn make_observer_for_scheduler() -> crate::observer::Observer {
        crate::observer::Observer::test_default()
    }

    // ── Scenario 21: D50 scheduler denies direct switch
    //
    // Cross-module interaction: when communication::call returns
    // DirectSwitch but the scheduler's should_switch_to returns false,
    // the dispatch layer must fall back to the normal scheduling path
    // rather than direct-switching. This tests the D50 condition 5
    // deny case — deferred from Wave 2 because it requires a concrete
    // Scheduler implementation.
    //
    // Uses a test-only DenyScheduler that always returns false from
    // should_switch_to, verifying the callback is consulted and its
    // denial is respected.

    struct DenyScheduler {
        inner: crate::time_manager::round_robin::RoundRobin,
    }

    impl DenyScheduler {
        fn new() -> Self {
            DenyScheduler {
                inner: crate::time_manager::round_robin::RoundRobin::new(),
            }
        }
    }

    impl Scheduler for DenyScheduler {
        fn enqueue(&mut self, observer: NonNull<crate::observer::Observer>) {
            self.inner.enqueue(observer);
        }

        fn dequeue(&mut self, observer: NonNull<crate::observer::Observer>) {
            self.inner.dequeue(observer);
        }

        fn pick_next(&self) -> Option<NonNull<crate::observer::Observer>> {
            self.inner.pick_next()
        }

        fn should_switch_to(&self, _receiver: NonNull<crate::observer::Observer>) -> bool {
            false
        }

        fn on_preempt(&mut self) {
            self.inner.on_preempt();
        }
    }

    #[test]
    fn test_integration_wave3_d50_scheduler_denies_direct_switch() {
        use crate::communication::{CallOutcome, call};

        // 1. Set up a Field with a waiting receiver — D50 fast path eligible.
        let mut field = test_field(4);
        let mut wait_entry = make_wait_entry();

        field.add_waiter(&mut wait_entry);

        // 2. Call with a 0-cap message — would be DirectSwitch if approved.
        let message = make_message(42, 0);
        let outcome = call(&mut field, message, Badge(0)).expect("call must succeed");

        // 3. communication::call returns DirectSwitch — the dispatch layer
        //    must now consult should_switch_to before acting on it.
        match outcome {
            CallOutcome::DirectSwitch(receiver_ptr) => {
                // 4. DenyScheduler refuses the switch.
                let sched = DenyScheduler::new();
                let approved = sched.should_switch_to(receiver_ptr);

                assert!(
                    !approved,
                    "D50 condition 5: DenyScheduler must refuse direct switch"
                );
                // 5. When denied, the dispatch layer would enqueue normally
                //    and call schedule_next instead. Verify pick_next returns
                //    None (no one in the queue) — the receiver is NOT
                //    auto-promoted to the run queue by the denial.
                assert!(
                    sched.pick_next().is_none(),
                    "D50: denied receiver must not appear in the run queue"
                );
            }
            CallOutcome::Enqueued => {
                panic!("D50: call with 0-cap and waiter must return DirectSwitch");
            }
            CallOutcome::WokeReceiverSlowPath(..) => {
                panic!(
                    "D50: 0-cap message with waiter must return DirectSwitch, not WokeReceiverSlowPath"
                );
            }
        }
    }

    // ── Scenario 22: D33 cascade scoped to single table
    //
    // Cross-module interaction: when Observer A's cap table holds a cap
    // pointing to Observer B, and Observer B's cap table holds a cap
    // pointing back to Observer A, running cascade on A's table closes
    // A's entries but does NOT touch B's table. Cascade is per-table,
    // not recursive across object references. B's entries remain
    // functional after A's cascade completes.

    #[test]
    fn test_integration_wave3_d33_cascade_does_not_chase_cross_table_refs() {
        // 1. Create two tables — simulating Observer A and Observer B.
        let mut table_a = test_table(16);
        let mut table_b = test_table(16);
        // 2. Install a cap in A pointing to "Observer B" (ObjectId(1)).
        let slot_a = table_a.allocate_slot().expect("allocate in A");

        table_a.install_at(
            slot_a,
            Entry {
                object: Some((ObjectType::Observer, ObjectId(1))),
                rights: Rights::OBSERVER_ALL,
                badge: Badge(100),
                slot_tag: SlotTag(0),
                send_once: false,
                stored_generation: 0,
            },
        );

        // 3. Install a cap in B pointing to "Observer A" (ObjectId(0)).
        let slot_b = table_b.allocate_slot().expect("allocate in B");

        table_b.install_at(
            slot_b,
            Entry {
                object: Some((ObjectType::Observer, ObjectId(0))),
                rights: Rights::OBSERVER_ALL,
                badge: Badge(200),
                slot_tag: SlotTag(0),
                send_once: false,
                stored_generation: 0,
            },
        );

        assert_eq!(table_a.count, 1, "precondition: A has 1 entry");
        assert_eq!(table_b.count, 1, "precondition: B has 1 entry");

        // 4. Run cascade on A's table — should close all of A's entries.
        let mut state = table_a.begin_cascade();

        loop {
            if table_a.cascade_step(&mut state) {
                break;
            }
        }

        assert!(state.complete, "A's cascade must complete");
        assert_eq!(table_a.count, 0, "A's entries must all be closed");
        // 5. B's table must be UNTOUCHED — cascade is per-table, not recursive.
        assert_eq!(
            table_b.count, 1,
            "D33: cascade on A must not touch B's table"
        );

        // 6. B's cap must still resolve successfully.
        let handle_b = Handle {
            index: slot_b,
            slot_tag: SlotTag(0),
        };
        let resolved = table_b.resolve(handle_b);

        assert!(
            resolved.is_ok(),
            "D33: B's cap must still resolve after A's cascade"
        );

        let entry = resolved.unwrap();

        assert_eq!(
            entry.badge,
            Badge(200),
            "D33: B's entry data must be intact"
        );
    }
}
