//! Bare-metal syscall fuzzer.
//!
//! Generates 1000 random syscall sequences against the kernel and verifies
//! it survives (no panic, no hang). Tests kernel robustness, not operation
//! correctness — every error result from a syscall is silently ignored.
//!
//! Seed: 0xDEAD_BEEF_CAFE_BABE — verified clean on 2026-04-28.
//!
//! FUZZ-01: 1000 random iterations with fixed seed for reproducibility (FUZZ-06).
//! FUZZ-02: Capability operations — Clone, Mint, Close, Destroy.
//! FUZZ-03: IPC operations — Send to random handles, Call to known echo server.
//! FUZZ-04: Memory operations — SpaceSplit, CreateField, FieldSplit, SpaceMerge.
//! FUZZ-05: Kernel survives all sequences — test completes with pass().

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

// ── Constants ─────────────────────────────────────────────────────────────────

/// Fixed seed for deterministic replay (FUZZ-06).
/// Re-running with this seed produces the identical syscall sequence every time.
const SEED: u64 = 0xDEAD_BEEF_CAFE_BABE;

/// Number of random operations to execute per run.
const ITERATIONS: u32 = 1000;

/// Maximum number of live handles tracked by the pool.
/// When full, new handles are silently dropped — the kernel object still
/// exists and will be reclaimed by auto-destroy (D107) once refcount reaches zero.
const MAX_TRACKED_HANDLES: usize = 32;

/// A bogus handle value the pool returns when empty, to exercise kernel
/// rejection paths on invalid handles. The kernel must not panic on these.
const BOGUS_HANDLE: u64 = 0xFFFF_FFFF_0000_0000;

// ── xorshift64 PRNG ──────────────────────────────────────────────────────────

/// xorshift64 pseudo-random number generator.
///
/// Deterministic from a fixed seed, no heap, no external dependencies.
/// Sequence is identical for any given seed (FUZZ-06).
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        // xorshift64 must not be seeded with zero — adjust if needed.
        let state = if seed == 0 { 1 } else { seed };

        Self { state }
    }

    fn next(&mut self) -> u64 {
        let mut x = self.state;

        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;

        self.state = x;

        x
    }

    fn range(&mut self, max: u64) -> u64 {
        self.next() % max
    }
}

// ── Handle pool ───────────────────────────────────────────────────────────────

/// Fixed-size pool of live handles created during fuzzing.
///
/// Tracks handles so later random operations can target them. No heap
/// allocation — backed by a fixed-size array.
struct HandlePool {
    handles: [u64; MAX_TRACKED_HANDLES],
    count: usize,
}

impl HandlePool {
    fn new() -> Self {
        Self {
            handles: [0u64; MAX_TRACKED_HANDLES],
            count: 0,
        }
    }

    /// Add a handle to the pool.
    ///
    /// If the pool is full, the handle is silently dropped. The kernel object
    /// remains live but unreachable from this pool.
    fn push(&mut self, handle: u64) {
        if self.count < MAX_TRACKED_HANDLES {
            self.handles[self.count] = handle;
            self.count += 1;
        }
    }

    /// Return a random handle from the pool, or BOGUS_HANDLE if empty.
    ///
    /// Does not remove the handle — the handle remains in the pool for
    /// future operations.
    fn random(&mut self, rng: &mut Rng) -> u64 {
        if self.count == 0 {
            return BOGUS_HANDLE;
        }

        let index = rng.range(self.count as u64) as usize;

        self.handles[index]
    }

    /// Remove and return a random handle from the pool.
    ///
    /// Returns None if the pool is empty. Removal is done by swapping
    /// the chosen entry with the last entry (O(1), preserves count).
    fn remove_random(&mut self, rng: &mut Rng) -> Option<u64> {
        if self.count == 0 {
            return None;
        }

        let index = rng.range(self.count as u64) as usize;
        let handle = self.handles[index];
        // Swap with last entry and shrink count.
        self.count -= 1;
        self.handles[index] = self.handles[self.count];

        Some(handle)
    }
}

// ── Entry point ───────────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    // Set up echo server for safe Call() exercising (FUZZ-03).
    // Call() blocks until the server replies — we must target a known
    // good server, never a random handle.
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);

    spawn_echo_server(handler_field, ipc_field);
    // Required before Call (SVC #3) can be used from this Observer.
    setup_reply_field();

    let mut rng = Rng::new(SEED);
    let mut pool = HandlePool::new();

    // Pre-populate the pool with a few space splits to give the fuzzer
    // initial material for capability operations.
    for _ in 0..4 {
        // 16384 bytes = 1 page — smallest valid split.
        let result = space_split(ROOT_SPACE_HANDLE, 16_384);

        if result.is_ok() {
            pool.push(result.value());
        }
    }

    // Main fuzzing loop (FUZZ-01).
    for _ in 0..ITERATIONS {
        // Pick operation category (0..=9).
        let category = rng.range(10);

        match category {
            // ── Capability operations (FUZZ-02) ──────────────────────────
            0 => {
                // Clone: duplicate a capability.
                let result = clone_cap(pool.random(&mut rng));

                if result.is_ok() {
                    pool.push(result.value());
                }
            }
            1 => {
                // Mint: attenuate a capability with random rights and badge.
                let result = mint(pool.random(&mut rng), rng.next(), rng.next());

                if result.is_ok() {
                    pool.push(result.value());
                }
            }
            2 => {
                // Close: release a random handle.
                // remove_random removes it from our tracking — the kernel
                // frees the slot. On error (stale/bogus handle), ignore.
                if let Some(handle) = pool.remove_random(&mut rng) {
                    let _ = close(handle);
                }
            }
            3 => {
                // Destroy: destroy the kernel object behind a handle.
                // remove_random removes it from tracking to avoid referencing
                // destroyed objects in future operations.
                if let Some(handle) = pool.remove_random(&mut rng) {
                    let _ = destroy(handle);
                }
            }

            // ── IPC operations (FUZZ-03) ─────────────────────────────────
            4 => {
                // Send: fire-and-forget to a random handle.
                // The kernel must handle sends to non-Field handles, full queues,
                // invalid handles, etc. — all must return error, not panic.
                // We do NOT check the return value — errors are expected.
                let data = [rng.next(), rng.next(), rng.next(), rng.next()];
                let _ = send(pool.random(&mut rng), rng.next(), data);
            }
            5 => {
                // Send to a completely random (potentially bogus) handle value.
                // Exercises the kernel's handle validation path.
                let data = [rng.next(), rng.next(), rng.next(), rng.next()];
                let _ = send(rng.next(), rng.next(), data);
            }
            6 => {
                // Call to the known echo server — safe to block because the
                // server always replies. Tests Call under random payload data.
                let data = [rng.next(), rng.next(), rng.next(), rng.next()];
                // We ignore the reply content — correctness is not the goal here.
                let _ = call(ipc_field, rng.next(), data, CAP_ABSENT, 0);
            }

            // ── Memory operations (FUZZ-04) ──────────────────────────────
            7 => {
                // SpaceSplit: allocate 1-4 pages from the root Space.
                // Will fail when root Space is exhausted — that's fine.
                let pages = rng.range(4) + 1;
                let size = pages * 16384;
                let result = space_split(ROOT_SPACE_HANDLE, size);

                if result.is_ok() {
                    pool.push(result.value());
                }
            }
            8 => {
                // CreateField: attempt to convert a random handle to a Field.
                // Likely fails (wrong type, invalid handle) — exercises
                // the type-checking path in the kernel.
                let capacity = rng.range(8) + 1;
                let _ = create_field(pool.random(&mut rng), capacity);
            }
            9 => {
                // FieldSplit and SpaceMerge — exercises validation paths.
                // These almost always fail with random handles, but the
                // kernel must handle them gracefully.
                if rng.range(2) == 0 {
                    // FieldSplit: random field, random space, random badge range.
                    let _ = field_split(
                        pool.random(&mut rng),
                        pool.random(&mut rng),
                        rng.next(),
                        rng.next(),
                    );
                } else {
                    // SpaceMerge: attempt to merge two random handles.
                    let _ = space_merge(pool.random(&mut rng), pool.random(&mut rng));
                }
            }
            // Exhaustive match — category is always 0..=9 due to range(10).
            _ => {
                yield_cpu();
            }
        }
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
