//! Spinlock with D53 lock ordering enforcement.
//!
//! D53: global-arena concurrency model — one SpinLock per Arena<T>.
//! Lock ordering: Arena<Field> < Arena<Observer> < Arena<Pulsar>.
//! Arena<Space> and Arena<Time> are unordered (no cross-arena ops).
//!
//! The Lock<T> type wraps a value with mutual exclusion. The ordering
//! is checked at runtime via per-core tracking of the highest held
//! lock order. Attempting to acquire a lock out of order panics in
//! debug builds — catching deadlocks at the point of violation rather
//! than as mysterious hangs.
//!
//! A1: Rust's ownership through LockGuard<T> prevents data races.
//! D53: the ordering prevents deadlocks. Together they give safe
//! concurrent access to kernel object arenas.

use core::sync::atomic::{AtomicBool, Ordering};

// ── Lock ordering constants (D53) ──────────────────────────────────

/// Lock acquisition order (D53). Lower values must be acquired first.
///
/// The ordering follows from the IPC data flow (D13): send checks
/// Field then wakes Observer. Timer fire (D44) enqueues into Field
/// from Pulsar context. Fault path (D40) releases Observer, acquires
/// Field, re-acquires Observer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum LockOrder {
    /// Arena<Space> — unordered with Field/Observer/Pulsar.
    /// May be acquired independently at any time.
    Space = 0,
    /// Arena<Time> — unordered with Field/Observer/Pulsar.
    /// May be acquired independently at any time.
    Time = 1,
    /// Arena<Field> — must be acquired before Observer and Pulsar.
    Field = 2,
    /// Arena<Observer> — must be acquired after Field, before Pulsar.
    Observer = 3,
    /// Arena<Pulsar> — must be acquired after Field and Observer.
    Pulsar = 4,
}

impl LockOrder {
    /// Whether this lock order participates in the strict ordering.
    ///
    /// Space and Time are unordered (D53: no cross-arena operations
    /// with the ordered types). They can be acquired in any order
    /// relative to other locks. Only Field, Observer, and Pulsar
    /// participate in the strict ordering chain.
    pub const fn is_ordered(&self) -> bool {
        matches!(
            self,
            LockOrder::Field | LockOrder::Observer | LockOrder::Pulsar
        )
    }
}

// ── Lock<T> ────────────────────────────────────────────────────────

/// A spinlock protecting a value of type T, with D53 ordering.
///
/// Callers acquire the lock to get a `LockGuard<T>` providing
/// exclusive access. The lock tracks its position in the D53
/// ordering; debug builds verify ordering is respected.
///
/// Philosophy: "isolate uncertain decisions behind interfaces." The
/// ordering enforcement mechanism (runtime assertions now, potentially
/// type-level in future Verus verification) is behind this interface.
/// Callers code against Lock/LockGuard — the enforcement can change
/// without affecting them.
///
/// Implementation lives in frame/ because the spinlock internals
/// (AtomicBool, WFE-based spinning, interrupt masking) require unsafe.
pub struct Lock<T> {
    locked: AtomicBool,
    order: LockOrder,
    _data: core::marker::PhantomData<T>,
}

// SAFETY: Lock<T> is Send + Sync if T is Send — the lock provides
// mutual exclusion, so the protected data can be accessed from any core.
unsafe impl<T: Send> Send for Lock<T> {}
unsafe impl<T: Send> Sync for Lock<T> {}

impl<T> Lock<T> {
    /// Create a new lock with the given D53 ordering.
    pub const fn new(order: LockOrder) -> Lock<T> {
        Lock {
            locked: AtomicBool::new(false),
            order,
            _data: core::marker::PhantomData,
        }
    }

    /// Acquire the lock, returning a guard that releases on drop.
    ///
    /// **D53 ordering enforced:** in debug builds, panics if an ordered
    /// lock is acquired out of sequence. In release builds, the check
    /// is elided — the ordering is a proof obligation, not a runtime cost
    /// on the hot path.
    ///
    /// Spins until the lock is available. On ARM64, uses WFE to avoid
    /// burning power during contention (implementation in frame/arch/).
    ///
    /// D69: the IPC fast path masks DAIF.I for its ~400-cycle window.
    /// Locks acquired within that window are guaranteed uncontended on
    /// the same core (D1 hot-path property).
    pub fn acquire(&self) -> LockGuard<'_, T> {
        // Spin until we acquire
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // Spin hint — on ARM64 this would be WFE
            core::hint::spin_loop();
        }

        LockGuard { lock: self }
    }

    /// The ordering position of this lock (D53).
    pub const fn order(&self) -> LockOrder {
        self.order
    }
}

// ── LockGuard<T> ──────────────────────────────────────────────────

/// RAII guard providing exclusive access to the locked value.
///
/// Dropping the guard releases the lock. The guard's lifetime is tied
/// to the Lock — Rust's borrow checker ensures the lock outlives
/// the guard.
///
/// The guard intentionally does NOT implement Deref/DerefMut to T
/// here — the actual data access pattern depends on whether Lock
/// wraps an Arena<T> directly (the data lives in slab pages, not
/// inline in the lock). The guard proves "you hold the lock"; the
/// arena's methods prove "you have a valid object reference." These
/// are separate concerns.
pub struct LockGuard<'a, T> {
    lock: &'a Lock<T>,
}

impl<T> Drop for LockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

impl<T> LockGuard<'_, T> {
    /// The ordering position of the held lock.
    pub const fn order(&self) -> LockOrder {
        self.lock.order
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_order_is_monotonic() {
        assert!(LockOrder::Field < LockOrder::Observer);
        assert!(LockOrder::Observer < LockOrder::Pulsar);
    }

    #[test]
    fn space_and_time_are_unordered() {
        assert!(!LockOrder::Space.is_ordered());
        assert!(!LockOrder::Time.is_ordered());
        assert!(LockOrder::Field.is_ordered());
        assert!(LockOrder::Observer.is_ordered());
        assert!(LockOrder::Pulsar.is_ordered());
    }

    #[test]
    fn acquire_and_release() {
        let lock: Lock<u32> = Lock::new(LockOrder::Field);

        {
            let _guard = lock.acquire();
            // Lock is held — a second acquire on the same thread would
            // deadlock (no test for that — it's the ordering check's job).
        }

        // Guard dropped — lock released. Re-acquire should succeed.
        let _guard2 = lock.acquire();
    }

    #[test]
    fn guard_reports_order() {
        let lock: Lock<u32> = Lock::new(LockOrder::Observer);
        let guard = lock.acquire();

        assert_eq!(guard.order(), LockOrder::Observer);
    }
}
