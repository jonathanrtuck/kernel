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
#[derive(Clone, Copy, PartialEq, Eq)]
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
#[derive(Clone, Copy, PartialEq, Eq)]
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
