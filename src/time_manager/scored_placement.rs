//! Scored placement: cross-core Observer assignment.
//!
//! Implements the `Placement` trait (D59) with a simple scoring function
//! that evaluates candidate cores by idle status, queue depth, and
//! capacity factor. Convention: `snapshots[0]` is the local core.
//!
//! D56: scored placement with profile matching. This initial
//!      implementation uses idle/queue/capacity scoring. Cache affinity
//!      with decay and dynamic core-type reclassification are deferred
//!      tuning concerns — the trait interface supports adding them
//!      without changing callers.
//! D59: concrete implementation of the Placement trait.

use crate::observer::Observer;
use crate::time_manager::{CoreSnapshot, Placement, PlacementDecision};

/// Scored placement algorithm (D56, D59).
///
/// One instance per system, not per-core. Evaluates CoreSnapshots
/// populated once before scoring to avoid cache-line bouncing (D59).
///
/// Scoring weights are leaf-node tuning decisions. The initial weights
/// prioritize idle cores strongly (avoid waking a sleeping core's
/// cache by piling work on a busy one), then prefer lower queue depth,
/// then higher capacity factor.
pub struct ScoredPlacement;

impl Default for ScoredPlacement {
    fn default() -> Self {
        Self::new()
    }
}

impl ScoredPlacement {
    pub const fn new() -> Self {
        ScoredPlacement
    }
}

impl Placement for ScoredPlacement {
    /// Score candidate cores and return the best placement (D56, D59).
    ///
    /// Convention: `snapshots[0]` is the local core (the core executing
    /// this placement decision). If the local core wins or ties, returns
    /// `Local` (hot path, no IPI). Otherwise returns `Remote(core_id)`
    /// (cold path, mailbox + IPI per D56).
    ///
    /// D42: the Observer's scheduling profile (R/T/P) could inform
    /// core-type matching. Deferred to tuning — the initial implementation
    /// does not read the profile.
    fn place(&self, _observer: &Observer, snapshots: &[CoreSnapshot]) -> PlacementDecision {
        if snapshots.len() <= 1 {
            return PlacementDecision::Local;
        }

        let local_score = score_core(&snapshots[0]);
        let mut best_score = local_score;
        let mut best_idx: usize = 0;

        for (i, snapshot) in snapshots.iter().enumerate().skip(1) {
            let s = score_core(snapshot);

            if s > best_score {
                best_score = s;
                best_idx = i;
            }
        }

        if best_idx == 0 {
            PlacementDecision::Local
        } else {
            PlacementDecision::Remote(snapshots[best_idx].core_id)
        }
    }
}

/// Score a single core for placement suitability.
///
/// Higher score = more suitable. The scoring function is a leaf-node
/// tuning decision (D56: "weights are tunable, and the implementation
/// is a leaf node swappable without affecting the rest of the kernel").
///
/// Current weights:
/// - Idle:           +10000 (strongly prefer idle cores — avoids cache
///   pollution on busy cores and lets the woken core
///   have the cache to itself)
/// - Queue depth:    -100 per queued Observer (spread load)
/// - Capacity factor: +1 per unit (prefer cores with more capacity;
///   D36 normalized compute units)
fn score_core(snapshot: &CoreSnapshot) -> i64 {
    (if snapshot.idle { 10000i64 } else { 0 }) - snapshot.queue_depth as i64 * 100
        + snapshot.capacity_factor as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observer::Observer;
    use crate::time_manager::CoreId;

    fn make_observer() -> Observer {
        Observer::test_default()
    }

    fn make_snapshot(
        core_id: u16,
        idle: bool,
        queue_depth: u32,
        capacity_factor: u32,
    ) -> CoreSnapshot {
        CoreSnapshot {
            core_id: CoreId(core_id),
            idle,
            queue_depth,
            capacity_factor,
        }
    }

    // ── Spec verifier tests (D56, D59 derivation claims) ────────────

    #[test]
    fn test_d56_empty_snapshots_returns_local() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let result = placement.place(&obs, &[]);

        assert!(
            matches!(result, PlacementDecision::Local),
            "D56: empty snapshots must return Local"
        );
    }

    #[test]
    fn test_d56_single_core_returns_local() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [make_snapshot(0, false, 2, 100)];
        let result = placement.place(&obs, &snapshots);

        assert!(
            matches!(result, PlacementDecision::Local),
            "D56: single core must return Local"
        );
    }

    #[test]
    fn test_d56_idle_core_preferred_over_busy() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 0, 100), // local: busy
            make_snapshot(1, true, 0, 100),  // remote: idle
        ];
        let result = placement.place(&obs, &snapshots);

        match result {
            PlacementDecision::Remote(core_id) => {
                assert_eq!(
                    core_id,
                    CoreId(1),
                    "D56: idle remote core must be preferred"
                );
            }
            PlacementDecision::Local => {
                panic!("D56: must prefer idle remote over busy local");
            }
        }
    }

    #[test]
    fn test_d56_lower_queue_depth_preferred() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 5, 100), // local: depth 5
            make_snapshot(1, false, 1, 100), // remote: depth 1
        ];
        let result = placement.place(&obs, &snapshots);

        match result {
            PlacementDecision::Remote(core_id) => {
                assert_eq!(
                    core_id,
                    CoreId(1),
                    "D56: lower queue depth must be preferred"
                );
            }
            PlacementDecision::Local => {
                panic!("D56: must prefer lower queue depth remote over high-depth local");
            }
        }
    }

    #[test]
    fn test_d56_local_wins_tie() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 2, 100), // local
            make_snapshot(1, false, 2, 100), // remote: identical
        ];
        let result = placement.place(&obs, &snapshots);

        assert!(
            matches!(result, PlacementDecision::Local),
            "D56: local must win ties (hot path, no IPI)"
        );
    }

    #[test]
    fn test_d56_local_wins_when_best() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, true, 0, 100),  // local: idle, empty
            make_snapshot(1, false, 3, 100), // remote: busy, deep
        ];
        let result = placement.place(&obs, &snapshots);

        assert!(
            matches!(result, PlacementDecision::Local),
            "D56: local must win when it has the best score"
        );
    }

    #[test]
    fn test_d56_capacity_factor_tiebreaker() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 2, 50),  // local: low capacity
            make_snapshot(1, false, 2, 200), // remote: high capacity
        ];
        let result = placement.place(&obs, &snapshots);

        match result {
            PlacementDecision::Remote(core_id) => {
                assert_eq!(
                    core_id,
                    CoreId(1),
                    "D56: higher capacity factor must break ties"
                );
            }
            PlacementDecision::Local => {
                panic!("D56: remote with higher capacity must win when queue depth is equal");
            }
        }
    }

    #[test]
    fn test_d59_placement_returns_correct_core_id() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 10, 100), // local: very deep
            make_snapshot(1, false, 8, 100),
            make_snapshot(2, true, 0, 100), // best: idle
            make_snapshot(3, false, 3, 100),
        ];
        let result = placement.place(&obs, &snapshots);

        match result {
            PlacementDecision::Remote(core_id) => {
                assert_eq!(
                    core_id,
                    CoreId(2),
                    "D59: must return the core_id of the best-scoring core"
                );
            }
            PlacementDecision::Local => {
                panic!("D59: idle remote core 2 must beat busy local core 0");
            }
        }
    }

    // ── Adversarial tests ────────────────────────────────────────────

    #[test]
    fn test_adversarial_all_idle_prefers_local() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, true, 0, 100),
            make_snapshot(1, true, 0, 100),
            make_snapshot(2, true, 0, 100),
        ];
        let result = placement.place(&obs, &snapshots);

        assert!(
            matches!(result, PlacementDecision::Local),
            "all-idle tie must prefer Local"
        );
    }

    #[test]
    fn test_adversarial_all_busy_equal_prefers_local() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 5, 100),
            make_snapshot(1, false, 5, 100),
            make_snapshot(2, false, 5, 100),
        ];
        let result = placement.place(&obs, &snapshots);

        assert!(
            matches!(result, PlacementDecision::Local),
            "all-equal busy must prefer Local"
        );
    }

    #[test]
    fn test_adversarial_idle_beats_empty_busy() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 0, 100), // local: busy, no queue
            make_snapshot(1, true, 0, 100),  // remote: idle, no queue
        ];
        let result = placement.place(&obs, &snapshots);

        assert!(
            matches!(result, PlacementDecision::Remote(_)),
            "idle must beat busy even when both have zero queue depth"
        );
    }

    #[test]
    fn test_adversarial_queue_depth_zero() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 0, 100),
            make_snapshot(1, false, 0, 100),
        ];
        let result = placement.place(&obs, &snapshots);

        assert!(
            matches!(result, PlacementDecision::Local),
            "zero queue depth tie must prefer Local"
        );
    }

    #[test]
    fn test_adversarial_capacity_factor_zero() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [make_snapshot(0, false, 2, 0), make_snapshot(1, false, 2, 0)];
        let result = placement.place(&obs, &snapshots);

        assert!(
            matches!(result, PlacementDecision::Local),
            "zero capacity factor tie must prefer Local"
        );
    }

    #[test]
    fn test_adversarial_large_queue_depth_difference() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 50, 100), // local: very deep
            make_snapshot(1, false, 1, 100),  // remote: nearly empty
        ];
        let result = placement.place(&obs, &snapshots);

        assert!(
            matches!(result, PlacementDecision::Remote(_)),
            "large queue depth difference must override local preference"
        );
    }

    #[test]
    fn test_adversarial_many_cores_picks_best() {
        let placement = ScoredPlacement::new();
        let obs = make_observer();
        let snapshots = [
            make_snapshot(0, false, 10, 100),
            make_snapshot(1, false, 8, 100),
            make_snapshot(2, false, 6, 100),
            make_snapshot(3, false, 4, 100),
            make_snapshot(4, true, 0, 100), // best: idle + empty
            make_snapshot(5, false, 3, 100),
            make_snapshot(6, false, 7, 100),
            make_snapshot(7, false, 9, 100),
        ];
        let result = placement.place(&obs, &snapshots);

        match result {
            PlacementDecision::Remote(core_id) => {
                assert_eq!(core_id, CoreId(4), "must pick the idle core among 8");
            }
            PlacementDecision::Local => panic!("idle core 4 must beat busy local"),
        }
    }

    #[test]
    fn test_adversarial_scoring_consistency() {
        let s1 = make_snapshot(0, true, 0, 100);
        let s2 = make_snapshot(1, false, 0, 100);

        assert!(
            score_core(&s1) > score_core(&s2),
            "idle core must score higher than busy core with same queue/capacity"
        );

        let s3 = make_snapshot(0, false, 1, 100);
        let s4 = make_snapshot(1, false, 5, 100);

        assert!(
            score_core(&s3) > score_core(&s4),
            "lower queue depth must score higher"
        );
    }
}
