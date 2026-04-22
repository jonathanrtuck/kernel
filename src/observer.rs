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
//! D42: three-value scheduling profile — responsiveness, throughput, precision.
//!      No priority integer. modify-scheduling gates these values.
//! D43: minimum schema settled. Metadata struct + structural backing split.
//!      No base/effective scheduling split (inheritance is userspace policy).
//!      Core assignment is transient (no struct field).
//!      Reply field is a cap-table reserved slot (D21 pattern).

use crate::capability;
use crate::field::Field;

// ---------------------------------------------------------------------------
// Register state handle — opaque reference into structural backing.
// ---------------------------------------------------------------------------

/// Opaque handle to saved register state in structural backing (D6/D35/D32).
///
/// The Observer carries this without knowing the register layout. Arch
/// context-switch code (inside the framekernel core) resolves it to the
/// concrete arch-specific layout for save/restore. This keeps arch types
/// confined to the core boundary (journal 023).
#[derive(Clone, Copy)]
pub struct RegisterStateHandle(*mut u8);

// ---------------------------------------------------------------------------
// Scheduling state — D39 five-state machine.
// ---------------------------------------------------------------------------

/// Primary Observer lifecycle state (D39).
///
/// Transitions:
/// - Inert → Runnable: resume (first start, D35)
/// - Runnable → Blocked: receive() with empty queue (D13)
/// - Blocked → Runnable: message arrives on waited Field
/// - Runnable → Faulted: hardware fault (page fault, invalid cap, etc.)
/// - Faulted → Runnable: resume (after handler resolves, D12)
///
/// Suspension (D39) is orthogonal — tracked by [`Observer::suspended`].
pub enum PrimaryState {
    Inert,
    Runnable,
    Blocked,
    Faulted,
}

// ---------------------------------------------------------------------------
// Wait-state linkage — D18 intrusive list, D19 multi-field accommodation.
// ---------------------------------------------------------------------------

/// Node linking an Observer into a Field's waiters or pending list.
///
/// In [`WaitState::Single`], one entry is stored inline (zero allocation).
/// In [`WaitState::Multi`], entries are allocated (source unsettled — D43
/// defers to downstream derivation).
pub struct WaitEntry {
    pub observer: *mut Observer,
    pub field: *mut Field,
    pub prev: *mut WaitEntry,
    pub next: *mut WaitEntry,
}

/// Wait-state for a blocked or fault-pending Observer (D18/D19).
///
/// Only one variant is active at a time (D18: states are mutually exclusive).
/// [`WaitState::None`] when the Observer is not waiting on any Field.
pub enum WaitState {
    None,
    Single(WaitEntry),
    Multi { head: *mut WaitEntry },
}

// ---------------------------------------------------------------------------
// Observer metadata struct — lives in root Space (D32).
// ---------------------------------------------------------------------------

/// The condition under which compute (Time) executes instructions within
/// specific memory (Space).
///
/// Each Observer holds capabilities to one or more Spaces and one or more Times
/// (D30). "Process" is a userspace convention (group of Observers sharing Space
/// caps).
///
/// Physically two regions (D32/D35):
/// - This metadata struct: root Space, ~80 bytes.
/// - Structural backing: consumed Space (register save area, cap table pages,
///   L0 page table root). Referenced via opaque handles/pointers.
///
/// Not in this struct (D43):
/// - Fault handler: cap-table reserved slot (D21).
/// - Reply field: cap-table reserved slot (D16 + D21 pattern).
/// - Time caps: cap-table regular slots (D30).
/// - Algorithm-specific scheduler state: per-core (D2).
/// - Core assignment: transient, re-decided per runnable transition (D31).
pub struct Observer {
    /// Opaque handle to saved register context in structural backing.
    /// Arch core code resolves this for save/restore on context switch.
    register_state: RegisterStateHandle,

    /// Physical address of the per-Observer page table root (D5/D26).
    /// Hot path: loaded into the hardware translation base on context switch.
    page_table_root: u64,

    /// Pointer to the flat capability array in structural backing (D4/D8).
    /// Hot path: indexed on every syscall to resolve capability handles.
    /// Updatable: table can grow via D8 table-full fault.
    cap_table: *mut capability::Entry,

    /// Primary lifecycle state (D39).
    state: PrimaryState,

    /// External suspension overlay (D39). Co-occurs with Blocked or Faulted.
    /// Resume clears this; underlying state remains.
    suspended: bool,

    /// Cached sum of held Time compute units (D30/D31/D36).
    /// Hot path: read by per-core scheduler.
    /// Cold path: updated on Time cap install/remove.
    compute_aggregate: u32,

    /// Three-value scheduling profile (D42). Budget: R + T + P <= budget.
    /// One set of values — no base/effective split (D43).
    /// Modified via modify-scheduling right (D39).
    responsiveness: u8,
    throughput: u8,
    precision: u8,

    /// Wait-state linkage for blocked/pending states (D18/D19).
    wait_state: WaitState,

    /// Outstanding capability references to this Observer (D11/D33).
    /// Decremented on cap close; object eligible for destruction at zero.
    refcount: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_is_small() {
        let size = core::mem::size_of::<Observer>();

        assert!(
            size <= 128,
            "Observer metadata struct should be small (D32); got {size} bytes"
        );
    }
}
