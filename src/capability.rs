//! Capability system: handles, rights, entries, and per-Observer tables.
//!
//! D4:  capability-based authority (designation = authority).
//! D8:  kernel-managed flat table, per-Observer, typed-memory backing.
//! D11: close-only + destroy, generational slot tags.
//! D17: badges (minter-assigned, immutable, kernel-attached to messages).
//! D51: send-once is a boolean flag on Entry, not a rights bit.
//! D52: per-type rights masks settled for all five types.
//! D57: reserved slots — 0: fault handler, 1: reply field, 2: self-cap.
//! D58: badge is u64.
//! D67: entries store object generation at creation for revocation check.

use crate::arena::ObjectId;
use core::ptr::NonNull;

// ── Reserved cap-table slot indices (D21, D43, D57) ─────────────────

/// Fault handler Field cap (D21).
pub const SLOT_FAULT_HANDLER: u32 = 0;

/// Reply Field cap (D43, D16 + D21 pattern).
pub const SLOT_REPLY_FIELD: u32 = 1;

/// Self-reference Observer cap (D57). Full rights — the Observer can
/// attenuate and delegate.
pub const SLOT_SELF: u32 = 2;

/// First user-available slot index.
pub const SLOT_USER_START: u32 = 3;

// ── Sentinel values (D49) ───────────────────────────────────────────

/// No cap present in this message register slot (D49).
/// Valid because cap-table slot indices are small non-negative integers;
/// u64::MAX cannot be a valid slot under D8's bounded typed-memory backing.
pub const CAP_ABSENT: u64 = u64::MAX;

// ── Object types ────────────────────────────────────────────────────

/// Kernel object types designated by capabilities (D14, D44).
///
/// Exactly five. Exhaustive — no extension point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectType {
    Space,
    Time,
    Field,
    Observer,
    Pulsar,
}

// ── Rights (D52) ────────────────────────────────────────────────────

/// Per-capability rights bitmask (D52).
///
/// 14 bits across all types. Shared rights (DESTROY, CLONE, SPLIT)
/// occupy fixed positions; type-specific rights occupy non-overlapping
/// positions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rights(u16);

impl Rights {
    // ── Shared rights — same bit position across all types ──

    pub const DESTROY: Rights = Rights(1 << 1);
    pub const CLONE: Rights = Rights(1 << 4);
    pub const SPLIT: Rights = Rights(1 << 12);

    // ── Field rights (D15, D17, D45) ──

    pub const SEND: Rights = Rights(1 << 0);
    pub const RECEIVE: Rights = Rights(1 << 2);
    pub const MINT: Rights = Rights(1 << 11);

    // ── Observer rights (D39) ──

    pub const RESUME: Rights = Rights(1 << 3);
    pub const INSTALL_CAP: Rights = Rights(1 << 5);
    pub const WRITE_REGISTERS: Rights = Rights(1 << 6);
    pub const READ_REGISTERS: Rights = Rights(1 << 7);
    pub const SUSPEND: Rights = Rights(1 << 8);
    pub const CHANGE_HANDLER: Rights = Rights(1 << 9);
    pub const MODIFY_SCHEDULING: Rights = Rights(1 << 10);

    // ── Space rights (D41) ──

    pub const MERGE: Rights = Rights(1 << 13);

    // ── Per-type complete masks (D52) ──

    /// Space: split + merge + destroy + clone (4 bits).
    pub const SPACE_ALL: Rights =
        Rights(Self::SPLIT.0 | Self::MERGE.0 | Self::DESTROY.0 | Self::CLONE.0);

    /// Time: split + destroy (2 bits). No clone — D38 linear.
    pub const TIME_ALL: Rights = Rights(Self::SPLIT.0 | Self::DESTROY.0);

    /// Field: send + receive + mint + split + destroy + clone (6 bits).
    pub const FIELD_ALL: Rights = Rights(
        Self::SEND.0
            | Self::RECEIVE.0
            | Self::MINT.0
            | Self::SPLIT.0
            | Self::DESTROY.0
            | Self::CLONE.0,
    );

    /// Observer: all nine rights (9 bits).
    pub const OBSERVER_ALL: Rights = Rights(
        Self::RESUME.0
            | Self::DESTROY.0
            | Self::INSTALL_CAP.0
            | Self::WRITE_REGISTERS.0
            | Self::CLONE.0
            | Self::READ_REGISTERS.0
            | Self::SUSPEND.0
            | Self::CHANGE_HANDLER.0
            | Self::MODIFY_SCHEDULING.0,
    );

    /// Pulsar: destroy + clone (2 bits).
    pub const PULSAR_ALL: Rights = Rights(Self::DESTROY.0 | Self::CLONE.0);

    /// Fault-message Observer rights subset (D61): 5 of 9.
    pub const FAULT_OBSERVER: Rights = Rights(
        Self::RESUME.0
            | Self::DESTROY.0
            | Self::INSTALL_CAP.0
            | Self::WRITE_REGISTERS.0
            | Self::READ_REGISTERS.0,
    );

    pub const fn empty() -> Rights {
        Rights(0)
    }

    pub const fn contains(self, other: Rights) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn union(self, other: Rights) -> Rights {
        Rights(self.0 | other.0)
    }

    pub const fn attenuate(self, mask: Rights) -> Rights {
        Rights(self.0 & mask.0)
    }

    pub const fn bits(self) -> u16 {
        self.0
    }
}

// ── Badge (D58) ─────────────────────────────────────────────────────

/// Minter-assigned badge value, immutable after creation (D17, D58).
///
/// u64 forced by ABI: badge delivered in x5, a 64-bit register (D47).
/// The minter provides a u64 at clone time; the receiver reads a u64.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Badge(pub u64);

// ── Handle (D8, D11) ────────────────────────────────────────────────

/// Opaque capability handle presented by userspace (D8).
///
/// Index into the Observer's flat table + generational slot tag for
/// ABA defense (D11). Userspace sees only this; the kernel resolves
/// it to an Entry.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Handle {
    pub index: u32,
    pub slot_tag: SlotTag,
}

// ── Slot tag (D11) ──────────────────────────────────────────────────

/// Generational slot tag for ABA prevention (D11).
///
/// Bumped on slot reuse. Prevents stale-handle aliasing of reused
/// table slots. ABA defense, not revocation (D67 generation is
/// revocation).
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct SlotTag(pub u32);

// ── Entry (D8, D11, D17, D51, D67) ─────────────────────────────────

/// Single entry in the per-Observer capability table (D8).
///
/// Empty slots have `object: None`. Occupied slots carry the full
/// capability: target, rights, badge, slot tag, send-once flag, and
/// stored generation for D67 revocation check.
pub struct Entry {
    /// Target object type and arena identifier. None = empty slot.
    pub object: Option<(ObjectType, ObjectId)>,

    /// Per-capability rights mask (D52).
    pub rights: Rights,

    /// Minter-assigned badge, kernel-injected into messages (D17).
    pub badge: Badge,

    /// ABA prevention tag, bumped on slot reuse (D11).
    pub slot_tag: SlotTag,

    /// Use-limited flag (D51). True = cap consumed after one Send.
    /// Outside the rights mask — attenuation cannot clear it.
    pub send_once: bool,

    /// Object generation at creation/clone time (D67).
    /// On use: compared against the object's live generation.
    /// Mismatch → stale cap → error, slot lazily rewritten to None.
    pub stored_generation: u64,
}

// ── Transferred cap (D28, D37) ──────────────────────────────────────

/// Capability in transit between cap tables (D28).
///
/// When a message carries a cap, the cap is removed from the sender's
/// table and stored in the Field queue until the receiver picks it up.
/// Contains the information needed to install in the receiver's table.
pub struct TransferredCap {
    pub object_type: ObjectType,
    pub object_id: ObjectId,
    pub rights: Rights,
    pub badge: Badge,
    pub send_once: bool,
    pub stored_generation: u64,
}

// ── Table (D8) ──────────────────────────────────────────────────────

/// Per-Observer flat capability table (D8).
///
/// Kernel-managed array of Entry. Backed by typed memory from the
/// Observer's Spaces — not a kernel-internal pool (D8). Growth via
/// table-full fault (D8) → handler provides Space (D40).
///
/// Reserved slots (D21, D43, D57):
/// - 0: fault handler (D21)
/// - 1: reply Field (D43)
/// - 2: self-cap (D57)
/// - 3+: user slots
pub struct Table {
    /// Always valid — the table is allocated at Observer creation.
    pub entries: NonNull<Entry>,
    pub capacity: u32,
    pub count: u32,
}

// ── Error types ────────────────────────────────────────────────────

/// Errors from capability operations (D4, D8, D11, D67).
///
/// Every typed kernel operation and IPC path begins with handle
/// resolution. These errors represent the ways that resolution or
/// subsequent rights checking can fail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapError {
    /// Handle index out of bounds or slot is empty.
    InvalidHandle,
    /// D11: slot tag mismatch — stale handle referencing a reused slot.
    SlotTagMismatch,
    /// D67: stored generation doesn't match the object's live generation.
    /// Slot should be lazily rewritten to empty (Coyotos pattern, A4).
    StaleGeneration,
    /// D52: required right not present in the cap's rights mask.
    InsufficientRights,
    /// Operation targets the wrong object type for this syscall.
    TypeMismatch,
    /// D8: no free slots in the cap table.
    /// Triggers the cap-table-full fault (D40), routing to the handler
    /// to provide more Space for table growth.
    TableFull,
    /// D51: send-once cap already consumed on a previous Send.
    SendOnceConsumed,
    /// D38: clone forbidden for this object type (Time is linear).
    CloneForbidden,
}

/// Outcome of closing a capability slot (D11, D17).
///
/// The close operation has three possible outcomes, distinguished so
/// the caller can handle badge-closure notifications (D17) and
/// object destruction on last-reference (D11/D33).
#[derive(Debug)]
pub enum CloseResult {
    /// Slot freed. The caller must decrement the target object's refcount
    /// and initiate destruction if it reached zero (D11/D33).
    Closed {
        object_type: ObjectType,
        object_id: ObjectId,
        was_last_reference: bool,
    },
    /// D17: last send cap with this badge on a tracked Field was closed.
    /// The caller must enqueue a LABEL_CLOSURE message to the Field.
    /// D11/D33: if `was_last_reference` is true, the Field itself must
    /// also be destroyed — both obligations apply to the same close.
    ClosedWithBadgeClosure {
        field_id: ObjectId,
        badge: Badge,
        was_last_reference: bool,
    },
    /// Slot was already empty — no action needed.
    AlreadyEmpty,
}

/// Continuation state for preemptible destroy cascade (D33).
///
/// Object destruction cascades through held capabilities: each cap is
/// closed, and objects reaching refcount zero are destroyed recursively.
/// The cascade is preemptible — the kernel processes cleanup in bounded
/// steps. Between steps, the timer interrupt can preempt and the
/// scheduler can run higher-priority Observers.
///
/// D33: the object is dead before cleanup begins (D11). No partially-
/// alive state is externally visible.
///
/// Performance: O(N + M) per Observer — N cap table entries closed,
/// M badge-closure checks. Preemption bounded by step count, not
/// total cascade size. seL4 MCS demonstrates feasibility.
pub struct CascadeState {
    /// Current position in the cap table being iterated.
    pub position: u32,
    /// Stack of Observer ObjectIds whose cascades are pending.
    /// Depth bounded by exclusively-held Observer chains.
    pub pending: [Option<ObjectId>; 8],
    /// Current depth in the pending stack.
    pub depth: u8,
    /// Whether the cascade has completed.
    pub complete: bool,
}

// ── Entry methods ──────────────────────────────────────────────────

impl Entry {
    /// Create an empty (unoccupied) entry for a given slot tag.
    pub const fn empty(tag: SlotTag) -> Entry {
        Entry {
            object: None,
            rights: Rights::empty(),
            badge: Badge(0),
            slot_tag: tag,
            send_once: false,
            stored_generation: 0,
        }
    }

    /// Whether this slot is occupied (has a live capability).
    pub const fn is_occupied(&self) -> bool {
        self.object.is_some()
    }

    /// D67: compare stored generation against the object's live generation.
    ///
    /// Returns `false` on mismatch — the capability has been revoked.
    /// The caller should lazily rewrite the slot to empty on mismatch
    /// (Coyotos pattern), maintaining A4 compliance.
    ///
    /// Performance: one comparison. The generation field shares a cache
    /// line with fields already loaded on the syscall path, so the
    /// branch predictor correctly predicts "match" in the common case.
    pub fn check_generation(&self, live_generation: u64) -> bool {
        self.stored_generation == live_generation
    }

    /// D52: check whether all required rights are present in this cap.
    pub fn check_rights(&self, required: Rights) -> bool {
        self.rights.contains(required)
    }

    /// Verify the entry targets the expected object type.
    pub fn check_type(&self, expected: ObjectType) -> bool {
        self.object.map(|(t, _)| t == expected).unwrap_or(false)
    }

    /// D51: check the send-once flag.
    pub const fn is_send_once(&self) -> bool {
        self.send_once
    }
}

// ── Table methods ──────────────────────────────────────────────────

impl Table {
    /// Resolve a handle to an entry reference.
    ///
    /// D4/D8: the universal entry point for every syscall. Validates
    /// the handle's index bounds and slot tag (D11 ABA defense).
    ///
    /// Does NOT check generation (D67) — the caller must do that after
    /// resolving, since it requires the object's live generation from
    /// the arena. This two-step pattern keeps Table independent of the
    /// arena types.
    ///
    /// Performance: O(1) — array index + tag comparison.
    /// Security: prevents stale-handle aliasing of reused table slots.
    pub fn resolve(&self, _handle: Handle) -> Result<&Entry, CapError> {
        todo!()
    }

    /// Mutable resolve for operations that modify the entry.
    pub fn resolve_mut(&mut self, _handle: Handle) -> Result<&mut Entry, CapError> {
        todo!()
    }

    /// Find and return the index of a free slot.
    ///
    /// D8: kernel-managed slot allocation. Returns `TableFull` if no
    /// free slots exist — this triggers the cap-table-full fault (D40),
    /// routing to the handler which provides Space for table growth.
    pub fn allocate_slot(&mut self) -> Result<u32, CapError> {
        todo!()
    }

    /// Free a slot, bumping its slot tag for ABA defense (D11).
    ///
    /// The slot becomes empty and available for reuse. The bumped tag
    /// ensures any outstanding handles to this slot will fail resolution.
    pub fn free_slot(&mut self, _index: u32) {
        todo!()
    }

    /// Install a capability entry at a specific slot index.
    ///
    /// Used for kernel-reserved slot setup: fault handler at slot 0
    /// (D21), reply field at slot 1 (D43), self-cap at slot 2 (D57).
    /// Also used for cap transfer during IPC (D28) and fault resolution
    /// (D40).
    pub fn install_at(&mut self, _index: u32, _entry: Entry) {
        todo!()
    }

    /// Install a capability at the next free slot, returning the index.
    ///
    /// D35: general-purpose cap installation. Same primitive serves
    /// Observer pre-start configuration, fault resolution (handler
    /// installs Space caps via D40), and dynamic capability delegation.
    ///
    /// Returns `TableFull` if no free slot exists.
    pub fn install(&mut self, _entry: Entry) -> Result<u32, CapError> {
        todo!()
    }

    /// Close a capability slot, returning the outcome.
    ///
    /// D11: drops the reference. The slot tag is bumped (ABA defense).
    /// For tracked Fields (D17), closing the last send cap with a given
    /// badge produces `ClosedWithBadgeClosure`.
    ///
    /// The caller is responsible for: decrementing the target object's
    /// refcount, handling badge-closure delivery, and initiating destroy
    /// cascade (D33) if the refcount reached zero.
    pub fn close(&mut self, _index: u32) -> CloseResult {
        todo!()
    }

    /// Begin preemptible destroy cascade for the Observer that owns
    /// this table (D33).
    ///
    /// Returns a `CascadeState` for incremental processing. The caller
    /// advances the cascade via `cascade_step`, yielding to the scheduler
    /// between steps. The cascade is cleanup of an already-dead object —
    /// no partially-alive state is externally visible (D11).
    ///
    /// D33: only Observers cascade (only Observers hold caps). Space,
    /// Time, Field, and Pulsar destruction is O(1).
    pub fn begin_cascade(&mut self) -> CascadeState {
        todo!()
    }

    /// Process the next bounded step of a destroy cascade.
    ///
    /// Returns `true` when the cascade is complete.
    ///
    /// D33: between steps, the timer interrupt can preempt and the
    /// scheduler can run higher-priority Observers. Each step processes
    /// a bounded number of cap-table entries.
    ///
    /// Performance: bounded per-step cost. Total cascade is
    /// O(N + M) — N entries closed, M badge-closure checks.
    pub fn cascade_step(&mut self, _state: &mut CascadeState) -> bool {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_rights_bits_are_consistent() {
        assert!(Rights::SPACE_ALL.contains(Rights::DESTROY));
        assert!(Rights::SPACE_ALL.contains(Rights::CLONE));
        assert!(Rights::SPACE_ALL.contains(Rights::SPLIT));
        assert!(Rights::TIME_ALL.contains(Rights::DESTROY));
        assert!(Rights::TIME_ALL.contains(Rights::SPLIT));
        assert!(!Rights::TIME_ALL.contains(Rights::CLONE));
        assert!(Rights::FIELD_ALL.contains(Rights::DESTROY));
        assert!(Rights::FIELD_ALL.contains(Rights::CLONE));
        assert!(Rights::FIELD_ALL.contains(Rights::SPLIT));
        assert!(Rights::OBSERVER_ALL.contains(Rights::DESTROY));
        assert!(Rights::OBSERVER_ALL.contains(Rights::CLONE));
        assert!(Rights::PULSAR_ALL.contains(Rights::DESTROY));
        assert!(Rights::PULSAR_ALL.contains(Rights::CLONE));
    }

    #[test]
    fn per_type_masks_use_correct_bit_counts() {
        assert_eq!(Rights::SPACE_ALL.bits().count_ones(), 4);
        assert_eq!(Rights::TIME_ALL.bits().count_ones(), 2);
        assert_eq!(Rights::FIELD_ALL.bits().count_ones(), 6);
        assert_eq!(Rights::OBSERVER_ALL.bits().count_ones(), 9);
        assert_eq!(Rights::PULSAR_ALL.bits().count_ones(), 2);
    }

    #[test]
    fn fault_observer_is_subset_of_observer_all() {
        assert!(Rights::OBSERVER_ALL.contains(Rights::FAULT_OBSERVER));
        assert_eq!(Rights::FAULT_OBSERVER.bits().count_ones(), 5);
    }

    #[test]
    fn all_14_bits_are_within_u16() {
        let all = Rights::SPACE_ALL
            .union(Rights::TIME_ALL)
            .union(Rights::FIELD_ALL)
            .union(Rights::OBSERVER_ALL)
            .union(Rights::PULSAR_ALL);

        assert_eq!(all.bits().count_ones(), 14);
    }

    const _: () = assert!(SLOT_REPLY_FIELD == SLOT_FAULT_HANDLER + 1);
    const _: () = assert!(SLOT_SELF == SLOT_REPLY_FIELD + 1);
    const _: () = assert!(SLOT_USER_START == SLOT_SELF + 1);

    #[test]
    fn attenuate_can_only_remove_rights() {
        let full = Rights::OBSERVER_ALL;
        let reduced = full.attenuate(Rights::FAULT_OBSERVER);

        assert_eq!(reduced, Rights::FAULT_OBSERVER);
        assert!(!reduced.contains(Rights::SUSPEND));
    }
}
