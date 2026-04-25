//! Framekernel core — the unsafe boundary.
//!
//! All `unsafe` code in the kernel lives inside this module tree. Everything
//! outside `frame` is safe Rust built against the abstractions exported here.
//! The crate-level `#![deny(unsafe_code)]` enforces this at compile time.

#[cfg(any(target_os = "none", test))]
pub mod arch;
pub mod capabilities;
pub mod cores;
pub mod fields;
pub mod firmware;
pub mod lock;
pub mod slab;

// ── Global KernelState (D75, D82) ────────────────────────────────

use crate::kernel_state::KernelState;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

/// Wrapper that allows `MaybeUninit<KernelState>` to live in a non-mut
/// static while providing interior mutability through raw pointer access.
///
/// Edition 2024 forbids references to `static mut`. This wrapper uses
/// `UnsafeCell` for interior mutability, making it safe to take raw
/// pointers without creating references to the outer static.
///
/// SAFETY: Sync is manually implemented because the boot protocol (D46)
/// guarantees init happens before any concurrent access, and all
/// subsequent access is read-only at the KernelState level (mutability
/// is mediated through Lock<T> on each field).
struct GlobalKernelState {
    inner: core::cell::UnsafeCell<MaybeUninit<KernelState>>,
}

// SAFETY: GlobalKernelState is Sync because:
// 1. The write (init_kernel_state) happens exactly once on the BSP before
//    secondary cores are activated (D46). No concurrent access during write.
// 2. All subsequent reads return &KernelState. KernelState fields are
//    Lock-wrapped, so concurrent access is safe.
unsafe impl Sync for GlobalKernelState {}

/// The global KernelState instance (D75, D82).
///
/// Initialized by the BSP during boot via `init_kernel_state()`, before
/// secondary core activation (D46). All cores share this single instance.
///
/// MaybeUninit is required because:
/// - no_std: no heap, no global constructors
/// - A4 + D46: boot-time init, not lazy
/// - The arenas and SpaceManager require runtime data (DTB-discovered RAM)
///
/// Lives in frame/ because MaybeUninit + assume_init_ref is genuinely unsafe
/// (reading uninitialized memory if called before init). Framekernel discipline
/// (journal 023): all unsafe in frame/.
static KERNEL_STATE: GlobalKernelState = GlobalKernelState {
    inner: core::cell::UnsafeCell::new(MaybeUninit::uninit()),
};

/// Whether the global KernelState has been initialized.
///
/// Set to true by `init_kernel_state()`. Checked by `kernel_state()` in
/// debug builds to catch use-before-init bugs. Uses AtomicBool to avoid
/// Edition 2024's `static mut` restriction while providing correct
/// ordering semantics.
static KERNEL_STATE_INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Initialize the global KernelState (D82).
///
/// Called once by the BSP during boot, before secondary core activation
/// (D46, PSCI CPU_ON). The value is moved into the static — no references
/// to stack-local data survive.
///
/// # Panics
///
/// Debug builds panic if called more than once (double-init is a boot
/// sequencing bug).
pub fn init_kernel_state(state: KernelState) {
    // SAFETY: Called exactly once by the BSP before any other core is
    // active (D46). No concurrent access is possible at this point —
    // secondary cores have not been activated. The write to the
    // UnsafeCell is therefore data-race-free.
    unsafe {
        debug_assert!(
            !KERNEL_STATE_INITIALIZED.load(Ordering::Relaxed),
            "init_kernel_state called more than once — boot sequencing bug"
        );

        let ptr = KERNEL_STATE.inner.get();

        (*ptr).write(state);
        KERNEL_STATE_INITIALIZED.store(true, Ordering::Release);
    }
}

/// Access the global KernelState (D82).
///
/// Returns `&'static KernelState` — valid for the lifetime of the kernel.
/// The reference is to immutable data (the KernelState struct itself);
/// mutability is mediated through `Lock<T>` on each field.
///
/// # Safety invariant
///
/// `init_kernel_state()` must have been called before this function.
/// The boot sequence (D46: BSP inits globals before PSCI CPU_ON)
/// guarantees this. Debug builds verify with an assertion.
pub fn kernel_state() -> &'static KernelState {
    // SAFETY: KERNEL_STATE was initialized by init_kernel_state() during
    // BSP boot, before any secondary core was activated (D46). The
    // assume_init_ref call is safe because the MaybeUninit was written to
    // before any core can reach this point. The returned reference is
    // &'static because the static lives for the duration of the kernel.
    // Concurrent reads are safe because KernelState fields are Lock-wrapped.
    unsafe {
        debug_assert!(
            KERNEL_STATE_INITIALIZED.load(Ordering::Acquire),
            "kernel_state() called before init_kernel_state() — boot sequencing bug"
        );

        (*KERNEL_STATE.inner.get()).assume_init_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::Arena;
    use crate::frame::lock::LockOrder;
    use crate::space_manager::{RootPool, SpaceManager};

    fn make_arena<T>() -> Arena<T> {
        Arena {
            store: crate::frame::slab::SlabStore::new(),
        }
    }

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

    /// D82: init_kernel_state + kernel_state roundtrip. The global
    /// must be accessible after initialization.
    ///
    /// Note: this test modifies global state. It is inherently
    /// non-repeatable within a single process, but cargo test runs
    /// each test in isolation.
    #[test]
    fn test_d82_init_and_access_kernel_state() {
        let state = KernelState::new(
            make_arena(),
            make_arena(),
            make_arena(),
            make_arena(),
            make_arena(),
            make_space_manager(),
        );

        // Initialize the global.
        init_kernel_state(state);

        // Access the global.
        let ks = kernel_state();

        // Verify each field's lock order.
        assert_eq!(ks.fields.order(), LockOrder::Field);
        assert_eq!(ks.observers.order(), LockOrder::Observer);
        assert_eq!(ks.pulsars.order(), LockOrder::Pulsar);
        assert_eq!(ks.spaces.order(), LockOrder::Space);
        assert_eq!(ks.times.order(), LockOrder::Time);
        assert_eq!(ks.space_manager.order(), LockOrder::SpaceManager);

        // Verify SpaceManager is functional through the global accessor.
        // (Arena<Field> allocation would panic because the slab store
        // zeroes memory, and Field contains NonNull fields that are not
        // zero-valid. SpaceManager has no such constraint.)
        let mut sm_guard = ks.space_manager.acquire();
        let result = sm_guard.allocate_pages(1);

        assert!(
            result.is_ok(),
            "D82: SpaceManager must be functional through global accessor"
        );
    }
}
