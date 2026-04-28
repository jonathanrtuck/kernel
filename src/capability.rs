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

/// Growth slot sentinel (D-3.1a, D40).
///
/// Used by the fault handler to target cap table growth via
/// ObserverInstallCap. Never conflicts with user slots because
/// u32::MAX is far beyond any realistic table capacity. The kernel
/// detects this sentinel and extends the faulting Observer's table
/// instead of performing a normal cap install.
pub const SLOT_GROWTH: u32 = u32::MAX;

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

    /// Construct a Rights from a raw u16 bitmask.
    pub const fn from_bits(bits: u16) -> Rights {
        Rights(bits)
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

// ── Handle encoding (D77) ───────────────────────────────────────────

/// Maximum cap table index encodable in the handle ABI.
/// The 16/48 split gives 65536 addressable slots and 2^48 tag space
/// (~281 trillion reuses before ABA aliasing).
pub const MAX_HANDLE_INDEX: u32 = 0xFFFF;

impl Handle {
    /// Encode a Handle into the u64 ABI representation (D77).
    ///
    /// Lower 16 bits = index, upper 48 bits = slot_tag.
    /// This is the format userspace presents in registers (x5 for IPC,
    /// x5 for typed ops). Index in low bits for cheap extraction.
    pub const fn encode(self) -> u64 {
        (self.index as u64 & 0xFFFF) | ((self.slot_tag.0 & 0xFFFF_FFFF_FFFF) << 16)
    }

    /// Decode a u64 ABI value into a Handle (D77).
    ///
    /// Lower 16 bits = index, upper 48 bits = slot_tag.
    pub const fn decode(raw: u64) -> Handle {
        Handle {
            index: (raw & 0xFFFF) as u32,
            slot_tag: SlotTag((raw >> 16) & 0xFFFF_FFFF_FFFF),
        }
    }
}

// ── Slot tag (D11) ──────────────────────────────────────────────────

/// Generational slot tag for ABA prevention (D11).
///
/// Bumped on slot reuse. Prevents stale-handle aliasing of reused
/// table slots. ABA defense, not revocation (D67 generation is
/// revocation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotTag(pub u64);

impl SlotTag {
    /// Compare against a handle-decoded tag using the lower 48 bits.
    /// The Handle ABI (u64 register) carries index(16) + tag(48), so
    /// 48 bits of the u64 tag survive the encode/decode round-trip.
    pub const fn abi_matches(self, other: SlotTag) -> bool {
        (self.0 & 0xFFFF_FFFF_FFFF) == (other.0 & 0xFFFF_FFFF_FFFF)
    }
}

// ── Entry (D8, D11, D17, D51, D67) ─────────────────────────────────

/// Single entry in the per-Observer capability table (D8).
///
/// Empty slots have `object: None`. Occupied slots carry the full
/// capability: target, rights, badge, slot tag, send-once flag, and
/// stored generation for D67 revocation check.
#[derive(Clone, Copy, Debug)]
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
#[derive(Clone, Copy)]
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
pub(crate) const FREELIST_END: u64 = u64::MAX;

pub struct Table {
    /// Always valid — the table is allocated at Observer creation.
    pub entries: NonNull<Entry>,
    pub capacity: u32,
    pub count: u32,
    /// Head of intrusive freelist through empty entries. Empty entries
    /// store the next-free index in `stored_generation`. None when all
    /// user slots are occupied.
    pub free_head: Option<u32>,
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
    /// Whether the cascade has completed.
    pub complete: bool,
}

// ── Cascade continuation (D98) ────────────────────────────────────

/// Maximum nesting depth for cascade continuations (D98).
///
/// Each level represents an exclusively-held Observer chain. In practice
/// depth is 1-2; 4 is a generous upper bound. 4 levels * 12 bytes = 48
/// bytes — fits comfortably in CoreState without dynamic allocation.
pub const MAX_CASCADE_DEPTH: usize = 4;

/// Per-level cascade state: which object is being cleaned up and where
/// the iteration cursor is (D98).
#[derive(Clone, Copy)]
pub struct CascadeLevel {
    /// Arena identity of the object whose cap table is being iterated.
    pub object_id: crate::arena::ObjectId,
    /// Current slot cursor in the cap table.
    pub slot_cursor: u32,
}

/// Preemptible cascade continuation state (D98).
///
/// Saved in CoreState between timer preemptions. The stack supports
/// nested cascades: closing a cap may trigger a secondary destroy when
/// the closed object's refcount reaches zero, pushing a new level.
///
/// D98: the destroyer is blocked while its cascade is in progress (D39).
/// Other Observers on the same core CAN run between cascade batches.
pub struct CascadeContinuation {
    /// Stack of active cascade levels. Index 0 = outermost destroy.
    pub levels: [Option<CascadeLevel>; MAX_CASCADE_DEPTH],
    /// Number of active levels (0 = cascade complete).
    pub depth: usize,
    /// The Observer that issued the Destroy (or Close for D107 auto-destroy).
    /// Blocked while the cascade runs (D98). Re-enqueued on completion.
    pub destroyer_ptr: Option<core::ptr::NonNull<crate::observer::Observer>>,
    /// Backing VA of the destroyed Observer (for return Space cap).
    pub backing_va: usize,
    /// Backing size of the destroyed Observer (for return Space cap).
    pub backing_size: usize,
    /// ObjectId of the Observer being destroyed (for arena free on completion).
    pub target_id: crate::arena::ObjectId,
    /// D107: true when cascade was triggered by auto-destroy (refcount=0
    /// on Close). Backing returns to root pool, not to the closer.
    pub auto_destroy: bool,
}

impl Default for CascadeContinuation {
    fn default() -> Self {
        Self::new()
    }
}

impl CascadeContinuation {
    pub const fn new() -> CascadeContinuation {
        CascadeContinuation {
            levels: [None; MAX_CASCADE_DEPTH],
            depth: 0,
            destroyer_ptr: None,
            backing_va: 0,
            backing_size: 0,
            target_id: crate::arena::ObjectId(0),
            auto_destroy: false,
        }
    }

    pub fn push(&mut self, object_id: crate::arena::ObjectId) -> bool {
        if self.depth >= MAX_CASCADE_DEPTH {
            return false;
        }

        self.levels[self.depth] = Some(CascadeLevel {
            object_id,
            slot_cursor: 0,
        });
        self.depth += 1;

        true
    }

    pub fn pop(&mut self) -> Option<CascadeLevel> {
        if self.depth == 0 {
            return None;
        }

        self.depth -= 1;

        self.levels[self.depth].take()
    }

    pub fn current_mut(&mut self) -> Option<&mut CascadeLevel> {
        if self.depth == 0 {
            return None;
        }

        self.levels[self.depth - 1].as_mut()
    }

    pub const fn is_empty(&self) -> bool {
        self.depth == 0
    }
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

// ── Cap resolution protocol (D77) ──────────────────────────────────

/// Successfully resolved capability (D77).
///
/// The full resolution path validated: bounds, slot tag (D11), occupancy,
/// generation (D67), rights (D52), and type. This struct carries the
/// verified information needed to proceed with the operation.
///
/// The caller holds the appropriate arena lock for the duration of the
/// operation — the ObjectId is valid only while that lock is held.
#[derive(Debug)]
pub struct ResolvedCap {
    /// Arena-internal identifier for the target object.
    pub object_id: ObjectId,
    /// Verified object type (matched against the expected type).
    pub object_type: ObjectType,
    /// Per-capability rights mask (already checked for required rights).
    pub rights: Rights,
    /// Minter-assigned badge (D17). Passed through to messages.
    pub badge: Badge,
    /// D51: whether this was a send-once capability.
    pub send_once: bool,
}

/// Resolve a raw u64 handle value to a verified capability (D77).
///
/// The full resolution sequence, in order:
/// 1. **Decode:** extract index (low 32) and slot_tag (high 32).
/// 2. **Bounds check:** index < capacity (Spectre-safe via frame/ barrier).
/// 3. **Entry lookup:** index into the Observer's cap table array.
/// 4. **Occupied check:** entry has an object (not an empty/freelist slot).
/// 5. **Slot tag check:** entry's tag matches handle's tag (D11 ABA defense).
/// 6. **Generation check:** entry's stored generation matches the object's
///    live generation from the arena (D67 revocation). On mismatch, the
///    entry is lazily rewritten to empty (Coyotos pattern, A4 compliance).
/// 7. **Rights check:** entry's rights contain all required rights (D52).
/// 8. **Type check:** entry's object type matches the expected type.
///
/// Steps 1–5 are delegated to `resolve_cap_entry`. Steps 6–8 run on top.
///
/// Lock acquisition: this function does NOT acquire any lock. It operates
/// on the Observer's cap table pointer (per-Observer, no lock needed on
/// the hot path — D1). The caller acquires the target arena's lock AFTER
/// resolution succeeds, using the returned ObjectType to select which lock.
///
/// Parameters:
/// - `raw_handle`: the u64 value from the userspace register (x5).
/// - `entries`: the Observer's cap_table pointer.
/// - `capacity`: the Observer's cap_table_capacity.
/// - `live_generation`: the target object's current generation from its
///   arena. The caller must have read this before calling (requires the
///   arena lock — see note below on two-phase resolution).
/// - `required_rights`: rights that must be present for this operation.
/// - `expected_type`: the object type this operation targets. Pass `None`
///   for generic operations (Destroy, Clone, Close, Mint) that accept
///   any type.
///
/// Two-phase resolution note: generation checking requires the object's
/// live generation, which lives in the arena behind a lock. For the
/// common case (typed operations targeting a known type), the caller:
/// 1. Calls `resolve_cap_entry` (steps 1–5, no lock needed).
/// 2. Reads the ObjectId and ObjectType from the entry.
/// 3. Acquires the arena lock, reads the live generation.
/// 4. Calls this function with the live generation.
///
/// To avoid this two-phase dance, this function accepts `live_generation`
/// as a parameter. If the caller cannot provide it (hasn't acquired the
/// arena lock yet), it should use `resolve_cap_entry` first to get the
/// entry, then acquire the lock and do the generation + rights + type
/// checks manually. This function is the composed convenience form.
pub fn resolve_cap(
    raw_handle: u64,
    entries: core::ptr::NonNull<Entry>,
    capacity: u32,
    live_generation: u64,
    required_rights: Rights,
    expected_type: Option<ObjectType>,
) -> Result<ResolvedCap, CapError> {
    // Steps 1–5: decode, bounds, lookup, occupied, slot tag.
    let entry = resolve_cap_entry(raw_handle, entries, capacity)?;

    // Step 6: generation check (D67).
    if !entry.check_generation(live_generation) {
        return Err(CapError::StaleGeneration);
    }
    // Step 7: rights check (D52).
    if !entry.check_rights(required_rights) {
        return Err(CapError::InsufficientRights);
    }

    // Step 8: type check (if expected_type is specified).
    // Occupied check passed in step 4, so object is guaranteed Some.
    let (object_type, object_id) = match entry.object {
        Some(pair) => pair,
        None => return Err(CapError::InvalidHandle),
    };

    if let Some(expected) = expected_type
        && object_type != expected
    {
        return Err(CapError::TypeMismatch);
    }

    Ok(ResolvedCap {
        object_id,
        object_type,
        rights: entry.rights,
        badge: entry.badge,
        send_once: entry.send_once,
    })
}

/// Resolve a raw u64 handle to a cap table entry reference (D77).
///
/// Performs steps 1-5 of the resolution sequence (decode, bounds,
/// lookup, tag, occupied) without checking generation, rights, or type.
/// Used for two-phase resolution where the caller needs the ObjectId
/// to acquire the arena lock before the generation check.
///
/// Returns: shared reference to the Entry. The caller must then:
/// - Read the ObjectId and ObjectType.
/// - Acquire the appropriate arena lock.
/// - Read the live generation from the arena object.
/// - Call `entry.check_generation(live_generation)`.
/// - Call `entry.check_rights(required_rights)`.
/// - Call `entry.check_type(expected_type)` if type-specific.
pub fn resolve_cap_entry(
    raw_handle: u64,
    entries: core::ptr::NonNull<Entry>,
    capacity: u32,
) -> Result<&'static Entry, CapError> {
    let handle = Handle::decode(raw_handle);
    let entry = crate::frame::capabilities::entry_ref(entries, capacity, handle.index)
        .ok_or(CapError::InvalidHandle)?;

    if !entry.is_occupied() {
        return Err(CapError::InvalidHandle);
    }
    if !entry.slot_tag.abi_matches(handle.slot_tag) {
        return Err(CapError::SlotTagMismatch);
    }

    Ok(entry)
}

// ── Table methods ──────────────────────────────────────────────────

/// Maximum entries processed per cascade step (D33 preemption bound).
const CASCADE_STEP_SIZE: u32 = 16;

impl Table {
    /// Validate that an entry is occupied and its slot tag matches the handle.
    fn validate_entry(entry: &Entry, handle: Handle) -> Result<(), CapError> {
        if !entry.is_occupied() {
            return Err(CapError::InvalidHandle);
        }
        if !entry.slot_tag.abi_matches(handle.slot_tag) {
            return Err(CapError::SlotTagMismatch);
        }

        Ok(())
    }

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
    pub fn resolve(&self, handle: Handle) -> Result<&Entry, CapError> {
        let entry =
            crate::frame::capabilities::entry_ref(self.entries, self.capacity, handle.index)
                .ok_or(CapError::InvalidHandle)?;

        Self::validate_entry(entry, handle)?;

        Ok(entry)
    }

    /// Mutable resolve for operations that modify the entry.
    pub fn resolve_mut(&mut self, handle: Handle) -> Result<&mut Entry, CapError> {
        let entry =
            crate::frame::capabilities::entry_mut(self.entries, self.capacity, handle.index)
                .ok_or(CapError::InvalidHandle)?;

        Self::validate_entry(entry, handle)?;

        Ok(entry)
    }

    /// Find and return the index of a free slot.
    ///
    /// D8: kernel-managed slot allocation. Returns `TableFull` if no
    /// free slots exist — this triggers the cap-table-full fault (D40),
    /// routing to the handler which provides Space for table growth.
    pub fn allocate_slot(&mut self) -> Result<u32, CapError> {
        let index = self.free_head.ok_or(CapError::TableFull)?;
        let entry = crate::frame::capabilities::entry_ref(self.entries, self.capacity, index)
            .ok_or(CapError::TableFull)?;
        let next = entry.stored_generation;

        self.free_head = if next == FREELIST_END {
            None
        } else {
            Some(next as u32)
        };

        Ok(index)
    }

    /// Free a slot, bumping its slot tag for ABA defense (D11).
    ///
    /// The slot becomes empty and available for reuse. The bumped tag
    /// ensures any outstanding handles to this slot will fail resolution.
    pub fn free_slot(&mut self, index: u32) {
        if index >= self.capacity {
            return;
        }

        let entry = crate::frame::capabilities::entry_mut(self.entries, self.capacity, index);

        if let Some(e) = entry {
            let was_occupied = e.is_occupied();

            e.slot_tag = SlotTag(e.slot_tag.0.wrapping_add(1));
            e.object = None;
            e.stored_generation = match self.free_head {
                Some(head) => head as u64,
                None => FREELIST_END,
            };
            self.free_head = Some(index);

            if was_occupied {
                self.count = self.count.saturating_sub(1);
            }
        }
    }

    /// Install a capability entry at a specific slot index.
    ///
    /// Used for kernel-reserved slot setup: fault handler at slot 0
    /// (D21), reply field at slot 1 (D43), self-cap at slot 2 (D57).
    /// Also used for cap transfer during IPC (D28) and fault resolution
    /// (D40).
    pub fn install_at(&mut self, index: u32, entry: Entry) {
        if index >= self.capacity {
            return;
        }

        let slot = crate::frame::capabilities::entry_mut(self.entries, self.capacity, index);

        if let Some(s) = slot {
            let was_occupied = s.is_occupied();
            let new_is_occupied = entry.is_occupied();

            *s = entry;

            if !was_occupied && new_is_occupied {
                self.count += 1;
            } else if was_occupied && !new_is_occupied {
                self.count = self.count.saturating_sub(1);
            }
        }
    }

    /// Install a capability at the next free slot, returning the index.
    ///
    /// D35: general-purpose cap installation. Same primitive serves
    /// Observer pre-start configuration, fault resolution (handler
    /// installs Space caps via D40), and dynamic capability delegation.
    ///
    /// Returns `TableFull` if no free slot exists.
    pub fn install(&mut self, entry: Entry) -> Result<u32, CapError> {
        let index = self.allocate_slot()?;

        self.install_at(index, entry);

        Ok(index)
    }

    /// D96: extract a capability from the table (move semantics for IPC
    /// cap transfer). Reads the entry, captures it as a TransferredCap,
    /// then frees the slot (D11 slot tag bump). Returns None if the index
    /// is out of bounds or the slot is empty.
    pub fn extract_cap(&mut self, index: u32) -> Option<TransferredCap> {
        let entry = crate::frame::capabilities::entry_ref(self.entries, self.capacity, index)?;
        let (object_type, object_id) = entry.object?;
        let transferred = TransferredCap {
            object_type,
            object_id,
            rights: entry.rights,
            badge: entry.badge,
            send_once: entry.send_once,
            stored_generation: entry.stored_generation,
        };

        self.free_slot(index);

        Some(transferred)
    }

    /// D96: install a transferred capability at the next free slot and
    /// return the encoded handle (D77: index(16) | slot_tag(48)).
    ///
    /// Preserves the slot's existing slot_tag (set by the last free_slot
    /// that released it) so the returned handle is valid for resolution.
    /// Returns TableFull if no free slots exist.
    pub fn install_transferred_cap(
        &mut self,
        transferred: &TransferredCap,
    ) -> Result<u64, CapError> {
        let index = self.allocate_slot()?;
        let slot_tag = crate::frame::capabilities::entry_ref(self.entries, self.capacity, index)
            .map(|e| e.slot_tag)
            .unwrap_or(SlotTag(0));

        self.install_at(
            index,
            Entry {
                object: Some((transferred.object_type, transferred.object_id)),
                rights: transferred.rights,
                badge: transferred.badge,
                slot_tag,
                send_once: transferred.send_once,
                stored_generation: transferred.stored_generation,
            },
        );

        Ok(Handle { index, slot_tag }.encode())
    }

    /// Read a cap table entry's core fields without modifying the table.
    ///
    /// Used for reading the reply Field entry at SLOT_REPLY_FIELD (slot 1)
    /// during Call to mint the reply cap (D96, D43). Returns None if the
    /// slot is empty or out of bounds.
    pub fn read_entry(&self, index: u32) -> Option<(ObjectType, crate::arena::ObjectId, u64)> {
        let entry = crate::frame::capabilities::entry_ref(self.entries, self.capacity, index)?;
        let (object_type, object_id) = entry.object?;

        Some((object_type, object_id, entry.stored_generation))
    }

    /// Read the full Entry at a slot index (D100).
    ///
    /// Returns None if out of bounds. Returns the Entry regardless of
    /// whether it is occupied — the caller checks `entry.object`.
    pub fn read_full_entry(&self, index: u32) -> Option<Entry> {
        let entry = crate::frame::capabilities::entry_ref(self.entries, self.capacity, index)?;

        Some(*entry)
    }

    /// Close a capability slot, returning the outcome.
    ///
    /// D11: drops the reference. The slot tag is bumped (ABA defense).
    /// For tracked Fields (D17), closing the last send cap with a given
    /// badge produces `ClosedWithBadgeClosure`. Badge tracking is
    /// deferred — the internal map data structure does not exist yet.
    ///
    /// The caller is responsible for: decrementing the target object's
    /// refcount, handling badge-closure delivery, and initiating
    /// auto-destroy (D107) if the refcount reached zero.
    pub fn close(&mut self, index: u32) -> CloseResult {
        if index >= self.capacity {
            return CloseResult::AlreadyEmpty;
        }

        let entry = crate::frame::capabilities::entry_mut(self.entries, self.capacity, index);
        let Some(e) = entry else {
            return CloseResult::AlreadyEmpty;
        };

        if !e.is_occupied() {
            return CloseResult::AlreadyEmpty;
        }

        let (object_type, object_id) = e.object.unwrap();

        e.object = None;
        e.slot_tag = SlotTag(e.slot_tag.0.wrapping_add(1));
        e.stored_generation = match self.free_head {
            Some(head) => head as u64,
            None => FREELIST_END,
        };
        self.free_head = Some(index);
        self.count = self.count.saturating_sub(1);

        // D107: was_last_reference is always false here — Table cannot
        // determine refcount (lives on the target object in its arena).
        // The caller checks the object's refcount and auto-destroys if
        // zero; backing returns to root Space, not to the closer.
        CloseResult::Closed {
            object_type,
            object_id,
            was_last_reference: false,
        }
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
        CascadeState {
            position: 0,
            complete: false,
        }
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
    pub fn cascade_step(&mut self, state: &mut CascadeState) -> bool {
        if state.complete {
            return true;
        }

        if self.count == 0 {
            state.complete = true;

            return true;
        }

        let end = self
            .capacity
            .min(state.position.saturating_add(CASCADE_STEP_SIZE));
        let mut position = state.position;

        while position < end {
            self.close(position);

            position += 1;
        }

        state.position = position;

        if self.count == 0 || state.position >= self.capacity {
            state.complete = true;

            return true;
        }

        false
    }

    /// D24/D97: check whether this table holds any cap referencing a specific
    /// (object_type, object_id) pair, excluding one slot index.
    ///
    /// Used by the D24 mapping bridge: when a Space cap is closed, the kernel
    /// scans the Observer's cap table to determine if any other cap to the
    /// same Space remains. If not, the Space is unmapped from the Observer's
    /// page table.
    ///
    /// The `exclude_index` parameter skips the slot being closed (it has
    /// already been freed and might be reused).
    ///
    /// O(capacity) — cold path. D91 established ~1 us for 1024 slots as
    /// acceptable for mapping operations.
    pub fn has_cap_to_object(
        &self,
        target_type: ObjectType,
        target_id: crate::arena::ObjectId,
        exclude_index: u32,
    ) -> bool {
        let mut remaining = self.count;

        for i in 0..self.capacity {
            if remaining == 0 {
                break;
            }
            if i == exclude_index {
                continue;
            }

            let entry = crate::frame::capabilities::entry_ref(self.entries, self.capacity, i);

            if let Some(e) = entry
                && e.is_occupied()
            {
                remaining -= 1;

                if let Some((obj_type, obj_id)) = e.object
                    && obj_type == target_type
                    && obj_id == target_id
                {
                    return true;
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── D-3.1a: growth slot constant ────────────────────────────────

    /// D-3.1a: SLOT_GROWTH is u32::MAX — never conflicts with user slots.
    #[test]
    fn test_d3_1a_growth_slot_is_u32_max() {
        assert_eq!(SLOT_GROWTH, u32::MAX);
    }

    /// D-3.1a: SLOT_GROWTH does not overlap with reserved or user slots.
    #[test]
    fn test_d3_1a_growth_slot_no_conflict() {
        assert_ne!(SLOT_GROWTH, SLOT_FAULT_HANDLER);
        assert_ne!(SLOT_GROWTH, SLOT_REPLY_FIELD);
        assert_ne!(SLOT_GROWTH, SLOT_SELF);
        assert!(SLOT_GROWTH > SLOT_USER_START);
    }

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

    // ── Test helpers ───────────────────────────────────────────────

    /// Construct a Table backed by real memory for testing.
    ///
    /// Allocates `capacity` empty entries via `frame::capabilities::alloc_test_entries`.
    /// All entries start as `Entry::empty(SlotTag(0))`.
    fn test_table(capacity: u32) -> Table {
        let entries = crate::frame::capabilities::alloc_test_entries(capacity);

        crate::frame::capabilities::init_freelist(entries, capacity, SLOT_USER_START);

        Table {
            entries,
            capacity,
            count: 0,
            free_head: if capacity > SLOT_USER_START {
                Some(SLOT_USER_START)
            } else {
                None
            },
        }
    }

    /// Construct a Table with dangling pointer for construction-only tests.
    ///
    /// Used when testing Table struct construction at extreme capacities
    /// (e.g. u32::MAX) where real allocation is impossible. Methods that
    /// dereference entries MUST NOT be called on this table.
    fn test_table_dangling(capacity: u32) -> Table {
        Table {
            entries: NonNull::dangling(),
            capacity,
            count: 0,
            free_head: None,
        }
    }

    /// Build an occupied Entry for a Field with the given badge.
    fn field_entry(badge_value: u64, slot_tag: u64) -> Entry {
        Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::FIELD_ALL,
            badge: Badge(badge_value),
            slot_tag: SlotTag(slot_tag),
            send_once: false,
            stored_generation: 0,
        }
    }

    /// Build an occupied Entry for an Observer.
    fn observer_entry(object_id: u32, slot_tag: u64) -> Entry {
        Entry {
            object: Some((ObjectType::Observer, ObjectId(object_id))),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(slot_tag),
            send_once: false,
            stored_generation: 0,
        }
    }

    // ── Spec verifier tests ──────────────────────────────────────
    //
    // Verify that each settled design decision (D-number) is
    // correctly realized in the implementation. One test per
    // assertion, named test_d{N}_{description}.

    // ── D4/D8: Handle resolution ─────────────────────────────────

    /// D4: resolve a valid handle to the installed entry.
    #[test]
    fn test_d4_resolve_valid_handle() {
        let mut table = test_table(16);
        let entry = field_entry(42, 0);

        table.install_at(3, entry);

        let handle = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        };
        let resolved = table.resolve(handle).unwrap();

        assert_eq!(resolved.badge, Badge(42));
        assert!(resolved.check_type(ObjectType::Field));
    }

    /// D8: resolve with index >= capacity returns InvalidHandle.
    #[test]
    fn test_d8_resolve_out_of_bounds() {
        let table = test_table(8);
        let handle = Handle {
            index: 8,
            slot_tag: SlotTag(0),
        };
        let result = table.resolve(handle);

        assert!(
            matches!(result, Err(CapError::InvalidHandle)),
            "D8: out-of-bounds index must return InvalidHandle"
        );
    }

    /// D8: resolve handle pointing to an empty slot returns InvalidHandle.
    #[test]
    fn test_d8_resolve_empty_slot() {
        let table = test_table(16);
        let handle = Handle {
            index: SLOT_USER_START,
            slot_tag: SlotTag(0),
        };
        let result = table.resolve(handle);

        assert!(
            matches!(result, Err(CapError::InvalidHandle)),
            "D8: empty slot must return InvalidHandle"
        );
    }

    // ── D11: ABA defense and close ───────────────────────────────

    /// D11: resolve with wrong slot_tag fails.
    #[test]
    fn test_d11_resolve_slot_tag_mismatch() {
        let mut table = test_table(16);

        table.install_at(3, field_entry(10, 0));

        let handle = Handle {
            index: 3,
            slot_tag: SlotTag(999),
        };
        let result = table.resolve(handle);

        assert!(
            matches!(result, Err(CapError::SlotTagMismatch)),
            "D11: wrong slot_tag must return SlotTagMismatch"
        );
    }

    /// D11: free_slot bumps the slot tag so old handles fail resolution.
    #[test]
    fn test_d11_free_slot_bumps_tag() {
        let mut table = test_table(16);

        table.install_at(3, field_entry(10, 0));

        let old_handle = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        };

        // Verify it resolves before freeing.
        assert!(table.resolve(old_handle).is_ok());

        table.free_slot(3);

        // Old handle must now fail — slot tag was bumped.
        let result = table.resolve(old_handle);

        assert!(
            result.is_err(),
            "stale handle after free_slot must fail resolution"
        );
    }

    /// D11: close an occupied slot returns Closed with correct type and id.
    #[test]
    fn test_d11_close_occupied_returns_closed() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Observer, ObjectId(7))),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        table.install_at(3, entry);

        let result = table.close(3);

        match result {
            CloseResult::Closed {
                object_type,
                object_id,
                ..
            } => {
                assert_eq!(object_type, ObjectType::Observer);
                assert_eq!(object_id, ObjectId(7));
            }
            other => panic!("expected CloseResult::Closed, got {other:?}"),
        }
    }

    /// D11: close an empty slot returns AlreadyEmpty.
    #[test]
    fn test_d11_close_empty_returns_already_empty() {
        let mut table = test_table(16);
        let result = table.close(SLOT_USER_START);

        assert!(
            matches!(result, CloseResult::AlreadyEmpty),
            "close on empty slot must return AlreadyEmpty"
        );
    }

    /// D11: close bumps the slot tag so old handles fail resolution.
    #[test]
    fn test_d11_close_bumps_slot_tag() {
        let mut table = test_table(16);

        table.install_at(3, field_entry(10, 0));

        let old_handle = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        };
        // Close bumps the tag.
        let _ = table.close(3);
        // Old handle must now fail.
        let result = table.resolve(old_handle);

        assert!(
            result.is_err(),
            "stale handle after close must fail resolution"
        );
    }

    // ── D8: Slot allocation ──────────────────────────────────────

    /// D8: allocate_slot on a fresh table returns a user slot.
    #[test]
    fn test_d8_allocate_slot_finds_free() {
        let mut table = test_table(16);
        let index = table.allocate_slot().unwrap();

        assert!(
            index >= SLOT_USER_START,
            "allocated slot must be >= SLOT_USER_START, got {index}"
        );
    }

    /// D8: allocate_slot on a full table returns TableFull.
    #[test]
    fn test_d8_allocate_slot_table_full() {
        // Table with capacity <= SLOT_USER_START has no user slots.
        let mut table = test_table(SLOT_USER_START);
        let result = table.allocate_slot();

        assert_eq!(result.unwrap_err(), CapError::TableFull);
    }

    /// D8: allocate_slot skips occupied slots, returning different indices.
    #[test]
    fn test_d8_allocate_slot_skips_occupied() {
        let mut table = test_table(16);
        let first = table.allocate_slot().unwrap();

        table.install_at(first, field_entry(1, 0));

        let second = table.allocate_slot().unwrap();

        assert_ne!(
            first, second,
            "second allocation must return a different slot"
        );
        assert!(second >= SLOT_USER_START);
    }

    // ── D8/D35: Installation ─────────────────────────────────────

    /// D8: install_at places an entry that resolves with correct badge/type.
    #[test]
    fn test_d8_install_at_places_entry() {
        let mut table = test_table(16);
        let entry = field_entry(99, 0);

        table.install_at(5, entry);

        let handle = Handle {
            index: 5,
            slot_tag: SlotTag(0),
        };
        let resolved = table.resolve(handle).unwrap();

        assert_eq!(resolved.badge, Badge(99));
        assert!(resolved.check_type(ObjectType::Field));
    }

    /// D8/D35: install finds a free slot and returns index >= SLOT_USER_START.
    #[test]
    fn test_d8_install_finds_free_slot() {
        let mut table = test_table(16);
        let entry = observer_entry(1, 0);
        let index = table.install(entry).unwrap();

        assert!(
            index >= SLOT_USER_START,
            "install must return index >= SLOT_USER_START, got {index}"
        );
    }

    /// D8: install on a full table returns TableFull.
    #[test]
    fn test_d8_install_table_full() {
        let mut table = test_table(8);

        // Fill all user slots (SLOT_USER_START..8).
        for _ in SLOT_USER_START..8 {
            let idx = table.allocate_slot().unwrap();

            table.install_at(idx, field_entry(0, 0));
        }

        let result = table.install(field_entry(0, 0));

        assert_eq!(result.unwrap_err(), CapError::TableFull);
    }

    // ── D17: Badge closure ─────────────────────────────────────────

    /// D17: Table::close always returns Closed, never ClosedWithBadgeClosure.
    /// Badge tracking lives in the dispatch layer (core_manager) because
    /// Table has no access to the Field arena.
    #[test]
    fn test_d17_table_close_returns_closed_for_field() {
        let mut table = test_table(8);
        let entry = field_entry(0xCAFE, 0);

        table.install_at(0, entry);

        let result = table.close(0);

        assert!(
            matches!(
                result,
                CloseResult::Closed {
                    object_type: ObjectType::Field,
                    ..
                }
            ),
            "D17: Table::close returns Closed (badge tracking in dispatch layer)"
        );
    }

    // ── D33: Preemptible cascade ─────────────────────────────────

    /// D33: begin_cascade returns initial state with position 0.
    #[test]
    fn test_d33_begin_cascade_returns_initial_state() {
        let mut table = test_table(16);
        let state = table.begin_cascade();

        assert_eq!(state.position, 0);
        assert!(!state.complete);
    }

    /// D33: cascade_step advances position past processed entries.
    #[test]
    fn test_d33_cascade_step_progresses() {
        let mut table = test_table(32);

        // Install entries to process.
        for i in 0..20 {
            table.install_at(i, observer_entry(i, 0));
        }

        let mut state = table.begin_cascade();
        let done = table.cascade_step(&mut state);

        assert!(
            state.position > 0,
            "cascade_step must advance position (got {})",
            state.position
        );

        // With 20 entries and CASCADE_STEP_SIZE=16, one step should not
        // complete the cascade.
        if !done {
            assert!(!state.complete);
        }
    }

    /// D33: cascade_step on an empty table completes immediately.
    #[test]
    fn test_d33_cascade_step_completes() {
        let mut table = test_table(8);
        let mut state = table.begin_cascade();
        let done = table.cascade_step(&mut state);

        assert!(done, "cascade on empty table must complete in one step");
        assert!(state.complete);
    }

    /// D33: cascade is bounded per step — many entries require multiple steps.
    #[test]
    fn test_d33_cascade_is_bounded_per_step() {
        let mut table = test_table(64);

        // Install entries across the full table.
        for i in 0..64 {
            table.install_at(i, observer_entry(i, 0));
        }

        let mut state = table.begin_cascade();
        let done = table.cascade_step(&mut state);

        // CASCADE_STEP_SIZE is 16 and we have 64 entries —
        // one step must not complete the entire cascade.
        assert!(
            !done,
            "cascade with 64 entries must not complete in one step"
        );
        assert!(!state.complete);
        assert!(
            state.position > 0 && state.position < 64,
            "position must have advanced but not reached the end (got {})",
            state.position
        );
    }

    // ── D51: Send-once flag ──────────────────────────────────────

    /// D51: send_once flag is preserved through install and resolve.
    #[test]
    fn test_d51_send_once_preserved_through_operations() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::FIELD_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: true,
            stored_generation: 0,
        };

        table.install_at(SLOT_USER_START, entry);

        let handle = Handle {
            index: SLOT_USER_START,
            slot_tag: SlotTag(0),
        };
        let resolved = table.resolve(handle).unwrap();

        assert!(
            resolved.is_send_once(),
            "D51: send_once must be true after install and resolve"
        );
    }

    // ── D67: Generation check ────────────────────────────────────

    /// D67: check_generation returns true when stored matches live.
    #[test]
    fn test_d67_entry_check_generation_match() {
        let entry = Entry {
            object: Some((ObjectType::Space, ObjectId(0))),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 5,
        };

        assert!(
            entry.check_generation(5),
            "D67: matching generation must return true"
        );
    }

    /// D67: check_generation returns false on mismatch.
    #[test]
    fn test_d67_entry_check_generation_mismatch() {
        let entry = Entry {
            object: Some((ObjectType::Space, ObjectId(0))),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 5,
        };

        assert!(
            !entry.check_generation(6),
            "D67: mismatched generation must return false"
        );
        assert!(
            !entry.check_generation(0),
            "D67: zero vs stored=5 must return false"
        );
    }

    // ── D57: Reserved slots ──────────────────────────────────────

    /// D57: allocate_slot never returns a reserved slot index.
    #[test]
    fn test_d57_allocate_slot_skips_reserved() {
        let mut table = test_table(16);

        // Allocate all available user slots.
        for _ in SLOT_USER_START..16 {
            let index = table.allocate_slot().unwrap();

            assert!(
                index >= SLOT_USER_START,
                "D57: allocated slot {index} is in the reserved range [0, {SLOT_USER_START})"
            );

            table.install_at(index, field_entry(0, 0));
        }
    }

    // ── Adversarial tests ─────────────────────────────────────────
    //
    // Boundary conditions, interleaved operations, state corruption
    // sequences, and edge cases designed to surface bugs in the
    // capability module implementation.

    // ── Table boundary conditions ─────────────────────────────────

    /// Resolve with index 0 — minimum valid index (reserved slot).
    #[test]

    fn test_adversarial_cap_resolve_index_zero() {
        let table = test_table(16);
        let handle = Handle {
            index: 0,
            slot_tag: SlotTag(0),
        };
        let _result = table.resolve(handle);
    }

    /// Resolve with index = capacity - 1 — last valid index.
    #[test]

    fn test_adversarial_cap_resolve_last_valid_index() {
        let table = test_table(16);
        let handle = Handle {
            index: 15,
            slot_tag: SlotTag(0),
        };
        let _result = table.resolve(handle);
    }

    /// Resolve with index = capacity — first invalid index (off-by-one).
    /// Must return InvalidHandle, not access out-of-bounds memory.
    #[test]

    fn test_adversarial_cap_resolve_at_capacity_boundary() {
        let table = test_table(16);
        let handle = Handle {
            index: 16,
            slot_tag: SlotTag(0),
        };
        let result = table.resolve(handle);

        assert!(matches!(result, Err(CapError::InvalidHandle)));
    }

    /// Resolve with index far beyond capacity — u32::MAX.
    #[test]

    fn test_adversarial_cap_resolve_index_u32_max() {
        let table = test_table(16);
        let handle = Handle {
            index: u32::MAX,
            slot_tag: SlotTag(0),
        };
        let result = table.resolve(handle);

        assert!(matches!(result, Err(CapError::InvalidHandle)));
    }

    /// Resolve on a zero-capacity table — any index is out of bounds.
    #[test]

    fn test_adversarial_cap_resolve_zero_capacity_table() {
        let table = test_table(0);
        let handle = Handle {
            index: 0,
            slot_tag: SlotTag(0),
        };
        let result = table.resolve(handle);

        assert!(matches!(result, Err(CapError::InvalidHandle)));
    }

    /// allocate_slot on a table with capacity 1 — single-slot table.
    #[test]

    fn test_adversarial_cap_allocate_slot_capacity_one() {
        let mut table = test_table(1);
        let _result = table.allocate_slot();
    }

    /// allocate_slot on a table with capacity 0 — empty table.
    /// Must return TableFull without panicking.
    #[test]

    fn test_adversarial_cap_allocate_slot_capacity_zero() {
        let mut table = test_table(0);
        let result = table.allocate_slot();

        assert!(matches!(result, Err(CapError::TableFull)));
    }

    /// install_at with index = capacity - 1 — boundary of valid range.
    #[test]

    fn test_adversarial_cap_install_at_last_valid_index() {
        let mut table = test_table(16);
        let entry = field_entry(99, 0);

        table.install_at(15, entry);
    }

    /// install_at with index = capacity — out of bounds.
    /// Must not silently corrupt memory.
    #[test]

    fn test_adversarial_cap_install_at_capacity_boundary() {
        let mut table = test_table(16);
        let entry = field_entry(99, 0);

        table.install_at(16, entry);
    }

    /// close with index 0 — minimum index (reserved fault handler slot).
    #[test]

    fn test_adversarial_cap_close_index_zero() {
        let mut table = test_table(16);
        let _result = table.close(0);
    }

    /// close with index = capacity — out of bounds.
    /// Must not access memory past the table.
    #[test]

    fn test_adversarial_cap_close_at_capacity_boundary() {
        let mut table = test_table(16);
        let _result = table.close(16);
    }

    /// close with index = u32::MAX — far out of bounds.
    #[test]

    fn test_adversarial_cap_close_index_u32_max() {
        let mut table = test_table(16);
        let _result = table.close(u32::MAX);
    }

    /// free_slot with index = capacity — out of bounds.
    #[test]

    fn test_adversarial_cap_free_slot_at_capacity_boundary() {
        let mut table = test_table(16);

        table.free_slot(16);
    }

    /// resolve_mut with valid index — mutable resolution path.
    #[test]

    fn test_adversarial_cap_resolve_mut_valid() {
        let mut table = test_table(16);
        let handle = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        };
        let _result = table.resolve_mut(handle);
    }

    /// resolve_mut with index = capacity — out of bounds.
    #[test]

    fn test_adversarial_cap_resolve_mut_out_of_bounds() {
        let mut table = test_table(16);
        let handle = Handle {
            index: 16,
            slot_tag: SlotTag(0),
        };
        let result = table.resolve_mut(handle);

        assert!(matches!(result, Err(CapError::InvalidHandle)));
    }

    // ── Interleaved operations ────────────────────────────────────

    /// allocate_slot -> install_at -> close -> allocate_slot.
    /// After closing a slot, it should be available for reuse.
    #[test]

    fn test_adversarial_cap_slot_reuse_after_close() {
        let mut table = test_table(4);
        let idx = table.allocate_slot().unwrap();

        table.install_at(idx, observer_entry(1, 0));

        let _close_result = table.close(idx);
        let idx2 = table.allocate_slot().unwrap();

        assert!(idx2 < 4);
    }

    /// install -> resolve -> close -> resolve.
    /// After close, resolve with the old handle must fail (stale handle).
    #[test]

    fn test_adversarial_cap_stale_handle_after_close() {
        let mut table = test_table(16);
        let entry = field_entry(42, 0);
        let idx = table.install(entry).unwrap();
        let handle = Handle {
            index: idx,
            slot_tag: SlotTag(0),
        };
        let _resolved = table.resolve(handle).unwrap();
        let _close_result = table.close(idx);
        let stale_result = table.resolve(handle);

        assert!(
            matches!(
                stale_result,
                Err(CapError::SlotTagMismatch) | Err(CapError::InvalidHandle)
            ),
            "stale handle after close must fail"
        );
    }

    /// Double close on the same index — must not corrupt state.
    #[test]

    fn test_adversarial_cap_double_close() {
        let mut table = test_table(16);

        table.install_at(5, observer_entry(1, 0));

        let first = table.close(5);

        assert!(matches!(first, CloseResult::Closed { .. }));

        let second = table.close(5);

        assert!(matches!(second, CloseResult::AlreadyEmpty));
    }

    /// Fill table completely, free one slot, allocate again.
    /// The freed slot must be the one returned.
    #[test]

    fn test_adversarial_cap_fill_free_reallocate() {
        let mut table = test_table(8);

        // Fill all user slots (3..8).
        for _ in SLOT_USER_START..8 {
            let idx = table.allocate_slot().unwrap();

            table.install_at(idx, field_entry(0, 0));
        }

        // Table should be full for user allocations.
        let full_result = table.allocate_slot();

        assert!(matches!(full_result, Err(CapError::TableFull)));

        // Free slot 5 specifically.
        table.free_slot(5);

        // Now allocate — must get slot 5 back (only free slot).
        let recovered = table.allocate_slot().unwrap();

        assert_eq!(recovered, 5);
    }

    // ── State corruption sequences ────────────────────────────────

    /// Install at every slot, close every slot, verify count is 0.
    #[test]

    fn test_adversarial_cap_install_all_close_all_count_zero() {
        let mut table = test_table(8);

        for i in 0..8 {
            table.install_at(i, observer_entry(i, 0));
        }
        for i in 0..8 {
            let result = table.close(i);

            assert!(
                matches!(result, CloseResult::Closed { .. }),
                "slot {i} should have been occupied"
            );
        }

        assert_eq!(table.count, 0, "count must be 0 after closing all slots");
    }

    /// Rapidly interleave install and close — count must stay consistent.
    #[test]

    fn test_adversarial_cap_interleaved_install_close_count() {
        let mut table = test_table(16);

        for i in 0..5u32 {
            table.install_at(SLOT_USER_START + i, field_entry(i as u64, 0));
        }
        for i in 0..3u32 {
            let _result = table.close(SLOT_USER_START + i);
        }

        let _a = table.install(field_entry(100, 0)).unwrap();
        let _b = table.install(field_entry(101, 0)).unwrap();

        // 5 installed - 3 closed + 2 installed = 4 occupied.
        assert_eq!(table.count, 4, "count must reflect install/close balance");
    }

    /// free_slot then close on the same index — free_slot makes it empty,
    /// close should return AlreadyEmpty.
    #[test]

    fn test_adversarial_cap_free_then_close_same_slot() {
        let mut table = test_table(16);

        table.install_at(5, field_entry(42, 0));
        table.free_slot(5);

        let result = table.close(5);

        assert!(
            matches!(result, CloseResult::AlreadyEmpty),
            "close after free_slot should return AlreadyEmpty"
        );
    }

    // ── CascadeState edge cases ───────────────────────────────────

    /// begin_cascade on an empty table (capacity 0).
    #[test]

    fn test_adversarial_cap_begin_cascade_empty_table() {
        let mut table = test_table(0);
        let state = table.begin_cascade();

        assert_eq!(state.position, 0);
        assert!(!state.complete);
    }

    /// cascade_step called repeatedly until complete — must terminate.
    #[test]

    fn test_adversarial_cap_cascade_terminates() {
        let mut table = test_table(64);

        table.count = 64;

        let mut state = table.begin_cascade();
        let mut steps = 0u32;

        loop {
            let done = table.cascade_step(&mut state);

            steps += 1;

            if done {
                break;
            }

            assert!(
                steps < 1000,
                "cascade did not terminate after {steps} steps"
            );
        }

        assert!(state.complete);
    }

    /// cascade_step on an already-complete state.
    /// Must not panic or corrupt state — should remain complete.
    #[test]

    fn test_adversarial_cap_cascade_step_on_complete() {
        let mut table = test_table(0);
        let mut state = table.begin_cascade();
        let done = table.cascade_step(&mut state);

        assert!(done);
        assert!(state.complete);

        let done_again = table.cascade_step(&mut state);

        assert!(done_again);
        assert!(state.complete);
    }

    /// CascadeState initial conditions — position 0, not complete.
    #[test]
    fn test_adversarial_cap_cascade_state_initial() {
        let state = CascadeState {
            position: 0,
            complete: false,
        };

        assert_eq!(state.position, 0);
        assert!(!state.complete);
    }

    // ── Entry edge cases ──────────────────────────────────────────

    /// check_rights with Rights::empty() — no rights required, always passes.
    #[test]
    fn test_adversarial_cap_check_rights_empty_required() {
        let entry = field_entry(0, 0);

        assert!(entry.check_rights(Rights::empty()));
    }

    /// check_rights with Rights::empty() on an entry with no rights.
    #[test]
    fn test_adversarial_cap_check_rights_empty_on_empty() {
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::empty(),
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        assert!(entry.check_rights(Rights::empty()));
        assert!(!entry.check_rights(Rights::SEND));
    }

    /// check_rights with the full type mask — all rights present.
    #[test]
    fn test_adversarial_cap_check_rights_full_mask() {
        let entry = Entry {
            object: Some((ObjectType::Observer, ObjectId(0))),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        assert!(entry.check_rights(Rights::OBSERVER_ALL));
        assert!(entry.check_rights(Rights::RESUME));
        assert!(entry.check_rights(Rights::DESTROY));
        assert!(entry.check_rights(Rights::INSTALL_CAP));
        assert!(entry.check_rights(Rights::WRITE_REGISTERS));
        assert!(entry.check_rights(Rights::READ_REGISTERS));
        assert!(entry.check_rights(Rights::SUSPEND));
        assert!(entry.check_rights(Rights::CHANGE_HANDLER));
        assert!(entry.check_rights(Rights::MODIFY_SCHEDULING));
        assert!(entry.check_rights(Rights::CLONE));
    }

    /// check_rights for a right NOT in the type's mask — cross-type leak.
    #[test]
    fn test_adversarial_cap_check_rights_cross_type_right() {
        let entry = Entry {
            object: Some((ObjectType::Space, ObjectId(0))),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        assert!(!entry.check_rights(Rights::SEND));
        assert!(!entry.check_rights(Rights::RESUME));
    }

    /// check_type with wrong type returns false.
    #[test]
    fn test_adversarial_cap_check_type_mismatch() {
        let entry = field_entry(0, 0);

        assert!(!entry.check_type(ObjectType::Space));
        assert!(!entry.check_type(ObjectType::Time));
        assert!(!entry.check_type(ObjectType::Observer));
        assert!(!entry.check_type(ObjectType::Pulsar));
        assert!(entry.check_type(ObjectType::Field));
    }

    /// check_type on an empty entry — must return false for any type.
    #[test]
    fn test_adversarial_cap_check_type_on_empty() {
        let entry = Entry::empty(SlotTag(0));

        assert!(!entry.check_type(ObjectType::Space));
        assert!(!entry.check_type(ObjectType::Time));
        assert!(!entry.check_type(ObjectType::Field));
        assert!(!entry.check_type(ObjectType::Observer));
        assert!(!entry.check_type(ObjectType::Pulsar));
    }

    /// is_occupied on Entry::empty() — must return false.
    #[test]
    fn test_adversarial_cap_is_occupied_on_empty() {
        let entry = Entry::empty(SlotTag(0));

        assert!(!entry.is_occupied());
    }

    /// is_occupied on an occupied entry — must return true.
    #[test]
    fn test_adversarial_cap_is_occupied_on_occupied() {
        let entry = observer_entry(0, 0);

        assert!(entry.is_occupied());
    }

    /// check_generation with 0 vs 0 — both zero, should match.
    #[test]
    fn test_adversarial_cap_check_generation_zero() {
        let entry = Entry::empty(SlotTag(0));

        assert!(entry.check_generation(0));
        assert!(!entry.check_generation(1));
    }

    /// check_generation at u64 boundary values.
    #[test]
    fn test_adversarial_cap_check_generation_u64_extremes() {
        let entry = Entry {
            object: Some((ObjectType::Space, ObjectId(0))),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: u64::MAX,
        };

        assert!(entry.check_generation(u64::MAX));
        assert!(!entry.check_generation(u64::MAX - 1));
        assert!(!entry.check_generation(0));
    }

    /// is_send_once on entry with flag set.
    #[test]
    fn test_adversarial_cap_is_send_once_true() {
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::FIELD_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: true,
            stored_generation: 0,
        };

        assert!(entry.is_send_once());
    }

    /// is_send_once on entry without flag — must return false.
    #[test]
    fn test_adversarial_cap_is_send_once_false() {
        let entry = field_entry(0, 0);

        assert!(!entry.is_send_once());
    }

    // ── SlotTag edge cases ────────────────────────────────────────

    /// free_slot called — slot tag wrapping concern documented.
    /// If the tag wraps around u32::MAX -> 0, it could alias old handles.
    #[test]

    fn test_adversarial_cap_slot_tag_wraps_on_overflow() {
        let mut table = test_table(4);

        table.free_slot(0);
    }

    // ── Handle construction extremes ──────────────────────────────

    /// Handle with index u32::MAX — must not cause arithmetic overflow
    /// during bounds checking.
    #[test]

    fn test_adversarial_cap_handle_index_u32_max() {
        let table = test_table(16);
        let handle = Handle {
            index: u32::MAX,
            slot_tag: SlotTag(0),
        };
        let result = table.resolve(handle);

        assert!(matches!(result, Err(CapError::InvalidHandle)));
    }

    /// Handle with slot_tag u32::MAX — must not cause comparison issues.
    #[test]

    fn test_adversarial_cap_handle_slot_tag_u32_max() {
        let table = test_table(16);
        let handle = Handle {
            index: 0,
            slot_tag: SlotTag(u32::MAX as u64),
        };
        let result = table.resolve(handle);

        assert!(
            matches!(
                result,
                Err(CapError::SlotTagMismatch) | Err(CapError::InvalidHandle)
            ),
            "extreme slot tag must not crash"
        );
    }

    /// Handle with both index and slot_tag at u32::MAX.
    #[test]

    fn test_adversarial_cap_handle_all_max() {
        let table = test_table(16);
        let handle = Handle {
            index: u32::MAX,
            slot_tag: SlotTag(u32::MAX as u64),
        };
        let result = table.resolve(handle);

        assert!(matches!(result, Err(CapError::InvalidHandle)));
    }

    // ── Rights algebra edge cases ─────────────────────────────────

    /// Attenuate with empty mask zeroes all rights.
    #[test]
    fn test_adversarial_cap_attenuate_with_empty_mask() {
        let full = Rights::OBSERVER_ALL;
        let result = full.attenuate(Rights::empty());

        assert_eq!(result, Rights::empty());
        assert_eq!(result.bits(), 0);
    }

    /// Attenuate is idempotent — applying the same mask twice gives
    /// the same result.
    #[test]
    fn test_adversarial_cap_attenuate_idempotent() {
        let full = Rights::FIELD_ALL;
        let mask = Rights::SEND.union(Rights::RECEIVE);
        let once = full.attenuate(mask);
        let twice = once.attenuate(mask);

        assert_eq!(once, twice);
    }

    /// Union is commutative.
    #[test]
    fn test_adversarial_cap_union_commutative() {
        let a = Rights::SEND;
        let b = Rights::DESTROY;

        assert_eq!(a.union(b), b.union(a));
    }

    /// Contains is reflexive — any rights set contains itself.
    #[test]
    fn test_adversarial_cap_contains_reflexive() {
        assert!(Rights::OBSERVER_ALL.contains(Rights::OBSERVER_ALL));
        assert!(Rights::empty().contains(Rights::empty()));
        assert!(Rights::SEND.contains(Rights::SEND));
    }

    /// No type-specific rights bits overlap between different types.
    #[test]
    fn test_adversarial_cap_type_specific_bits_disjoint() {
        let shared = Rights::DESTROY.union(Rights::CLONE).union(Rights::SPLIT);
        let space_specific = Rights(Rights::SPACE_ALL.bits() & !shared.bits());
        let time_specific = Rights(Rights::TIME_ALL.bits() & !shared.bits());
        let field_specific = Rights(Rights::FIELD_ALL.bits() & !shared.bits());
        let observer_specific = Rights(Rights::OBSERVER_ALL.bits() & !shared.bits());
        let pulsar_specific = Rights(Rights::PULSAR_ALL.bits() & !shared.bits());

        assert_eq!(
            field_specific.bits() & observer_specific.bits(),
            0,
            "Field and Observer type-specific rights overlap"
        );
        assert_eq!(
            space_specific.bits() & field_specific.bits(),
            0,
            "Space and Field type-specific rights overlap"
        );
        assert_eq!(
            space_specific.bits() & observer_specific.bits(),
            0,
            "Space and Observer type-specific rights overlap"
        );
        assert_eq!(
            time_specific.bits(),
            0,
            "Time has unexpected specific rights"
        );
        assert_eq!(
            pulsar_specific.bits(),
            0,
            "Pulsar has unexpected specific rights"
        );
    }

    // ── Badge edge cases ──────────────────────────────────────────

    /// Badge with u64::MAX — maximum value.
    #[test]
    fn test_adversarial_cap_badge_u64_max() {
        let badge = Badge(u64::MAX);

        assert_eq!(badge.0, u64::MAX);

        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::FIELD_ALL,
            badge,
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        assert_eq!(entry.badge, Badge(u64::MAX));
    }

    /// Badge 0 is valid — distinguishable from CAP_ABSENT.
    #[test]
    fn test_adversarial_cap_badge_zero_not_absent() {
        let badge = Badge(0);

        assert_ne!(badge.0, CAP_ABSENT);
    }

    // ── Entry::empty edge cases ───────────────────────────────────

    /// Entry::empty preserves the given slot tag.
    #[test]
    fn test_adversarial_cap_empty_entry_preserves_tag() {
        let tag = SlotTag(42);
        let entry = Entry::empty(tag);

        assert_eq!(entry.slot_tag.0, 42);
        assert!(!entry.is_occupied());
        assert_eq!(entry.rights, Rights::empty());
        assert_eq!(entry.badge, Badge(0));
        assert!(!entry.send_once);
        assert_eq!(entry.stored_generation, 0);
    }

    /// Entry::empty with SlotTag(u32::MAX as u64).
    #[test]
    fn test_adversarial_cap_empty_entry_max_tag() {
        let entry = Entry::empty(SlotTag(u32::MAX as u64));

        assert_eq!(entry.slot_tag.0, u32::MAX as u64);
        assert!(!entry.is_occupied());
    }

    // ── TransferredCap construction ───────────────────────────────

    /// TransferredCap can represent all object types with extreme values.
    #[test]
    fn test_adversarial_cap_transferred_cap_extreme_values() {
        let cap = TransferredCap {
            object_type: ObjectType::Field,
            object_id: ObjectId(u32::MAX),
            rights: Rights::FIELD_ALL,
            badge: Badge(u64::MAX),
            send_once: true,
            stored_generation: u64::MAX,
        };

        assert_eq!(cap.object_id, ObjectId(u32::MAX));
        assert_eq!(cap.badge, Badge(u64::MAX));
        assert_eq!(cap.stored_generation, u64::MAX);
        assert!(cap.send_once);
    }

    // ── Sentinel value correctness ────────────────────────────────

    /// CAP_ABSENT must never collide with a valid slot index.
    #[test]
    fn test_adversarial_cap_absent_not_valid_index() {
        assert_eq!(CAP_ABSENT, u64::MAX);
        assert!(CAP_ABSENT > u32::MAX as u64);
    }

    // ── Table with large capacity ─────────────────────────────────

    /// Table with capacity u32::MAX — construction should not panic.
    #[test]
    fn test_adversarial_cap_table_max_capacity() {
        let table = test_table_dangling(u32::MAX);

        assert_eq!(table.capacity, u32::MAX);
        assert_eq!(table.count, 0);
    }

    /// Resolve on a large table at the last valid index — the entry is
    /// empty so resolve returns InvalidHandle. Uses a realistically-sized
    /// table (not u32::MAX, which cannot be allocated in test memory).
    #[test]
    fn test_adversarial_cap_resolve_large_table_last_index() {
        let table = test_table(1024);
        let handle = Handle {
            index: 1023,
            slot_tag: SlotTag(0),
        };
        let result = table.resolve(handle);

        assert!(
            matches!(result, Err(CapError::InvalidHandle)),
            "empty slot at last valid index must return InvalidHandle"
        );
    }

    // ── D77: Handle encoding/decoding ────────────────────────────────

    /// D77: encode packs index in low 16, slot_tag in high 48.
    #[test]
    fn test_d77_handle_encode_packs_correctly() {
        let handle = Handle {
            index: 42,
            slot_tag: SlotTag(7),
        };
        let encoded = handle.encode();

        assert_eq!(encoded & 0xFFFF, 42, "low 16 must be index");
        assert_eq!(encoded >> 16, 7, "high 48 must be slot_tag");
    }

    /// D77: decode extracts index from low 16, slot_tag from high 48.
    #[test]
    fn test_d77_handle_decode_extracts_correctly() {
        let raw: u64 = 42 | (7u64 << 16);
        let handle = Handle::decode(raw);

        assert_eq!(handle.index, 42);
        assert_eq!(handle.slot_tag, SlotTag(7));
    }

    /// D77: encode/decode roundtrip is identity.
    #[test]
    fn test_d77_handle_encode_decode_roundtrip() {
        let original = Handle {
            index: 1000,
            slot_tag: SlotTag(999),
        };
        let decoded = Handle::decode(original.encode());

        assert_eq!(decoded.index, original.index);
        assert_eq!(decoded.slot_tag, original.slot_tag);
    }

    /// D77: decode with maximum ABI values does not overflow.
    #[test]
    fn test_d77_handle_decode_max_values() {
        let raw: u64 = 0xFFFF | (0xFFFF_FFFF_FFFFu64 << 16);
        let handle = Handle::decode(raw);

        assert_eq!(handle.index, 0xFFFF);
        assert_eq!(handle.slot_tag, SlotTag(0xFFFF_FFFF_FFFF));
    }

    /// D77: slot tag ABI carries 48 bits — 2^48 reuses before wrap.
    #[test]
    fn test_d77_slot_tag_abi_48bit_coverage() {
        let tag_large = SlotTag(0xFFFF_FFFF_FFFF);
        let handle = Handle {
            index: 0,
            slot_tag: tag_large,
        };
        let decoded = Handle::decode(handle.encode());

        assert_eq!(
            decoded.slot_tag, tag_large,
            "full 48-bit tag must survive encode/decode"
        );
    }

    /// D77: decode with zero produces index=0, slot_tag=0.
    #[test]
    fn test_d77_handle_decode_zero() {
        let handle = Handle::decode(0);

        assert_eq!(handle.index, 0);
        assert_eq!(handle.slot_tag, SlotTag(0));
    }

    /// D77: encode with index=0, slot_tag=0 produces 0.
    #[test]
    fn test_d77_handle_encode_zero() {
        let handle = Handle {
            index: 0,
            slot_tag: SlotTag(0),
        };

        assert_eq!(handle.encode(), 0);
    }

    // ── D77: resolve_cap — full resolution protocol ──────────────────

    /// D77: resolve_cap succeeds for a valid handle with matching
    /// generation, sufficient rights, and correct type.
    #[test]
    fn test_d77_resolve_cap_valid() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(5))),
            rights: Rights::FIELD_ALL,
            badge: Badge(42),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 10,
        };

        table.install_at(3, entry);

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap(
            raw,
            table.entries,
            table.capacity,
            10, // live generation matches
            Rights::SEND,
            Some(ObjectType::Field),
        );

        assert!(result.is_ok(), "valid cap must resolve");

        let resolved = result.unwrap();

        assert_eq!(resolved.object_id, ObjectId(5));
        assert_eq!(resolved.object_type, ObjectType::Field);
        assert_eq!(resolved.badge, Badge(42));
        assert!(!resolved.send_once);
    }

    /// D77: resolve_cap fails with InvalidHandle for out-of-bounds index.
    #[test]
    fn test_d77_resolve_cap_out_of_bounds() {
        let table = test_table(16);
        let raw = Handle {
            index: 16,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap(raw, table.entries, table.capacity, 0, Rights::empty(), None);

        assert_eq!(result.unwrap_err(), CapError::InvalidHandle);
    }

    /// D77: resolve_cap fails with InvalidHandle for empty slot.
    #[test]
    fn test_d77_resolve_cap_empty_slot() {
        let table = test_table(16);
        let raw = Handle {
            index: SLOT_USER_START,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap(raw, table.entries, table.capacity, 0, Rights::empty(), None);

        assert_eq!(result.unwrap_err(), CapError::InvalidHandle);
    }

    /// D77: resolve_cap fails with SlotTagMismatch for stale handle
    /// (D11 ABA defense).
    #[test]
    fn test_d77_resolve_cap_slot_tag_mismatch() {
        let mut table = test_table(16);

        table.install_at(3, field_entry(42, 0));

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(999), // wrong tag
        }
        .encode();
        let result = resolve_cap(raw, table.entries, table.capacity, 0, Rights::empty(), None);

        assert_eq!(result.unwrap_err(), CapError::SlotTagMismatch);
    }

    /// D77: resolve_cap fails with StaleGeneration for revoked cap
    /// (D67 generation mismatch).
    #[test]
    fn test_d77_resolve_cap_stale_generation() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::FIELD_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 5, // stored at creation
        };

        table.install_at(3, entry);

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap(
            raw,
            table.entries,
            table.capacity,
            6, // live generation bumped — cap is stale
            Rights::empty(),
            None,
        );

        assert_eq!(result.unwrap_err(), CapError::StaleGeneration);
    }

    /// D77: resolve_cap fails with InsufficientRights when required
    /// rights are not present (D52).
    #[test]
    fn test_d77_resolve_cap_insufficient_rights() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::SEND, // only SEND
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        table.install_at(3, entry);

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap(
            raw,
            table.entries,
            table.capacity,
            0,
            Rights::RECEIVE, // requires RECEIVE, entry only has SEND
            None,
        );

        assert_eq!(result.unwrap_err(), CapError::InsufficientRights);
    }

    /// D77: resolve_cap fails with TypeMismatch for wrong object type.
    #[test]
    fn test_d77_resolve_cap_type_mismatch() {
        let mut table = test_table(16);

        table.install_at(3, field_entry(0, 0));

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap(
            raw,
            table.entries,
            table.capacity,
            0,
            Rights::empty(),
            Some(ObjectType::Observer), // entry is Field, not Observer
        );

        assert_eq!(result.unwrap_err(), CapError::TypeMismatch);
    }

    /// D77: resolve_cap with expected_type=None accepts any type
    /// (for generic operations like Destroy/Clone/Close).
    #[test]
    fn test_d77_resolve_cap_no_type_check() {
        let mut table = test_table(16);

        table.install_at(3, field_entry(0, 0));

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap(
            raw,
            table.entries,
            table.capacity,
            0,
            Rights::empty(),
            None, // no type check — generic operation
        );

        assert!(result.is_ok());
        assert_eq!(result.unwrap().object_type, ObjectType::Field);
    }

    /// D77: resolve_cap preserves send_once flag from the entry.
    #[test]
    fn test_d77_resolve_cap_preserves_send_once() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::FIELD_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: true,
            stored_generation: 0,
        };

        table.install_at(3, entry);

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        }
        .encode();
        let resolved =
            resolve_cap(raw, table.entries, table.capacity, 0, Rights::empty(), None).unwrap();

        assert!(
            resolved.send_once,
            "D77: resolve_cap must carry send_once from the entry"
        );
    }

    /// D77: resolve_cap checks are ordered — slot tag checked before
    /// generation. A handle with wrong tag AND stale generation returns
    /// SlotTagMismatch, not StaleGeneration.
    #[test]
    fn test_d77_resolve_cap_check_order_tag_before_generation() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::FIELD_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 5,
        };

        table.install_at(3, entry);

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(999), // wrong tag
        }
        .encode();
        let result = resolve_cap(
            raw,
            table.entries,
            table.capacity,
            99, // also stale generation
            Rights::empty(),
            None,
        );

        // Tag check comes before generation check.
        assert_eq!(result.unwrap_err(), CapError::SlotTagMismatch);
    }

    /// D77: resolve_cap checks are ordered — generation checked before
    /// rights. A cap with stale generation AND insufficient rights
    /// returns StaleGeneration, not InsufficientRights.
    #[test]
    fn test_d77_resolve_cap_check_order_generation_before_rights() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::SEND,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 5,
        };

        table.install_at(3, entry);

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap(
            raw,
            table.entries,
            table.capacity,
            6,               // stale
            Rights::RECEIVE, // also insufficient
            None,
        );

        assert_eq!(result.unwrap_err(), CapError::StaleGeneration);
    }

    /// D77: resolve_cap checks are ordered — rights checked before type.
    /// A cap with insufficient rights AND wrong type returns
    /// InsufficientRights, not TypeMismatch.
    #[test]
    fn test_d77_resolve_cap_check_order_rights_before_type() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Field, ObjectId(0))),
            rights: Rights::SEND,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        table.install_at(3, entry);

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap(
            raw,
            table.entries,
            table.capacity,
            0,
            Rights::RECEIVE,            // insufficient
            Some(ObjectType::Observer), // also wrong type
        );

        assert_eq!(result.unwrap_err(), CapError::InsufficientRights);
    }

    // ── D77: resolve_cap_entry — partial resolution ──────────────────

    /// D77: resolve_cap_entry returns the entry for a valid handle.
    #[test]
    fn test_d77_resolve_cap_entry_valid() {
        let mut table = test_table(16);

        table.install_at(3, field_entry(42, 0));

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap_entry(raw, table.entries, table.capacity);

        assert!(result.is_ok());

        let entry = result.unwrap();

        assert_eq!(entry.badge, Badge(42));
    }

    /// D77: resolve_cap_entry fails for empty slot.
    #[test]
    fn test_d77_resolve_cap_entry_empty_slot() {
        let table = test_table(16);
        let raw = Handle {
            index: SLOT_USER_START,
            slot_tag: SlotTag(0),
        }
        .encode();
        let result = resolve_cap_entry(raw, table.entries, table.capacity);

        assert_eq!(result.unwrap_err(), CapError::InvalidHandle);
    }

    /// D77: resolve_cap_entry fails for wrong slot tag.
    #[test]
    fn test_d77_resolve_cap_entry_wrong_tag() {
        let mut table = test_table(16);

        table.install_at(3, field_entry(0, 0));

        let raw = Handle {
            index: 3,
            slot_tag: SlotTag(99),
        }
        .encode();
        let result = resolve_cap_entry(raw, table.entries, table.capacity);

        assert_eq!(result.unwrap_err(), CapError::SlotTagMismatch);
    }

    // ── D77: adversarial handle encoding ─────────────────────────────

    /// D77: handle with u64::MAX — decode must not panic.
    #[test]
    fn test_d77_handle_decode_u64_max() {
        let handle = Handle::decode(u64::MAX);

        assert_eq!(handle.index, 0xFFFF);
        assert_eq!(handle.slot_tag, SlotTag(0xFFFF_FFFF_FFFF));
    }

    /// D77: resolve_cap with u64::MAX raw handle on a small table
    /// must return InvalidHandle (index u32::MAX > capacity).
    #[test]
    fn test_d77_resolve_cap_max_raw_handle() {
        let table = test_table(16);
        let result = resolve_cap(
            u64::MAX,
            table.entries,
            table.capacity,
            0,
            Rights::empty(),
            None,
        );

        assert_eq!(result.unwrap_err(), CapError::InvalidHandle);
    }

    /// D77: resolve_cap with raw handle 0 on a table where slot 0
    /// has a reserved entry (empty by default) returns InvalidHandle.
    #[test]
    fn test_d77_resolve_cap_zero_handle() {
        let table = test_table(16);
        let result = resolve_cap(0, table.entries, table.capacity, 0, Rights::empty(), None);

        assert_eq!(result.unwrap_err(), CapError::InvalidHandle);
    }

    /// D77: resolve_cap returns correct rights and badge from
    /// the entry — no mixing between fields.
    #[test]
    fn test_d77_resolve_cap_returns_entry_fields() {
        let mut table = test_table(16);
        let entry = Entry {
            object: Some((ObjectType::Observer, ObjectId(99))),
            rights: Rights::FAULT_OBSERVER,
            badge: Badge(0xDEAD_BEEF),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 7,
        };

        table.install_at(5, entry);

        let raw = Handle {
            index: 5,
            slot_tag: SlotTag(0),
        }
        .encode();
        let resolved = resolve_cap(
            raw,
            table.entries,
            table.capacity,
            7,
            Rights::RESUME,
            Some(ObjectType::Observer),
        )
        .unwrap();

        assert_eq!(resolved.object_id, ObjectId(99));
        assert_eq!(resolved.object_type, ObjectType::Observer);
        assert_eq!(resolved.rights, Rights::FAULT_OBSERVER);
        assert_eq!(resolved.badge, Badge(0xDEAD_BEEF));
        assert!(!resolved.send_once);
    }

    // ── Handle encoding/decoding ─────────────────────────────────────

    #[test]
    fn handle_encode_decode_roundtrip() {
        let handle = Handle {
            index: 42,
            slot_tag: SlotTag(7),
        };
        let encoded = handle.encode();
        let decoded = Handle::decode(encoded);

        assert_eq!(decoded.index, 42);
        assert_eq!(decoded.slot_tag, SlotTag(7));
    }

    #[test]
    fn handle_encode_zero_index_and_tag() {
        let h = Handle {
            index: 0,
            slot_tag: SlotTag(0),
        };

        assert_eq!(h.encode(), 0);
    }

    #[test]
    fn handle_encode_max_index() {
        let h = Handle {
            index: MAX_HANDLE_INDEX,
            slot_tag: SlotTag(0),
        };

        assert_eq!(h.encode(), MAX_HANDLE_INDEX as u64);
        assert_eq!(Handle::decode(h.encode()).index, MAX_HANDLE_INDEX);
    }

    #[test]
    fn handle_encode_max_tag() {
        let h = Handle {
            index: 0,
            slot_tag: SlotTag(u32::MAX as u64),
        };
        let decoded = Handle::decode(h.encode());

        assert_eq!(decoded.index, 0);
        assert_eq!(decoded.slot_tag, SlotTag(u32::MAX as u64));
    }

    #[test]
    fn handle_decode_arbitrary_u64() {
        let h = Handle::decode(u64::MAX);

        assert_eq!(h.index, 0xFFFF);
        assert_eq!(h.slot_tag, SlotTag(0xFFFF_FFFF_FFFF));
    }

    // ── Rights ───────────────────────────────────────────────────────

    #[test]
    fn rights_empty_has_no_bits() {
        assert_eq!(Rights::empty().bits(), 0);
    }

    #[test]
    fn rights_union_combines_bits() {
        let r = Rights::SEND.union(Rights::RECEIVE);

        assert!(r.contains(Rights::SEND));
        assert!(r.contains(Rights::RECEIVE));
    }

    #[test]
    fn rights_all_object_rights_are_nonzero() {
        assert_ne!(Rights::FIELD_ALL.bits(), 0);
        assert_ne!(Rights::OBSERVER_ALL.bits(), 0);
        assert_ne!(Rights::SPACE_ALL.bits(), 0);
        assert_ne!(Rights::TIME_ALL.bits(), 0);
        assert_ne!(Rights::PULSAR_ALL.bits(), 0);
    }

    #[test]
    fn rights_contains_subset() {
        let all = Rights::FIELD_ALL;

        assert!(all.contains(Rights::SEND));
        assert!(all.contains(Rights::RECEIVE));
    }

    #[test]
    fn rights_does_not_contain_disjoint() {
        let send_only = Rights::SEND;

        assert!(!send_only.contains(Rights::RECEIVE));
    }

    // ── Entry operations ─────────────────────────────────────────────

    #[test]
    fn entry_empty_is_not_occupied() {
        let e = Entry::empty(SlotTag(0));

        assert!(!e.is_occupied());
    }

    #[test]
    fn entry_check_generation_matches() {
        let e = Entry {
            object: Some((ObjectType::Field, ObjectId(1))),
            rights: Rights::SEND,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 42,
        };

        assert!(e.check_generation(42));
        assert!(!e.check_generation(41));
        assert!(!e.check_generation(43));
    }

    #[test]
    fn entry_check_rights_subset() {
        let e = Entry {
            object: Some((ObjectType::Field, ObjectId(1))),
            rights: Rights::SEND.union(Rights::RECEIVE),
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        assert!(e.check_rights(Rights::SEND));
        assert!(e.check_rights(Rights::RECEIVE));
        assert!(e.check_rights(Rights::SEND.union(Rights::RECEIVE)));
        assert!(!e.check_rights(Rights::SPLIT));
    }

    #[test]
    fn entry_check_type_matches() {
        let e = Entry {
            object: Some((ObjectType::Space, ObjectId(5))),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        assert!(e.check_type(ObjectType::Space));
        assert!(!e.check_type(ObjectType::Field));
        assert!(!e.check_type(ObjectType::Observer));
    }

    #[test]
    fn entry_is_send_once_flag() {
        let mut e = Entry::empty(SlotTag(0));

        e.send_once = true;

        assert!(e.is_send_once());

        e.send_once = false;

        assert!(!e.is_send_once());
    }

    // ── Badge ────────────────────────────────────────────────────────

    #[test]
    fn badge_zero_is_valid() {
        let b = Badge(0);

        assert_eq!(b.0, 0);
        assert_ne!(b.0, CAP_ABSENT);
    }

    // ── SlotTag ──────────────────────────────────────────────────────

    #[test]
    fn slot_tag_equality() {
        assert_eq!(SlotTag(0), SlotTag(0));
        assert_ne!(SlotTag(0), SlotTag(1));
    }

    // ── ObjectType ───────────────────────────────────────────────────

    #[test]
    fn object_types_are_distinct() {
        let types = [
            ObjectType::Field,
            ObjectType::Observer,
            ObjectType::Pulsar,
            ObjectType::Space,
            ObjectType::Time,
        ];

        for i in 0..types.len() {
            for j in (i + 1)..types.len() {
                assert_ne!(types[i], types[j]);
            }
        }
    }

    // ── CAP_ABSENT sentinel ──────────────────────────────────────────

    #[test]
    fn cap_absent_is_max_u64() {
        assert_eq!(CAP_ABSENT, u64::MAX);
    }
}
