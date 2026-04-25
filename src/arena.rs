//! Per-type object arena with generation counters.
//!
//! D53: global-arena concurrency model — one SpinLock per Arena<T>.
//! D67: every kernel object carries a generation counter for revocation.
//! D70: per-type slab allocator with page return.

/// Kernel-internal object identifier.
///
/// Index into a per-type Arena<T> slab. The object's own `generation`
/// field (D67) is the revocation counter — checked against the stored
/// generation in each capability entry on use.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ObjectId(pub u32);

/// Per-type kernel object arena (D53, D70).
///
/// Five arenas total: one per kernel object type. Lock ordering (D53):
/// `Arena<Field>` < `Arena<Observer>` < `Arena<Pulsar>`.
/// `Arena<Space>` and `Arena<Time>` are unordered (no cross-arena ops).
///
/// Internal structure (D70): hardware pages divided into N fixed-size
/// slots, intrusive freelist through freed slots. When all slots on a
/// page are free, the page returns to the root Space pool.
pub struct Arena<T> {
    _marker: core::marker::PhantomData<T>,
}
