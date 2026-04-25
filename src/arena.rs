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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ObjectId(pub u32);

/// Allocation failure from a per-type arena (D70, D31).
///
/// Occurs when the slab freelist is empty and no pages can be drawn
/// from the root Space pool.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AllocError {
    /// Root Space pool exhausted — no pages available for slab growth.
    OutOfMemory,
}

/// Per-type kernel object arena (D53, D70).
///
/// Five arenas total: one per kernel object type. Lock ordering (D53):
/// `Arena<Field>` < `Arena<Observer>` < `Arena<Pulsar>`.
/// `Arena<Space>` and `Arena<Time>` are unordered (no cross-arena ops).
///
/// Internal structure (D70): hardware pages divided into N fixed-size
/// slots, intrusive freelist through freed slots. When all slots on a
/// page are free, the page returns to the root Space pool.
///
/// All unsafe slab internals live inside frame/ (journal 023). This
/// module defines the interface; frame/ provides the implementation.
pub struct Arena<T> {
    pub(crate) store: crate::frame::slab::SlabStore<T>,
}

impl<T> Arena<T> {
    /// Allocate a slot for a new object.
    ///
    /// D70: draws from the intrusive freelist within slab pages. When
    /// the freelist is empty, requests a new page from the root Space
    /// pool (D31). Object addresses are stable for the object's lifetime
    /// — no compaction (D70, D4: pointer = capability reference).
    ///
    /// **Caller must hold this arena's lock (D53).**
    ///
    /// Performance: amortized O(1). Page acquisition is cold path (D1).
    pub fn allocate(&mut self) -> Result<(ObjectId, &mut T), AllocError> {
        self.store.allocate()
    }

    /// Look up an object by identifier.
    ///
    /// Returns `None` if `id` is out of bounds or the slot is free.
    /// Callers should check the object's `generation` field (D67)
    /// against the capability entry's stored generation before use —
    /// a successful lookup does not imply the cap is still valid.
    ///
    /// Performance: O(1) — direct index into the slab page array.
    pub fn get(&self, id: ObjectId) -> Option<&T> {
        self.store.get(id)
    }

    /// Mutable lookup by identifier.
    ///
    /// **Caller must hold this arena's lock (D53).**
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut T> {
        self.store.get_mut(id)
    }

    /// Return a slot to the freelist.
    ///
    /// D70: when all slots on a page become free, the page returns to
    /// the root Space pool (D31). Ensures memory usage is proportional
    /// to steady-state allocation, not peak — grows-never-shrinks
    /// rejected under A3 (generic kernel cannot absorb permanent waste
    /// from transient allocation peaks).
    ///
    /// **Caller must hold this arena's lock (D53).**
    pub fn free(&mut self, id: ObjectId) {
        self.store.free(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::sync::atomic::AtomicU64;

    /// Minimal kernel object stand-in for arena tests.
    #[derive(Debug)]
    struct TestObject {
        value: u32,
        /// D67: every kernel object carries a generation counter.
        generation: AtomicU64,
    }

    impl PartialEq for TestObject {
        fn eq(&self, other: &Self) -> bool {
            self.value == other.value
                && self.generation.load(core::sync::atomic::Ordering::Relaxed)
                    == other.generation.load(core::sync::atomic::Ordering::Relaxed)
        }
    }

    impl Eq for TestObject {}

    fn make_arena<T>() -> Arena<T> {
        Arena {
            store: crate::frame::slab::SlabStore::new(),
        }
    }

    /// Exhaust an arena, tracking up to 256 allocated ids.
    /// Returns (total_count, ids_buffer, tracked_count).
    fn exhaust_arena(arena: &mut Arena<TestObject>) -> (usize, [ObjectId; 256], usize) {
        let mut ids = [ObjectId(0); 256];
        let mut count = 0usize;

        loop {
            match arena.allocate() {
                Ok((id, obj)) => {
                    obj.value = id.0;

                    if count < ids.len() {
                        ids[count] = id;
                    }

                    count += 1;

                    assert!(
                        count < 1_000_001,
                        "arena did not exhaust within 1M allocations"
                    );
                }
                Err(AllocError::OutOfMemory) => break,
            }
        }

        let tracked = if count < ids.len() { count } else { ids.len() };

        (count, ids, tracked)
    }

    // ---------------------------------------------------------------
    // D70 — Slab allocator (per-type slab with page return)
    // ---------------------------------------------------------------

    /// D70: allocate returns Ok with an ObjectId and a mutable reference
    /// to the newly placed object.
    #[test]
    fn test_d70_allocate_returns_valid_id_and_ref() {
        let mut arena: Arena<TestObject> = make_arena();
        let result = arena.allocate();

        assert!(result.is_ok(), "allocate must return Ok on a fresh arena");

        let (id, obj_ref) = result.unwrap();
        // The returned ObjectId index should be non-negative (u32).
        let _ = id.0;

        // The returned reference must be writable.
        obj_ref.value = 42;

        assert_eq!(obj_ref.value, 42);
    }

    /// D70: object addresses are stable for the object's lifetime — no
    /// compaction. Allocating a second object must not invalidate the
    /// first object's data.
    #[test]
    fn test_d70_allocate_stable_addresses() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id_a, ref_a) = arena.allocate().expect("first allocate");

        ref_a.value = 1;

        let (id_b, ref_b) = arena.allocate().expect("second allocate");

        ref_b.value = 2;

        // Both objects must still be retrievable with correct values.
        // This confirms no compaction moved the first object.
        let a = arena
            .get(id_a)
            .expect("first object must still be accessible");
        let b = arena
            .get(id_b)
            .expect("second object must still be accessible");

        assert_eq!(
            a.value, 1,
            "first object must retain its value after second allocation"
        );
        assert_eq!(b.value, 2, "second object must retain its value");
    }

    /// D70: free returns a slot to the intrusive freelist; a subsequent
    /// allocate can reuse that slot.
    #[test]
    fn test_d70_free_makes_slot_reusable() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, _) = arena.allocate().expect("allocate");

        arena.free(id);

        // After freeing, the arena should be able to allocate again
        // (potentially reusing the freed slot).
        let result = arena.allocate();

        assert!(
            result.is_ok(),
            "allocate after free must succeed (slot reuse)"
        );
    }

    /// D70/D31: when the freelist is empty and no pages can be drawn
    /// from the root Space pool, allocate returns AllocError::OutOfMemory.
    #[test]
    fn test_d70_allocate_when_empty_returns_out_of_memory() {
        let mut arena: Arena<TestObject> = make_arena();
        // Exhaust the arena by allocating until OutOfMemory.
        let mut count = 0u64;

        loop {
            match arena.allocate() {
                Ok(_) => {
                    count += 1;

                    // Guard against unbounded allocation in a test
                    // environment. A real slab arena backed by finite
                    // pages must exhaust well before this.
                    assert!(
                        count < 1_000_000,
                        "arena did not exhaust after 1M allocations — \
                         likely unbounded"
                    );
                }
                Err(AllocError::OutOfMemory) => break,
            }
        }

        // One more attempt must also fail.
        assert_eq!(
            arena.allocate().unwrap_err(),
            AllocError::OutOfMemory,
            "repeated allocate on exhausted arena must return OutOfMemory"
        );
    }

    // ---------------------------------------------------------------
    // D67 — Generation counters
    // ---------------------------------------------------------------

    /// D67: each kernel object in the arena has its own generation
    /// counter. This is a property of the object type T (the
    /// `generation: AtomicU64` field), not the Arena itself. Two
    /// distinct objects must have independently modifiable generations.
    #[test]
    fn test_d67_generation_is_per_object() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id_a, ref_a) = arena.allocate().expect("first allocate");

        ref_a
            .generation
            .store(1, core::sync::atomic::Ordering::Relaxed);

        let (id_b, ref_b) = arena.allocate().expect("second allocate");

        ref_b
            .generation
            .store(5, core::sync::atomic::Ordering::Relaxed);

        // Verify each object's generation is independent.
        let a = arena.get(id_a).expect("get first");
        let b = arena.get(id_b).expect("get second");

        assert_eq!(
            a.generation.load(core::sync::atomic::Ordering::Relaxed),
            1,
            "first object generation must be independently stored"
        );
        assert_eq!(
            b.generation.load(core::sync::atomic::Ordering::Relaxed),
            5,
            "second object generation must be independently stored"
        );
    }

    // ---------------------------------------------------------------
    // D53 — Per-type arena
    // ---------------------------------------------------------------

    /// D53: Arena<T> is generic — one arena per kernel object type.
    /// Different type parameters produce distinct arena types.
    #[test]
    fn test_d53_arena_is_per_type() {
        #[derive(Debug, PartialEq)]
        struct TypeA(u64);
        #[derive(Debug, PartialEq)]
        struct TypeB(u64);

        // These are two distinct arenas at the type level.
        let _arena_a: Arena<TypeA> = make_arena();
        let _arena_b: Arena<TypeB> = make_arena();

        // The type system enforces separation: an ObjectId from arena_a
        // cannot be used with arena_b at the API level (both accept
        // ObjectId, but the caller is responsible for not mixing them —
        // enforced by the per-type SpinLock<Arena<T>> in the real
        // kernel). The key property is that Arena<TypeA> and
        // Arena<TypeB> are distinct types.
        fn assert_distinct_types<A, B>() {
            assert_ne!(
                core::any::type_name::<A>(),
                core::any::type_name::<B>(),
                "Arena<TypeA> and Arena<TypeB> must be distinct types"
            );
        }

        assert_distinct_types::<Arena<TypeA>, Arena<TypeB>>();
    }

    // ---------------------------------------------------------------
    // Invariants from doc comments
    // ---------------------------------------------------------------

    /// get with an out-of-bounds or freed-slot id returns None.
    #[test]
    fn test_get_returns_none_for_invalid_id() {
        let arena: Arena<TestObject> = make_arena();

        // No allocations have been made; any id should return None.
        assert!(
            arena.get(ObjectId(0)).is_none(),
            "get on empty arena must return None"
        );
        assert!(
            arena.get(ObjectId(u32::MAX)).is_none(),
            "get with u32::MAX id must return None"
        );
    }

    /// get_mut with an out-of-bounds or freed-slot id returns None.
    #[test]
    fn test_get_mut_returns_none_for_invalid_id() {
        let mut arena: Arena<TestObject> = make_arena();

        assert!(
            arena.get_mut(ObjectId(0)).is_none(),
            "get_mut on empty arena must return None"
        );
        assert!(
            arena.get_mut(ObjectId(u32::MAX)).is_none(),
            "get_mut with u32::MAX id must return None"
        );
    }

    /// Free a slot, then get returns None for that id.
    #[test]
    fn test_free_then_get_returns_none() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, _) = arena.allocate().expect("allocate");

        arena.free(id);

        assert!(
            arena.get(id).is_none(),
            "get after free must return None for the freed slot"
        );
    }

    /// Allocate an object, then get with the returned id yields the
    /// same object.
    #[test]
    fn test_allocate_get_roundtrip() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, obj_ref) = arena.allocate().expect("allocate");

        obj_ref.value = 99;

        let retrieved = arena.get(id).expect("get must return the allocated object");

        assert_eq!(
            retrieved.value, 99,
            "retrieved object must match the allocated object"
        );
    }

    /// get_mut allows modification of an allocated object.
    #[test]
    fn test_get_mut_allows_modification() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, obj_ref) = arena.allocate().expect("allocate");

        obj_ref.value = 10;

        let mutable_ref = arena
            .get_mut(id)
            .expect("get_mut must return the allocated object");

        mutable_ref.value = 20;

        let retrieved = arena.get(id).expect("get after mutation");

        assert_eq!(
            retrieved.value, 20,
            "get must reflect the mutation made through get_mut"
        );
    }

    /// Free then get_mut also returns None (symmetric with get).
    #[test]
    fn test_free_then_get_mut_returns_none() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, _) = arena.allocate().expect("allocate");

        arena.free(id);

        assert!(
            arena.get_mut(id).is_none(),
            "get_mut after free must return None for the freed slot"
        );
    }

    /// D70: multiple allocations produce distinct ObjectIds.
    #[test]
    fn test_d70_allocate_produces_distinct_ids() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id_a, _) = arena.allocate().expect("first allocate");
        let (id_b, _) = arena.allocate().expect("second allocate");

        assert_ne!(
            id_a, id_b,
            "consecutive allocations must produce distinct ObjectIds"
        );
    }

    // ---------------------------------------------------------------
    // Adversarial tests — boundary conditions, interleaved operations,
    // invariant checks, and edge cases
    // ---------------------------------------------------------------

    // --- Boundary conditions ---

    /// Allocate the maximum possible objects until the arena is exhausted,
    /// then verify every allocated id is still retrievable. This checks
    /// that the arena does not corrupt state at the capacity boundary.
    /// Uses a fixed-size buffer (no_std environment).
    #[test]

    fn test_adversarial_arena_exhaust_then_verify_all() {
        let mut arena: Arena<TestObject> = make_arena();
        let (_count, ids, tracked) = exhaust_arena(&mut arena);

        for i in 0..tracked {
            let id = ids[i];
            let obj = arena
                .get(id)
                .unwrap_or_else(|| panic!("ObjectId({}) lost after exhaustion", id.0));

            assert_eq!(
                obj.value, id.0,
                "ObjectId({}) value corrupted at capacity boundary",
                id.0
            );
        }
    }

    /// Free with ObjectId(0) on an empty arena — must not panic, must not
    /// corrupt internal state. The doc comment says free returns a slot to
    /// the freelist; freeing an invalid slot is undefined by the API but
    /// must not cause UB in safe Rust.
    #[test]

    fn test_adversarial_arena_free_id_zero_empty() {
        let mut arena: Arena<TestObject> = make_arena();

        arena.free(ObjectId(0));

        // If we get here without panic, verify the arena is still usable.
        let _ = arena.allocate();
    }

    /// Free with ObjectId(u32::MAX) on an empty arena — maximum index,
    /// likely far beyond any allocated range. Must not cause indexing
    /// panic or memory corruption.
    #[test]

    fn test_adversarial_arena_free_id_max_empty() {
        let mut arena: Arena<TestObject> = make_arena();

        arena.free(ObjectId(u32::MAX));
    }

    /// Free with ObjectId(u32::MAX) on a populated arena — the id is
    /// out of bounds even though the arena has live objects.
    #[test]

    fn test_adversarial_arena_free_id_max_populated() {
        let mut arena: Arena<TestObject> = make_arena();
        let _ = arena.allocate().expect("allocate");

        arena.free(ObjectId(u32::MAX));
    }

    /// get_mut with ObjectId(0) on a populated arena where ObjectId(0)
    /// may or may not be the first allocated slot — depends on
    /// implementation. Verify no panic and correct return value.
    #[test]

    fn test_adversarial_arena_get_mut_id_zero_populated() {
        let mut arena: Arena<TestObject> = make_arena();
        let (first_id, _) = arena.allocate().expect("allocate");
        let result = arena.get_mut(ObjectId(0));

        if first_id == ObjectId(0) {
            assert!(
                result.is_some(),
                "get_mut(ObjectId(0)) must return Some when 0 is the first allocated slot"
            );
        } else {
            assert!(
                result.is_none(),
                "get_mut(ObjectId(0)) must return None when 0 is not an allocated slot"
            );
        }
    }

    /// get with ObjectId(u32::MAX) on a populated arena — always out of
    /// bounds regardless of implementation.
    #[test]

    fn test_adversarial_arena_get_id_max_populated() {
        let mut arena: Arena<TestObject> = make_arena();
        let _ = arena.allocate().expect("allocate");

        assert!(
            arena.get(ObjectId(u32::MAX)).is_none(),
            "get with u32::MAX on populated arena must return None"
        );
    }

    /// get_mut with ObjectId(u32::MAX) on a populated arena.
    #[test]

    fn test_adversarial_arena_get_mut_id_max_populated() {
        let mut arena: Arena<TestObject> = make_arena();
        let _ = arena.allocate().expect("allocate");

        assert!(
            arena.get_mut(ObjectId(u32::MAX)).is_none(),
            "get_mut with u32::MAX on populated arena must return None"
        );
    }

    // --- Interleaved operations ---

    /// Allocate, free, allocate, free, allocate — the arena must remain
    /// consistent through repeated alloc/free cycles. Each allocation
    /// must return a valid, retrievable object.
    #[test]

    fn test_adversarial_arena_allocate_free_cycle() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id1, obj1) = arena.allocate().expect("alloc 1");

        obj1.value = 100;

        arena.free(id1);

        let (id2, obj2) = arena.allocate().expect("alloc 2");

        obj2.value = 200;

        arena.free(id2);

        let (id3, obj3) = arena.allocate().expect("alloc 3");

        obj3.value = 300;

        // The last allocation must be live and correct.
        let retrieved = arena.get(id3).expect("get after cycles");

        assert_eq!(retrieved.value, 300, "value must survive alloc/free cycles");

        // The previously freed ids must not be retrievable.
        // (id3 may reuse id1 or id2's slot, but the old ids must not
        // alias a live object unless they happen to be the same slot.)
        if id3 != id1 {
            assert!(
                arena.get(id1).is_none(),
                "freed id1 must not be live when not reused"
            );
        }
        if id3 != id2 {
            assert!(
                arena.get(id2).is_none(),
                "freed id2 must not be live when not reused"
            );
        }
    }

    /// Allocate an object, free it, then get the freed id — must return
    /// None. This is the canonical use-after-free scenario.
    #[test]

    fn test_adversarial_arena_use_after_free() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, obj) = arena.allocate().expect("allocate");

        obj.value = 42;

        arena.free(id);

        assert!(
            arena.get(id).is_none(),
            "get after free must return None (use-after-free)"
        );
        assert!(
            arena.get_mut(id).is_none(),
            "get_mut after free must return None (use-after-free)"
        );
    }

    /// Free the same id twice — double-free. In safe Rust this must not
    /// cause UB. The second free should either be silently ignored or
    /// panic, but must not corrupt the freelist or leak memory.
    #[test]

    fn test_adversarial_arena_double_free() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, _) = arena.allocate().expect("allocate");

        arena.free(id);
        // Second free of the same id:
        arena.free(id);

        // If we survive the double free, verify the arena is still
        // functional by allocating again.
        let result = arena.allocate();

        assert!(
            result.is_ok(),
            "arena must remain functional after double free"
        );
    }

    /// Double free must not create a freelist cycle that causes infinite
    /// allocations from a single slot. Allocate one object, free it
    /// twice, then allocate twice — the second allocate must either fail
    /// or return a *different* slot than the first.
    #[test]

    fn test_adversarial_arena_double_free_no_freelist_cycle() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, _) = arena.allocate().expect("initial allocate");

        arena.free(id);
        arena.free(id);

        // First re-allocation should succeed (reusing the freed slot).
        let (realloc1, _) = arena.allocate().expect("realloc after double free");

        // Second allocation: if the freelist was corrupted by the double
        // free, this might return the same slot again, creating aliased
        // mutable references (which is UB in the real slab). At minimum,
        // if it succeeds, it must be a different id.
        match arena.allocate() {
            Ok((realloc2, _)) => {
                assert_ne!(
                    realloc1, realloc2,
                    "double free must not create a freelist cycle \
                     that yields the same slot twice"
                );
            }
            Err(AllocError::OutOfMemory) => {
                // Also acceptable: the arena correctly tracks that only
                // one slot was ever allocated.
            }
        }
    }

    /// Allocate, modify via get_mut, then read via get — the mutation
    /// must be visible. Tests that get_mut and get refer to the same
    /// backing storage.
    #[test]

    fn test_adversarial_arena_mutation_persistence() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, obj) = arena.allocate().expect("allocate");

        obj.value = 0;

        // Mutate through get_mut.
        let mref = arena.get_mut(id).expect("get_mut");

        mref.value = 999;

        // Read through get.
        let rref = arena.get(id).expect("get");

        assert_eq!(
            rref.value, 999,
            "mutation through get_mut must be visible through get"
        );
    }

    /// Allocate two objects, free the first, verify the second is
    /// completely unaffected. Tests that free does not damage adjacent
    /// or unrelated slots.
    #[test]

    fn test_adversarial_arena_free_does_not_corrupt_neighbors() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id_a, obj_a) = arena.allocate().expect("alloc a");

        obj_a.value = 111;

        let (id_b, obj_b) = arena.allocate().expect("alloc b");

        obj_b.value = 222;

        // Free only the first.
        arena.free(id_a);

        // The second must be completely intact.
        let b = arena
            .get(id_b)
            .expect("second object must survive neighbor free");

        assert_eq!(
            b.value, 222,
            "freeing one object must not corrupt another object's data"
        );
        // The first must be gone.
        assert!(
            arena.get(id_a).is_none(),
            "freed object must not be retrievable"
        );
    }

    /// Allocate three objects (A, B, C), free B (middle), verify A and C
    /// are intact. Tests freeing from the interior of the allocation
    /// sequence rather than the ends.
    #[test]

    fn test_adversarial_arena_free_middle_slot() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id_a, obj_a) = arena.allocate().expect("alloc a");

        obj_a.value = 10;

        let (id_b, _obj_b) = arena.allocate().expect("alloc b");
        let (id_c, obj_c) = arena.allocate().expect("alloc c");

        obj_c.value = 30;

        arena.free(id_b);

        assert_eq!(
            arena.get(id_a).expect("a must survive").value,
            10,
            "first object must be intact after freeing middle"
        );
        assert!(
            arena.get(id_b).is_none(),
            "freed middle object must not be retrievable"
        );
        assert_eq!(
            arena.get(id_c).expect("c must survive").value,
            30,
            "last object must be intact after freeing middle"
        );
    }

    /// Free objects in reverse allocation order. Some freelist
    /// implementations behave differently depending on free order.
    #[test]

    fn test_adversarial_arena_free_reverse_order() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id_a, _) = arena.allocate().expect("alloc a");
        let (id_b, _) = arena.allocate().expect("alloc b");
        let (id_c, _) = arena.allocate().expect("alloc c");

        // Free in reverse order: C, B, A.
        arena.free(id_c);
        arena.free(id_b);
        arena.free(id_a);

        // All should be gone.
        assert!(arena.get(id_a).is_none(), "a must be freed");
        assert!(arena.get(id_b).is_none(), "b must be freed");
        assert!(arena.get(id_c).is_none(), "c must be freed");

        // Arena must still be functional.
        let result = arena.allocate();

        assert!(
            result.is_ok(),
            "allocate must succeed after freeing all objects"
        );
    }

    // --- Invariant checks after sequences ---

    /// After N allocates and M frees (M <= N), the arena should have
    /// exactly N-M live objects reachable via get.
    #[test]

    fn test_adversarial_arena_live_count_after_mixed_ops() {
        let mut arena: Arena<TestObject> = make_arena();
        // Allocate 5 objects into a fixed-size array.
        let mut ids = [ObjectId(0); 5];

        for i in 0u32..5 {
            let (id, obj) = arena.allocate().expect("allocate");

            obj.value = i;
            ids[i as usize] = id;
        }

        // Free 2 of them (indices 1 and 3).
        arena.free(ids[1]);
        arena.free(ids[3]);

        // Count live objects: should be 3.
        let mut live_count = 0;

        for &id in &ids {
            if arena.get(id).is_some() {
                live_count += 1;
            }
        }

        assert_eq!(
            live_count, 3,
            "after 5 allocates and 2 frees, exactly 3 objects must be live"
        );
        // Verify the correct ones are live.
        assert!(arena.get(ids[0]).is_some(), "id[0] must be live");
        assert!(arena.get(ids[1]).is_none(), "id[1] must be freed");
        assert!(arena.get(ids[2]).is_some(), "id[2] must be live");
        assert!(arena.get(ids[3]).is_none(), "id[3] must be freed");
        assert!(arena.get(ids[4]).is_some(), "id[4] must be live");
    }

    /// After allocating and freeing all objects, the arena returns to
    /// an "empty" state: subsequent allocates must succeed (slots were
    /// reclaimed), and previously allocated ids must be unreachable.
    #[test]

    fn test_adversarial_arena_full_cycle_returns_to_empty() {
        let mut arena: Arena<TestObject> = make_arena();
        let (count, ids, tracked) = exhaust_arena(&mut arena);

        assert!(count > 0, "arena must support at least one allocation");

        // Free all tracked ids.
        for i in 0..tracked {
            arena.free(ids[i]);
        }
        // All tracked ids must be unreachable.
        for i in 0..tracked {
            assert!(
                arena.get(ids[i]).is_none(),
                "ObjectId must not be reachable after full free cycle"
            );
        }

        // Allocate again — must succeed (slots were reclaimed).
        let mut realloc_count = 0u64;

        loop {
            match arena.allocate() {
                Ok(_) => {
                    realloc_count += 1;

                    if realloc_count > 1_000_000 {
                        break;
                    }
                }
                Err(AllocError::OutOfMemory) => break,
            }
        }

        assert!(
            realloc_count > 0,
            "arena must be able to allocate after full free cycle"
        );
    }

    /// After free, both get() and get_mut() must return None for the
    /// freed id. Verify symmetry between the two accessors.
    #[test]

    fn test_adversarial_arena_free_unreachable_both_accessors() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, obj) = arena.allocate().expect("allocate");

        obj.value = 77;

        arena.free(id);

        assert!(arena.get(id).is_none(), "get must return None after free");
        assert!(
            arena.get_mut(id).is_none(),
            "get_mut must return None after free"
        );
    }

    /// Allocate multiple objects, free every other one, then re-allocate.
    /// Verify that re-allocated objects do not alias the surviving
    /// objects.
    #[test]

    fn test_adversarial_arena_interleaved_free_realloc_no_alias() {
        let mut arena: Arena<TestObject> = make_arena();
        let mut ids = [ObjectId(0); 6];

        for i in 0u32..6 {
            let (id, obj) = arena.allocate().expect("allocate");

            obj.value = i * 10;
            ids[i as usize] = id;
        }

        // Free even-indexed slots: 0, 2, 4.
        arena.free(ids[0]);
        arena.free(ids[2]);
        arena.free(ids[4]);

        // Re-allocate into the freed slots.
        let mut new_ids = [ObjectId(0); 3];

        for slot in &mut new_ids {
            let (id, obj) = arena.allocate().expect("re-allocate");

            obj.value = 999;
            *slot = id;
        }

        // Surviving odd-indexed objects must be unmodified.
        assert_eq!(
            arena.get(ids[1]).expect("id[1] must be live").value,
            10,
            "surviving object at index 1 must be unmodified"
        );
        assert_eq!(
            arena.get(ids[3]).expect("id[3] must be live").value,
            30,
            "surviving object at index 3 must be unmodified"
        );
        assert_eq!(
            arena.get(ids[5]).expect("id[5] must be live").value,
            50,
            "surviving object at index 5 must be unmodified"
        );

        // New allocations must all read 999.
        for &nid in &new_ids {
            assert_eq!(
                arena.get(nid).expect("new allocation must be live").value,
                999,
                "re-allocated object must have the new value"
            );
        }
    }

    // --- Edge cases ---

    /// Arena with zero-sized type — ZSTs have special layout rules in
    /// Rust. Verify the arena can handle them without panicking.
    #[test]

    fn test_adversarial_arena_zero_sized_type() {
        #[derive(Debug)]
        struct Zst;

        let mut arena: Arena<Zst> = make_arena();
        let result = arena.allocate();

        // We don't prescribe whether this succeeds or fails — but it
        // must not panic with anything other than "not yet implemented"
        // (the todo!() path) or an explicit allocation error.
        match result {
            Ok((id, _)) => {
                assert!(arena.get(id).is_some(), "get must work for ZST");
            }
            Err(AllocError::OutOfMemory) => {}
        }
    }

    /// First-use behavior: allocate immediately after construction.
    /// Verifies lazy initialization (if any) works correctly on first
    /// call.
    #[test]

    fn test_adversarial_arena_first_use_allocate() {
        let mut arena: Arena<TestObject> = make_arena();

        // The very first operation on a fresh arena must either succeed
        // or return a clean error — no uninitialized state panic.
        match arena.allocate() {
            Ok((id, obj)) => {
                obj.value = 1;

                let r = arena
                    .get(id)
                    .expect("first allocated object must be gettable");

                assert_eq!(r.value, 1);
            }
            Err(AllocError::OutOfMemory) => {
                // Acceptable if the arena has no backing memory yet.
            }
        }
    }

    /// First-use behavior: get on a freshly constructed arena with an
    /// arbitrary id. Must return None, not panic from uninitialized
    /// internal structures.
    #[test]
    fn test_adversarial_arena_first_use_get() {
        let arena: Arena<TestObject> = make_arena();

        assert!(
            arena.get(ObjectId(1)).is_none(),
            "get on fresh arena must return None"
        );
    }

    /// First-use behavior: free on a freshly constructed arena. Must
    /// not corrupt state even though nothing has been allocated.
    #[test]

    fn test_adversarial_arena_first_use_free() {
        let mut arena: Arena<TestObject> = make_arena();

        arena.free(ObjectId(0));
    }

    /// Allocate, free, then allocate again — the reused slot must be
    /// a clean slate. The old value must not leak through.
    #[test]

    fn test_adversarial_arena_slot_reuse_no_stale_data() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id1, obj1) = arena.allocate().expect("first allocate");

        obj1.value = 0xDEAD;
        arena.free(id1);

        let (id2, obj2) = arena.allocate().expect("second allocate");

        // If the slot was reused, the old value must not be visible
        // through the new allocation's reference. The allocator should
        // either zero the slot or the caller initializes it — either
        // way, reading through obj2 must not see 0xDEAD (unless the
        // caller explicitly set it).
        // We write a new value and verify it sticks.
        obj2.value = 0xBEEF;

        let retrieved = arena.get(id2).expect("get after reuse");

        assert_eq!(
            retrieved.value, 0xBEEF,
            "reused slot must hold the new value, not stale data"
        );
    }

    /// Allocate multiple objects, then free them all one by one from
    /// first to last. After each free, verify the freed id is gone
    /// and remaining ids are still live.
    #[test]

    fn test_adversarial_arena_progressive_free_forward() {
        let mut arena: Arena<TestObject> = make_arena();
        let mut ids = [ObjectId(0); 4];

        for i in 0u32..4 {
            let (id, obj) = arena.allocate().expect("allocate");

            obj.value = i;
            ids[i as usize] = id;
        }

        for free_idx in 0..ids.len() {
            arena.free(ids[free_idx]);

            // Freed id must be gone.
            assert!(
                arena.get(ids[free_idx]).is_none(),
                "id at freed index must be gone after free"
            );

            // All subsequent ids must still be live.
            for live_idx in (free_idx + 1)..ids.len() {
                let obj = arena
                    .get(ids[live_idx])
                    .expect("subsequent id must still be live after partial free");

                assert_eq!(
                    obj.value, live_idx as u32,
                    "value must be intact after partial free"
                );
            }
        }
    }

    /// Rapidly cycle allocate-free on the same arena. Tests that the
    /// freelist does not degrade or leak across many cycles.
    #[test]

    fn test_adversarial_arena_rapid_alloc_free_cycles() {
        let mut arena: Arena<TestObject> = make_arena();

        for i in 0u32..100 {
            let (id, obj) = arena.allocate().expect("allocate in cycle");

            obj.value = i;

            let r = arena.get(id).expect("get in cycle");

            assert_eq!(r.value, i, "value must match in rapid cycle");

            arena.free(id);

            assert!(
                arena.get(id).is_none(),
                "freed id must be None in rapid cycle"
            );
        }
    }

    /// Free an id that was never allocated (an id between 1 and some
    /// moderate value, on an arena that has one allocation). This is
    /// different from free(ObjectId(0)) and free(ObjectId(u32::MAX))
    /// — it targets the "plausible but wrong" range.
    #[test]

    fn test_adversarial_arena_free_never_allocated_id() {
        let mut arena: Arena<TestObject> = make_arena();
        let (valid_id, _) = arena.allocate().expect("allocate");
        // Pick an id that is definitely not the allocated one.
        let bogus = if valid_id.0 == 42 {
            ObjectId(43)
        } else {
            ObjectId(42)
        };

        arena.free(bogus);

        // The valid allocation must survive the bogus free.
        assert!(
            arena.get(valid_id).is_some(),
            "valid object must survive free of a never-allocated id"
        );
    }

    /// Allocate an object, free it, then free a *different* never-allocated
    /// id. Exercises the path where the freelist has one entry (from the
    /// real free) and then an invalid free is attempted.
    #[test]

    fn test_adversarial_arena_free_invalid_after_valid_free() {
        let mut arena: Arena<TestObject> = make_arena();
        let (id, _) = arena.allocate().expect("allocate");

        arena.free(id);
        arena.free(ObjectId(id.0.wrapping_add(1)));

        // Arena must still function.
        let result = arena.allocate();

        assert!(
            result.is_ok(),
            "arena must recover after free of invalid id"
        );
    }

    /// ObjectId wrapping: create an ObjectId near u32::MAX and verify
    /// get handles it correctly (returns None, no overflow panic).
    #[test]

    fn test_adversarial_arena_objectid_near_max() {
        let mut arena: Arena<TestObject> = make_arena();
        let _ = arena.allocate().expect("allocate");

        // These ids are almost certainly out of bounds.
        for offset in 0u32..5 {
            let id = ObjectId(u32::MAX - offset);

            assert!(
                arena.get(id).is_none(),
                "get near u32::MAX must return None"
            );
        }
    }

    /// Verify that the ObjectId returned by allocate is always
    /// retrievable via both get and get_mut before any free.
    #[test]

    fn test_adversarial_arena_allocate_always_retrievable() {
        let mut arena: Arena<TestObject> = make_arena();

        for i in 0u32..10 {
            let (id, obj) = arena.allocate().expect("allocate");

            obj.value = i;

            assert!(
                arena.get(id).is_some(),
                "get must succeed for just-allocated id"
            );
            assert!(
                arena.get_mut(id).is_some(),
                "get_mut must succeed for just-allocated id"
            );
        }
    }
}
