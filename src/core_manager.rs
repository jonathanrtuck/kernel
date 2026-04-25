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
pub fn current_core<S: Scheduler>() -> &'static CoreState<S> {
    todo!()
}

/// Mutable access to the current core's state.
///
/// Same as `current_core` but returns a mutable reference. Safe
/// because A4 guarantees the kernel is non-reentrant on a single
/// core — only one exception handler runs at a time, so there can
/// be no aliasing.
pub fn current_core_mut<S: Scheduler>() -> &'static mut CoreState<S> {
    todo!()
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
    pub fn dispatch_ipc(&mut self) -> DispatchResult {
        todo!()
    }

    /// Handle a typed kernel operation (SVC #0, code in x4).
    ///
    /// D7: typed ops are the other syscall family. The core manager
    /// resolves the target cap, verifies type and rights (D4/D52),
    /// then dispatches to the type-specific operation.
    ///
    /// D49: 20 operations in dense table dispatch (codes 0–19).
    pub fn dispatch_typed(&mut self) -> DispatchResult {
        todo!()
    }

    /// Handle a timer interrupt (preemption tick).
    ///
    /// D2: calls `scheduler.on_preempt()` for accounting, then
    /// `scheduler.pick_next()` to decide whether to switch.
    /// D44: also checks pending Pulsar deadlines.
    pub fn handle_timer(&mut self) -> DispatchResult {
        todo!()
    }

    /// Handle a device interrupt routed to this core.
    ///
    /// D22: the kernel reads the GIC IAR, masks the interrupt,
    /// constructs a message with a per-IRQ badge and send-once ack cap
    /// (D16), and enqueues to the registered driver Field. The driver
    /// acks by using the send-once cap, which unmasks the interrupt.
    pub fn handle_irq(&mut self, _irq: u32) -> DispatchResult {
        todo!()
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
