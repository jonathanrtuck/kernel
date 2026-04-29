//! Loom model of the IPC send/receive rendezvous protocol (communication.rs).
//!
//! This is an ABSTRACT model — it does not import or test the actual
//! communication::send/receive functions (which are no_std). Instead it
//! replicates the algorithm using loom::sync primitives so Loom can
//! exhaustively explore all thread interleavings.
//!
//! Protocol modeled (from communication.rs, send/receive):
//!   send: pop_waiter() → if Some: WokeReceiver; else: enqueue → Enqueued
//!   receive: dequeue() → if Some: Received; else: add_waiter → Blocked
//!
//! The mutual exclusion that prevents lost wakeups in the kernel (via the
//! Field lock held across the entire send/receive decision) is replicated
//! here by holding the queue Mutex across the check-and-set of the waiter
//! flag. This captures the same protocol invariant.
//!
//! Tests:
//!   loom_ipc_send_then_receive      — sender enqueues, receiver dequeues
//!   loom_ipc_receive_then_send      — receiver blocks, sender wakes (direct delivery)
//!   loom_ipc_two_senders_one_receiver — no message lost under concurrent sends

#[cfg(test)]
mod tests {
    extern crate alloc;

    use alloc::collections::VecDeque;
    use loom::sync::Arc;
    use loom::sync::Mutex;
    use loom::sync::atomic::{AtomicBool, Ordering};
    use loom::thread;

    // ── ModelField ────────────────────────────────────────────────────

    /// Abstract model of the kernel's Field object.
    ///
    /// The Field lock in the kernel ensures that the check-and-set of
    /// the waiter flag and the queue enqueue/dequeue are atomic with
    /// respect to each other. This is modeled by holding the queue
    /// Mutex across the full send/receive decision.
    struct ModelField {
        /// Models the Field's message queue. The Mutex provides the
        /// same mutual exclusion as the kernel's Field spinlock.
        queue: Mutex<VecDeque<u64>>,
        /// Whether a receiver is currently waiting (has set the waiter
        /// flag and is spinning on waiter_woken). Accessed under queue
        /// lock to prevent the lost-wakeup race.
        waiter: AtomicBool,
        /// Sender sets this to true (Release) to wake a blocked receiver.
        /// Receiver spins on this with yield_now().
        waiter_woken: AtomicBool,
        /// Direct delivery channel: sender stores the value here when
        /// it finds a waiting receiver (the WokeReceiver path).
        direct_delivery: Mutex<Option<u64>>,
    }

    impl ModelField {
        fn new() -> Self {
            ModelField {
                queue: Mutex::new(VecDeque::new()),
                waiter: AtomicBool::new(false),
                waiter_woken: AtomicBool::new(false),
                direct_delivery: Mutex::new(None),
            }
        }
    }

    /// Model of communication::send.
    ///
    /// Holds the queue lock across the check-and-set so the decision
    /// "is there a waiter?" is atomic with respect to model_receive's
    /// decision "should I set the waiter flag?". This prevents the
    /// lost-wakeup race and mirrors the kernel's Field lock discipline.
    fn model_send(field: &ModelField, value: u64) {
        let mut queue = field.queue.lock().expect("queue lock");

        if field.waiter.load(Ordering::Acquire) {
            // Pop the waiter atomically (under the lock) and deliver directly.
            field.waiter.store(false, Ordering::Relaxed);
            let mut delivery = field.direct_delivery.lock().expect("delivery lock");
            *delivery = Some(value);
            drop(delivery);
            drop(queue);
            // Signal the blocked receiver.
            field.waiter_woken.store(true, Ordering::Release);
        } else {
            // No waiter — enqueue for later dequeue by a receiver.
            queue.push_back(value);
        }
    }

    /// Model of communication::receive.
    ///
    /// Holds the queue lock across the check-and-set so the decision
    /// "is there a queued message?" is atomic with respect to model_send's
    /// decision "is there a waiter?". Returns the received value.
    fn model_receive(field: &ModelField) -> u64 {
        {
            let mut queue = field.queue.lock().expect("queue lock");

            if let Some(value) = queue.pop_front() {
                return value;
            }

            // Queue empty — register as a waiter while still holding the lock.
            // This must happen under the lock so model_send cannot miss us:
            // if send reads waiter=false before we set it, it will enqueue
            // and we will find the message on our next check. If send reads
            // waiter=true, it delivers directly and signals waiter_woken.
            field.waiter.store(true, Ordering::Release);
        }

        // Lock released. Spin until the sender wakes us.
        loop {
            if field.waiter_woken.load(Ordering::Acquire) {
                break;
            }

            thread::yield_now();
        }

        // Read the directly-delivered value.
        let mut delivery = field.direct_delivery.lock().expect("delivery lock");
        delivery
            .take()
            .expect("direct delivery must be set by sender")
    }

    // ── Tests ─────────────────────────────────────────────────────────

    /// Sender-before-receiver: sender enqueues, receiver dequeues.
    ///
    /// Loom explores all interleavings including the one where the sender
    /// completes entirely before the receiver starts. The receiver must
    /// find the message in the queue and return it without blocking.
    #[test]
    fn loom_ipc_send_then_receive() {
        loom::model(|| {
            let field = Arc::new(ModelField::new());

            let field_sender = field.clone();
            let sender = thread::spawn(move || {
                model_send(&field_sender, 42);
            });

            let field_receiver = field.clone();
            let receiver = thread::spawn(move || model_receive(&field_receiver));

            sender.join().expect("sender");
            let value = receiver.join().expect("receiver");

            assert_eq!(
                value, 42,
                "send_then_receive: message value must be preserved"
            );
        });
    }

    /// Receiver-before-sender: receiver blocks, sender wakes (direct delivery).
    ///
    /// This is the interesting interleaving. Loom explores the case where
    /// the receiver sets waiter=true before the sender checks it, causing
    /// direct delivery via the waiter_woken signal path.
    #[test]
    fn loom_ipc_receive_then_send() {
        loom::model(|| {
            let field = Arc::new(ModelField::new());

            let field_receiver = field.clone();
            let receiver = thread::spawn(move || model_receive(&field_receiver));

            let field_sender = field.clone();
            let sender = thread::spawn(move || {
                model_send(&field_sender, 99);
            });

            let value = receiver.join().expect("receiver");
            sender.join().expect("sender");

            assert_eq!(
                value, 99,
                "receive_then_send: message value must be preserved"
            );
        });
    }

    /// Two senders, one receiver: no message is lost under concurrent sends.
    ///
    /// Two senders each send a distinct value. The receiver makes two
    /// receive calls and collects both values. All sent values must appear
    /// exactly once in the received set, regardless of delivery order.
    ///
    /// This verifies the protocol handles concurrent senders without dropping
    /// or duplicating messages under any Loom-explored interleaving.
    #[test]
    fn loom_ipc_two_senders_one_receiver() {
        loom::model(|| {
            let field = Arc::new(ModelField::new());

            let field_a = field.clone();
            let sender_a = thread::spawn(move || {
                model_send(&field_a, 10);
            });

            let field_b = field.clone();
            let sender_b = thread::spawn(move || {
                model_send(&field_b, 20);
            });

            sender_a.join().expect("sender a");
            sender_b.join().expect("sender b");

            // Both senders have finished: both values are in the queue.
            // Receive both messages (no blocking needed — both enqueued).
            let first = model_receive(&field);
            let second = model_receive(&field);

            // Order may vary — assert both values arrived exactly once.
            let mut received = [first, second];

            received.sort_unstable();

            assert_eq!(
                received,
                [10, 20],
                "two_senders_one_receiver: both messages must arrive exactly once"
            );
        });
    }
}
