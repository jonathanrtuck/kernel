//! Boot-time initialization helpers (D93, D94).
//!
//! Allocates and wires up the root Observer, per-core data, and user page
//! table entries needed to enter EL0 for the first time. All unsafe operations
//! are confined here (framekernel discipline).
//!
//! Phase E strategy: user pages are added to the kernel's existing TTBR0
//! identity map at L2 index 0 (VA 0x0–0x01FF_FFFF). The kernel's RAM
//! mappings at L2 indices 32+ are EL1-only. __restore_observer skips the
//! TTBR switch since TTBR0 doesn't change. This avoids the full D88 TTBR
//! split while proving the complete EL0 ↔ kernel dispatch path.

#[cfg(target_os = "none")]
use crate::arena::AllocError;
#[cfg(target_os = "none")]
use crate::core_manager::{CoreState, MAX_DEADLINES_PER_CORE};
#[cfg(target_os = "none")]
use crate::frame::arch::mmu;
#[cfg(target_os = "none")]
use crate::frame::arch::page_table;
#[cfg(target_os = "none")]
use crate::frame::arch::register_state::RegisterState;
#[cfg(target_os = "none")]
use crate::frame::cores::PerCoreData;
#[cfg(target_os = "none")]
use crate::kernel_state::KernelState;
#[cfg(target_os = "none")]
use crate::observer::{
    DEFAULT_RESPONSIVENESS, DEFAULT_THROUGHPUT, Observer, PrimaryState, RegisterStateHandle,
    WaitState,
};
#[cfg(target_os = "none")]
use crate::time_manager::CoreId;
#[cfg(target_os = "none")]
use crate::time_manager::round_robin::RoundRobin;
#[cfg(target_os = "none")]
use core::ptr::NonNull;
#[cfg(target_os = "none")]
use core::sync::atomic::AtomicU64;

// ── User VA layout (Phase E) ─────────────────────────────────────
//
// L2 index 0 (VA 0x0–0x01FF_FFFF):
//   L3 index 0: unmapped (null guard page)
//   L3 index 1: code page (VA 0x4000, RO+EL0-executable)
//   L3 index 2: stack page (VA 0x8000, RW+EL0, not executable)

#[cfg(target_os = "none")]
const PAGE_SIZE: usize = mmu::page_size();
#[cfg(target_os = "none")]
const USER_CODE_VA: usize = PAGE_SIZE;
#[cfg(target_os = "none")]
const USER_STACK_VA: usize = 2 * PAGE_SIZE;
#[cfg(target_os = "none")]
const USER_STACK_TOP: usize = USER_STACK_VA + PAGE_SIZE;

// ── Test binary (D94, D102) ──────────────────────────────────────
//
// Minimal EL0 program that:
// 1. Loads a marker into x0 (proves register state is live)
// 2. Executes SVC #5 (Yield — proves syscall round-trip)
// 3. After return from Yield, executes BRK #0x42
//    (debug exception → fault handler → kernel prints pass → exit)

#[cfg(target_os = "none")]
const TEST_BINARY: &[u8] = &{
    // ARM64 instructions are 4 bytes, little-endian.
    let mut buf = [0u8; 16];

    // mov x0, #0x42         → 0xD2800840
    buf[0] = 0x40;
    buf[1] = 0x08;
    buf[2] = 0x80;
    buf[3] = 0xD2;

    // svc #5                → 0xD40000A1
    buf[4] = 0xA1;
    buf[5] = 0x00;
    buf[6] = 0x00;
    buf[7] = 0xD4;

    // mov x0, #0            → 0xD2800000 (clear x0 — proves yield returned)
    buf[8] = 0x00;
    buf[9] = 0x00;
    buf[10] = 0x80;
    buf[11] = 0xD2;

    // brk #0x42             → 0xD4200840
    buf[12] = 0x40;
    buf[13] = 0x08;
    buf[14] = 0x20;
    buf[15] = 0xD4;

    buf
};

/// Allocate `count` pages from SpaceManager, zeroed. Returns the physical address.
#[cfg(target_os = "none")]
fn alloc_zeroed_pages(ks: &KernelState, count: usize) -> Result<usize, AllocError> {
    let mut sm = ks.space_manager.acquire();
    let page_size = sm.root_pool.page_size;
    let pa = sm.allocate_pages(count)?;

    drop(sm);

    // SAFETY: `pa` is a valid physical address returned by SpaceManager.
    // Identity mapping: PA = VA. The pages are exclusively ours.
    unsafe {
        core::ptr::write_bytes(pa as *mut u8, 0, count * page_size);
    }

    Ok(pa)
}

/// Build the user L3 table and install it in the kernel's L2 root (Phase E).
///
/// Maps code_pa at USER_CODE_VA (RO, executable) and stack_pa at
/// USER_STACK_VA (RW, not executable). L2 index 0 is currently unmapped
/// in the kernel's identity map.
#[cfg(target_os = "none")]
fn setup_user_pages(ks: &KernelState, code_pa: usize, stack_pa: usize) -> Result<(), AllocError> {
    let l3_pa = alloc_zeroed_pages(ks, 1)?;

    // SAFETY: l3_pa points to a zeroed page exclusively owned by us.
    // Identity mapping: PA = VA. We write L3 page descriptors.
    unsafe {
        let l3 = &mut *(l3_pa as *mut [u64; page_table::ENTRIES_PER_TABLE]);

        l3[1] = page_table::user_code_descriptor(code_pa as u64);
        l3[2] = page_table::user_data_descriptor(stack_pa as u64);
    }

    mmu::install_user_l3_in_kernel_l2(0, l3_pa);

    Ok(())
}

/// Copy the test binary to the code page.
#[cfg(target_os = "none")]
fn install_test_binary(code_pa: usize) {
    // SAFETY: code_pa points to a zeroed page exclusively owned by us.
    // TEST_BINARY.len() < PAGE_SIZE. Identity mapping: PA = VA.
    unsafe {
        core::ptr::copy_nonoverlapping(TEST_BINARY.as_ptr(), code_pa as *mut u8, TEST_BINARY.len());
    }
}

/// Allocate a RegisterState and set initial values (D94).
///
/// PC = entry point, SP = stack top, pstate = EL0 AArch64.
/// Returns the physical address of the RegisterState.
#[cfg(target_os = "none")]
fn setup_register_state(ks: &KernelState) -> Result<usize, AllocError> {
    let rs_pa = alloc_zeroed_pages(ks, 1)?;

    // SAFETY: rs_pa points to a zeroed page. RegisterState is 816 bytes,
    // fits in one 16 KiB page. Identity mapping: PA = VA.
    unsafe {
        let rs = &mut *(rs_pa as *mut RegisterState);

        rs.pc = USER_CODE_VA as u64;
        rs.sp = USER_STACK_TOP as u64;
        // pstate = 0: EL0t (AArch64), IRQs unmasked, no flags set
    }

    Ok(rs_pa)
}

/// Create and initialize the root Observer in the Observer arena (D94).
///
/// The Observer starts in Inert state. The caller must transition it to
/// Runnable and set it as the current Observer before context switching.
#[cfg(target_os = "none")]
fn create_root_observer(
    ks: &KernelState,
    rs_pa: usize,
    page_table_root: u64,
) -> Result<(crate::arena::ObjectId, NonNull<Observer>), AllocError> {
    let mut observers = ks.observers.acquire();
    let (obs_id, obs) = observers.allocate()?;

    obs.object_id = obs_id;
    obs.register_state =
        RegisterStateHandle::new(NonNull::new(rs_pa as *mut u8).expect("rs_pa must be non-null"));
    obs.page_table_root = page_table_root;
    obs.cap_table = NonNull::dangling();
    obs.cap_table_capacity = 0;
    obs.cap_table_free_head = None;
    obs.cap_table_count = 0;
    obs.state = PrimaryState::Runnable;
    obs.suspended = false;
    obs.compute_aggregate = 100;
    obs.responsiveness = DEFAULT_RESPONSIVENESS;
    obs.throughput = DEFAULT_THROUGHPUT;
    obs.clock_access = false;
    obs.wait_state = WaitState::None;
    obs.refcount = 1;
    obs.generation = AtomicU64::new(0);

    let ptr = NonNull::from(&*obs);

    drop(observers);

    Ok((obs_id, ptr))
}

// ── BSP per-core data (D83, D93) ────────────────────────────────

/// Static storage for the BSP's per-core data and core state.
///
/// These must be 'static because TPIDR_EL1 holds a pointer to PerCoreData
/// for the lifetime of the kernel. Using statics avoids lifetime issues
/// with stack-allocated data.
#[cfg(target_os = "none")]
static mut BSP_PER_CORE_DATA: PerCoreData = PerCoreData {
    register_state_ptr: core::ptr::null_mut(),
    core_state_ptr: core::ptr::null_mut(),
    kernel_stack_top: core::ptr::null_mut(),
};

#[cfg(target_os = "none")]
static mut BSP_CORE_STATE: CoreState<RoundRobin> = CoreState {
    core_id: CoreId(0),
    current: None,
    scheduler: RoundRobin::new(),
    deadlines: [None; MAX_DEADLINES_PER_CORE],
    deadline_count: 0,
    cascade_continuation: None,
};

// Linker symbol for the BSP boot stack top (link.ld).
#[cfg(target_os = "none")]
unsafe extern "C" {
    static __stack_top: u8;
}

/// Initialize the BSP's PerCoreData and write TPIDR_EL1.
///
/// D83: TPIDR_EL1 → PerCoreData → CoreState. Must be called before
/// any exception handler runs (the handler reads TPIDR_EL1).
#[cfg(target_os = "none")]
fn init_bsp_per_core_data(rs_ptr: *mut RegisterState) {
    // SAFETY: Single-threaded BSP boot — no concurrent access to statics.
    // The statics are 'static and stable for the kernel's lifetime.
    // TPIDR_EL1 is per-core writable state (D83). Raw pointer writes
    // avoid Edition 2024's prohibition on references to mutable statics.
    unsafe {
        let cs = &raw mut BSP_CORE_STATE;

        core::ptr::write(
            cs,
            CoreState {
                core_id: CoreId(0),
                current: None,
                scheduler: RoundRobin::new(),
                deadlines: [None; MAX_DEADLINES_PER_CORE],
                deadline_count: 0,
                cascade_continuation: None,
            },
        );

        let pcd = &raw mut BSP_PER_CORE_DATA;

        core::ptr::write(
            pcd,
            PerCoreData {
                register_state_ptr: rs_ptr,
                core_state_ptr: cs as *mut u8,
                kernel_stack_top: &raw const __stack_top as *mut u8,
            },
        );

        crate::frame::arch::set_tpidr_el1(pcd as u64);
    }
}

/// Set the current Observer on the BSP core state.
///
/// Only sets `current` — does NOT enqueue. The Observer is about to be
/// restored directly via __restore_observer, bypassing the scheduler.
/// The Yield dispatch path handles enqueuing when the Observer yields.
#[cfg(target_os = "none")]
fn set_current_observer(observer_ptr: NonNull<Observer>) {
    // SAFETY: BSP_CORE_STATE is initialized by init_bsp_per_core_data.
    // Single-threaded boot context. Raw pointer writes avoid Edition 2024's
    // prohibition on references to mutable statics.
    unsafe {
        let cs = &raw mut BSP_CORE_STATE;

        (*cs).current = Some(observer_ptr);
    }
}

// ── Public boot entry point ──────────────────────────────────────

/// Boot the kernel into EL0 with a test Observer (D94).
///
/// Allocates physical pages, builds user mappings, creates the root
/// Observer, initializes per-core data, and context switches to EL0.
/// This function does not return.
#[cfg(target_os = "none")]
pub fn enter_first_observer(ks: &KernelState) -> ! {
    crate::println!("boot: allocating root observer resources");

    let code_pa = alloc_zeroed_pages(ks, 1).expect("allocate code page");

    install_test_binary(code_pa);

    let stack_pa = alloc_zeroed_pages(ks, 1).expect("allocate stack page");

    setup_user_pages(ks, code_pa, stack_pa).expect("setup user page table");

    crate::println!(
        "boot: code={:#x} stack={:#x} entry={:#x}",
        code_pa,
        stack_pa,
        USER_CODE_VA,
    );

    let rs_pa = setup_register_state(ks).expect("allocate register state");
    let page_table_root = mmu::current_ttbr0();
    let (_obs_id, obs_ptr) =
        create_root_observer(ks, rs_pa, page_table_root).expect("create root observer");

    init_bsp_per_core_data(rs_pa as *mut RegisterState);
    set_current_observer(obs_ptr);

    crate::println!("boot: entering EL0 at {:#x}", USER_CODE_VA);

    let (rs_ptr, pt_root, clock_access) = crate::frame::cores::observer_restore_info(obs_ptr);

    crate::frame::cores::update_register_state_ptr(rs_ptr);

    // SAFETY: rs_ptr points to a valid, fully initialized RegisterState
    // (setup_register_state wrote PC/SP/pstate). pt_root is the current
    // TTBR0 value (identity map with user pages installed). clock_access
    // is 0 (no EL0 counter access). IRQs are currently masked (hardware
    // default from boot). PerCoreData is initialized above.
    unsafe {
        crate::frame::arch::exception::__restore_observer(rs_ptr, pt_root, clock_access);
    }
}
