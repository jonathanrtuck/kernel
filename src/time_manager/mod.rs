//! Time manager: scheduling traits, placement, and compute allocation.
//!
//! Named after the graph.d2 component nested inside each core manager.
//! The time manager owns per-core scheduling: algorithm selection,
//! run queue management, and placement decisions.
//!
//! D2:  per-core schedulers may run different algorithms.
//! D29: Time is a capability-held kernel object (managed here).
//! D50: scheduler callback for IPC fast-path approval.
//! D56: scored placement with profile matching.
//! D59: two traits — Scheduler (per-core, 5 methods) and Placement
//!      (cross-core). Separate because D1 (per-core hot path) conflicts
//!      with D56's placement function reading cross-core state.
//!
//! Algorithm implementations live in sibling files within this module
//! (e.g., `round_robin.rs`, `edf.rs`). The traits here are the
//! interface; implementations are leaf nodes (philosophy: push
//! complexity to the leaves).

use crate::observer::Observer;
use core::ptr::NonNull;

/// Core identifier. Not exposed to Observers (D46).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

pub mod earliest_eligible_virtual_deadline;
pub mod round_robin;
pub mod scored_placement;

/// Runtime-selectable scheduling algorithm (D2).
///
/// Wraps all Scheduler implementations in an enum so each core can
/// run a different algorithm and the Governor can swap algorithms at
/// runtime. Adding a new algorithm means adding a variant here and
/// forwarding the trait methods.
#[allow(clippy::large_enum_variant)]
pub enum SchedulerAlgorithm {
    RoundRobin(round_robin::RoundRobin),
    Eevdf(earliest_eligible_virtual_deadline::EarliestEligibleVirtualDeadline),
}

impl SchedulerAlgorithm {
    pub const fn round_robin() -> Self {
        SchedulerAlgorithm::RoundRobin(round_robin::RoundRobin::new())
    }

    pub const fn eevdf() -> Self {
        SchedulerAlgorithm::Eevdf(
            earliest_eligible_virtual_deadline::EarliestEligibleVirtualDeadline::new(),
        )
    }

    pub fn queue_depth(&self) -> u32 {
        match self {
            SchedulerAlgorithm::RoundRobin(rr) => rr.queue_depth(),
            SchedulerAlgorithm::Eevdf(eevdf) => eevdf.queue_depth(),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            SchedulerAlgorithm::RoundRobin(rr) => rr.is_empty(),
            SchedulerAlgorithm::Eevdf(eevdf) => eevdf.is_empty(),
        }
    }

    pub fn contains(&self, observer: NonNull<Observer>) -> bool {
        match self {
            SchedulerAlgorithm::RoundRobin(rr) => rr.contains(observer),
            SchedulerAlgorithm::Eevdf(eevdf) => eevdf.contains(observer),
        }
    }
}

impl Scheduler for SchedulerAlgorithm {
    fn enqueue(&mut self, observer: NonNull<Observer>) {
        match self {
            SchedulerAlgorithm::RoundRobin(rr) => rr.enqueue(observer),
            SchedulerAlgorithm::Eevdf(eevdf) => eevdf.enqueue(observer),
        }
    }

    fn dequeue(&mut self, observer: NonNull<Observer>) {
        match self {
            SchedulerAlgorithm::RoundRobin(rr) => rr.dequeue(observer),
            SchedulerAlgorithm::Eevdf(eevdf) => eevdf.dequeue(observer),
        }
    }

    fn pick_next(&self) -> Option<NonNull<Observer>> {
        match self {
            SchedulerAlgorithm::RoundRobin(rr) => rr.pick_next(),
            SchedulerAlgorithm::Eevdf(eevdf) => eevdf.pick_next(),
        }
    }

    fn should_switch_to(&self, receiver: NonNull<Observer>) -> bool {
        match self {
            SchedulerAlgorithm::RoundRobin(rr) => rr.should_switch_to(receiver),
            SchedulerAlgorithm::Eevdf(eevdf) => eevdf.should_switch_to(receiver),
        }
    }

    fn on_preempt(&mut self) {
        match self {
            SchedulerAlgorithm::RoundRobin(rr) => rr.on_preempt(),
            SchedulerAlgorithm::Eevdf(eevdf) => eevdf.on_preempt(),
        }
    }
}
