//! Scheduler and placement traits.
//!
//! D2:  per-core schedulers may run different algorithms.
//! D50: scheduler callback for IPC fast-path approval.
//! D56: scored placement with profile matching.
//! D59: two traits — Scheduler (per-core, 5 methods) and Placement
//!      (cross-core). Separate because D1 (per-core hot path) conflicts
//!      with D56's placement function reading cross-core state.

use crate::observer::Observer;
use core::ptr::NonNull;

/// Core identifier. Not exposed to Observers (D46).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CoreId(pub u16);

/// Snapshot of per-core state for placement decisions (D56, D59).
///
/// Populated once before scoring to avoid cache-line bouncing.
pub struct CoreSnapshot {
    pub core_id: CoreId,
    pub idle: bool,
    pub queue_depth: u32,
    pub capacity_factor: u32,
}

/// Placement decision (D56, D59).
pub enum PlacementDecision {
    /// Schedule on the current core (hot path, no IPI).
    Local,
    /// Schedule on a remote core (cold path, mailbox + IPI).
    Remote(CoreId),
}

/// Per-core scheduler trait (D59).
///
/// Each core owns one implementation. Algorithm-agnostic — D2 allows
/// different algorithms per core (throughput on big, fixed-priority on
/// LITTLE, deadline on RT-dedicated).
///
/// Lock discipline (D53, D59):
/// - `enqueue`/`dequeue`: called while holding Arena<Observer> lock.
/// - `pick_next`, `should_switch_to`, `on_preempt`: called WITHOUT locks.
pub trait Scheduler {
    /// Observer joins the run queue.
    fn enqueue(&mut self, observer: NonNull<Observer>);

    /// Observer leaves the run queue.
    fn dequeue(&mut self, observer: NonNull<Observer>);

    /// Select next Observer to resume. None → WFI (D46).
    fn pick_next(&self) -> Option<NonNull<Observer>>;

    /// IPC fast-path predicate (D50). Read-only, ≤50 cycle budget.
    fn should_switch_to(&self, receiver: NonNull<Observer>) -> bool;

    /// Timer tick accounting.
    fn on_preempt(&mut self);
}

/// Cross-core placement trait (D56, D59).
///
/// One instance per system, not per-core. D56's scored placement
/// requires comparing idle status, queue depth, and profile
/// compatibility across all candidate cores. Snapshots are populated
/// once before scoring to avoid cache-line bouncing (D59).
pub trait Placement {
    fn place(&self, observer: &Observer, snapshots: &[CoreSnapshot]) -> PlacementDecision;
}
