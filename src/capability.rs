//! Capability system: handles, rights, entries, and per-Observer tables.
//!
//! D4:  capability-based authority (designation = authority).
//! D8:  kernel-managed flat table, per-Observer.
//! D11: close-only + destroy, generational slot tags.
//! D17: badges (minter-assigned, immutable, kernel-attached to messages).

/// Kernel object types designated by capabilities (D14).
///
/// Exactly four. Exhaustive — no extension point.
pub enum ObjectType {
    Space,
    Time,
    Field,
    Observer,
}

/// Per-capability rights mask (D8, D15, D17, D33, D38, D39).
///
/// Rights are per-type, not universal (D38). Each object type defines its valid
/// rights; clone appears in Space/Field/Observer but not Time.
///
/// Observer rights (D39): resume, destroy, install-cap, write-registers, clone,
/// read-registers, suspend, change-handler, modify-scheduling. Nine bits.
pub struct Rights {
    // Settled rights:
    //   Field:    send, receive (D15), mint (D17), send-once (D16), destroy (D33).
    //   Observer: resume, destroy, install-cap, write-registers, clone,
    //             read-registers, suspend, change-handler, modify-scheduling (D39).
    //   Space:    read, write, execute, clone, destroy, create (D31). TBD.
    //   Time:     split, destroy. No clone (D38). TBD.
    // Open:
    //   send-once encoding (D16), grant (D28), duplicate-control (D23 deferred,
    //   D8 derivation — applies to all types uniformly).
}

/// Generational slot tag for ABA prevention (D11).
///
/// Bumped on slot reuse. Prevents stale-handle aliasing.
/// ABA defense, not revocation.
pub struct SlotTag {
    // Size open (D11).
}

/// Minter-assigned badge, immutable after creation (D17).
///
/// Kernel attaches to every message sent through this cap.
/// Sender cannot read, choose, or modify its badge.
pub struct Badge {
    // Size open — 64-bit default (D17).
}

/// Opaque handle presented by userspace (D8 + D11).
///
/// Index into the Observer's flat table, paired with a generation tag for ABA
/// defense.
pub struct Handle {
    // Encoding open.
}

/// Single entry in the capability table (D8).
///
/// Fields: object ref, rights, badge, slot tag.
pub struct Entry {
    // Layout open.
}

/// Per-Observer capability table (D8).
///
/// Flat array, kernel-managed. Backed by typed memory from the Observer's
/// Spaces — not a kernel-internal pool.
/// D21: slot 0 reserved for fault handler.
/// D39: change-handler right gates writes to the reserved slot;
/// install-cap gates writes to regular slots.
pub struct Table {
    // Representation open.
    // Growth: fault on full, handler provides Space, retry (D8).
}
