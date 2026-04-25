//! Round-robin scheduler: simple per-core scheduling algorithm.
//!
//! Implements the `Scheduler` trait (D59) with a FIFO run queue. Each
//! `on_preempt` tick rotates the head to the tail — classic round-robin.
//! The `should_switch_to` predicate always approves direct switches for
//! IPC fast-path (D50) because round-robin has no priority hierarchy.
//!
//! D2:  one of potentially many per-core algorithm implementations.
//! D42: throughput value could inform time-slice length (deferred to
//!      tuning — initial implementation rotates on every preempt tick).
//! D59: concrete implementation of the five-method Scheduler trait.

use crate::observer::Observer;
use crate::time_manager::Scheduler;
use core::ptr::NonNull;

/// Maximum Observers in a single core's run queue.
///
/// Sized for initial development. D56 specifies boot-sized arrays for
/// production; this constant is the leaf-node decision that gets
/// replaced when boot-time sizing is implemented.
const MAX_QUEUE_DEPTH: usize = 64;

/// FIFO round-robin scheduler (D2, D59).
///
/// Circular buffer of runnable Observer pointers. `head` marks the
/// logical front; `count` tracks the active element count. Enqueue
/// appends at `(head + count) % N`; `pick_next` reads at `head`.
/// `on_preempt` advances `head` — O(1) rotation, efficient for the
/// timer-interrupt hot path. Dequeue by identity is O(N) linear
/// scan — acceptable because N is bounded (≤64) and dequeue frequency
/// (Observer blocks) is lower than preemption frequency.
pub struct RoundRobin {
    queue: [Option<NonNull<Observer>>; MAX_QUEUE_DEPTH],
    head: u8,
    count: u8,
}

impl Default for RoundRobin {
    fn default() -> Self {
        Self::new()
    }
}

impl RoundRobin {
    pub const fn new() -> Self {
        RoundRobin {
            queue: [None; MAX_QUEUE_DEPTH],
            head: 0,
            count: 0,
        }
    }

    pub fn queue_depth(&self) -> u32 {
        self.count as u32
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn contains(&self, observer: NonNull<Observer>) -> bool {
        for logical in 0..self.count as usize {
            if self.queue[self.physical(logical)] == Some(observer) {
                return true;
            }
        }

        false
    }

    fn physical(&self, logical: usize) -> usize {
        (self.head as usize + logical) % MAX_QUEUE_DEPTH
    }
}

impl Scheduler for RoundRobin {
    /// Add an Observer to the tail of the run queue (D59).
    ///
    /// Called while holding Arena<Observer> lock (D53 lock discipline).
    /// The Observer must not already be in the queue — double-enqueue
    /// is a caller bug (the caller transitions the Observer to Runnable
    /// before enqueuing, and Runnable → Runnable is not a valid
    /// transition).
    fn enqueue(&mut self, observer: NonNull<Observer>) {
        debug_assert!(
            (self.count as usize) < MAX_QUEUE_DEPTH,
            "run queue overflow: {} observers already enqueued (max {MAX_QUEUE_DEPTH})",
            self.count
        );
        debug_assert!(
            !self.contains(observer),
            "double-enqueue: Observer already in run queue"
        );

        if (self.count as usize) >= MAX_QUEUE_DEPTH {
            return;
        }

        self.queue[self.physical(self.count as usize)] = Some(observer);
        self.count += 1;
    }

    /// Remove a specific Observer from the run queue (D59).
    ///
    /// Called while holding Arena<Observer> lock (D53 lock discipline).
    /// If the Observer is not in the queue (already dequeued or never
    /// enqueued), this is a no-op — defensive, per the principle that
    /// correct usage should be easy.
    fn dequeue(&mut self, observer: NonNull<Observer>) {
        let count = self.count as usize;

        for logical in 0..count {
            if self.queue[self.physical(logical)] == Some(observer) {
                for j in logical..count - 1 {
                    self.queue[self.physical(j)] = self.queue[self.physical(j + 1)];
                }

                self.queue[self.physical(count - 1)] = None;
                self.count -= 1;

                return;
            }
        }
    }

    /// Select the next Observer to run (D59).
    ///
    /// Returns the head of the queue, or None if empty (D46: WFI).
    /// Called WITHOUT arena locks (D59 lock discipline).
    fn pick_next(&self) -> Option<NonNull<Observer>> {
        if self.count > 0 {
            self.queue[self.head as usize]
        } else {
            None
        }
    }

    /// IPC fast-path predicate (D50, D59).
    ///
    /// Round-robin always approves direct switches. The sender is
    /// voluntarily blocking (Call/ReplyRecv), so there is no priority
    /// reason to deny. Approving maximizes IPC responsiveness, which
    /// is the right default for a scheduler with no priority hierarchy.
    ///
    /// Read-only, ≤50 cycle budget (D59). This is a single return
    /// instruction — well within budget.
    fn should_switch_to(&self, _receiver: NonNull<Observer>) -> bool {
        true
    }

    /// Timer tick accounting (D59).
    ///
    /// O(1) rotation: copies the head element to the slot past the
    /// tail, then advances `head`. This gives each Observer one tick
    /// of execution before yielding to the next. Called WITHOUT arena
    /// locks (D59 lock discipline).
    ///
    /// When count == MAX_QUEUE_DEPTH, all slots are occupied and
    /// advancing head alone rotates the logical view (the old head
    /// is already at the new tail position by wraparound).
    ///
    /// D42: a more sophisticated implementation would derive the time
    /// slice from the Observer's throughput value (higher T = longer
    /// slice before rotation). The initial implementation rotates on
    /// every preempt tick — correct but not optimal.
    fn on_preempt(&mut self) {
        if self.count <= 1 {
            return;
        }

        let head = self.head as usize;
        let count = self.count as usize;

        if count < MAX_QUEUE_DEPTH {
            self.queue[(head + count) % MAX_QUEUE_DEPTH] = self.queue[head];
            self.queue[head] = None;
        }

        self.head = ((head + 1) % MAX_QUEUE_DEPTH) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observer::Observer;

    fn make_observer() -> Observer {
        Observer::test_default()
    }

    // ── Spec verifier tests (D59 derivation claims) ──────────────────

    #[test]
    fn test_d59_new_scheduler_is_empty() {
        let sched = RoundRobin::new();

        assert!(sched.is_empty());
        assert_eq!(sched.queue_depth(), 0);
        assert!(sched.pick_next().is_none());
    }

    #[test]
    fn test_d59_enqueue_adds_to_run_queue() {
        let mut sched = RoundRobin::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);

        assert_eq!(sched.queue_depth(), 1);
        assert!(sched.contains(ptr));
    }

    #[test]
    fn test_d59_dequeue_removes_from_run_queue() {
        let mut sched = RoundRobin::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);
        sched.dequeue(ptr);

        assert_eq!(sched.queue_depth(), 0);
        assert!(!sched.contains(ptr));
    }

    #[test]
    fn test_d59_pick_next_returns_head() {
        let mut sched = RoundRobin::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        sched.enqueue(ptr_a);
        sched.enqueue(ptr_b);

        assert_eq!(sched.pick_next(), Some(ptr_a));
    }

    #[test]
    fn test_d59_pick_next_returns_none_when_empty() {
        let sched = RoundRobin::new();

        assert!(sched.pick_next().is_none());
    }

    #[test]
    fn test_d50_should_switch_to_always_approves() {
        let sched = RoundRobin::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        assert!(
            sched.should_switch_to(ptr),
            "D50: round-robin must approve direct switch (no priority hierarchy)"
        );
    }

    #[test]
    fn test_d59_on_preempt_rotates_head_to_tail() {
        let mut sched = RoundRobin::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let mut obs_c = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);
        let ptr_c = NonNull::from(&mut obs_c);

        sched.enqueue(ptr_a);
        sched.enqueue(ptr_b);
        sched.enqueue(ptr_c);

        assert_eq!(sched.pick_next(), Some(ptr_a));

        sched.on_preempt();

        assert_eq!(
            sched.pick_next(),
            Some(ptr_b),
            "D59: on_preempt must rotate head (A) to tail"
        );

        sched.on_preempt();

        assert_eq!(
            sched.pick_next(),
            Some(ptr_c),
            "D59: second rotation must advance to C"
        );

        sched.on_preempt();

        assert_eq!(
            sched.pick_next(),
            Some(ptr_a),
            "D59: full rotation cycle must return to A"
        );
    }

    #[test]
    fn test_d46_pick_next_signals_idle_when_empty() {
        let sched = RoundRobin::new();

        assert!(
            sched.pick_next().is_none(),
            "D46: empty queue must return None (WFI)"
        );
    }

    #[test]
    fn test_d59_enqueue_fifo_order() {
        let mut sched = RoundRobin::new();
        let mut observers: [Observer; 4] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 4] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            sched.enqueue(*ptr);
        }

        for (i, expected) in ptrs.iter().enumerate() {
            assert_eq!(
                sched.pick_next(),
                Some(*expected),
                "FIFO violated at position {i}"
            );

            sched.dequeue(*expected);
        }
    }

    // ── Adversarial tests ────────────────────────────────────────────

    #[test]
    fn test_adversarial_dequeue_nonexistent_is_noop() {
        let mut sched = RoundRobin::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        sched.enqueue(ptr_a);
        sched.dequeue(ptr_b);

        assert_eq!(
            sched.queue_depth(),
            1,
            "dequeue of non-enqueued Observer must be a no-op"
        );
        assert_eq!(sched.pick_next(), Some(ptr_a));
    }

    #[test]
    fn test_adversarial_dequeue_empty_is_noop() {
        let mut sched = RoundRobin::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.dequeue(ptr);

        assert_eq!(sched.queue_depth(), 0);
    }

    #[test]
    fn test_adversarial_dequeue_middle_preserves_order() {
        let mut sched = RoundRobin::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let mut obs_c = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);
        let ptr_c = NonNull::from(&mut obs_c);

        sched.enqueue(ptr_a);
        sched.enqueue(ptr_b);
        sched.enqueue(ptr_c);
        sched.dequeue(ptr_b);

        assert_eq!(sched.queue_depth(), 2);
        assert_eq!(
            sched.pick_next(),
            Some(ptr_a),
            "dequeue middle must preserve head"
        );

        sched.dequeue(ptr_a);

        assert_eq!(
            sched.pick_next(),
            Some(ptr_c),
            "dequeue head must promote next"
        );
    }

    #[test]
    fn test_adversarial_dequeue_head_promotes_next() {
        let mut sched = RoundRobin::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        sched.enqueue(ptr_a);
        sched.enqueue(ptr_b);
        sched.dequeue(ptr_a);

        assert_eq!(sched.pick_next(), Some(ptr_b));
    }

    #[test]
    fn test_adversarial_dequeue_tail_preserves_order() {
        let mut sched = RoundRobin::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let mut obs_c = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);
        let ptr_c = NonNull::from(&mut obs_c);

        sched.enqueue(ptr_a);
        sched.enqueue(ptr_b);
        sched.enqueue(ptr_c);
        sched.dequeue(ptr_c);

        assert_eq!(sched.queue_depth(), 2);
        assert_eq!(sched.pick_next(), Some(ptr_a));

        sched.on_preempt();

        assert_eq!(sched.pick_next(), Some(ptr_b));
    }

    #[test]
    fn test_adversarial_on_preempt_single_is_noop() {
        let mut sched = RoundRobin::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);
        sched.on_preempt();

        assert_eq!(
            sched.pick_next(),
            Some(ptr),
            "on_preempt with 1 Observer must be a no-op"
        );
    }

    #[test]
    fn test_adversarial_on_preempt_empty_is_noop() {
        let mut sched = RoundRobin::new();

        sched.on_preempt();

        assert_eq!(sched.queue_depth(), 0);
    }

    #[test]
    fn test_adversarial_enqueue_after_full_drain() {
        let mut sched = RoundRobin::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        sched.enqueue(ptr_a);
        sched.dequeue(ptr_a);

        assert!(sched.is_empty());

        sched.enqueue(ptr_b);

        assert_eq!(sched.queue_depth(), 1);
        assert_eq!(sched.pick_next(), Some(ptr_b));
    }

    #[test]
    fn test_adversarial_n_rotations_returns_to_start() {
        let mut sched = RoundRobin::new();
        let mut observers: [Observer; 5] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 5] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            sched.enqueue(*ptr);
        }

        for _ in 0..5 {
            sched.on_preempt();
        }

        assert_eq!(
            sched.pick_next(),
            Some(ptrs[0]),
            "N rotations of N elements must cycle back to head"
        );
    }

    #[test]
    fn test_adversarial_dequeue_during_rotation_cycle() {
        let mut sched = RoundRobin::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let mut obs_c = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);
        let ptr_c = NonNull::from(&mut obs_c);

        sched.enqueue(ptr_a);
        sched.enqueue(ptr_b);
        sched.enqueue(ptr_c);
        sched.on_preempt();
        // Now: B, C, A
        sched.dequeue(ptr_c);
        // Now: B, A

        assert_eq!(sched.pick_next(), Some(ptr_b));

        sched.on_preempt();
        // Now: A, B

        assert_eq!(sched.pick_next(), Some(ptr_a));
    }

    #[test]
    fn test_adversarial_queue_depth_consistency() {
        let mut sched = RoundRobin::new();
        let mut observers: [Observer; 8] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 8] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for (i, ptr) in ptrs.iter().enumerate() {
            sched.enqueue(*ptr);

            assert_eq!(sched.queue_depth(), (i + 1) as u32);
        }
        for (i, ptr) in ptrs.iter().enumerate() {
            sched.dequeue(*ptr);

            assert_eq!(sched.queue_depth(), (8 - i - 1) as u32);
        }
    }

    #[test]
    fn test_adversarial_should_switch_to_with_full_queue() {
        let mut sched = RoundRobin::new();
        let mut observers: [Observer; 4] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 4] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            sched.enqueue(*ptr);
        }

        let mut target = make_observer();
        let target_ptr = NonNull::from(&mut target);

        assert!(
            sched.should_switch_to(target_ptr),
            "should_switch_to must approve even with full queue"
        );
    }

    #[test]
    fn test_adversarial_repeated_dequeue_same_observer() {
        let mut sched = RoundRobin::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);
        sched.dequeue(ptr);
        sched.dequeue(ptr);

        assert_eq!(
            sched.queue_depth(),
            0,
            "double dequeue must not underflow count"
        );
    }

    // ── Circular buffer wrap-around tests ────────────────────────────
    //
    // These exercise behavior after heavy rotation — the logical
    // sequence is identical but the internal state has wrapped past
    // the physical array boundary. Any off-by-one in index math,
    // stale data in cleared slots, or capacity-boundary bugs surface
    // here.

    #[test]
    fn test_wrap_heavy_rotation_then_operations() {
        let mut sched = RoundRobin::new();
        let mut observers: [Observer; 3] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 3] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            sched.enqueue(*ptr);
        }

        // 100 rotations of 3 elements: 100 % 3 = 1 remainder
        for _ in 0..100 {
            sched.on_preempt();
        }

        assert_eq!(
            sched.pick_next(),
            Some(ptrs[1]),
            "100 rotations of 3 ≡ 1 rotation: head must be B"
        );

        sched.dequeue(ptrs[1]);

        assert_eq!(sched.pick_next(), Some(ptrs[2]), "C after B removed");

        let mut new_obs = make_observer();
        let new_ptr = NonNull::from(&mut new_obs);

        sched.enqueue(new_ptr);

        assert_eq!(sched.queue_depth(), 3);
        assert!(sched.contains(new_ptr), "newly enqueued must be found");
    }

    #[test]
    fn test_wrap_dequeue_preserves_order_across_rotation() {
        let mut sched = RoundRobin::new();
        let mut observers: [Observer; 4] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 4] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            sched.enqueue(*ptr);
        }

        // 50 rotations of 4: 50 % 4 = 2 remainder → [C, D, A, B]
        for _ in 0..50 {
            sched.on_preempt();
        }

        assert_eq!(
            sched.pick_next(),
            Some(ptrs[2]),
            "head is C after 50 rotations"
        );

        sched.dequeue(ptrs[0]);

        // Queue is now [C, D, B] — verify full rotation
        assert_eq!(sched.pick_next(), Some(ptrs[2]), "C still head");

        sched.on_preempt();

        assert_eq!(sched.pick_next(), Some(ptrs[3]), "D next");

        sched.on_preempt();

        assert_eq!(sched.pick_next(), Some(ptrs[1]), "B last");
    }

    #[test]
    fn test_wrap_contains_no_stale_after_dequeue() {
        let mut sched = RoundRobin::new();
        let mut observers: [Observer; 3] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 3] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            sched.enqueue(*ptr);
        }

        for _ in 0..20 {
            sched.on_preempt();
        }

        sched.dequeue(ptrs[1]);

        assert!(
            !sched.contains(ptrs[1]),
            "dequeued Observer must not be found after rotation"
        );
        assert!(sched.contains(ptrs[0]));
        assert!(sched.contains(ptrs[2]));
    }

    #[test]
    fn test_wrap_drain_and_refill_after_rotation() {
        let mut sched = RoundRobin::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        sched.enqueue(ptr_a);
        sched.enqueue(ptr_b);

        for _ in 0..37 {
            sched.on_preempt();
        }

        sched.dequeue(ptr_a);
        sched.dequeue(ptr_b);

        assert!(sched.is_empty());

        let mut obs_c = make_observer();
        let ptr_c = NonNull::from(&mut obs_c);

        sched.enqueue(ptr_c);

        assert_eq!(sched.queue_depth(), 1);
        assert_eq!(
            sched.pick_next(),
            Some(ptr_c),
            "refill after drain+rotation must work"
        );
    }

    #[test]
    fn test_wrap_high_depth_multi_cycle() {
        let mut sched = RoundRobin::new();
        let mut observers: [Observer; 32] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 32] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            sched.enqueue(*ptr);
        }

        // 96 rotations = 3 full cycles through 32 elements
        for i in 0..96 {
            assert_eq!(
                sched.pick_next(),
                Some(ptrs[i % 32]),
                "rotation {i}: expected observer {}",
                i % 32
            );

            sched.on_preempt();
        }

        assert_eq!(
            sched.pick_next(),
            Some(ptrs[0]),
            "3 full cycles must return to start"
        );
    }
}
