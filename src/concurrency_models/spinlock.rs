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
//!
//! Tests:
//!   loom_spinlock_mutual_exclusion     — increments through lock are never lost
//!   loom_spinlock_no_concurrent_holders — lock is never held by two threads at once

#[cfg(test)]
mod tests {
    use loom::sync::Arc;
    use loom::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use loom::thread;

    // ── ModelLock ─────────────────────────────────────────────────────

    /// Abstract model of frame/lock.rs Lock<T>.
    ///
    /// Uses the identical Acquire/Release CAS protocol as the kernel spinlock.
    /// Data is represented as an AtomicUsize rather than UnsafeCell<T> so
    /// Loom can observe all accesses to the protected value.
    struct ModelLock {
        locked: AtomicBool,
        value: AtomicUsize,
    }

    impl ModelLock {
        fn new(initial: usize) -> Self {
            ModelLock {
                locked: AtomicBool::new(false),
                value: AtomicUsize::new(initial),
            }
        }

        /// Acquire the lock. Spins using loom::thread::yield_now() until the
        /// CAS succeeds. Mirrors frame/lock.rs: compare_exchange_weak(false,
        /// true, Acquire, Relaxed).
        fn acquire(&self) {
            while self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
            {
                // Loom requires yield_now() in spin loops (not spin_loop hint).
                thread::yield_now();
            }
        }

        /// Release the lock. Mirrors frame/lock.rs: store(false, Release).
        fn release(&self) {
            self.locked.store(false, Ordering::Release);
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────

    /// Verify mutual exclusion by showing all increments through the lock
    /// are visible in the final counter — no increment is lost, which proves
    /// concurrent access to the critical section never overlaps.
    ///
    /// Two threads each acquire the lock, increment the shared counter,
    /// and release. N=2 iterations per thread → final value must be 4 under
    /// all Loom-explored interleavings.
    #[test]
    fn loom_spinlock_mutual_exclusion() {
        loom::model(|| {
            const ITERS: usize = 2;

            let lock = Arc::new(ModelLock::new(0));

            let lock_a = lock.clone();
            let handle_a = thread::spawn(move || {
                for _ in 0..ITERS {
                    lock_a.acquire();
                    let old = lock_a.value.load(Ordering::Relaxed);
                    lock_a.value.store(old + 1, Ordering::Relaxed);
                    lock_a.release();
                }
            });

            let lock_b = lock.clone();
            let handle_b = thread::spawn(move || {
                for _ in 0..ITERS {
                    lock_b.acquire();
                    let old = lock_b.value.load(Ordering::Relaxed);
                    lock_b.value.store(old + 1, Ordering::Relaxed);
                    lock_b.release();
                }
            });

            handle_a.join().expect("thread a");
            handle_b.join().expect("thread b");

            assert_eq!(
                lock.value.load(Ordering::Relaxed),
                ITERS * 2,
                "mutual exclusion: all increments must be visible"
            );
        });
    }

    /// Verify the lock is never held by two threads simultaneously.
    ///
    /// A shared holder-count AtomicUsize is set from 0 to 1 on lock acquire
    /// (asserting it was 0 — no concurrent holder) and back to 0 on release.
    /// Loom explores every interleaving and will trigger the assertion if any
    /// execution allows two threads to hold the lock at the same time.
    #[test]
    fn loom_spinlock_no_concurrent_holders() {
        loom::model(|| {
            let locked = Arc::new(AtomicBool::new(false));
            let holder_count = Arc::new(AtomicUsize::new(0));

            let locked_a = locked.clone();
            let count_a = holder_count.clone();
            let handle_a = thread::spawn(move || {
                // Acquire
                while locked_a
                    .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_err()
                {
                    thread::yield_now();
                }
                // Critical section: assert no concurrent holder.
                let prev = count_a.fetch_add(1, Ordering::Relaxed);
                assert_eq!(
                    prev, 0,
                    "thread A acquired lock but holder_count was non-zero"
                );
                // Release
                count_a.fetch_sub(1, Ordering::Relaxed);
                locked_a.store(false, Ordering::Release);
            });

            let locked_b = locked.clone();
            let count_b = holder_count.clone();
            let handle_b = thread::spawn(move || {
                // Acquire
                while locked_b
                    .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                    .is_err()
                {
                    thread::yield_now();
                }
                // Critical section: assert no concurrent holder.
                let prev = count_b.fetch_add(1, Ordering::Relaxed);
                assert_eq!(
                    prev, 0,
                    "thread B acquired lock but holder_count was non-zero"
                );
                // Release
                count_b.fetch_sub(1, Ordering::Relaxed);
                locked_b.store(false, Ordering::Release);
            });

            handle_a.join().expect("thread a");
            handle_b.join().expect("thread b");
        });
    }
}
