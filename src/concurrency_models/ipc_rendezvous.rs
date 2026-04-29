//! Loom model of the IPC send/receive rendezvous protocol (communication.rs).
//!
//! This is an ABSTRACT model — it does not import or test the actual
//! communication::send/receive functions (which are no_std). Instead it
//! replicates the algorithm using loom::sync primitives so Loom can
//! exhaustively explore all thread interleavings.
//!
//! Protocol modeled (from communication.rs, send/receive):
//!   send: check for waiter → deliver directly (WokeReceiver) or enqueue
//!   receive: check queue → dequeue or add_waiter (Blocked)
