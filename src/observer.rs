//! Observer: schedulable execution unit.
//!
//! D6:  single execution unit — one register state, one PC.
//! D14: capability-held kernel object type.
//! D20: per-Observer fault handler.
//! D21: fault handler is a cap-table entry at reserved slot.
//! D23: Observer handles are clonable.
//! D30: one or more Time caps in regular cap-table slots.
//! D39: nine rights — resume, destroy, install-cap, write-registers, clone,
//!      read-registers, suspend, change-handler, modify-scheduling.

/// The condition under which compute (Time) executes instructions within
/// specific memory (Space).
///
/// Each Observer holds capabilities to one or more Spaces and one or more Times
/// (D30). "Process" is a userspace convention (group of Observers sharing Space
/// caps).
pub struct Observer {
    // Settled fields (D6, D14, D20, D30, D35, D36, D39):
    //   register state, capability table, cached scheduling aggregate (D36:
    //   total compute units, precomputed per-core fraction), lifecycle state,
    //   pending-list linkage (D18).
    //
    // Lifecycle state (D39):
    //   inert, runnable, blocked, faulted, externally-suspended.
    //   Suspended can co-occur with blocked or faulted.
    //
    // Fault handler:
    //   cap-table entry at reserved slot (D21), not a struct field.
    //   Change-handler is a separate right from install-cap (D39).
    // Time caps:
    //   regular cap-table slots (D30), not struct fields.
    //   Cached aggregate IS a struct field (D36: sum of held compute units).
    // Creation: D35 — minimal create + separate start + composable operations.
    //
    // Minimum schema: open (concrete field set needs derivation).
}
