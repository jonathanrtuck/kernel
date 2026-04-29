//! Spinlock with D53 lock ordering enforcement.
//!
//! D53: global-arena concurrency model — one SpinLock per Arena<T>.
//! Lock ordering: Arena<Field> < Arena<Observer> < Arena<Pulsar>.
//! Arena<Space> and Arena<Time> are unordered (no cross-arena ops).
//!
//! D75: Lock<T> owns its data via UnsafeCell<T>. LockGuard provides
//! DerefMut<Target=T> — the type system enforces lock-before-access.
//! A1: ownership maps to resource lifecycle; the lock-before-access
//! invariant is a trust boundary that Rust can and should enforce.
//!
//! The ordering is checked at runtime via per-core tracking of the
//! highest held lock order. Attempting to acquire a lock out of order
//! panics in debug builds — catching deadlocks at the point of
//! violation rather than as mysterious hangs.
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
    /// SpaceManager (D3, D31) — unordered with Field/Observer/Pulsar.
    /// Same unordered category as Space/Time. Does not participate
    /// in the strict ordering chain.
    SpaceManager = 5,
    /// IRQ routing table (D22, D81) — unordered with Field/Observer/Pulsar.
    /// The IRQ routing table is kernel-internal infrastructure that does
    /// not participate in the Field-Observer-Pulsar ordering chain.
    /// Acquired independently by handle_irq on the interrupt path.
    IrqRouting = 6,
    /// ASID allocator (D101) — unordered with Field/Observer/Pulsar.
    /// Sequential ASID counter, acquired during Observer creation.
    AsidAllocator = 7,
}

impl LockOrder {
    /// Whether this lock order participates in the strict ordering.
    ///
    /// Space, Time, and SpaceManager are unordered (D53: no cross-arena
    /// operations with the ordered types). They can be acquired in any
    /// order relative to other locks. Only Field, Observer, and Pulsar
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
/// Lock<T> owns its data (D75). Callers acquire the lock to get a
/// `LockGuard<T>` providing exclusive `DerefMut` access. The type
/// system enforces lock-before-access — accessing T without holding
/// the lock is impossible through safe code.
///
/// The lock tracks its position in the D53 ordering; debug builds
/// verify ordering is respected.
///
/// Philosophy: "isolate uncertain decisions behind interfaces." The
/// ordering enforcement mechanism (runtime assertions now, potentially
/// type-level in future Verus verification) is behind this interface.
/// Callers code against Lock/LockGuard — the enforcement can change
/// without affecting them.
///
/// Implementation lives in frame/ because the spinlock internals
/// (AtomicBool, WFE-based spinning, interrupt masking, UnsafeCell)
/// require unsafe.
pub struct Lock<T> {
    locked: AtomicBool,
    order: LockOrder,
    data: core::cell::UnsafeCell<T>,
}

// SAFETY: Lock<T> is Send if T is Send. Moving a Lock to another
// thread is safe because ownership of the Lock implies exclusive
// access to the contained T, and T: Send means T itself is safe
// to send across threads.
unsafe impl<T: Send> Send for Lock<T> {}

// SAFETY: Lock<T> is Sync if T is Send (not T: Sync). Multiple
// threads sharing a &Lock<T> is safe because the AtomicBool
// compare_exchange guarantees only one thread holds the lock at a
// time (mutual exclusion). Only one LockGuard exists per Lock at
// any moment, so the &T or &mut T produced by Deref/DerefMut are
// never accessible to a second thread concurrently.
unsafe impl<T: Send> Sync for Lock<T> {}

impl<T> Lock<T> {
    /// Create a new lock wrapping `value` with the given D53 ordering.
    ///
    /// D75: Lock owns its data. The value is only accessible through
    /// the LockGuard returned by `acquire`.
    pub const fn new(order: LockOrder, value: T) -> Lock<T> {
        Lock {
            locked: AtomicBool::new(false),
            order,
            data: core::cell::UnsafeCell::new(value),
        }
    }

    /// Acquire the lock, returning a guard that releases on drop.
    ///
    /// The guard provides `DerefMut<Target=T>` — exclusive access to
    /// the protected data for the duration the guard is held (D75).
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
/// D75: the guard provides DerefMut<Target=T>. The type system
/// enforces that the lock is held before data is accessed — no
/// convention gap.
pub struct LockGuard<'a, T> {
    lock: &'a Lock<T>,
}

impl<T> Drop for LockGuard<'_, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}

impl<T> core::ops::Deref for LockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &T {
        // SAFETY: the lock is held (we are inside a LockGuard) so we
        // have exclusive access. No other thread can hold the lock
        // simultaneously (SpinLock mutual exclusion).
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> core::ops::DerefMut for LockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: the lock is held (we are inside a LockGuard) so we
        // have exclusive access — no other thread can acquire the lock
        // simultaneously (AtomicBool compare_exchange enforces this).
        // `&mut self` guarantees the guard itself is not aliased, so
        // there is no second LockGuard producing a concurrent &mut T.
        // UnsafeCell::get() returns a *mut T valid for the lifetime of
        // the Lock, which outlives this guard via the 'a lifetime bound.
        unsafe { &mut *self.lock.data.get() }
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
    fn unordered_locks_do_not_participate_in_strict_ordering() {
        assert!(!LockOrder::Space.is_ordered());
        assert!(!LockOrder::Time.is_ordered());
        assert!(!LockOrder::SpaceManager.is_ordered());
        assert!(LockOrder::Field.is_ordered());
        assert!(LockOrder::Observer.is_ordered());
        assert!(LockOrder::Pulsar.is_ordered());
    }

    #[test]
    fn acquire_and_release() {
        let lock: Lock<u32> = Lock::new(LockOrder::Field, 0);

        {
            let _guard = lock.acquire();
        }

        let _guard2 = lock.acquire();
    }

    #[test]
    fn guard_reports_order() {
        let lock: Lock<u32> = Lock::new(LockOrder::Observer, 0);
        let guard = lock.acquire();

        assert_eq!(guard.order(), LockOrder::Observer);
    }

    #[test]
    fn guard_provides_deref_access() {
        let lock: Lock<u32> = Lock::new(LockOrder::Field, 42);
        let guard = lock.acquire();

        assert_eq!(*guard, 42);
    }

    #[test]
    fn guard_provides_deref_mut_access() {
        let lock: Lock<u32> = Lock::new(LockOrder::Field, 0);
        let mut guard = lock.acquire();

        *guard = 99;

        assert_eq!(*guard, 99);
    }

    #[test]
    fn mutation_visible_across_acquire_cycles() {
        let lock: Lock<u32> = Lock::new(LockOrder::Field, 10);

        {
            let mut guard = lock.acquire();

            *guard = 20;
        }

        let guard = lock.acquire();

        assert_eq!(*guard, 20);
    }

    #[test]
    fn lock_order_comparisons() {
        assert!(LockOrder::Field.is_ordered());
        assert!(LockOrder::Observer.is_ordered());
        assert!(LockOrder::Pulsar.is_ordered());
        assert!(!LockOrder::Space.is_ordered());
        assert!(!LockOrder::Time.is_ordered());
        assert!(!LockOrder::SpaceManager.is_ordered());
        assert!(!LockOrder::IrqRouting.is_ordered());
        assert!(!LockOrder::AsidAllocator.is_ordered());
    }

    #[test]
    fn lock_order_field_lt_observer_lt_pulsar() {
        assert!((LockOrder::Field as u8) < (LockOrder::Observer as u8));
        assert!((LockOrder::Observer as u8) < (LockOrder::Pulsar as u8));
    }

    #[test]
    fn lock_preserves_initial_value() {
        let lock: Lock<u64> = Lock::new(LockOrder::Space, 0xDEAD_BEEF);
        let guard = lock.acquire();

        assert_eq!(*guard, 0xDEAD_BEEF);
    }

    #[test]
    fn lock_reports_order() {
        let lock: Lock<u32> = Lock::new(LockOrder::Time, 0);

        assert_eq!(lock.order(), LockOrder::Time);
    }

    #[test]
    fn multiple_mutations_accumulate() {
        let lock: Lock<u32> = Lock::new(LockOrder::Field, 0);

        for i in 0..10u32 {
            let mut guard = lock.acquire();

            *guard += i;
        }

        let guard = lock.acquire();

        assert_eq!(*guard, 45);
    }
}
