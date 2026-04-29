//! EEVDF scheduler: Earliest Eligible Virtual Deadline First.
//!
//! Proportional-share scheduler with bounded latency. Stoica &
//! Abdel-Wahab 1995, adapted for this kernel's R/T/P profile model.
//!
//! Each Observer in the queue tracks virtual eligible time (VET) and
//! virtual deadline (VD). The global virtual time advances by
//! `SCALE / total_weight` per timer tick. An Observer is eligible when
//! `VET <= global_virtual_time`. Among eligible Observers, `pick_next`
//! selects the one with the earliest VD.
//!
//! R/T/P mapping:
//! - Weight: `max(1, compute_aggregate)` — CPU share proportional to
//!   held Time caps (D36). Equal weight = equal share.
//! - Slice: derived from T. Higher T = longer slice = later deadlines
//!   = fewer context switches. Lower T (high R or high P) = shorter
//!   slice = earlier deadlines = more responsive.
//!
//! D2:  per-core algorithm, state lives in this struct (not Observer).
//! D42: R/T/P profile interpreted through weight and slice mapping.
//! D50: should_switch_to checks receiver's hypothetical deadline.
//! D59: implements the five-method Scheduler trait.

use crate::observer::Observer;
use crate::time_manager::Scheduler;
use core::ptr::NonNull;

const MAX_QUEUE_DEPTH: usize = 64;

/// Fixed-point scale factor for virtual time arithmetic.
/// Multiply all virtual times by this to avoid precision loss
/// when dividing by weight.
const SCALE: u64 = 1 << 20;

/// Minimum slice in logical ticks. Even the most responsive Observer
/// gets at least this many ticks before its deadline advances.
const MIN_SLICE_TICKS: u64 = 1;

/// Maximum slice in logical ticks. The most throughput-oriented
/// Observer gets this many ticks per scheduling quantum.
const MAX_SLICE_TICKS: u64 = 16;

/// Per-Observer scheduling state within the EEVDF queue.
///
/// Stored alongside the Observer pointer in the scheduler's array.
/// This is algorithm-specific state (D2) — it lives in the scheduler,
/// not in the Observer struct.
#[derive(Clone, Copy)]
struct EevdfEntry {
    observer: NonNull<Observer>,
    virtual_eligible_time: u64,
    virtual_deadline: u64,
    weight: u32,
    slice: u64,
}

/// EEVDF scheduler (D2, D59).
///
/// Unsorted flat array with linear scan. At MAX_QUEUE_DEPTH=64, a
/// full scan is ~200 cycles — comparable to RoundRobin's dequeue.
pub struct EarliestEligibleVirtualDeadline {
    entries: [Option<EevdfEntry>; MAX_QUEUE_DEPTH],
    count: u8,
    global_virtual_time: u64,
    total_weight: u32,
    /// Cached pointer to the Observer last returned by pick_next.
    /// Used by on_preempt to update the running Observer's state.
    current: Option<usize>,
}

impl Default for EarliestEligibleVirtualDeadline {
    fn default() -> Self {
        Self::new()
    }
}

impl EarliestEligibleVirtualDeadline {
    pub const fn new() -> Self {
        EarliestEligibleVirtualDeadline {
            entries: [None; MAX_QUEUE_DEPTH],
            count: 0,
            global_virtual_time: SCALE,
            total_weight: 0,
            current: None,
        }
    }

    pub fn queue_depth(&self) -> u32 {
        self.count as u32
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn contains(&self, observer: NonNull<Observer>) -> bool {
        self.find(observer).is_some()
    }

    fn find(&self, observer: NonNull<Observer>) -> Option<usize> {
        for i in 0..self.count as usize {
            if let Some(ref entry) = self.entries[i]
                && entry.observer == observer
            {
                return Some(i);
            }
        }

        None
    }

    /// Derive weight from Observer's compute_aggregate (D36).
    fn weight_of(observer: NonNull<Observer>) -> u32 {
        let (aggregate, _, _) = crate::frame::cores::observer_scheduling_params(observer);

        if aggregate == 0 { 1 } else { aggregate }
    }

    /// Derive slice (in scaled virtual ticks) from Observer's throughput.
    ///
    /// Higher T = longer slice = later deadlines = more throughput.
    /// Lower T (high R or high P) = shorter slice = earlier deadlines.
    fn slice_of(observer: NonNull<Observer>, weight: u32) -> u64 {
        let (_, _, throughput_val) = crate::frame::cores::observer_scheduling_params(observer);
        let throughput = throughput_val as u64;
        let ticks = MIN_SLICE_TICKS + throughput * (MAX_SLICE_TICKS - MIN_SLICE_TICKS) / 128;

        ticks * SCALE / weight as u64
    }

    /// Find the index of the eligible Observer with the earliest VD.
    fn best_eligible(&self) -> Option<usize> {
        let mut best_idx = None;
        let mut best_vd = u64::MAX;

        for i in 0..self.count as usize {
            if let Some(ref entry) = self.entries[i]
                && entry.virtual_eligible_time <= self.global_virtual_time
                && entry.virtual_deadline < best_vd
            {
                best_vd = entry.virtual_deadline;
                best_idx = Some(i);
            }
        }

        best_idx
    }
}

impl Scheduler for EarliestEligibleVirtualDeadline {
    /// Add an Observer to the run queue (D59).
    ///
    /// On enqueue (wake from block or initial resume), the Observer is
    /// immediately eligible: VET = global_virtual_time. Its deadline is
    /// VD = VET + slice. This gives recently-woken Observers favorable
    /// treatment — they are eligible immediately with a near deadline,
    /// matching the EEVDF sleeper-fairness property.
    fn enqueue(&mut self, observer: NonNull<Observer>) {
        if self.find(observer).is_some() {
            return;
        }
        if (self.count as usize) >= MAX_QUEUE_DEPTH {
            return;
        }

        let weight = Self::weight_of(observer);
        let slice = Self::slice_of(observer, weight);
        let entry = EevdfEntry {
            observer,
            virtual_eligible_time: self.global_virtual_time,
            virtual_deadline: self.global_virtual_time + slice,
            weight,
            slice,
        };

        self.entries[self.count as usize] = Some(entry);
        self.count += 1;
        self.total_weight += weight;
    }

    /// Remove an Observer from the run queue (D59).
    ///
    /// On dequeue (block on Receive/Call), EEVDF state is discarded.
    /// Re-enqueue will re-derive VET/VD from the current global time,
    /// matching standard EEVDF wakeup behavior.
    fn dequeue(&mut self, observer: NonNull<Observer>) {
        if let Some(idx) = self.find(observer) {
            let weight = self.entries[idx].as_ref().unwrap().weight;

            self.total_weight = self.total_weight.saturating_sub(weight);

            // Swap-remove: move last entry into the gap.
            let last = self.count as usize - 1;

            if idx != last {
                self.entries[idx] = self.entries[last];
            }

            self.entries[last] = None;
            self.count -= 1;

            // Invalidate current index if it pointed to the removed or
            // swapped entry.
            if let Some(cur) = self.current {
                if cur == idx {
                    self.current = None;
                } else if cur == last {
                    self.current = Some(idx);
                }
            }
        }
    }

    /// Select the eligible Observer with the earliest virtual deadline.
    ///
    /// If no Observer is eligible (all have VET > global_virtual_time),
    /// falls back to the Observer with the earliest VD regardless of
    /// eligibility — prevents starvation when virtual time drifts.
    fn pick_next(&self) -> Option<NonNull<Observer>> {
        if self.count == 0 {
            return None;
        }

        // Primary: earliest deadline among eligible Observers.
        if let Some(idx) = self.best_eligible() {
            // Cache the index for on_preempt (interior mutability not
            // needed — pick_next is always followed by on_preempt which
            // is &mut self and will set current).
            return Some(self.entries[idx].as_ref().unwrap().observer);
        }

        // Fallback: no eligible Observers. Pick earliest VD overall.
        // This happens when all Observers have consumed ahead of their
        // share (negative lag). Picking the earliest VD prevents
        // starvation and allows virtual time to catch up.
        let mut best_idx = 0;
        let mut best_vd = u64::MAX;

        for i in 0..self.count as usize {
            if let Some(ref entry) = self.entries[i]
                && entry.virtual_deadline < best_vd
            {
                best_vd = entry.virtual_deadline;
                best_idx = i;
            }
        }

        Some(self.entries[best_idx].as_ref().unwrap().observer)
    }

    /// IPC fast-path predicate (D50, D59).
    ///
    /// Approves the direct switch if the receiver would have an earlier
    /// deadline than the current best eligible Observer. The receiver is
    /// not in the queue (it was blocked), so we compute its hypothetical
    /// deadline: global_virtual_time + slice_of(receiver).
    ///
    /// Budget: ~20 cycles (two field reads + arithmetic + comparison).
    fn should_switch_to(&self, receiver: NonNull<Observer>) -> bool {
        if self.count == 0 {
            return true;
        }

        let weight = Self::weight_of(receiver);
        let receiver_vd = self.global_virtual_time + Self::slice_of(receiver, weight);

        // Compare against the best eligible in the queue.
        if let Some(idx) = self.best_eligible() {
            let best_vd = self.entries[idx].as_ref().unwrap().virtual_deadline;

            return receiver_vd <= best_vd;
        }

        true
    }

    /// Timer tick accounting (D59).
    ///
    /// Advances global virtual time and updates the running Observer's
    /// VET and VD. The running Observer is the one last returned by
    /// pick_next — tracked via `current` index.
    fn on_preempt(&mut self) {
        if self.count == 0 {
            return;
        }

        // Advance global virtual time by one tick scaled by total weight.
        if self.total_weight > 0 {
            self.global_virtual_time += SCALE / self.total_weight as u64;
        }

        // Find the running Observer (the one pick_next last selected).
        // If current is stale (dequeue invalidated it), re-derive from
        // pick_next's logic.
        let running_idx = match self.current {
            Some(idx) if idx < self.count as usize => idx,
            _ => {
                // Re-derive: find best eligible, or earliest VD.
                match self.best_eligible() {
                    Some(idx) => idx,
                    None => {
                        // Fallback: earliest VD.
                        let mut best = 0;
                        let mut best_vd = u64::MAX;

                        for i in 0..self.count as usize {
                            if let Some(ref entry) = self.entries[i]
                                && entry.virtual_deadline < best_vd
                            {
                                best_vd = entry.virtual_deadline;
                                best = i;
                            }
                        }

                        best
                    }
                }
            }
        };

        // Update the running Observer: it consumed one tick of CPU.
        // VET advances to its old VD (it has now "used" its quantum).
        // VD advances by slice/weight (its next deadline).
        if let Some(ref mut entry) = self.entries[running_idx] {
            entry.virtual_eligible_time = entry.virtual_deadline;
            entry.virtual_deadline += entry.slice;
            // Refresh weight and slice in case profile changed.
            entry.weight = Self::weight_of(entry.observer);
            entry.slice = Self::slice_of(entry.observer, entry.weight);
        }

        // Update current for next cycle. pick_next will be called
        // immediately after on_preempt; cache the best for consistency.
        self.current = self.best_eligible().or_else(|| {
            let mut best = 0;
            let mut best_vd = u64::MAX;

            for i in 0..self.count as usize {
                if let Some(ref entry) = self.entries[i]
                    && entry.virtual_deadline < best_vd
                {
                    best_vd = entry.virtual_deadline;
                    best = i;
                }
            }

            Some(best)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observer::Observer;

    fn make_observer() -> Observer {
        Observer::test_default()
    }

    fn make_observer_with_profile(r: u8, t: u8) -> Observer {
        let mut obs = Observer::test_default();

        obs.responsiveness = r;
        obs.throughput = t;

        obs
    }

    // ── Basic trait compliance ──────────────────────────────────────

    #[test]
    fn new_scheduler_is_empty() {
        let sched = EarliestEligibleVirtualDeadline::new();

        assert!(sched.is_empty());
        assert_eq!(sched.queue_depth(), 0);
        assert!(sched.pick_next().is_none());
    }

    #[test]
    fn enqueue_adds_to_queue() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);

        assert_eq!(sched.queue_depth(), 1);
        assert!(sched.contains(ptr));
    }

    #[test]
    fn dequeue_removes_from_queue() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);
        sched.dequeue(ptr);

        assert_eq!(sched.queue_depth(), 0);
        assert!(!sched.contains(ptr));
    }

    #[test]
    fn pick_next_returns_enqueued_observer() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);

        assert_eq!(sched.pick_next(), Some(ptr));
    }

    #[test]
    fn pick_next_returns_none_when_empty() {
        let sched = EarliestEligibleVirtualDeadline::new();

        assert!(sched.pick_next().is_none());
    }

    #[test]
    fn dequeue_nonexistent_is_noop() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        sched.enqueue(ptr_a);
        sched.dequeue(ptr_b);

        assert_eq!(sched.queue_depth(), 1);
    }

    #[test]
    fn duplicate_enqueue_rejected() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);
        sched.enqueue(ptr);

        assert_eq!(sched.queue_depth(), 1);
    }

    // ── EEVDF-specific behavior ─────────────────────────────────────

    #[test]
    fn high_responsiveness_observer_selected_first() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut interactive = make_observer_with_profile(120, 0);
        let mut batch = make_observer_with_profile(0, 120);
        let iptr = NonNull::from(&mut interactive);
        let bptr = NonNull::from(&mut batch);

        sched.enqueue(iptr);
        sched.enqueue(bptr);

        // Both are eligible (VET = global_virtual_time). Interactive
        // has shorter slice → earlier deadline → selected first.
        assert_eq!(
            sched.pick_next(),
            Some(iptr),
            "EEVDF must select high-R observer (earlier deadline)"
        );
    }

    #[test]
    fn equal_profiles_round_robin_like() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        sched.enqueue(ptr_a);
        sched.enqueue(ptr_b);

        // With equal profiles, both have the same slice. First enqueued
        // has (marginally) earlier VD. After on_preempt, A's VD advances
        // past B's, so B should be selected.
        assert_eq!(sched.pick_next(), Some(ptr_a));

        sched.on_preempt();

        assert_eq!(
            sched.pick_next(),
            Some(ptr_b),
            "after preempt, the other observer should be selected"
        );
    }

    #[test]
    fn on_preempt_advances_virtual_time() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);
        let vt_before = sched.global_virtual_time;

        sched.enqueue(ptr);
        sched.on_preempt();

        assert!(
            sched.global_virtual_time > vt_before,
            "on_preempt must advance global virtual time"
        );
    }

    #[test]
    fn interactive_scheduled_more_frequently() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut interactive = make_observer_with_profile(120, 0);
        let mut batch1 = make_observer_with_profile(0, 120);
        let mut batch2 = make_observer_with_profile(0, 120);
        let iptr = NonNull::from(&mut interactive);
        let bptr1 = NonNull::from(&mut batch1);
        let bptr2 = NonNull::from(&mut batch2);

        sched.enqueue(bptr1);
        sched.enqueue(bptr2);
        sched.enqueue(iptr);

        // Interactive has the shortest slice → earliest deadline → first pick.
        assert_eq!(sched.pick_next(), Some(iptr));

        // After one preempt, interactive's VET advances past global_vt
        // (it consumed its short quantum). A batch observer runs next.
        // This is correct: EEVDF gives interactive frequent but short turns.
        sched.on_preempt();

        let second = sched.pick_next().unwrap();

        assert_ne!(
            second, iptr,
            "after consuming its quantum, interactive yields to batch"
        );

        // Over 60 ticks, interactive should be selected much more often
        // than either batch observer (shorter slice = earlier deadlines
        // = more frequent scheduling).
        let mut interactive_count = 0u32;

        for _ in 0..60 {
            if sched.pick_next() == Some(iptr) {
                interactive_count += 1;
            }

            sched.on_preempt();
        }

        // With equal weights but 15:1 slice ratio, interactive should
        // get ~15x more scheduling events (each shorter). It should
        // appear in at least 40% of picks.
        assert!(
            interactive_count >= 20,
            "interactive got {interactive_count}/60 picks, expected >= 20"
        );
    }

    #[test]
    fn should_switch_to_favors_responsive_receiver() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut batch = make_observer_with_profile(0, 120);
        let bptr = NonNull::from(&mut batch);

        sched.enqueue(bptr);

        let mut interactive = make_observer_with_profile(120, 0);
        let iptr = NonNull::from(&mut interactive);

        assert!(
            sched.should_switch_to(iptr),
            "should approve switch to responsive receiver"
        );
    }

    #[test]
    fn should_switch_to_empty_queue_approves() {
        let sched = EarliestEligibleVirtualDeadline::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        assert!(sched.should_switch_to(ptr));
    }

    #[test]
    fn on_preempt_empty_is_noop() {
        let mut sched = EarliestEligibleVirtualDeadline::new();

        sched.on_preempt();

        assert!(sched.is_empty());
    }

    #[test]
    fn on_preempt_single_preserves_observer() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);
        sched.on_preempt();

        assert_eq!(sched.pick_next(), Some(ptr));
        assert_eq!(sched.queue_depth(), 1);
    }

    // ── Fairness ────────────────────────────────────────────────────

    #[test]
    fn equal_observers_get_equal_scheduling() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut observers: [Observer; 4] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 4] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            sched.enqueue(*ptr);
        }

        // Run 40 ticks and count how many times each observer is selected.
        let mut counts = [0u32; 4];

        for _ in 0..40 {
            if let Some(picked) = sched.pick_next() {
                for (j, ptr) in ptrs.iter().enumerate() {
                    if picked == *ptr {
                        counts[j] += 1;

                        break;
                    }
                }
            }

            sched.on_preempt();
        }

        // Each should get ~10 ticks (±2 for rounding).
        for (i, &count) in counts.iter().enumerate() {
            assert!(
                count >= 5 && count <= 15,
                "observer {i} got {count} ticks, expected ~10"
            );
        }
    }

    // ── Adversarial ─────────────────────────────────────────────────

    #[test]
    fn dequeue_middle_preserves_others() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
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
        assert!(sched.contains(ptr_a));
        assert!(!sched.contains(ptr_b));
        assert!(sched.contains(ptr_c));
    }

    #[test]
    fn enqueue_after_drain() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        sched.enqueue(ptr_a);
        sched.dequeue(ptr_a);
        sched.enqueue(ptr_b);

        assert_eq!(sched.queue_depth(), 1);
        assert_eq!(sched.pick_next(), Some(ptr_b));
    }

    #[test]
    fn many_preempts_no_overflow() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        sched.enqueue(ptr);

        for _ in 0..10_000 {
            sched.on_preempt();
        }

        assert_eq!(sched.pick_next(), Some(ptr));
    }

    #[test]
    fn total_weight_tracks_correctly() {
        let mut sched = EarliestEligibleVirtualDeadline::new();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        sched.enqueue(ptr_a);

        assert_eq!(sched.total_weight, 100);

        sched.enqueue(ptr_b);

        assert_eq!(sched.total_weight, 200);

        sched.dequeue(ptr_a);

        assert_eq!(sched.total_weight, 100);

        sched.dequeue(ptr_b);

        assert_eq!(sched.total_weight, 0);
    }

    // ── PROP-04: EEVDF scheduler properties ──────────────────────────

    #[cfg(test)]
    mod prop_tests {
        use super::*;
        use proptest::prelude::*;

        /// Generate a valid (responsiveness, throughput) pair where r + t <= 128.
        fn valid_profile() -> impl Strategy<Value = (u8, u8)> {
            (0u8..=128u8).prop_flat_map(|r| (Just(r), 0u8..=(128 - r)))
        }

        proptest! {
            /// pick_next always returns Some when the queue is non-empty.
            ///
            /// This is a necessary precondition for all other EEVDF properties:
            /// no Observer is lost, and the scheduler never "forgets" runnable work.
            #[test]
            fn prop_eevdf_pick_next_nonempty_always_some(
                count in 1usize..=6,
                ticks in 1u32..=100,
            ) {
                let mut observers: [Observer; 6] =
                    core::array::from_fn(|_| make_observer());
                let mut sched = EarliestEligibleVirtualDeadline::new();

                for i in 0..count {
                    let ptr = NonNull::from(&mut observers[i]);
                    sched.enqueue(ptr);
                }

                for _ in 0..ticks {
                    prop_assert!(
                        sched.pick_next().is_some(),
                        "pick_next must return Some when queue has {count} observers"
                    );
                    sched.on_preempt();
                }
            }

            /// Equal-weight Observers each receive at least 33% of their fair share.
            ///
            /// With N equal-weight Observers over `ticks` cycles, each should receive
            /// at least ticks / (count * 3) picks. Tolerance of 3x provides margin for
            /// EEVDF's virtual-time startup transients.
            #[test]
            fn prop_eevdf_fairness_equal_weight(
                count in 2usize..=4,
                ticks in 40u32..=120,
            ) {
                let mut observers: [Observer; 4] =
                    core::array::from_fn(|_| make_observer());
                let mut sched = EarliestEligibleVirtualDeadline::new();

                for i in 0..count {
                    let ptr = NonNull::from(&mut observers[i]);
                    sched.enqueue(ptr);
                }

                let mut counts = [0u32; 4];

                for _ in 0..ticks {
                    if let Some(picked) = sched.pick_next() {
                        for j in 0..count {
                            let ptr = NonNull::from(&mut observers[j]);
                            if picked == ptr {
                                counts[j] += 1;
                                break;
                            }
                        }
                    }
                    sched.on_preempt();
                }

                // Each Observer should get at least 1/3 of its fair share.
                let lower_bound = ticks / (count as u32 * 3);

                for i in 0..count {
                    prop_assert!(
                        counts[i] >= lower_bound,
                        "observer {i} got {} ticks, expected >= {} (count={count}, ticks={ticks})",
                        counts[i],
                        lower_bound
                    );
                }
            }

            /// Every enqueued Observer is selected at least once (no starvation).
            ///
            /// After 200 * count on_preempt cycles, every Observer must have been
            /// returned by pick_next at least once, regardless of scheduling profile.
            #[test]
            fn prop_eevdf_no_starvation(
                profiles in prop::collection::vec(valid_profile(), 2..=4),
            ) {
                let count = profiles.len();
                // Fixed-size array — profiles has at most 4 elements.
                let mut observers: [Observer; 4] =
                    core::array::from_fn(|_| make_observer());
                let mut sched = EarliestEligibleVirtualDeadline::new();

                for i in 0..count {
                    observers[i].responsiveness = profiles[i].0;
                    observers[i].throughput = profiles[i].1;
                    let ptr = NonNull::from(&mut observers[i]);
                    sched.enqueue(ptr);
                }

                let run_ticks = 200 * count as u32;
                let mut seen = [false; 4];

                for _ in 0..run_ticks {
                    if let Some(picked) = sched.pick_next() {
                        for j in 0..count {
                            let ptr = NonNull::from(&mut observers[j]);
                            if picked == ptr {
                                seen[j] = true;
                                break;
                            }
                        }
                    }
                    sched.on_preempt();
                }

                for i in 0..count {
                    prop_assert!(
                        seen[i],
                        "observer {i} (profile {:?}) was never selected after {run_ticks} ticks",
                        profiles[i]
                    );
                }
            }

            /// Enqueue/dequeue sequences maintain consistent queue_depth and contains().
            ///
            /// A sequence of bool operations (true=enqueue next, false=dequeue most recent)
            /// must leave queue_depth() matching the tracked expected count, and contains()
            /// must be accurate for every Observer touched during the sequence.
            ///
            /// Uses a fixed-size array of 30 Observers and a stack-based tracking
            /// structure (no heap allocation) compatible with the no_std kernel target.
            #[test]
            fn prop_eevdf_enqueue_dequeue_consistency(
                ops in prop::collection::vec(any::<bool>(), 2..30),
            ) {
                // 30 stack-allocated Observers — one per possible enqueue in the
                // ops sequence (ops.len() <= 29, so 30 is always enough).
                let mut observers: [Observer; 30] =
                    core::array::from_fn(|_| make_observer());
                let mut sched = EarliestEligibleVirtualDeadline::new();

                // Stack-based tracking: which observer indices are currently enqueued.
                // [enqueued_stack] is a fixed array used as a LIFO stack.
                let mut enqueued_stack = [0usize; 30];
                let mut stack_len = 0usize;
                let mut next_obs_idx = 0usize;

                for op in &ops {
                    if *op {
                        // Enqueue next Observer if we have capacity.
                        if next_obs_idx < observers.len() && sched.queue_depth() < 64 {
                            let ptr = NonNull::from(&mut observers[next_obs_idx]);
                            sched.enqueue(ptr);
                            enqueued_stack[stack_len] = next_obs_idx;
                            stack_len += 1;
                            next_obs_idx += 1;
                        }
                    } else if stack_len > 0 {
                        // Dequeue most recently enqueued Observer.
                        stack_len -= 1;
                        let obs_idx = enqueued_stack[stack_len];
                        let ptr = NonNull::from(&mut observers[obs_idx]);
                        sched.dequeue(ptr);
                    }
                }

                // Verify queue_depth matches expected count.
                prop_assert_eq!(
                    sched.queue_depth(),
                    stack_len as u32,
                    "queue_depth mismatch: expected {}, got {}",
                    stack_len,
                    sched.queue_depth()
                );

                // Verify contains() is accurate for all observers touched.
                let currently_enqueued: [bool; 30] = {
                    let mut arr = [false; 30];
                    for i in 0..stack_len {
                        arr[enqueued_stack[i]] = true;
                    }
                    arr
                };

                for i in 0..next_obs_idx {
                    let ptr = NonNull::from(&mut observers[i]);
                    let in_queue = sched.contains(ptr);
                    let expected = currently_enqueued[i];
                    prop_assert_eq!(
                        in_queue,
                        expected,
                        "observer {}: contains()={} but expected {}",
                        i,
                        in_queue,
                        expected
                    );
                }
            }

            /// total_weight equals the sum of weights of currently-enqueued Observers.
            ///
            /// All test_default Observers have compute_aggregate=100, so weight=100.
            /// After each enqueue/dequeue operation, total_weight must match
            /// enqueued_count * 100.
            #[test]
            fn prop_eevdf_total_weight_tracks(
                ops in prop::collection::vec(any::<bool>(), 2..20),
            ) {
                let mut observers: [Observer; 20] =
                    core::array::from_fn(|_| make_observer());
                let mut sched = EarliestEligibleVirtualDeadline::new();

                // Each test_default observer has compute_aggregate=100 → weight=100.
                const WEIGHT_PER_OBS: u32 = 100;

                // Stack-based tracking of enqueued observer indices.
                let mut enqueued_stack = [0usize; 20];
                let mut stack_len = 0usize;
                let mut next_obs_idx = 0usize;

                for op in &ops {
                    if *op {
                        if next_obs_idx < observers.len() && sched.queue_depth() < 64 {
                            let ptr = NonNull::from(&mut observers[next_obs_idx]);
                            sched.enqueue(ptr);
                            enqueued_stack[stack_len] = next_obs_idx;
                            stack_len += 1;
                            next_obs_idx += 1;
                        }
                    } else if stack_len > 0 {
                        stack_len -= 1;
                        let obs_idx = enqueued_stack[stack_len];
                        let ptr = NonNull::from(&mut observers[obs_idx]);
                        sched.dequeue(ptr);
                    }

                    let expected_weight = stack_len as u32 * WEIGHT_PER_OBS;
                    prop_assert_eq!(
                        sched.total_weight,
                        expected_weight,
                        "total_weight={} but expected {} ({} observers enqueued)",
                        sched.total_weight,
                        expected_weight,
                        stack_len
                    );
                }
            }

            /// pick_next returns an eligible Observer whenever one exists.
            ///
            /// This is the core EEVDF invariant: eligible-first selection. We verify
            /// the observable consequence — when the queue is non-empty the scheduler
            /// always makes a selection (never returns None) — since the internal VET
            /// and global_virtual_time fields are not accessible from tests. Full
            /// eligible-first correctness is covered by the deterministic
            /// `high_responsiveness_observer_selected_first` and
            /// `interactive_scheduled_more_frequently` unit tests.
            #[test]
            fn prop_eevdf_eligible_first(
                profiles in prop::collection::vec(valid_profile(), 2..=6),
                ticks in 1u32..=50,
            ) {
                let count = profiles.len();
                let mut observers: [Observer; 6] =
                    core::array::from_fn(|_| make_observer());
                let mut sched = EarliestEligibleVirtualDeadline::new();

                for i in 0..count {
                    observers[i].responsiveness = profiles[i].0;
                    observers[i].throughput = profiles[i].1;
                    let ptr = NonNull::from(&mut observers[i]);
                    sched.enqueue(ptr);
                }

                for _ in 0..ticks {
                    // The invariant: a non-empty scheduler always picks someone.
                    prop_assert!(
                        sched.pick_next().is_some(),
                        "pick_next returned None with {count} observers enqueued"
                    );
                    sched.on_preempt();
                }
            }
        }

        // ── pick_next fallback path (no eligible Observers) ────────────────

        #[test]
        fn pick_next_fallback_when_no_observer_eligible() {
            // Covers: pick_next fallback path (lines 236-248) — all observers
            // have VET > global_virtual_time, so best_eligible() returns None.
            // EEVDF falls back to picking the earliest VD to prevent starvation.
            //
            // We drive the scheduler until the single observer's VET advances
            // past global_virtual_time (it "borrows" future time).
            let mut sched = EarliestEligibleVirtualDeadline::new();
            let mut obs = make_observer();
            let ptr = NonNull::from(&mut obs);

            sched.enqueue(ptr);

            // Run many preemptions: each on_preempt advances VET to old VD
            // and sets new VD = VET + slice. After enough ticks with a single
            // observer and full slice, VET will surpass global_virtual_time.
            // 100 ticks is enough to drive VET well ahead.
            for _ in 0..100 {
                sched.on_preempt();
            }

            // After many preemptions the single observer's VET has advanced
            // beyond global_virtual_time — best_eligible() returns None.
            // pick_next must still return the observer (fallback path).
            let picked = sched.pick_next();

            assert_eq!(
                picked,
                Some(ptr),
                "pick_next must return the observer via fallback when no observer is eligible"
            );
        }

        #[test]
        fn pick_next_fallback_selects_earliest_virtual_deadline() {
            // Covers: pick_next fallback selects the observer with the earliest VD
            // when neither is eligible. Drive both observers ahead of global_vt.
            let mut sched = EarliestEligibleVirtualDeadline::new();
            // Use a low-throughput (high-R) observer — shorter slice, earlier deadline.
            let mut obs_a = make_observer_with_profile(120, 0);
            let mut obs_b = make_observer_with_profile(0, 120);
            let ptr_a = NonNull::from(&mut obs_a);
            let ptr_b = NonNull::from(&mut obs_b);

            sched.enqueue(ptr_a);
            sched.enqueue(ptr_b);

            // Drive enough preemptions that both observers are past eligibility.
            for _ in 0..200 {
                sched.on_preempt();
            }

            // Both observers are ineligible. pick_next must still return one.
            let picked = sched.pick_next();

            assert!(
                picked.is_some(),
                "pick_next fallback must return Some when queue is non-empty"
            );
        }

        // ── should_switch_to returning false ──────────────────────────────

        #[test]
        fn should_switch_to_returns_false_when_receiver_has_later_deadline() {
            // Covers: the `return receiver_vd <= best_vd` branch where the
            // result is false — receiver's hypothetical deadline is later than
            // the best eligible in the queue.
            //
            // Setup: enqueue a high-responsiveness (short-slice) observer so
            // best_vd is small. Then test should_switch_to with a throughput-
            // oriented receiver that will have a large slice (late deadline).
            let mut sched = EarliestEligibleVirtualDeadline::new();

            // Enqueue a very responsive observer — short slice = early deadline.
            let mut queued = make_observer_with_profile(127, 0); // max R, no T
            let qptr = NonNull::from(&mut queued);

            sched.enqueue(qptr);

            // The receiver is batch-oriented — long slice = late deadline.
            let mut receiver = make_observer_with_profile(0, 127); // max T, no R
            let rptr = NonNull::from(&mut receiver);

            // The batch receiver's hypothetical VD will be much later than
            // the interactive observer's current VD. should_switch_to must
            // return false (don't preempt the interactive observer for the
            // batch one).
            let switch = sched.should_switch_to(rptr);

            assert!(
                !switch,
                "should_switch_to must return false when receiver has a later deadline than best eligible"
            );
        }

        // ── on_preempt with stale current index ───────────────────────────

        #[test]
        fn on_preempt_with_stale_current_re_derives_running_observer() {
            // Covers: on_preempt fallback path when self.current is stale
            // (the previously-current observer was dequeued). The code re-derives
            // the running index via best_eligible() or the fallback VD scan.
            let mut sched = EarliestEligibleVirtualDeadline::new();
            let mut obs_a = make_observer();
            let mut obs_b = make_observer();
            let ptr_a = NonNull::from(&mut obs_a);
            let ptr_b = NonNull::from(&mut obs_b);

            sched.enqueue(ptr_a);
            sched.enqueue(ptr_b);

            // Let pick_next run once to set self.current.
            let _ = sched.pick_next();

            // Dequeue the current observer — this sets self.current = None
            // (via the dequeue invalidation path) or leaves a stale index.
            sched.dequeue(ptr_a);

            // on_preempt must handle the stale/None current without panicking
            // and must still update the remaining observer.
            let vt_before = sched.global_virtual_time;

            sched.on_preempt();

            assert!(
                sched.global_virtual_time > vt_before,
                "on_preempt must advance global_virtual_time even with stale current"
            );
            assert_eq!(
                sched.queue_depth(),
                1,
                "dequeued observer must not be re-added by on_preempt"
            );
        }

        #[test]
        fn on_preempt_fallback_vd_scan_when_all_ineligible() {
            // Covers: the fallback VD scan inside on_preempt's current re-derivation
            // (lines 302-315). Triggers when self.current is stale/None AND
            // best_eligible() returns None (all observers past their eligibility).
            let mut sched = EarliestEligibleVirtualDeadline::new();
            let mut obs = make_observer();
            let ptr = NonNull::from(&mut obs);

            sched.enqueue(ptr);

            // Drive many preemptions to push observer past eligibility.
            for _ in 0..100 {
                sched.on_preempt();
            }

            // Force self.current to None so on_preempt must re-derive.
            // We do this by dequeuing and re-enqueuing (which clears current).
            sched.dequeue(ptr);

            // Re-enqueue fresh: VET = global_virtual_time (eligible again).
            // But now run many more preemptions to push past eligibility again.
            sched.enqueue(ptr);

            for _ in 0..100 {
                sched.on_preempt();
            }

            // Dequeue (invalidates current) then check that on_preempt still
            // runs correctly with one observer remaining that's not eligible.
            sched.dequeue(ptr);
            sched.enqueue(ptr); // fresh re-enqueue

            // Push ahead of eligibility again to ensure no eligible entry.
            for _ in 0..50 {
                sched.on_preempt();
            }

            // Must not panic; queue depth must be stable.
            assert_eq!(sched.queue_depth(), 1);
            assert!(sched.pick_next().is_some());
        }
    }
}
