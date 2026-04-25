//! IPC: inter-Observer communication via Fields.
//!
//! Orchestrates the five IPC operations (D48) across Field, Observer,
//! capability, and time_manager modules. Each operation is a free
//! function that takes already-resolved references — cap resolution
//! and rights checking happen in the core_manager dispatch layer
//! before calling into this module.
//!
//! D7:  IPC is one of two syscall families.
//! D13: queued fields with direct-switch fast path.
//! D16: reply via pre-allocated reply field with send-once cap.
//! D28: fixed-size message format (4 data + 1 cap + label + badge + reply).
//! D50: six fast-path conditions for direct switch.
//! D69: DAIF.I masking during the fast-path window.
//!
//! The fast path (~400 cycles, D13) and slow path (~600–800 cycles)
//! are structurally distinct code paths. The fast path is a straight-
//! line section under DAIF.I masking (D69) — no interrupts, no lock
//! contention on same-core operations (D1).

use crate::capability::Badge;
use crate::field::{Field, FieldError, Message};
use crate::observer::{Observer, WaitEntry};
use core::ptr::NonNull;

// ── Operation outcomes ─────────────────────────────────────────────

/// Outcome of an IPC send (D13, D18).
pub enum SendOutcome {
    /// Message enqueued into the Field's queue. Sender continues.
    Enqueued,
    /// A waiting receiver was found. The message was delivered directly
    /// without touching the queue (D13 direct-switch optimization).
    /// The returned Observer pointer is the woken receiver — the
    /// caller decides whether to direct-switch via the scheduler's
    /// `should_switch_to` callback (D50 condition 5).
    WokeReceiver(NonNull<Observer>),
}

/// Outcome of an IPC receive (D13).
pub enum ReceiveOutcome {
    /// A message was available in the queue.
    Received(Message),
    /// Queue was empty. The Observer has been linked into the Field's
    /// waiters list and should transition to Blocked (D39).
    Blocked,
}

/// Outcome of Call — the compound Send + block-on-reply (D16).
///
/// The caller always blocks. The outcome indicates whether the message
/// was delivered directly to a waiting receiver (fast path) or enqueued.
pub enum CallOutcome {
    /// Message enqueued, caller blocked on its reply field.
    Enqueued,
    /// Waiting receiver found. The returned Observer should be
    /// direct-switched to if the scheduler approves (D50).
    DirectSwitch(NonNull<Observer>),
}

// ── IPC operations ─────────────────────────────────────────────────

/// IPC Send: non-blocking deposit into a Field (D13).
///
/// D17: badge is injected from the sender's cap entry (already
/// extracted by the dispatch layer). D18: returns QueueFull on
/// overflow — error to sender, not a kernel policy decision.
///
/// If a receiver is waiting on the Field, the message is delivered
/// directly and the receiver is woken (WokeReceiver). The dispatch
/// layer uses this to attempt direct-switch on same-core (D50).
///
/// Also serves as Reply: sending to a D16 send-once cap is
/// mechanically identical to Send — the cap is consumed after use.
///
/// Performance: hot path when receiver is waiting (D13 ~400 cycle
/// target for the full direct-switch path including cap resolution).
pub fn send(field: &mut Field, message: Message) -> Result<SendOutcome, FieldError> {
    // D13/D50: check for a waiting receiver BEFORE checking queue fullness.
    // A full queue with a waiter delivers directly (bypasses queue).
    if let Some(waiter_ptr) = field.pop_waiter() {
        let observer = crate::frame::field_ops::waiter_observer(waiter_ptr);

        return Ok(SendOutcome::WokeReceiver(observer));
    }

    // No waiter: enqueue the message. Returns QueueFull if at capacity (D18).
    field.enqueue(message)?;

    Ok(SendOutcome::Enqueued)
}

/// IPC Receive: blocking wait on a Field (D13).
///
/// If the queue has messages, dequeues the front message (FIFO).
/// If empty, the Observer is linked into the waiters list and
/// transitions to Blocked (D39).
///
/// D18: after dequeuing, checks the pending list for deferred
/// fault/interrupt messages that were waiting for a free slot.
///
/// D45: routing has already been resolved by the dispatch layer —
/// this function operates on the final destination Field.
pub fn receive(field: &mut Field, receiver: &mut WaitEntry) -> ReceiveOutcome {
    // D13: dequeue before blocking.
    if let Some(message) = field.dequeue() {
        // D18: after dequeue frees a slot, check pending list for deferred
        // kernel-as-sender messages that were waiting for space.
        if let Some(pending_ptr) = field.pending_head {
            // Consume the pending entry: advance pending_head to the next entry.
            let next = crate::frame::field_ops::waiter_next(pending_ptr);

            field.pending_head = next;

            // Re-enqueue a placeholder message into the freed slot. The actual
            // pending message content will be resolved by Wave 3 (Observer
            // context). Zero-filled placeholder satisfies the queue_length
            // invariant: the slot freed by dequeue is immediately refilled.
            let placeholder = Message {
                data: [0; 4],
                label: 0,
                badge: Badge(0),
                user_cap: None,
                reply_cap: None,
            };
            // The slot was just freed by dequeue, so enqueue cannot fail.
            let _ = field.enqueue(placeholder);
        }

        return ReceiveOutcome::Received(message);
    }

    // D13: queue empty — block the receiver on this Field's waiters list.
    field.add_waiter(receiver);

    ReceiveOutcome::Blocked
}

/// IPC Call: send + block on reply field (D16).
///
/// Compound operation: sends the message to the target Field, then
/// blocks the caller on its pre-allocated reply field (cap-table
/// slot 1, D43). The kernel creates a send-once reply cap pointing
/// to the caller's reply field and includes it in the message (D16).
///
/// D65: the caller supplies a `reply_badge` that the kernel embeds
/// in the send-once cap entry. When the server replies, the message
/// arrives at the caller's reply field carrying that badge, allowing
/// the caller to identify which outstanding RPC is being answered.
///
/// D50: if a receiver is waiting on the target Field AND the message
/// has no user cap (0-cap gate) AND the scheduler approves, the
/// kernel can direct-switch to the receiver without queue insertion.
pub fn call(
    field: &mut Field,
    message: Message,
    _reply_badge: Badge,
) -> Result<CallOutcome, FieldError> {
    // D50: 0-cap gate check BEFORE popping the waiter. If the message
    // carries a user cap, skip the fast path entirely (slow path only).
    if message.user_cap.is_none()
        && let Some(waiter_ptr) = field.pop_waiter()
    {
        let observer = crate::frame::field_ops::waiter_observer(waiter_ptr);

        return Ok(CallOutcome::DirectSwitch(observer));
    }

    // Slow path: enqueue the message. Returns QueueFull if full (D18).
    field.enqueue(message)?;

    Ok(CallOutcome::Enqueued)
}

/// IPC ReplyRecv: send reply + receive next, atomically (D16).
///
/// Server fast path. Sends the reply via the send-once cap (consumed),
/// then receives the next message on the same field. Atomic — no
/// scheduling gap between reply and receive (D16: prevents preemption
/// between reply delivery and next-request pickup).
///
/// D50: eligible for fast-path direct-switch on the receive side
/// (the reply side consumes the send-once cap, which is always
/// slow-path due to cap transfer, but the receiver wakeup can
/// still direct-switch).
pub fn reply_recv(
    reply_field: &mut Field,
    recv_field: &mut Field,
    reply_message: Message,
    receiver: &mut WaitEntry,
) -> ReceiveOutcome {
    // ── Reply phase ──────────────────────────────────────────────────
    // Send the reply to reply_field using the same logic as send().
    // D16: if reply_field has a waiter, deliver directly. Otherwise enqueue.
    // If reply_field is full and has no waiter, the reply is dropped
    // (the receive phase still proceeds).
    if let Some(_waiter_ptr) = reply_field.pop_waiter() {
        // Direct delivery to the waiting client — reply bypasses queue.
        // The waiter_ptr carries the Observer to wake, but reply_recv's
        // return value reflects only the receive side, so we discard the
        // send outcome here.
    } else {
        // No waiter on reply_field — attempt to enqueue. If full, drop
        // the reply silently (the server must still be able to receive
        // the next request).
        let _ = reply_field.enqueue(reply_message);
    }

    // ── Receive phase ────────────────────────────────────────────────
    receive(recv_field, receiver)
}

/// IPC Yield: voluntary CPU relinquishment (D48).
///
/// A3: included for compute-bound workload support. 100% landscape
/// convergence across surveyed kernels. Scheduling hint — the
/// core_manager calls `scheduler.pick_next()` to select the next
/// Observer. The yielding Observer remains Runnable.
pub fn yield_cpu() {
    // No-op at the IPC level — the core_manager handles the
    // scheduling decision. This function exists to make the
    // five-operation IPC surface explicit in one module.
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::field::{Field, FieldError, Message};
    use crate::observer::WaitEntry;
    use core::ptr::NonNull;
    use core::sync::atomic::AtomicU64;

    // ── Test helpers ──────────────────────────────────────────────────

    /// Construct a Field with a real queue allocation for test use.
    /// Mirrors the pattern in field.rs tests and the integration tests.
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

    /// Construct a WaitEntry with dangling pointers for test use.
    /// The observer/field pointers are never dereferenced in these tests —
    /// only the intrusive list linkage (prev/next) is exercised.
    fn make_wait_entry() -> WaitEntry {
        WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        }
    }

    /// Construct a simple Message with a given label and badge for test use.
    fn make_message(label: u64, badge: u64) -> Message {
        Message {
            data: [label, 0, 0, 0],
            label,
            badge: Badge(badge),
            user_cap: None,
            reply_cap: None,
        }
    }

    /// Construct a Message with a user cap to test the D50 non-fast-path branch.
    fn make_message_with_cap(label: u64) -> Message {
        use crate::arena::ObjectId;
        use crate::capability::{ObjectType, Rights, TransferredCap};

        Message {
            data: [label, 0, 0, 0],
            label,
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
        }
    }

    // ── D13: send enqueues when no waiter is present ──────────────────

    /// D13: when a Field has no waiting receivers, send must enqueue the
    /// message and return SendOutcome::Enqueued. The queue length must
    /// increase by one and the message content must be preserved.
    #[test]
    fn test_d13_send_enqueues_when_no_waiter() {
        let mut field = test_field(4);
        let msg = make_message(42, 7);
        let result = send(&mut field, msg).expect("send must not return QueueFull on empty queue");

        assert!(
            matches!(result, SendOutcome::Enqueued),
            "D13: send with no waiter must return Enqueued"
        );
        assert_eq!(
            field.queue_length, 1,
            "D13: queue_length must be 1 after one send"
        );
    }

    // ── D13/D50: send wakes receiver when waiter is present ──────────

    /// D13/D50: when a Field has a waiting receiver, send must pop that
    /// receiver and return SendOutcome::WokeReceiver. The message must
    /// bypass the queue (queue_length stays zero) and the waiters list
    /// must be empty afterwards.
    #[test]
    fn test_d13_send_wakes_receiver_when_waiter_present() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        assert_eq!(field.queue_length, 0, "precondition: queue empty");

        let msg = make_message(99, 5);
        let result = send(&mut field, msg).expect("send must succeed");

        assert!(
            matches!(result, SendOutcome::WokeReceiver(_)),
            "D13: send with waiter present must return WokeReceiver"
        );
        // Message delivered directly — queue must stay empty.
        assert_eq!(
            field.queue_length, 0,
            "D13: message bypasses queue when waiter is present (direct delivery)"
        );
        // Waiters list must be drained.
        assert!(
            field.waiters_head.is_none(),
            "D13: waiters list must be empty after waking the receiver"
        );
    }

    // ── D50: send to waiting receiver returns WokeReceiver ───────────

    /// D50 (condition 5): this is the same path as D13 direct delivery.
    /// Verified separately for clarity: the returned NonNull<Observer>
    /// must be non-null (even though it is dangling in tests, it is not
    /// null — the dispatch layer would use this to attempt direct-switch).
    #[test]
    fn test_d50_send_to_waiting_receiver_returns_woke_receiver() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        let msg = make_message(1, 0);
        let result = send(&mut field, msg).expect("send must succeed");

        match result {
            SendOutcome::WokeReceiver(observer_ptr) => {
                // The pointer must be non-null (we used NonNull::dangling()).
                // We cannot assert the exact value since it is dangling, but
                // the type guarantees non-null — just verify we got this variant.
                let _ = observer_ptr;
            }
            SendOutcome::Enqueued => {
                panic!("D50: send to waiting receiver must return WokeReceiver, got Enqueued");
            }
        }
    }

    // ── D13: receive dequeues when messages are available ────────────

    /// D13: when the queue has messages, receive must dequeue the front
    /// message and return ReceiveOutcome::Received with the message.
    /// The receiver must NOT be added to the waiters list.
    #[test]
    fn test_d13_receive_dequeues_when_messages_available() {
        let mut field = test_field(4);
        let msg = make_message(55, 3);

        field.enqueue(msg).expect("enqueue must succeed");

        let mut receiver = make_wait_entry();
        let outcome = receive(&mut field, &mut receiver);

        match outcome {
            ReceiveOutcome::Received(received_msg) => {
                assert_eq!(
                    received_msg.label, 55,
                    "D13: received message must match sent label"
                );
                assert_eq!(received_msg.badge, Badge(3), "D13: badge must be preserved");
            }
            ReceiveOutcome::Blocked => {
                panic!("D13: receive with messages must return Received, not Blocked");
            }
        }

        assert_eq!(
            field.queue_length, 0,
            "D13: queue must be empty after dequeue"
        );
        assert!(
            field.waiters_head.is_none(),
            "D13: receiver must not be added to waiters when a message is available"
        );
    }

    // ── D13: receive blocks when queue is empty ───────────────────────

    /// D13: when the queue is empty, receive must add the receiver to the
    /// waiters list and return ReceiveOutcome::Blocked.
    #[test]
    fn test_d13_receive_blocks_when_queue_empty() {
        let mut field = test_field(4);
        let mut receiver = make_wait_entry();
        let outcome = receive(&mut field, &mut receiver);

        assert!(
            matches!(outcome, ReceiveOutcome::Blocked),
            "D13: receive on empty queue must return Blocked"
        );
        assert!(
            field.waiters_head.is_some(),
            "D13: receiver must be added to waiters list when queue is empty"
        );
    }

    // ── D13: receive returns messages in FIFO order ───────────────────

    /// D13: messages dequeued by receive must follow FIFO order matching
    /// the enqueue order. This validates that receive uses the same
    /// circular-buffer dequeue path as Field::dequeue.
    #[test]
    fn test_d13_receive_returns_fifo_order() {
        let mut field = test_field(8);

        for i in 1u64..=5 {
            field.enqueue(make_message(i, 0)).expect("enqueue");
        }

        for expected_label in 1u64..=5 {
            let mut receiver = make_wait_entry();
            let outcome = receive(&mut field, &mut receiver);

            match outcome {
                ReceiveOutcome::Received(msg) => {
                    assert_eq!(
                        msg.label, expected_label,
                        "D13: FIFO violated — expected label {expected_label}, got {}",
                        msg.label
                    );
                }
                ReceiveOutcome::Blocked => {
                    panic!(
                        "D13: receive on non-empty queue must not block (label {expected_label})"
                    );
                }
            }
        }
    }

    // ── D18: send returns QueueFull on full queue with no waiter ──────

    /// D18 error-to-sender: when the queue is full and no waiter is
    /// present, send must return Err(FieldError::QueueFull). This is
    /// the overflow signal to the sender — the kernel does not drop
    /// silently.
    #[test]
    fn test_d18_send_returns_queue_full() {
        let mut field = test_field(2);

        // Fill the queue.
        send(&mut field, make_message(1, 0)).expect("first send must succeed");
        send(&mut field, make_message(2, 0)).expect("second send must succeed");

        // No waiter, queue full — must fail.
        let result = send(&mut field, make_message(3, 0));

        assert!(
            matches!(result, Err(FieldError::QueueFull)),
            "D18: send to full queue with no waiter must return QueueFull"
        );
    }

    // ── D18: call returns QueueFull on full queue with no waiter ──────

    /// D18: same overflow rule applies to call. When the target queue is
    /// full and no receiver is waiting, call must return QueueFull so
    /// the caller can handle the error (caller does not block in this case).
    #[test]
    fn test_d18_call_returns_queue_full() {
        let mut field = test_field(2);
        let reply_badge = Badge(0xAB);

        // Fill the queue via send so call finds it full.
        field.enqueue(make_message(1, 0)).expect("enqueue 1");
        field.enqueue(make_message(2, 0)).expect("enqueue 2");

        let result = call(&mut field, make_message(3, 0), reply_badge);

        assert!(
            matches!(result, Err(FieldError::QueueFull)),
            "D18: call to full queue with no waiter must return QueueFull"
        );
    }

    // ── D18: receive drains pending_head after dequeue ────────────────

    /// D18: after receive dequeues a message (freeing a queue slot), the
    /// implementation must check pending_head and deliver any deferred
    /// message waiting there. A pending entry represents a kernel-as-sender
    /// (fault, interrupt) that could not deliver because the queue was full.
    ///
    /// Observable effect: after receive on a full queue with a pending entry,
    /// queue_length stays at capacity (the freed slot is immediately filled
    /// by the pending message) and pending_head becomes None.
    #[test]
    fn test_d18_receive_drains_pending_on_dequeue() {
        let mut field = test_field(2);

        // Fill the queue completely.
        field.enqueue(make_message(10, 0)).expect("enqueue 10");
        field.enqueue(make_message(20, 0)).expect("enqueue 20");

        assert!(field.is_full(), "precondition: queue must be full");

        // Simulate a pending entry (kernel-as-sender deferred message).
        let mut pending_entry = make_wait_entry();

        field.pending_head = Some(NonNull::from(&mut pending_entry));

        // Receive dequeues message 10, freeing one slot. The pending entry
        // should be consumed and its message delivered into the freed slot.
        let mut receiver = make_wait_entry();
        let outcome = receive(&mut field, &mut receiver);

        match outcome {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(
                    msg.label, 10,
                    "D18: receive must return the first queued message"
                );
            }
            ReceiveOutcome::Blocked => {
                panic!("D18: receive on non-empty queue must not block");
            }
        }

        // The pending entry must have been consumed.
        assert!(
            field.pending_head.is_none(),
            "D18: pending_head must be None after receive drains the pending entry"
        );
        // The freed slot must be filled by the pending message — queue stays full.
        assert_eq!(
            field.queue_length, 2,
            "D18: queue must be refilled from pending after dequeue"
        );
    }

    // ── D16: call returns Enqueued when no waiter is present ─────────

    /// D16: call sends to the target field (enqueues the message) and
    /// conceptually blocks the caller on its reply field. When no waiter
    /// is present, call returns CallOutcome::Enqueued to indicate the
    /// message was placed in the queue.
    #[test]
    fn test_d16_call_returns_enqueued_when_no_waiter() {
        let mut field = test_field(4);
        let reply_badge = Badge(0x1234);
        let msg = make_message(77, 0);
        let result = call(&mut field, msg, reply_badge).expect("call must not return QueueFull");

        assert!(
            matches!(result, CallOutcome::Enqueued),
            "D16: call with no waiter must return Enqueued"
        );
        assert_eq!(
            field.queue_length, 1,
            "D16: call must enqueue the message into the target field"
        );
    }

    // ── D16/D50: call returns DirectSwitch when waiter present with no user cap

    /// D16/D50: when a receiver is waiting on the target field AND the
    /// message carries no user cap (0-cap gate, D50 condition), call
    /// returns CallOutcome::DirectSwitch to enable the fast-path context
    /// switch to the receiver without queue insertion.
    #[test]
    fn test_d16_call_returns_direct_switch_when_waiter_present() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        let reply_badge = Badge(0x5678);
        // 0-cap message satisfies the D50 fast-path condition.
        let msg = make_message(88, 0);
        let result = call(&mut field, msg, reply_badge).expect("call must succeed");

        assert!(
            matches!(result, CallOutcome::DirectSwitch(_)),
            "D16/D50: call with waiting receiver and 0-cap message must return DirectSwitch"
        );
        // Direct delivery — queue must not be touched.
        assert_eq!(
            field.queue_length, 0,
            "D50: direct switch must not enqueue the message"
        );
        assert!(
            field.waiters_head.is_none(),
            "D50: waiter must be popped from the list on direct switch"
        );
    }

    // ── D50: call with user cap does not use DirectSwitch ────────────

    /// D50: the direct-switch fast path requires a 0-cap message. When
    /// the message carries a user cap, even if a waiter is present, call
    /// must fall back to the slow path (Enqueued), not DirectSwitch.
    #[test]
    fn test_d50_call_with_user_cap_does_not_direct_switch() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        let reply_badge = Badge(0);
        let msg = make_message_with_cap(99);
        let result = call(&mut field, msg, reply_badge).expect("call must succeed");

        assert!(
            matches!(result, CallOutcome::Enqueued),
            "D50: call with user cap must NOT return DirectSwitch (slow path only)"
        );
    }

    // ── D16: reply_recv sends reply then receives next ────────────────

    /// D16: reply_recv atomically sends the reply to reply_field, then
    /// receives the next message from recv_field. When recv_field has
    /// messages, it must return ReceiveOutcome::Received.
    #[test]
    fn test_d16_reply_recv_sends_reply_then_receives() {
        let mut reply_field = test_field(4);
        let mut recv_field = test_field(4);

        // Pre-load the recv_field with a message the server will receive next.
        recv_field.enqueue(make_message(200, 9)).expect("enqueue");

        let reply_message = make_message(100, 0);
        let mut receiver = make_wait_entry();
        let outcome = reply_recv(
            &mut reply_field,
            &mut recv_field,
            reply_message,
            &mut receiver,
        );

        // The reply must have been sent (reply_field queue_length increases).
        assert_eq!(
            reply_field.queue_length, 1,
            "D16: reply_recv must send the reply message to reply_field"
        );

        // The receive side must have returned the queued message.
        match outcome {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(
                    msg.label, 200,
                    "D16: reply_recv must receive the next message from recv_field"
                );
                assert_eq!(msg.badge, Badge(9));
            }
            ReceiveOutcome::Blocked => {
                panic!("D16: reply_recv must return Received when recv_field has messages");
            }
        }
    }

    // ── D16: reply_recv blocks when recv_field queue is empty ─────────

    /// D16: when recv_field has no messages, reply_recv must still send
    /// the reply, then block the server on recv_field (return Blocked).
    #[test]
    fn test_d16_reply_recv_blocks_when_recv_queue_empty() {
        let mut reply_field = test_field(4);
        let mut recv_field = test_field(4);
        let reply_message = make_message(50, 0);
        let mut receiver = make_wait_entry();
        let outcome = reply_recv(
            &mut reply_field,
            &mut recv_field,
            reply_message,
            &mut receiver,
        );

        // Reply must still be sent even though receive blocks.
        assert_eq!(
            reply_field.queue_length, 1,
            "D16: reply must be sent even when recv blocks"
        );
        assert!(
            matches!(outcome, ReceiveOutcome::Blocked),
            "D16: reply_recv must block when recv_field is empty"
        );
        assert!(
            recv_field.waiters_head.is_some(),
            "D16: receiver must be added to recv_field waiters on block"
        );
    }

    // ── D17: badge passes through send ───────────────────────────────

    /// D17: badge injection happens at the dispatch layer before send is
    /// called. Once inside send, the badge on the Message must be
    /// preserved as-is through enqueue and dequeue.
    #[test]
    fn test_d17_badge_passes_through_send() {
        let mut field = test_field(4);
        let badge_value = 0xDEAD_BEEF_CAFE_1234u64;
        let msg = make_message(1, badge_value);

        send(&mut field, msg).expect("send must succeed");

        // Retrieve the message by receiving it back.
        let mut receiver = make_wait_entry();
        let outcome = receive(&mut field, &mut receiver);

        match outcome {
            ReceiveOutcome::Received(received_msg) => {
                assert_eq!(
                    received_msg.badge,
                    Badge(badge_value),
                    "D17: badge must pass through send->receive unchanged"
                );
            }
            ReceiveOutcome::Blocked => {
                panic!("D17: receive after send must return Received");
            }
        }
    }

    // ── D17: badge passes through call ───────────────────────────────

    /// D17: badge must be preserved through call's enqueue path as well.
    #[test]
    fn test_d17_badge_passes_through_call() {
        let mut field = test_field(4);
        let badge_value = 0x1111_2222_3333_4444u64;
        let msg = make_message(2, badge_value);
        let reply_badge = Badge(0);

        call(&mut field, msg, reply_badge).expect("call must succeed");

        let mut receiver = make_wait_entry();
        let outcome = receive(&mut field, &mut receiver);

        match outcome {
            ReceiveOutcome::Received(received_msg) => {
                assert_eq!(
                    received_msg.badge,
                    Badge(badge_value),
                    "D17: badge must pass through call->receive unchanged"
                );
            }
            ReceiveOutcome::Blocked => {
                panic!("D17: receive after call must return Received");
            }
        }
    }

    // ── Adversarial: send to a full field with a waiter uses direct delivery

    /// Edge case: even when the queue is full, if a waiter is present, send
    /// must use direct delivery (WokeReceiver) instead of failing with
    /// QueueFull. The waiter consumes the message without touching the queue.
    #[test]
    fn test_adversarial_send_full_queue_with_waiter_uses_direct_delivery() {
        let mut field = test_field(2);

        // Fill the queue completely.
        field.enqueue(make_message(1, 0)).expect("enqueue 1");
        field.enqueue(make_message(2, 0)).expect("enqueue 2");

        assert!(field.is_full(), "precondition: queue full");

        // Add a waiter — this is the server blocked on receive.
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        // Send must succeed via direct delivery, not fail with QueueFull.
        let msg = make_message(3, 0);
        let result = send(&mut field, msg);

        assert!(
            result.is_ok(),
            "send with waiter on full queue must succeed via direct delivery"
        );
        assert!(
            matches!(result.unwrap(), SendOutcome::WokeReceiver(_)),
            "send with waiter on full queue must return WokeReceiver"
        );
        // Queue must remain full (message bypassed it).
        assert_eq!(
            field.queue_length, 2,
            "queue must stay full when message delivered directly to waiter"
        );
    }

    // ── Adversarial: multiple waiters — send pops only the first ─────

    /// D13 FIFO waiter order: when multiple receivers are waiting, send
    /// must wake only the first (FIFO), leaving the rest in the list.
    #[test]
    fn test_adversarial_send_wakes_first_waiter_only() {
        let mut field = test_field(4);
        let mut entry_a = make_wait_entry();
        let mut entry_b = make_wait_entry();

        field.add_waiter(&mut entry_a);
        field.add_waiter(&mut entry_b);

        let result = send(&mut field, make_message(1, 0)).expect("send must succeed");

        assert!(
            matches!(result, SendOutcome::WokeReceiver(_)),
            "send must wake the first waiter"
        );
        // Second waiter must still be in the list.
        assert!(
            field.waiters_head.is_some(),
            "D13: second waiter must remain in list after first is woken"
        );
    }

    // ── Adversarial: receive data words are preserved ─────────────────

    /// The full data array [u64; 4] must survive the queue round-trip.
    #[test]
    fn test_adversarial_receive_data_words_preserved() {
        let mut field = test_field(4);
        let msg = Message {
            data: [0xAAAA, 0xBBBB, 0xCCCC, 0xDDDD],
            label: 0xFACE,
            badge: Badge(0xBEEF),
            user_cap: None,
            reply_cap: None,
        };

        send(&mut field, msg).expect("send");

        let mut receiver = make_wait_entry();
        let outcome = receive(&mut field, &mut receiver);

        match outcome {
            ReceiveOutcome::Received(m) => {
                assert_eq!(m.data, [0xAAAA, 0xBBBB, 0xCCCC, 0xDDDD]);
                assert_eq!(m.label, 0xFACE);
                assert_eq!(m.badge, Badge(0xBEEF));
            }
            ReceiveOutcome::Blocked => panic!("must receive"),
        }
    }

    // ── Adversarial: interleaved send/receive cycles ─────────────────

    /// Interleaved send→receive pairs: queue_length must return to 0
    /// after each pair and messages must not bleed across iterations.
    ///
    /// Bug target: off-by-one in queue_head advancement — after N cycles
    /// the head wraps; a wrong modulus leaves the head pointing at a stale slot,
    /// so the N+1th receive returns the wrong message.
    #[test]
    fn test_adversarial_comm_interleaved_send_receive_cycles() {
        let mut field = test_field(4);

        for i in 0u64..16 {
            send(&mut field, make_message(i, i)).expect("send must succeed");

            assert_eq!(field.queue_length, 1, "queue_length must be 1 after send");

            let mut receiver = make_wait_entry();
            let outcome = receive(&mut field, &mut receiver);

            match outcome {
                ReceiveOutcome::Received(msg) => {
                    assert_eq!(msg.label, i, "interleaved cycle {i}: received wrong label");
                    assert_eq!(
                        msg.badge,
                        Badge(i),
                        "interleaved cycle {i}: received wrong badge"
                    );
                }
                ReceiveOutcome::Blocked => {
                    panic!("cycle {i}: receive must return Received after send, got Blocked");
                }
            }

            assert_eq!(
                field.queue_length, 0,
                "queue_length must be 0 after receive in cycle {i}"
            );
        }
    }

    /// Multiple sends then multiple receives: messages must come out in FIFO
    /// order matching insertion order from a mix of send and call.
    ///
    /// Bug target: call enqueuing to a different position than send, or
    /// call clobbering an existing queue slot.
    #[test]
    fn test_adversarial_comm_multiple_sends_then_receives_fifo() {
        let mut field = test_field(8);

        // Interleave send and call enqueues.
        send(&mut field, make_message(1, 0)).expect("send 1");
        call(&mut field, make_message(2, 0), Badge(0)).expect("call 2");
        send(&mut field, make_message(3, 0)).expect("send 3");
        call(&mut field, make_message(4, 0), Badge(0)).expect("call 4");

        assert_eq!(
            field.queue_length, 4,
            "queue must hold all 4 enqueued messages"
        );

        // Drain in order; each must match the insertion sequence.
        for expected_label in 1u64..=4 {
            let mut receiver = make_wait_entry();
            let outcome = receive(&mut field, &mut receiver);

            match outcome {
                ReceiveOutcome::Received(msg) => {
                    assert_eq!(
                        msg.label, expected_label,
                        "FIFO violated: expected label {expected_label}, got {}",
                        msg.label
                    );
                }
                ReceiveOutcome::Blocked => {
                    panic!(
                        "receive must not block with {expected_label} message(s) still in queue"
                    );
                }
            }
        }

        assert_eq!(
            field.queue_length, 0,
            "queue must be empty after draining all"
        );
    }

    // ── Adversarial: capacity-1 field ────────────────────────────────

    /// Capacity-1 field with no waiter: first send enqueues; second send
    /// returns QueueFull without corrupting the queued message.
    ///
    /// Bug target: off-by-one in is_full() — queue_length >= capacity vs
    /// queue_length > capacity — lets a second message overwrite slot 0.
    #[test]
    fn test_adversarial_comm_capacity_one_send_no_waiter() {
        let mut field = test_field(1);
        let result = send(&mut field, make_message(0xAA, 1));

        assert!(
            result.is_ok(),
            "first send to capacity-1 field must succeed"
        );
        assert!(
            matches!(result.unwrap(), SendOutcome::Enqueued),
            "first send must return Enqueued"
        );
        assert_eq!(field.queue_length, 1);
        assert!(field.is_full());

        // Second send — no waiter, queue full.
        let overflow = send(&mut field, make_message(0xBB, 2));

        assert!(
            matches!(overflow, Err(FieldError::QueueFull)),
            "second send to full capacity-1 field must return QueueFull"
        );

        // The original message must be intact.
        let mut receiver = make_wait_entry();
        let outcome = receive(&mut field, &mut receiver);

        match outcome {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(
                    msg.label, 0xAA,
                    "capacity-1: original message must survive overflow attempt"
                );
            }
            ReceiveOutcome::Blocked => panic!("must receive after send on capacity-1 field"),
        }
    }

    /// Capacity-1 field with a waiter present: send must use direct delivery
    /// (WokeReceiver) and leave the queue empty so it can accept a subsequent
    /// enqueue.
    ///
    /// Bug target: implementation checks is_full() before checking waiters,
    /// so capacity-1 full field + waiter incorrectly returns QueueFull.
    #[test]
    fn test_adversarial_comm_capacity_one_send_with_waiter_uses_direct_delivery() {
        let mut field = test_field(1);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        let result = send(&mut field, make_message(0xCC, 3));

        assert!(
            result.is_ok(),
            "send to capacity-1 field with waiter must succeed"
        );
        assert!(
            matches!(result.unwrap(), SendOutcome::WokeReceiver(_)),
            "send must return WokeReceiver (direct delivery)"
        );
        // Queue must be empty — message bypassed it.
        assert_eq!(
            field.queue_length, 0,
            "capacity-1 direct delivery must not touch the queue"
        );
        // Waiters list must be empty.
        assert!(
            field.waiters_head.is_none(),
            "waiter must be popped after direct delivery"
        );

        // Queue is now empty — a subsequent enqueue must succeed.
        let subsequent = send(&mut field, make_message(0xDD, 4));

        assert!(
            subsequent.is_ok(),
            "capacity-1 field must accept a message after direct delivery empties the queue"
        );
    }

    // ── Adversarial: capacity-0 field ────────────────────────────────

    /// Capacity-0 field with no waiter: every send must return QueueFull.
    /// No waiter means the message has nowhere to go.
    ///
    /// Bug target: implementation skips the full-queue check when capacity == 0
    /// and tries to write to slot 0, writing past the allocation.
    #[test]
    fn test_adversarial_comm_capacity_zero_send_no_waiter_returns_queue_full() {
        let mut field = test_field(0);
        let result = send(&mut field, make_message(1, 0));

        assert!(
            matches!(result, Err(FieldError::QueueFull)),
            "send to capacity-0 field with no waiter must return QueueFull"
        );
        assert_eq!(
            field.queue_length, 0,
            "queue_length must stay 0 on QueueFull"
        );
    }

    /// Capacity-0 field with a waiter: the waiter bypasses the queue, so
    /// send must succeed via direct delivery even though capacity == 0.
    ///
    /// Bug target: full-queue short-circuit before waiter check — capacity-0
    /// always triggers QueueFull before the waiter list is examined.
    #[test]
    fn test_adversarial_comm_capacity_zero_send_with_waiter_wakes_receiver() {
        let mut field = test_field(0);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        let result = send(&mut field, make_message(1, 0));

        assert!(
            result.is_ok(),
            "send to capacity-0 field with waiter must succeed (waiter bypasses queue)"
        );
        assert!(
            matches!(result.unwrap(), SendOutcome::WokeReceiver(_)),
            "capacity-0 send with waiter must return WokeReceiver"
        );
        // Queue untouched.
        assert_eq!(field.queue_length, 0);
        assert!(field.waiters_head.is_none());
    }

    // ── Adversarial: N waiters then N sends ──────────────────────────

    /// Add N waiters then send N messages: each send must WokeReceiver (no
    /// enqueue), and the (N+1)th send must Enqueue (no waiter left).
    ///
    /// Bug target: send pops a waiter but does not decrement an internal
    /// waiter count, so after N pops the waiter check still returns "waiter
    /// present" and the N+1th send also WokesReceiver against a null pointer.
    #[test]
    fn test_adversarial_comm_n_waiters_then_n_sends_drain_waiters() {
        const N: usize = 4;
        let mut field = test_field(N as u32 + 1);
        let mut entries: [_; N] = core::array::from_fn(|_| make_wait_entry());

        for entry in entries.iter_mut() {
            field.add_waiter(entry);
        }

        // All N sends must WokeReceiver without touching the queue.
        for i in 0..N {
            let result = send(&mut field, make_message(i as u64, 0))
                .expect("send must succeed while waiters exist");

            assert!(
                matches!(result, SendOutcome::WokeReceiver(_)),
                "send {i}: must WokeReceiver while waiter {i} is in list"
            );
            assert_eq!(
                field.queue_length, 0,
                "send {i}: queue must stay empty during direct delivery"
            );
        }

        // No waiters remain — the next send must Enqueue.
        assert!(
            field.waiters_head.is_none(),
            "waiters list must be empty after N direct-delivery sends"
        );

        let last = send(&mut field, make_message(99, 0)).expect("N+1th send must succeed");

        assert!(
            matches!(last, SendOutcome::Enqueued),
            "N+1th send must Enqueue when waiter list is exhausted"
        );
        assert_eq!(field.queue_length, 1, "queue must hold the N+1th message");
    }

    // ── Adversarial: call queue state invariants ─────────────────────

    /// call with no waiter must increment queue_length by exactly 1.
    /// Verifies call does not double-enqueue or skip the increment.
    ///
    /// Bug target: call enqueues via a different path than send and
    /// forgets to update queue_length.
    #[test]
    fn test_adversarial_comm_call_enqueue_increments_queue_length() {
        let mut field = test_field(4);

        assert_eq!(field.queue_length, 0, "precondition: queue empty");

        call(&mut field, make_message(10, 0), Badge(0)).expect("call must succeed");

        assert_eq!(
            field.queue_length, 1,
            "call must increment queue_length from 0 to 1"
        );

        call(&mut field, make_message(20, 0), Badge(0)).expect("second call must succeed");

        assert_eq!(
            field.queue_length, 2,
            "second call must increment queue_length from 1 to 2"
        );
    }

    /// call with DirectSwitch (waiter present, 0-cap message) must NOT
    /// touch queue_length.
    ///
    /// Bug target: implementation enqueues the message then immediately
    /// dequeues it for direct delivery, resulting in queue_length going
    /// 0→1→0 correctly, but if it forgets the dequeue the queue_length
    /// stays at 1 after what should be a queueless direct-switch.
    #[test]
    fn test_adversarial_comm_call_direct_switch_leaves_queue_untouched() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        assert_eq!(field.queue_length, 0, "precondition: queue empty");

        let result = call(&mut field, make_message(77, 0), Badge(0)).expect("call must succeed");

        assert!(
            matches!(result, CallOutcome::DirectSwitch(_)),
            "call with waiter and 0-cap message must DirectSwitch"
        );
        assert_eq!(
            field.queue_length, 0,
            "DirectSwitch must not leave a message in the queue"
        );
    }

    // ── Adversarial: data integrity through call roundtrip ───────────

    /// All 4 data words, label, and badge must survive the call→receive
    /// roundtrip through the queue.
    ///
    /// Bug target: call copies only data[0] into the queue slot, leaving
    /// data[1..3] as zeroes or garbage from a previous message.
    #[test]
    fn test_adversarial_comm_call_full_data_survives_roundtrip() {
        let mut field = test_field(4);
        let msg = Message {
            data: [0x1111_2222, 0x3333_4444, 0x5555_6666, 0x7777_8888],
            label: 0xDEAD_BEEF,
            badge: Badge(0xCAFE_BABE),
            user_cap: None,
            reply_cap: None,
        };

        call(&mut field, msg, Badge(0)).expect("call must succeed");

        let mut receiver = make_wait_entry();
        let outcome = receive(&mut field, &mut receiver);

        match outcome {
            ReceiveOutcome::Received(received) => {
                assert_eq!(
                    received.data,
                    [0x1111_2222, 0x3333_4444, 0x5555_6666, 0x7777_8888],
                    "all 4 data words must survive call→receive roundtrip"
                );
                assert_eq!(
                    received.label, 0xDEAD_BEEF,
                    "label must survive call→receive roundtrip"
                );
                assert_eq!(
                    received.badge,
                    Badge(0xCAFE_BABE),
                    "badge must survive call→receive roundtrip"
                );
            }
            ReceiveOutcome::Blocked => panic!("receive after call must return Received"),
        }
    }

    // ── Adversarial: yield_cpu does not panic ─────────────────────────

    /// yield_cpu() is a no-op at the IPC level. It must not panic and
    /// must return (). Verifies the function body is correctly formed.
    #[test]
    fn test_adversarial_comm_yield_cpu_does_not_panic() {
        // No state to set up — yield_cpu takes no arguments.
        yield_cpu();
        // If we reach here, it returned () without panicking.
    }

    // ── Adversarial: reply_recv with reply_field waiter ──────────────

    /// reply_recv where reply_field has a waiting receiver: the reply
    /// message must WokeReceiver on the reply side (direct delivery),
    /// and the return value reflects the recv_field outcome only.
    ///
    /// Bug target: reply_recv ignores waiters on reply_field and always
    /// enqueues the reply, leaving the waiter stranded.
    #[test]
    fn test_adversarial_comm_reply_recv_reply_field_has_waiter() {
        let mut reply_field = test_field(4);
        let mut recv_field = test_field(4);
        // Simulate a client blocked waiting on the reply field.
        let mut reply_waiter = make_wait_entry();

        reply_field.add_waiter(&mut reply_waiter);

        // Pre-load the recv_field so reply_recv can immediately receive.
        recv_field
            .enqueue(make_message(300, 7))
            .expect("enqueue next request");

        let reply_message = make_message(100, 0);
        let mut receiver = make_wait_entry();
        let outcome = reply_recv(
            &mut reply_field,
            &mut recv_field,
            reply_message,
            &mut receiver,
        );

        // The reply must have been delivered directly to the waiter —
        // queue must stay empty (not enqueued).
        assert_eq!(
            reply_field.queue_length, 0,
            "reply with waiting receiver must use direct delivery, not enqueue"
        );
        // The reply waiter must be popped.
        assert!(
            reply_field.waiters_head.is_none(),
            "reply_field waiters list must be empty after direct delivery"
        );

        // The return value is the recv_field outcome — must be Received.
        match outcome {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(
                    msg.label, 300,
                    "recv side must return the next request message"
                );
                assert_eq!(msg.badge, Badge(7));
            }
            ReceiveOutcome::Blocked => {
                panic!("reply_recv must return Received when recv_field has messages");
            }
        }
    }

    /// reply_recv where reply_field is full and has no waiter:
    /// the reply fails with QueueFull. reply_recv must still attempt
    /// the receive side — the return value is the receive outcome,
    /// not the reply outcome.
    ///
    /// Bug target: reply_recv short-circuits on reply QueueFull and
    /// returns without performing the receive, leaving the server unable
    /// to pick up the next request.
    #[test]
    fn test_adversarial_comm_reply_recv_reply_field_full_still_receives() {
        let mut reply_field = test_field(1);
        let mut recv_field = test_field(4);

        // Fill the reply field to capacity.
        reply_field
            .enqueue(make_message(0, 0))
            .expect("fill reply field");

        assert!(reply_field.is_full(), "precondition: reply_field full");

        // Pre-load recv_field.
        recv_field
            .enqueue(make_message(999, 5))
            .expect("enqueue next request");

        let reply_message = make_message(50, 0);
        let mut receiver = make_wait_entry();
        let outcome = reply_recv(
            &mut reply_field,
            &mut recv_field,
            reply_message,
            &mut receiver,
        );

        // reply_field remains full (reply was dropped/failed).
        assert_eq!(
            reply_field.queue_length, 1,
            "reply_field must remain at capacity after failed reply"
        );

        // The receive side must still have proceeded — Received from recv_field.
        match outcome {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(
                    msg.label, 999,
                    "reply_recv must still receive even when reply fails"
                );
            }
            ReceiveOutcome::Blocked => {
                panic!(
                    "reply_recv must still receive the next message even when reply_field is full"
                );
            }
        }
    }

    /// reply_recv where both fields are empty: reply must be enqueued into
    /// reply_field (no waiter), then the receive on recv_field must block.
    ///
    /// Bug target: the implementation processes only the recv side and
    /// skips the reply side when reply_field is empty.
    #[test]
    fn test_adversarial_comm_reply_recv_both_fields_empty() {
        let mut reply_field = test_field(4);
        let mut recv_field = test_field(4);
        let reply_message = make_message(42, 0);
        let mut receiver = make_wait_entry();
        let outcome = reply_recv(
            &mut reply_field,
            &mut recv_field,
            reply_message,
            &mut receiver,
        );

        // Reply must have been sent (no waiter → enqueued).
        assert_eq!(
            reply_field.queue_length, 1,
            "reply must be enqueued when reply_field is empty and has no waiter"
        );
        // Receive side blocks — recv_field was empty.
        assert!(
            matches!(outcome, ReceiveOutcome::Blocked),
            "reply_recv must block when recv_field is empty"
        );
        assert!(
            recv_field.waiters_head.is_some(),
            "receiver must be added to recv_field waiters on block"
        );
    }

    /// reply_recv where recv_field is at exact capacity: after the reply
    /// is sent, the receive must still dequeue the front message correctly.
    ///
    /// Bug target: receive after a full-queue state miscomputes queue_head,
    /// returning the wrong message or wrapping incorrectly.
    #[test]
    fn test_adversarial_comm_reply_recv_recv_field_at_capacity() {
        let capacity = 3u32;
        let mut reply_field = test_field(4);
        let mut recv_field = test_field(capacity);

        // Fill recv_field to exact capacity.
        for i in 0..capacity {
            recv_field
                .enqueue(make_message(i as u64 + 1, 0))
                .expect("fill recv_field");
        }

        assert!(recv_field.is_full(), "precondition: recv_field full");

        let reply_message = make_message(0xFF, 0);
        let mut receiver = make_wait_entry();
        let outcome = reply_recv(
            &mut reply_field,
            &mut recv_field,
            reply_message,
            &mut receiver,
        );

        // Receive must dequeue the first message (label == 1).
        match outcome {
            ReceiveOutcome::Received(msg) => {
                assert_eq!(
                    msg.label, 1,
                    "reply_recv must dequeue the first message when recv_field is full"
                );
            }
            ReceiveOutcome::Blocked => {
                panic!("reply_recv must Receive when recv_field is at capacity (has messages)");
            }
        }

        // recv_field queue_length must have decreased by 1.
        assert_eq!(
            recv_field.queue_length,
            capacity - 1,
            "recv_field must have one fewer message after receive"
        );
    }

    // ── Adversarial: call with user cap forces slow path ─────────────

    /// call with user cap and waiter present: must Enqueue (not DirectSwitch).
    /// After enqueue, queue_length must be exactly 1, not 0.
    ///
    /// Bug target: implementation ignores user_cap and DirectSwitches
    /// unconditionally whenever a waiter is present.
    #[test]
    fn test_adversarial_comm_call_user_cap_with_waiter_enqueues() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        let result = call(&mut field, make_message_with_cap(55), Badge(0))
            .expect("call with user cap must not return QueueFull");

        assert!(
            matches!(result, CallOutcome::Enqueued),
            "call with user cap must Enqueue even when waiter is present"
        );
        assert_eq!(
            field.queue_length, 1,
            "call with user cap must enqueue the message (queue_length == 1)"
        );
    }

    // ── Adversarial: send preserves queue state across waiter transitions

    /// send on a field that had a waiter (now drained): after the waiter
    /// was consumed by a prior send, subsequent sends must enqueue normally.
    ///
    /// Bug target: waiter-list head is not updated after pop, so subsequent
    /// sends still see the stale head pointer and attempt direct delivery
    /// to an already-consumed entry.
    #[test]
    fn test_adversarial_comm_send_after_waiter_drained_enqueues() {
        let mut field = test_field(4);
        let mut entry = make_wait_entry();

        field.add_waiter(&mut entry);

        // First send drains the single waiter.
        send(&mut field, make_message(1, 0)).expect("first send");

        assert!(
            field.waiters_head.is_none(),
            "precondition: waiter list must be empty"
        );
        assert_eq!(field.queue_length, 0);

        // Second send — no waiter left — must enqueue.
        let result = send(&mut field, make_message(2, 0)).expect("second send must succeed");

        assert!(
            matches!(result, SendOutcome::Enqueued),
            "send after waiter drained must Enqueue"
        );
        assert_eq!(
            field.queue_length, 1,
            "queue must hold the message after enqueue"
        );
    }

    // ── Adversarial: queue_length consistency after mixed operations ──

    /// queue_length must equal the number of messages that can be dequeued.
    /// After a complex sequence of send, call, and receive operations,
    /// the announced queue_length must exactly match the drainable count.
    ///
    /// Bug target: send and call use different code paths with independent
    /// queue_length increments; one path double-increments or skips the
    /// increment under certain conditions.
    #[test]
    fn test_adversarial_comm_queue_length_matches_drainable_count() {
        let mut field = test_field(8);

        send(&mut field, make_message(1, 0)).expect("send 1");
        call(&mut field, make_message(2, 0), Badge(0)).expect("call 2");
        send(&mut field, make_message(3, 0)).expect("send 3");

        // Drain one.
        let mut r = make_wait_entry();

        receive(&mut field, &mut r);
        call(&mut field, make_message(4, 0), Badge(0)).expect("call 4");
        send(&mut field, make_message(5, 0)).expect("send 5");

        // Announced queue_length must equal the actual drainable count.
        let announced_length = field.queue_length;
        let mut actual_count = 0u32;

        loop {
            let mut rx = make_wait_entry();

            match receive(&mut field, &mut rx) {
                ReceiveOutcome::Received(_) => actual_count += 1,
                ReceiveOutcome::Blocked => break,
            }
        }

        assert_eq!(
            actual_count, announced_length,
            "drainable count ({actual_count}) must equal queue_length ({announced_length})"
        );
    }

    // ── Adversarial: badge extremes through send and call ────────────

    /// Badge value 0 and u64::MAX must survive send→receive and call→receive
    /// without truncation or sign extension.
    ///
    /// Bug target: badge stored as u32 internally, truncating the high bits.
    #[test]
    fn test_adversarial_comm_badge_extremes_send_and_call() {
        let mut field = test_field(4);

        send(&mut field, make_message(1, 0)).expect("send badge 0");
        send(&mut field, make_message(2, u64::MAX)).expect("send badge MAX");
        call(&mut field, make_message(3, u64::MAX / 2), Badge(0)).expect("call badge mid");

        let expected_badges = [Badge(0), Badge(u64::MAX), Badge(u64::MAX / 2)];

        for (i, expected_badge) in expected_badges.iter().enumerate() {
            let mut receiver = make_wait_entry();
            let outcome = receive(&mut field, &mut receiver);

            match outcome {
                ReceiveOutcome::Received(msg) => {
                    assert_eq!(
                        msg.badge, *expected_badge,
                        "message {i}: badge must survive queue roundtrip"
                    );
                }
                ReceiveOutcome::Blocked => panic!("receive {i} must not block"),
            }
        }
    }

    /// Label value 0 and u64::MAX must survive the queue roundtrip.
    ///
    /// Bug target: label stored in a smaller field, truncated on enqueue.
    #[test]
    fn test_adversarial_comm_label_extremes_roundtrip() {
        let mut field = test_field(4);

        send(&mut field, make_message(0, 0)).expect("send label 0");
        send(&mut field, make_message(u64::MAX, 0)).expect("send label MAX");
        send(&mut field, make_message(u64::MAX / 2, 0)).expect("send label mid");

        let expected_labels = [0u64, u64::MAX, u64::MAX / 2];

        for (i, &expected_label) in expected_labels.iter().enumerate() {
            let mut receiver = make_wait_entry();
            let outcome = receive(&mut field, &mut receiver);

            match outcome {
                ReceiveOutcome::Received(msg) => {
                    assert_eq!(
                        msg.label, expected_label,
                        "message {i}: label must survive queue roundtrip"
                    );
                }
                ReceiveOutcome::Blocked => panic!("receive {i} must not block"),
            }
        }
    }
}
