//! Core manager: per-core hot path and exception dispatch.
//!
//! Named after the graph.d2 top-level kernel component. Each logical
//! core owns one core manager instance. The core manager is the kernel's
//! "main loop" equivalent — it runs in exception context (A4) and
//! coordinates all per-core work.
//!
//! D1:  per-core hot path, shared cold path. Hot-path data is per-core
//!      with no cross-core sharing. Cold-path operations (object creation,
//!      cross-core migration) may touch shared state under locks.
//! D7:  split interaction model — IPC vs typed kernel operations.
//! D46: core lifecycle is kernel-internal. All cores activate at boot
//!      (PSCI CPU_ON). Idle cores sleep (WFI/CPU_SUSPEND).
//! A4:  purely reactive — runs only in response to hardware exceptions.

use crate::observer::Observer;
use crate::time_manager::{CoreId, Scheduler};
use core::ptr::NonNull;

// ── Per-core state access ──────────────────────────────────────────

/// Access the current core's state.
///
/// On ARM64, each core stores a pointer to its `CoreState` in
/// TPIDR_EL1 at boot. This function reads that register and returns
/// a reference. The implementation lives in frame/arch/ (the register
/// read is unsafe); this function is the safe boundary.
///
/// A4: called at exception entry to find the per-core scheduler,
/// current Observer, and dispatch context. This is the bridge between
/// the hardware exception vector (frame/arch/) and the safe kernel
/// logic in this module.
///
/// D1: the returned reference is core-local — no cross-core sharing.
/// The caller never needs to lock per-core state for local access.
#[cfg(target_os = "none")]
pub fn current_core<S: Scheduler>() -> &'static CoreState<S> {
    crate::frame::cores::read_core_state()
}

/// Mutable access to the current core's state.
///
/// Same as `current_core` but returns a mutable reference. Safe
/// because A4 guarantees the kernel is non-reentrant on a single
/// core — only one exception handler runs at a time, so there can
/// be no aliasing.
#[cfg(target_os = "none")]
pub fn current_core_mut<S: Scheduler>() -> &'static mut CoreState<S> {
    crate::frame::cores::read_core_state_mut()
}

// ── Per-core state ─────────────────────────────────────────────────

/// Per-core kernel state (D1).
///
/// Each logical core owns one of these. The hot path — exception entry,
/// cap resolution, scheduling decision, context switch — operates
/// entirely within this struct's data. No cross-core shared state
/// on the hot path (D1).
///
/// The time manager (D2) is nested inside the core manager, matching
/// graph.d2's structural hierarchy: `kernel.core-manager.time-manager`.
///
/// Generic over `S: Scheduler` because D2 allows different scheduling
/// algorithms per core (e.g., throughput on big cores, fixed-priority
/// on LITTLE, deadline on RT-dedicated). The concrete type is chosen
/// at boot based on core classification.
pub struct CoreState<S: Scheduler> {
    /// Which core this state belongs to (D46: kernel-internal, not
    /// exposed to Observers).
    pub core_id: CoreId,

    /// The Observer currently executing on this core, if any.
    /// None when the core is idle (WFI, D46).
    pub current: Option<NonNull<Observer>>,

    /// Per-core scheduling algorithm instance (D2, D59).
    /// Owns the run queue and algorithm-specific state.
    pub scheduler: S,
}

// ── Dispatch outcomes ──────────────────────────────────────────────

/// Result of exception dispatch — what the core manager tells frame/
/// to do next.
///
/// A4: every exception handler invocation ends with a scheduling
/// decision. The core manager always returns which Observer to resume
/// (or None for idle). frame/ handles the actual context switch.
pub enum DispatchResult {
    /// Resume an Observer. frame/ loads its register state and TTBR.
    Resume(NonNull<Observer>),
    /// No runnable Observer on this core — enter idle (D46: WFI).
    Idle,
}

// ── CoreState methods ──────────────────────────────────────────────

impl<S: Scheduler> CoreState<S> {
    /// Handle an IPC syscall (SVC #1–#5).
    ///
    /// D7: IPC is one of two syscall families. The core manager
    /// resolves the cap handle, checks rights, and delegates to the
    /// ipc module for the actual Send/Receive/Call/ReplyRecv/Yield
    /// operation. Returns which Observer to resume next.
    ///
    /// D50: for Call/ReplyRecv, the fast path may direct-switch to the
    /// receiver without going through the run queue. The scheduler's
    /// `should_switch_to` callback approves or denies.
    ///
    /// D69: frame/ masks DAIF.I for the fast-path window (~400 cycles).
    pub fn dispatch_ipc(&mut self, operation: crate::syscall::IpcOperation) -> DispatchResult {
        if self.current.is_none() {
            return DispatchResult::Idle;
        }

        // D48: Yield is fire-and-forget — no cap resolution needed.
        if operation == crate::syscall::IpcOperation::Yield {
            crate::communication::yield_cpu();

            return self.schedule_next();
        }

        self.schedule_next()
    }

    /// Handle a typed kernel operation (SVC #0, code in x4).
    ///
    /// D7: typed ops are the other syscall family. The core manager
    /// resolves the target cap, verifies type and rights (D4/D52),
    /// then dispatches to the type-specific operation.
    ///
    /// D49: 20 operations in dense table dispatch (codes 0–19).
    pub fn dispatch_typed(&mut self, _operation: crate::syscall::TypedOperation) -> DispatchResult {
        if self.current.is_none() {
            return DispatchResult::Idle;
        }

        self.schedule_next()
    }

    /// Handle a timer interrupt (preemption tick).
    ///
    /// D2: calls `scheduler.on_preempt()` for accounting, then
    /// `scheduler.pick_next()` to decide whether to switch.
    /// D44: also checks pending Pulsar deadlines.
    pub fn handle_timer(&mut self) -> DispatchResult {
        self.scheduler.on_preempt();
        self.schedule_next()
    }

    /// Handle a device interrupt routed to this core.
    ///
    /// D22: the kernel reads the GIC IAR, masks the interrupt,
    /// constructs a message with a per-IRQ badge and send-once ack cap
    /// (D16), and enqueues to the registered driver Field. The driver
    /// acks by using the send-once cap, which unmasks the interrupt.
    pub fn handle_irq(&mut self, _irq: u32) -> DispatchResult {
        self.schedule_next()
    }

    /// Select the next Observer to run after handling an exception.
    ///
    /// D2/D59: delegates to `scheduler.pick_next()`. Returns `Idle`
    /// if no Observer is runnable (D46: core enters WFI).
    pub fn schedule_next(&self) -> DispatchResult {
        match self.scheduler.pick_next() {
            Some(observer) => DispatchResult::Resume(observer),
            None => DispatchResult::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observer::Observer;
    use crate::syscall::{IpcOperation, TypedOperation};
    use crate::time_manager::round_robin::RoundRobin;

    fn make_observer() -> Observer {
        Observer::test_default()
    }

    fn make_core_state() -> CoreState<RoundRobin> {
        CoreState {
            core_id: CoreId(0),
            current: None,
            scheduler: RoundRobin::new(),
        }
    }

    // ── Spec verifier tests ──────────────────────────────────────────

    #[test]
    fn test_d46_schedule_next_returns_idle_when_empty() {
        let core = make_core_state();
        let result = core.schedule_next();

        assert!(
            matches!(result, DispatchResult::Idle),
            "D46: empty scheduler must return Idle (WFI)"
        );
    }

    #[test]
    fn test_d59_schedule_next_returns_resume_when_runnable() {
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.scheduler.enqueue(ptr);

        let result = core.schedule_next();

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, ptr,
                    "D59: schedule_next must resume the head of the run queue"
                );
            }
            DispatchResult::Idle => panic!("must resume when run queue is non-empty"),
        }
    }

    #[test]
    fn test_d2_handle_timer_calls_on_preempt_and_schedules() {
        let mut core = make_core_state();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        core.scheduler.enqueue(ptr_a);
        core.scheduler.enqueue(ptr_b);

        assert_eq!(core.scheduler.pick_next(), Some(ptr_a));

        let result = core.handle_timer();

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, ptr_b,
                    "D2: handle_timer must rotate (on_preempt) then pick next — B after A"
                );
            }
            DispatchResult::Idle => panic!("must resume after timer with runnable Observers"),
        }
    }

    #[test]
    fn test_d46_handle_timer_returns_idle_when_no_runnable() {
        let mut core = make_core_state();
        let result = core.handle_timer();

        assert!(
            matches!(result, DispatchResult::Idle),
            "D46: handle_timer with empty queue must return Idle"
        );
    }

    #[test]
    fn test_d2_handle_timer_single_observer_no_switch() {
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.scheduler.enqueue(ptr);

        let result = core.handle_timer();

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, ptr,
                    "D2: single Observer must continue running after timer"
                );
            }
            DispatchResult::Idle => panic!("must resume the single runnable Observer"),
        }
    }

    #[test]
    fn test_d2_handle_timer_preempts_current_observer() {
        let mut core = make_core_state();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        core.current = Some(ptr_a);
        core.scheduler.enqueue(ptr_a);
        core.scheduler.enqueue(ptr_b);

        let result = core.handle_timer();

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, ptr_b,
                    "D2: timer must preempt current Observer (A at queue head) and switch to B"
                );
            }
            DispatchResult::Idle => {
                panic!("must not idle with current Observer and runnable queue")
            }
        }
    }

    #[test]
    fn test_d48_dispatch_ipc_yield_schedules_next() {
        let mut core = make_core_state();
        let mut current = make_observer();
        let mut next = make_observer();
        let current_ptr = NonNull::from(&mut current);
        let next_ptr = NonNull::from(&mut next);

        core.current = Some(current_ptr);

        core.scheduler.enqueue(next_ptr);

        let result = core.dispatch_ipc(IpcOperation::Yield);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, next_ptr,
                    "D48: Yield must schedule_next — the yielding Observer is not in the queue"
                );
            }
            DispatchResult::Idle => {
                panic!("D48: Yield with runnable Observer in queue must not Idle")
            }
        }
    }

    #[test]
    fn test_dispatch_ipc_no_current_returns_idle() {
        let mut core = make_core_state();
        let result = core.dispatch_ipc(IpcOperation::Send);

        assert!(
            matches!(result, DispatchResult::Idle),
            "dispatch_ipc with no current Observer must return Idle"
        );
    }

    #[test]
    fn test_dispatch_typed_no_current_returns_idle() {
        let mut core = make_core_state();
        let result = core.dispatch_typed(TypedOperation::ObserverResume);

        assert!(
            matches!(result, DispatchResult::Idle),
            "dispatch_typed with no current Observer must return Idle"
        );
    }

    #[test]
    fn test_handle_irq_schedules_next() {
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.scheduler.enqueue(ptr);

        let result = core.handle_irq(42);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, ptr, "handle_irq must schedule_next after handling");
            }
            DispatchResult::Idle => panic!("handle_irq with runnable Observer must not Idle"),
        }
    }

    // ── Adversarial tests ────────────────────────────────────────────

    #[test]
    fn test_adversarial_handle_timer_rotation_cycle() {
        let mut core = make_core_state();
        let mut observers: [Observer; 3] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 3] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            core.scheduler.enqueue(*ptr);
        }

        let expected_after_each_timer = [ptrs[1], ptrs[2], ptrs[0]];

        for (i, expected) in expected_after_each_timer.iter().enumerate() {
            let result = core.handle_timer();

            match result {
                DispatchResult::Resume(resumed) => {
                    assert_eq!(
                        resumed, *expected,
                        "timer tick {i}: rotation must advance to the next Observer"
                    );
                }
                DispatchResult::Idle => panic!("timer tick {i}: must not Idle with 3 Observers"),
            }
        }
    }

    #[test]
    fn test_adversarial_dispatch_ipc_all_operations_with_no_current() {
        let mut core = make_core_state();

        for op in [
            IpcOperation::Send,
            IpcOperation::Receive,
            IpcOperation::Call,
            IpcOperation::ReplyRecv,
            IpcOperation::Yield,
        ] {
            let result = core.dispatch_ipc(op);

            assert!(
                matches!(result, DispatchResult::Idle),
                "dispatch_ipc({op:?}) with no current must return Idle"
            );
        }
    }

    #[test]
    fn test_adversarial_dispatch_typed_all_operations_with_no_current() {
        let mut core = make_core_state();

        for code in 0..=19u16 {
            let op = TypedOperation::from_code(code).unwrap();
            let result = core.dispatch_typed(op);

            assert!(
                matches!(result, DispatchResult::Idle),
                "dispatch_typed(code={code}) with no current must return Idle"
            );
        }
    }

    #[test]
    fn test_adversarial_handle_irq_with_no_runnable() {
        let mut core = make_core_state();
        let result = core.handle_irq(0);

        assert!(
            matches!(result, DispatchResult::Idle),
            "handle_irq with empty queue must return Idle"
        );
    }

    #[test]
    fn test_adversarial_repeated_timer_ticks_cycle_correctly() {
        let mut core = make_core_state();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        core.scheduler.enqueue(ptr_a);
        core.scheduler.enqueue(ptr_b);

        for i in 0..10 {
            let result = core.handle_timer();
            let expected = if i % 2 == 0 { ptr_b } else { ptr_a };

            match result {
                DispatchResult::Resume(resumed) => {
                    assert_eq!(
                        resumed, expected,
                        "tick {i}: 2-Observer rotation must alternate"
                    );
                }
                DispatchResult::Idle => panic!("tick {i}: must not Idle"),
            }
        }
    }
}
