//! Global kernel state bundle (D75, D82).
//!
//! D75: the five per-type arenas and the SpaceManager live in a single global
//! `KernelState` struct. Cold-path code accesses arenas through this global.
//! The hot path (D1, D50, D74) never touches it.
//!
//! D82: settles the concrete struct definition. Six Lock-wrapped fields:
//! five arenas (Field, Observer, Pulsar, Space, Time) plus SpaceManager.
//! Each Lock uses the D53 ordering. Constructor `new()` takes empty arenas
//! and a SpaceManager for test setup. The global static placement and safe
//! accessor live in frame/ (MaybeUninit is unsafe).
//!
//! D81: IRQ routing table added — maps INTID to (field_id, badge, generation).
//! Direct-indexed by INTID (max 1024). Lock<IrqRoutingTable> with unordered
//! LockOrder::IrqRouting (does not participate in Field-Observer-Pulsar chain).
//!
//! D53: lock ordering — Field < Observer < Pulsar. Space, Time, SpaceManager,
//! and IrqRouting are unordered (no cross-arena operations with the ordered types).

use crate::arena::{Arena, ObjectId};
use crate::capability::Badge;
use crate::field::Field;
use crate::frame::lock::{Lock, LockOrder};
use crate::observer::Observer;
use crate::pulsar::Pulsar;
use crate::space::Space;
use crate::space_manager::SpaceManager;
use crate::time::Time;

// ── IRQ routing (D22, D81) ─────────────────────────────────────────

/// Maximum number of IRQ routes (D81).
///
/// Direct-indexed by GIC INTID. 1024 covers the full GICv3 SPI range
/// (INTIDs 0–1023). 16 KiB static array. The hardware already bounds
/// the space — no translation layer needed.
pub const MAX_IRQS: usize = 1024;

/// A single IRQ-to-Field route (D22, D81).
///
/// Maps a device interrupt (identified by GIC INTID) to a delivery Field.
/// The kernel looks up this route on every device IRQ, constructs a
/// `Message::device_irq` with the route's badge, and enqueues it into
/// the target Field.
///
/// D67: the `generation` field enables stale-route detection. If the
/// target Field has been revoked (generation mismatch), the route is
/// stale and the interrupt is silently dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IrqRoute {
    /// Target Field for interrupt delivery (D22).
    pub field_id: ObjectId,

    /// Badge injected into the interrupt message (D17, D22).
    /// Identifies the specific IRQ to the driver Observer.
    pub badge: Badge,

    /// Generation of the target Field at route installation time (D67).
    /// Checked against the live Field's generation on every delivery.
    /// Mismatch means the Field was revoked — route is stale.
    pub generation: u64,
}

/// IRQ routing table: INTID -> Option<IrqRoute> (D22, D81).
///
/// Direct-indexed by INTID. `routes[intid]` is `Some(route)` if the
/// interrupt is routed, `None` if unrouted. Unrouted interrupts are
/// logged and ignored.
///
/// The table lives in KernelState behind a Lock<IrqRoutingTable> with
/// LockOrder::IrqRouting (unordered — does not participate in the
/// Field-Observer-Pulsar ordering chain).
pub struct IrqRoutingTable {
    /// Direct-indexed array: `routes[intid as usize]` for INTID < MAX_IRQS.
    pub routes: [Option<IrqRoute>; MAX_IRQS],
}

impl Default for IrqRoutingTable {
    fn default() -> Self {
        Self::new()
    }
}

impl IrqRoutingTable {
    /// Create an empty routing table with no routes installed.
    pub const fn new() -> IrqRoutingTable {
        IrqRoutingTable {
            routes: [None; MAX_IRQS],
        }
    }

    /// Look up the route for a given INTID (D81).
    ///
    /// Returns `None` if the INTID is out of range or unrouted.
    /// The caller must check the route's generation against the live
    /// Field before delivering (D67 stale-route detection).
    pub fn lookup(&self, intid: u32) -> Option<&IrqRoute> {
        let index = intid as usize;

        if index >= MAX_IRQS {
            return None;
        }

        self.routes[index].as_ref()
    }

    /// Install a route for an INTID (D22, D81).
    ///
    /// Overwrites any existing route. Returns `true` if a previous
    /// route was replaced, `false` if the slot was empty.
    ///
    /// Returns `None` if the INTID is out of range.
    pub fn install(&mut self, intid: u32, route: IrqRoute) -> Option<bool> {
        let index = intid as usize;

        if index >= MAX_IRQS {
            return None;
        }

        let was_occupied = self.routes[index].is_some();
        self.routes[index] = Some(route);

        Some(was_occupied)
    }

    /// Remove a route for an INTID.
    ///
    /// Returns the removed route, or `None` if the slot was empty
    /// or the INTID is out of range.
    pub fn remove(&mut self, intid: u32) -> Option<IrqRoute> {
        let index = intid as usize;

        if index >= MAX_IRQS {
            return None;
        }

        self.routes[index].take()
    }

    /// Batch-populate routes for device INTIDs (D99 boot-time population).
    ///
    /// Each INTID in `[start_intid, end_intid)` gets a route to the same
    /// target Field with `badge = INTID`. This is the initial state at
    /// boot: every device interrupt routes to the root interrupt Field,
    /// and the badge distinguishes which device fired.
    ///
    /// INTIDs outside `[0, MAX_IRQS)` are silently skipped.
    /// Returns the number of routes installed.
    pub fn populate_device_routes(
        &mut self,
        field_id: ObjectId,
        generation: u64,
        start_intid: u32,
        end_intid: u32,
    ) -> u32 {
        let mut count = 0;

        for intid in start_intid..end_intid {
            let index = intid as usize;

            if index >= MAX_IRQS {
                continue;
            }

            self.routes[index] = Some(IrqRoute {
                field_id,
                badge: Badge(intid as u64),
                generation,
            });

            count += 1;
        }

        count
    }

    /// Repoint routes whose badge falls within `[badge_low, badge_high]`
    /// to a new destination Field (D99 FieldSplit IRQ routing table update).
    ///
    /// When FieldSplit creates a sub-Field for a badge range, the IRQ
    /// routing table entries whose badge falls in that range must be
    /// updated to point to the new sub-Field. This is the kernel-internal
    /// materialization of the Field-based authority model — parallel to
    /// how page tables materialize capability-based memory state (D24).
    ///
    /// Returns the number of routes updated.
    pub fn update_routes_for_split(
        &mut self,
        badge_low: u64,
        badge_high: u64,
        new_field_id: ObjectId,
        new_generation: u64,
    ) -> u32 {
        let mut count = 0;

        for route in self.routes.iter_mut().flatten() {
            let badge_val = route.badge.0;

            if badge_val >= badge_low && badge_val <= badge_high {
                route.field_id = new_field_id;
                route.generation = new_generation;
                count += 1;
            }
        }

        count
    }
}

/// Kernel-wide shared cold-path state (D75, D82).
///
/// Bundles all per-type arenas and the SpaceManager in one namespace.
/// Each field is wrapped in `Lock<T>` (D75: lock owns data via UnsafeCell;
/// LockGuard provides DerefMut). The bundle is a single point of change
/// if arena sharding (D53's flagged SMP optimization) is implemented.
///
/// Access pattern: cold-path code calls `frame::kernel_state()` to get
/// `&'static KernelState`, then acquires individual locks as needed.
/// The hot path (per-core scheduler, context switch) never touches this.
///
/// D53 lock ordering for the ordered locks:
/// - `fields` (LockOrder::Field) must be acquired before `observers` and `pulsars`.
/// - `observers` (LockOrder::Observer) must be acquired after `fields`, before `pulsars`.
/// - `pulsars` (LockOrder::Pulsar) must be acquired after `fields` and `observers`.
///
/// `spaces`, `times`, `space_manager`, and `irq_routes` are unordered —
/// they do not participate in the Field-Observer-Pulsar ordering chain
/// and may be acquired independently at any time.
pub struct KernelState {
    /// Per-type arena for Field objects (D15, D53).
    pub fields: Lock<Arena<Field>>,
    /// Per-type arena for Observer objects (D6, D53).
    pub observers: Lock<Arena<Observer>>,
    /// Per-type arena for Pulsar objects (D44, D53).
    pub pulsars: Lock<Arena<Pulsar>>,
    /// Per-type arena for Space objects (D9, D53).
    pub spaces: Lock<Arena<Space>>,
    /// Per-type arena for Time objects (D29, D53).
    pub times: Lock<Arena<Time>>,
    /// Physical memory allocation and VA assignment (D3, D31).
    pub space_manager: Lock<SpaceManager>,
    /// IRQ routing table: INTID -> Field delivery route (D22, D81).
    /// Unordered lock — acquired independently by handle_irq on the
    /// interrupt path.
    pub irq_routes: Lock<IrqRoutingTable>,
}

impl KernelState {
    /// Construct a new KernelState with the given SpaceManager.
    ///
    /// D82: tests construct this locally. The boot path constructs it and
    /// passes it to `frame::init_kernel_state()` for global placement.
    ///
    /// Arenas are created empty internally — first allocations draw pages
    /// from the SpaceManager's root pool (D70, D31).
    pub fn new(space_manager: SpaceManager) -> KernelState {
        KernelState {
            fields: Lock::new(LockOrder::Field, Arena::new()),
            observers: Lock::new(LockOrder::Observer, Arena::new()),
            pulsars: Lock::new(LockOrder::Pulsar, Arena::new()),
            spaces: Lock::new(LockOrder::Space, Arena::new()),
            times: Lock::new(LockOrder::Time, Arena::new()),
            space_manager: Lock::new(LockOrder::SpaceManager, space_manager),
            irq_routes: Lock::new(LockOrder::IrqRouting, IrqRoutingTable::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::space_manager::RootPool;

    // ── Test helpers ──────────────────────────────────────────────────

    fn make_space_manager() -> SpaceManager {
        SpaceManager {
            root_pool: RootPool {
                total_bytes: 16 * 4096,
                free_bytes: 16 * 4096,
                page_size: 4096,
            },
            next_physical_base: 4096,
            next_va_base: 4096,
        }
    }

    fn make_kernel_state() -> KernelState {
        KernelState::new(make_space_manager())
    }

    // ── D82 — KernelState construction ────────────────────────────────

    /// D82: KernelState::new() constructs a valid struct with all six fields.
    /// Each field must be accessible and its lock must be acquirable.
    #[test]
    fn test_d82_kernel_state_new_constructs_all_fields() {
        let state = make_kernel_state();
        // Verify each lock is acquirable (not poisoned, not stuck).
        let _fields = state.fields.acquire();

        drop(_fields);

        let _observers = state.observers.acquire();

        drop(_observers);

        let _pulsars = state.pulsars.acquire();

        drop(_pulsars);

        let _spaces = state.spaces.acquire();

        drop(_spaces);

        let _times = state.times.acquire();

        drop(_times);

        let _sm = state.space_manager.acquire();

        drop(_sm);
    }

    // ── D53 — Lock ordering ──────────────────────────────────────────

    /// D53: each Lock in KernelState must have the correct LockOrder.
    /// Field < Observer < Pulsar (ordered). Space, Time, SpaceManager (unordered).
    #[test]
    fn test_d53_lock_orders_match_spec() {
        let state = make_kernel_state();

        assert_eq!(
            state.fields.order(),
            LockOrder::Field,
            "fields arena must use LockOrder::Field"
        );
        assert_eq!(
            state.observers.order(),
            LockOrder::Observer,
            "observers arena must use LockOrder::Observer"
        );
        assert_eq!(
            state.pulsars.order(),
            LockOrder::Pulsar,
            "pulsars arena must use LockOrder::Pulsar"
        );
        assert_eq!(
            state.spaces.order(),
            LockOrder::Space,
            "spaces arena must use LockOrder::Space"
        );
        assert_eq!(
            state.times.order(),
            LockOrder::Time,
            "times arena must use LockOrder::Time"
        );
        assert_eq!(
            state.space_manager.order(),
            LockOrder::SpaceManager,
            "space_manager must use LockOrder::SpaceManager"
        );
    }

    /// D53: ordered locks must follow the ordering chain. Verify the
    /// ordering values enforce Field < Observer < Pulsar.
    #[test]
    fn test_d53_ordered_locks_have_correct_relative_order() {
        let state = make_kernel_state();

        assert!(
            state.fields.order() < state.observers.order(),
            "D53: Field must be acquired before Observer"
        );
        assert!(
            state.observers.order() < state.pulsars.order(),
            "D53: Observer must be acquired before Pulsar"
        );
    }

    /// D53: unordered locks must not participate in strict ordering.
    #[test]
    fn test_d53_unordered_locks_not_in_strict_ordering() {
        let state = make_kernel_state();

        assert!(
            !state.spaces.order().is_ordered(),
            "D53: Space must be unordered"
        );
        assert!(
            !state.times.order().is_ordered(),
            "D53: Time must be unordered"
        );
        assert!(
            !state.space_manager.order().is_ordered(),
            "D53: SpaceManager must be unordered"
        );
    }

    // ── D75 — Lock<T> owns data ──────────────────────────────────────

    /// D75: Lock<T> owns data via UnsafeCell. LockGuard provides DerefMut.
    /// Verify that acquiring the lock gives mutable access to the arena.
    ///
    /// Uses Arena<Space> because Space has only primitive fields (usize,
    /// u32, AtomicU64) that are zero-valid. Arena<Field> and Arena<Observer>
    /// contain NonNull pointers that panic on zero-initialization.
    #[test]
    fn test_d75_lock_guard_provides_deref_mut_to_arena() {
        let state = make_kernel_state();
        let mut guard = state.spaces.acquire();
        // Allocate through the guard — this exercises DerefMut<Target=Arena<Space>>.
        let result = guard.allocate();

        assert!(
            result.is_ok(),
            "D75: arena must be accessible through LockGuard"
        );
    }

    /// D75: mutation through LockGuard is visible in subsequent acquisitions.
    #[test]
    fn test_d75_mutation_visible_across_lock_cycles() {
        let state = make_kernel_state();
        // First acquisition: allocate an object.
        // Uses Arena<Space> (zero-safe fields).
        let object_id = {
            let mut guard = state.spaces.acquire();
            let (id, _space) = guard.allocate().expect("allocate must succeed");

            id
        };

        // Second acquisition: the allocated object must be present.
        {
            let guard = state.spaces.acquire();

            assert!(
                guard.get(object_id).is_some(),
                "D75: object allocated in first cycle must be visible in second"
            );
        }
    }

    /// D75: SpaceManager is accessible through its lock, same pattern as arenas.
    #[test]
    fn test_d75_space_manager_accessible_through_lock() {
        let state = make_kernel_state();
        let mut guard = state.space_manager.acquire();
        // Allocate pages through the guard.
        let result = guard.allocate_pages(1);

        assert!(
            result.is_ok(),
            "D75: SpaceManager must be accessible through LockGuard"
        );
    }

    // ── D82 — Test-local construction ────────────────────────────────

    /// D82: tests construct KernelState locally without needing the global
    /// static. This is the testability seam: domain logic tests create a
    /// KernelState on the stack.
    ///
    /// Uses Arena<Space> for allocation (zero-safe fields).
    #[test]
    fn test_d82_local_construction_for_tests() {
        // Construct directly — no frame/ global needed.
        let state = KernelState::new(make_space_manager());
        // Must be fully functional. Uses spaces arena (zero-safe).
        let mut spaces = state.spaces.acquire();
        let (id, _) = spaces.allocate().expect("local state must be functional");

        drop(spaces);

        let spaces = state.spaces.acquire();

        assert!(
            spaces.get(id).is_some(),
            "local KernelState must be fully functional"
        );
    }

    // ── Cross-arena lock acquisition ─────────────────────────────────

    /// D53: multiple locks can be held simultaneously when respecting
    /// the ordering. Acquire Field then Observer (ordered correctly).
    #[test]
    fn test_d53_acquire_field_then_observer_is_valid() {
        let state = make_kernel_state();
        let _field_guard = state.fields.acquire();
        let _observer_guard = state.observers.acquire();

        // Both held simultaneously — this is the correct D53 order.
    }

    /// D53: acquire Field, Observer, then Pulsar — full ordered chain.
    #[test]
    fn test_d53_full_ordered_chain_acquisition() {
        let state = make_kernel_state();
        let _field_guard = state.fields.acquire();
        let _observer_guard = state.observers.acquire();
        let _pulsar_guard = state.pulsars.acquire();

        // All three held in D53 order.
    }

    /// D53: unordered locks can be acquired alongside ordered locks
    /// in any order.
    #[test]
    fn test_d53_unordered_with_ordered_any_order() {
        let state = make_kernel_state();
        // Acquire Space before Field — valid because Space is unordered.
        let _space_guard = state.spaces.acquire();
        let _field_guard = state.fields.acquire();
        // Acquire Time after Pulsar — valid because Time is unordered.
        let _pulsar_guard = state.pulsars.acquire();
        let _time_guard = state.times.acquire();
    }

    // ── Adversarial tests ────────────────────────────────────────────

    /// Zero-safe arenas (Space, Time) must be independently operable:
    /// allocate in each, then verify each allocation is retrievable.
    /// No cross-contamination between arenas.
    ///
    /// Field, Observer, and Pulsar arenas contain NonNull pointer fields
    /// that panic on zero-initialization. Their locks are verified as
    /// acquirable but not allocated from.
    #[test]
    fn test_adversarial_all_arenas_independent() {
        let state = make_kernel_state();
        // Allocate in zero-safe arenas (Space, Time).
        let space_id = {
            let mut g = state.spaces.acquire();

            g.allocate().expect("space allocate").0
        };
        let time_id = {
            let mut g = state.times.acquire();

            g.allocate().expect("time allocate").0
        };

        // Verify each is retrievable from its own arena.
        assert!(
            state.spaces.acquire().get(space_id).is_some(),
            "space must be retrievable"
        );
        assert!(
            state.times.acquire().get(time_id).is_some(),
            "time must be retrievable"
        );

        // Verify non-zero-safe arena locks are acquirable (no allocation).
        let _fields = state.fields.acquire();

        drop(_fields);

        let _observers = state.observers.acquire();

        drop(_observers);

        let _pulsars = state.pulsars.acquire();

        drop(_pulsars);

        // Verify arenas are independent: mutate space object, verify
        // time object is not affected.
        {
            let mut sg = state.spaces.acquire();
            let space = sg.get_mut(space_id).expect("space must exist");

            space.va_base = 0xDEAD_BEEF;
        }
        {
            let tg = state.times.acquire();
            let time = tg.get(time_id).expect("time must exist");

            // Time has no va_base — if arenas were shared, the mutation
            // above would corrupt data at the same slot. The fact that
            // time still reads valid (zero-initialized) values confirms
            // independence.
            assert_eq!(
                time.compute_units, 0,
                "time object must be unaffected by space mutation"
            );
        }
    }

    /// SpaceManager allocation and return through KernelState locks
    /// preserves the conservation invariant.
    #[test]
    fn test_adversarial_space_manager_conservation_through_lock() {
        let state = make_kernel_state();
        let initial_free = {
            let guard = state.space_manager.acquire();

            guard.root_pool.free_bytes
        };
        // Allocate 4 pages.
        let base = {
            let mut guard = state.space_manager.acquire();

            guard.allocate_pages(4).expect("allocate 4 pages")
        };

        // Return 4 pages.
        {
            let mut guard = state.space_manager.acquire();

            guard.return_pages(base, 4);
        }

        // free_bytes must be restored.
        let final_free = {
            let guard = state.space_manager.acquire();

            guard.root_pool.free_bytes
        };

        assert_eq!(
            initial_free, final_free,
            "conservation: free_bytes must be restored after allocate+return"
        );
    }

    /// Releasing a guard makes the lock acquirable again. No deadlock.
    #[test]
    fn test_adversarial_lock_release_and_reacquire() {
        let state = make_kernel_state();

        {
            let _g = state.fields.acquire();
        }

        // Must not deadlock.
        let _g2 = state.fields.acquire();
    }

    // ── D81 — IRQ routing table ─────────────────────────────────────

    /// D81: KernelState includes irq_routes field.
    #[test]
    fn test_d81_kernel_state_has_irq_routes() {
        let state = make_kernel_state();
        let routes = state.irq_routes.acquire();

        // Must be empty on creation.
        assert!(
            routes.lookup(0).is_none(),
            "D81: irq_routes must be empty on construction"
        );
    }

    /// D81: irq_routes lock uses LockOrder::IrqRouting (unordered).
    #[test]
    fn test_d81_irq_routes_lock_order() {
        let state = make_kernel_state();

        assert_eq!(
            state.irq_routes.order(),
            LockOrder::IrqRouting,
            "D81: irq_routes must use LockOrder::IrqRouting"
        );
        assert!(
            !state.irq_routes.order().is_ordered(),
            "D81: IrqRouting must be unordered"
        );
    }

    /// D81: IrqRoutingTable install, lookup, remove roundtrip.
    #[test]
    fn test_d81_irq_routing_table_roundtrip() {
        let state = make_kernel_state();
        let mut routes = state.irq_routes.acquire();

        routes.install(
            42,
            IrqRoute {
                field_id: ObjectId(7),
                badge: Badge(0xBEEF),
                generation: 3,
            },
        );

        let found = routes
            .lookup(42)
            .expect("D81: installed route must be found");

        assert_eq!(found.field_id, ObjectId(7));
        assert_eq!(found.badge, Badge(0xBEEF));
        assert_eq!(found.generation, 3);

        let removed = routes.remove(42);

        assert!(removed.is_some());
        assert!(routes.lookup(42).is_none());
    }

    /// D81: MAX_IRQS is 1024, matching GICv3 SPI range.
    #[test]
    fn test_d81_max_irqs_is_1024() {
        assert_eq!(MAX_IRQS, 1024, "D81: MAX_IRQS must be 1024");
    }

    /// D81: IrqRoute is 24 bytes (ObjectId + Badge + u64).
    #[test]
    fn test_d81_irq_route_size() {
        // ObjectId(4) + Badge(8) + generation(8) = 20 bytes, padded to 24.
        let size = core::mem::size_of::<IrqRoute>();

        assert!(
            size <= 24,
            "D81: IrqRoute should be compact (got {size} bytes)"
        );
    }

    /// D81: irq_routes lock can be acquired alongside field arena lock
    /// (both unordered or different ordering categories).
    #[test]
    fn test_d81_irq_routes_acquirable_with_fields() {
        let state = make_kernel_state();
        // Acquire irq_routes then fields — valid because irq_routes is unordered.
        let _routes = state.irq_routes.acquire();
        let _fields = state.fields.acquire();
    }

    // ── D99 — Boot-time IRQ route population ──────────────────────────

    /// D99: populate_device_routes installs routes for a range of INTIDs.
    #[test]
    fn test_d99_populate_device_routes_basic() {
        let mut table = IrqRoutingTable::new();
        let field_id = ObjectId(7);
        let count = table.populate_device_routes(field_id, 0, 32, 64);

        assert_eq!(count, 32, "D99: must install 32 routes for INTIDs 32..64");

        for intid in 32..64u32 {
            let route = table
                .lookup(intid)
                .expect("D99: route must exist for populated INTID");

            assert_eq!(route.field_id, field_id);
            assert_eq!(route.generation, 0);
        }

        assert!(
            table.lookup(31).is_none(),
            "D99: INTIDs before range must be unrouted"
        );
        assert!(
            table.lookup(64).is_none(),
            "D99: INTIDs at/after range end must be unrouted"
        );
    }

    /// D99: badge equals INTID — each device distinguished by badge value.
    #[test]
    fn test_d99_populate_device_routes_badge_equals_intid() {
        let mut table = IrqRoutingTable::new();

        table.populate_device_routes(ObjectId(1), 0, 100, 105);

        for intid in 100..105u32 {
            let route = table.lookup(intid).unwrap();

            assert_eq!(
                route.badge,
                Badge(intid as u64),
                "D99: badge must equal INTID for device route"
            );
        }
    }

    /// D99: empty range installs zero routes.
    #[test]
    fn test_d99_populate_device_routes_empty_range() {
        let mut table = IrqRoutingTable::new();
        let count = table.populate_device_routes(ObjectId(1), 0, 50, 50);

        assert_eq!(count, 0, "D99: empty range must install zero routes");
    }

    /// D99: populate overwrites existing routes.
    #[test]
    fn test_d99_populate_device_routes_overwrites_existing() {
        let mut table = IrqRoutingTable::new();

        table.install(
            42,
            IrqRoute {
                field_id: ObjectId(99),
                badge: Badge(42),
                generation: 5,
            },
        );
        table.populate_device_routes(ObjectId(1), 0, 40, 45);

        let route = table.lookup(42).unwrap();

        assert_eq!(
            route.field_id,
            ObjectId(1),
            "D99: populate must overwrite existing route"
        );
        assert_eq!(route.generation, 0, "D99: populate must set new generation");
    }

    /// D99: populate full SPI range (32–1019).
    #[test]
    fn test_d99_populate_device_routes_full_spi_range() {
        let mut table = IrqRoutingTable::new();
        let count = table.populate_device_routes(ObjectId(1), 0, 32, 1020);

        assert_eq!(count, 988, "D99: full SPI range (32..1020) = 988 routes");
    }

    /// D99: INTIDs at MAX_IRQS boundary are handled correctly.
    #[test]
    fn test_d99_populate_device_routes_boundary() {
        let mut table = IrqRoutingTable::new();
        let count = table.populate_device_routes(ObjectId(1), 0, 1020, 1030);

        assert_eq!(count, 4, "D99: only INTIDs < 1024 should be installed");
        assert!(table.lookup(1023).is_some());
        assert!(table.lookup(1024).is_none());
    }

    // ── D99 — FieldSplit IRQ routing table update ─────────────────────

    /// D99: update_routes_for_split repoints matching routes.
    #[test]
    fn test_d99_update_routes_for_split_basic() {
        let mut table = IrqRoutingTable::new();
        let old_field = ObjectId(1);
        let new_field = ObjectId(2);

        table.populate_device_routes(old_field, 0, 32, 48);

        let updated = table.update_routes_for_split(40, 47, new_field, 1);

        assert_eq!(updated, 8, "D99: 8 routes in badge range [40,47]");

        for intid in 40..48u32 {
            let route = table.lookup(intid).unwrap();

            assert_eq!(
                route.field_id, new_field,
                "D99: route must point to new Field"
            );
            assert_eq!(route.generation, 1, "D99: route must have new generation");
        }
    }

    /// D99: routes outside the split range are not affected.
    #[test]
    fn test_d99_update_routes_for_split_preserves_unaffected() {
        let mut table = IrqRoutingTable::new();
        let old_field = ObjectId(1);

        table.populate_device_routes(old_field, 0, 32, 64);
        table.update_routes_for_split(40, 50, ObjectId(2), 1);

        for intid in 32..40u32 {
            let route = table.lookup(intid).unwrap();

            assert_eq!(
                route.field_id, old_field,
                "D99: route outside split range must be unchanged"
            );
            assert_eq!(route.generation, 0);
        }
        for intid in 51..64u32 {
            let route = table.lookup(intid).unwrap();

            assert_eq!(route.field_id, old_field);
            assert_eq!(route.generation, 0);
        }
    }

    /// D99: update with no matching routes returns 0.
    #[test]
    fn test_d99_update_routes_for_split_no_matching_routes() {
        let mut table = IrqRoutingTable::new();

        table.populate_device_routes(ObjectId(1), 0, 32, 48);

        let updated = table.update_routes_for_split(100, 200, ObjectId(2), 1);

        assert_eq!(updated, 0, "D99: no routes in badge range → 0 updated");
    }

    /// D99: update works on partially populated table.
    #[test]
    fn test_d99_update_routes_for_split_partial_overlap() {
        let mut table = IrqRoutingTable::new();

        table.populate_device_routes(ObjectId(1), 0, 40, 50);

        let updated = table.update_routes_for_split(45, 55, ObjectId(2), 1);

        assert_eq!(
            updated, 5,
            "D99: only routes 45-49 exist in [45,55] → 5 updated"
        );
    }
}
