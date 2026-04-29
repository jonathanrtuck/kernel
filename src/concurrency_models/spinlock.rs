//! Loom model of the spinlock mutual exclusion protocol (frame/lock.rs).
//!
//! This is an ABSTRACT model — it does not import or test the actual Lock<T>
//! type from frame/lock.rs (which is no_std). Instead it replicates the
//! protocol using loom::sync primitives so Loom can exhaustively explore
//! all thread interleavings.
//!
//! Protocol modeled (from frame/lock.rs, lines 138-147):
//!   acquire: compare_exchange_weak(false, true, Acquire, Relaxed)
//!   release: store(false, Release)
//!   data access: only while the lock is held (via RAII guard)
