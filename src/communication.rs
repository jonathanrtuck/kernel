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
pub fn send(_field: &mut Field, _message: Message) -> Result<SendOutcome, FieldError> {
    todo!()
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
pub fn receive(_field: &mut Field, _receiver: &mut WaitEntry) -> ReceiveOutcome {
    todo!()
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
    _field: &mut Field,
    _message: Message,
    _reply_badge: Badge,
) -> Result<CallOutcome, FieldError> {
    todo!()
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
    _reply_field: &mut Field,
    _recv_field: &mut Field,
    _reply_message: Message,
    _receiver: &mut WaitEntry,
) -> ReceiveOutcome {
    todo!()
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
