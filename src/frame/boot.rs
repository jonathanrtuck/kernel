//! Boot-time initialization helpers (D93, D94).
//!
//! Allocates and wires up the root Observer, per-core data, and user page
//! table entries needed to enter EL0 for the first time. All unsafe operations
//! are confined here (framekernel discipline).
//!
//! D88 TTBR split: TTBR1 provides the kernel linear map (VA = PA +
//! KERNEL_VIRT_OFFSET). All dynamically allocated memory is accessed
//! through TTBR1 via `phys_to_virt()`. TTBR0 provides identity-mapped
//! kernel code and user pages (L1_ROOT[0] → L2_ROOT). User pages are
//! added at L2 index 0 (VA 0x0–0x01FF_FFFF), EL1-only kernel pages at
//! L2 indices 32+.

#[cfg(target_os = "none")]
use crate::arena::AllocError;
#[cfg(target_os = "none")]
use crate::capability::{self, Badge, ObjectType, Rights, SlotTag};
#[cfg(target_os = "none")]
use crate::core_manager::{CoreState, MAX_DEADLINES_PER_CORE};
#[cfg(target_os = "none")]
use crate::field::Field;
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
use crate::time_manager::Scheduler;
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

// ── Fallback test binary (D94, Phase 2) ─────────────────────────
//
// Minimal EL0 program used when no DTB module is present.
// Phase 2.2–2.5: IPC, fault, timer, destroy.
//
// 1. SVC #2 (Receive on IPC Field, slot 4) — blocks until child sends
// 2. BRK #0x44 — IPC verify (non-divergent)
// 3. SVC #2 (Receive on handler Field, slot 5) — gets fault message
// 4. BRK #0x45 — fault verify (non-divergent)
// 5. SVC #2 (Receive on timer Field, slot 6) — blocks until Pulsar fires
// 6. BRK #0x46 — timer verify (non-divergent)
// 7. SVC #0 (Destroy child Observer, op=7, handle=slot 7)
// 8. BRK #0x47 — destroy verify (divergent, system_off)

#[cfg(target_os = "none")]
const FALLBACK_BINARY: &[u8] = &{
    let mut buf = [0u8; 52];

    // movz x5, #4           → 0xD2800085  (IPC Field Receive cap)
    buf[0] = 0x85;
    buf[1] = 0x00;
    buf[2] = 0x80;
    buf[3] = 0xD2;

    // svc #2                → 0xD4000041  (Receive)
    buf[4] = 0x41;
    buf[5] = 0x00;
    buf[6] = 0x00;
    buf[7] = 0xD4;

    // brk #0x44             → 0xD4200880  (IPC verify, non-divergent)
    buf[8] = 0x80;
    buf[9] = 0x08;
    buf[10] = 0x20;
    buf[11] = 0xD4;

    // movz x5, #5           → 0xD28000A5  (handler Field Receive cap)
    buf[12] = 0xA5;
    buf[13] = 0x00;
    buf[14] = 0x80;
    buf[15] = 0xD2;

    // svc #2                → 0xD4000041  (Receive)
    buf[16] = 0x41;
    buf[17] = 0x00;
    buf[18] = 0x00;
    buf[19] = 0xD4;

    // brk #0x45             → 0xD42008A0  (fault verify, non-divergent)
    buf[20] = 0xA0;
    buf[21] = 0x08;
    buf[22] = 0x20;
    buf[23] = 0xD4;

    // movz x5, #6           → 0xD28000C5  (timer Field Receive cap)
    buf[24] = 0xC5;
    buf[25] = 0x00;
    buf[26] = 0x80;
    buf[27] = 0xD2;

    // svc #2                → 0xD4000041  (Receive)
    buf[28] = 0x41;
    buf[29] = 0x00;
    buf[30] = 0x00;
    buf[31] = 0xD4;

    // brk #0x46             → 0xD42008C0  (timer verify, non-divergent)
    buf[32] = 0xC0;
    buf[33] = 0x08;
    buf[34] = 0x20;
    buf[35] = 0xD4;

    // movz x4, #7           → 0xD28000E4  (Destroy op code)
    buf[36] = 0xE4;
    buf[37] = 0x00;
    buf[38] = 0x80;
    buf[39] = 0xD2;

    // movz x5, #7           → 0xD28000E5  (child Observer cap, slot 7)
    buf[40] = 0xE5;
    buf[41] = 0x00;
    buf[42] = 0x80;
    buf[43] = 0xD2;

    // svc #0                → 0xD4000001  (typed operation)
    buf[44] = 0x01;
    buf[45] = 0x00;
    buf[46] = 0x00;
    buf[47] = 0xD4;

    // brk #0x47             → 0xD42008E0  (destroy verify, divergent)
    buf[48] = 0xE0;
    buf[49] = 0x08;
    buf[50] = 0x20;
    buf[51] = 0xD4;

    buf
};

/// Allocate `count` pages from SpaceManager, zeroed. Returns the physical address.
#[cfg(target_os = "none")]
pub(super) fn alloc_zeroed_pages(ks: &KernelState, count: usize) -> Result<usize, AllocError> {
    let mut sm = ks.space_manager.acquire();
    let page_size = sm.root_pool.page_size;
    let pa = sm.allocate_pages(count)?;

    drop(sm);

    // SAFETY: `pa` is a valid physical address returned by SpaceManager.
    // D88: phys_to_virt converts PA to the TTBR1 linear map VA. The
    // pages are exclusively ours.
    unsafe {
        core::ptr::write_bytes(mmu::phys_to_virt(pa) as *mut u8, 0, count * page_size);
    }

    Ok(pa)
}

/// Allocate and initialize a per-Observer L1 page table (D89).
///
/// Allocates one zeroed page, sets L1\[0\] → kernel L2_ROOT (identity map
/// for kernel code), and returns the physical address of the L1 table.
/// The remaining L1 entries are zero (invalid) — user mappings are
/// installed later via the map/unmap protocol (Phase 1.4).
#[cfg(target_os = "none")]
pub fn allocate_observer_l1(ks: &KernelState) -> Result<usize, AllocError> {
    let l1_pa = alloc_zeroed_pages(ks, 1)?;
    let l2_root_pa = mmu::kernel_l2_root_pa();

    // SAFETY: l1_pa points to a zeroed page exclusively owned by us.
    // D88: phys_to_virt converts to TTBR1 linear map VA. We write one
    // L1 table descriptor at index 0 to chain to the kernel's L2 root.
    unsafe {
        let l1 = &mut *(mmu::phys_to_virt(l1_pa) as *mut [u64; page_table::ENTRIES_PER_TABLE]);

        l1[0] = page_table::table_descriptor(l2_root_pa as u64);
    }

    Ok(l1_pa)
}

/// Allocate and populate an L3 page table for a Space (D90).
///
/// Allocates one zeroed page, fills it with user page descriptors for
/// `page_count` pages starting at `pa_base`, and returns the L3 PA.
/// Clamps to `ENTRIES_PER_TABLE` entries (2048 pages per L3).
#[cfg(target_os = "none")]
pub fn allocate_space_l3(
    ks: &KernelState,
    pa_base: u64,
    page_count: usize,
) -> Result<u64, AllocError> {
    let l3_pa = alloc_zeroed_pages(ks, 1)?;

    // SAFETY: l3_pa points to a zeroed page exclusively owned by us.
    // populate_l3_at_pa converts PA to TTBR1 VA and writes page descriptors.
    unsafe {
        crate::frame::mapping::populate_l3_at_pa(l3_pa as u64, pa_base, page_count);
    }

    Ok(l3_pa as u64)
}

/// Clear L3 entries in a Space's L3 table after a split (D90).
///
/// Clears entries from `from_idx` to `to_idx` (exclusive), clamped to
/// `ENTRIES_PER_TABLE`. Called on the parent Space after SpaceSplit to
/// remove descriptors for pages transferred to the child.
#[cfg(target_os = "none")]
pub fn clear_space_l3_entries(l3_table_pa: u64, from_idx: usize, to_idx: usize) {
    let clamped_to = to_idx.min(page_table::ENTRIES_PER_TABLE);

    if from_idx >= clamped_to {
        return;
    }

    // SAFETY: l3_table_pa points to a valid L3 page owned by this Space.
    // D88: phys_to_virt converts to TTBR1 VA. We clear entries for pages
    // that were transferred to the child Space.
    unsafe {
        let l3 = &mut *(mmu::phys_to_virt(l3_table_pa as usize)
            as *mut [u64; page_table::ENTRIES_PER_TABLE]);

        for i in from_idx..clamped_to {
            page_table::clear_page_entry(l3, i);
        }
    }
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
    // D88: phys_to_virt converts to TTBR1 linear map VA.
    unsafe {
        let l3 = &mut *(mmu::phys_to_virt(l3_pa) as *mut [u64; page_table::ENTRIES_PER_TABLE]);

        l3[1] = page_table::user_code_descriptor(code_pa as u64);
        l3[2] = page_table::user_data_descriptor(stack_pa as u64);
    }

    mmu::install_user_l3_in_kernel_l2(0, l3_pa);

    Ok(())
}

/// Copy a DTB-discovered module binary to the code page (D102).
///
/// The hypervisor loaded the flat binary into guest RAM at `module_pa`.
/// We copy `module_size` bytes to `code_pa`. The caller must ensure
/// `module_size <= PAGE_SIZE`.
#[cfg(target_os = "none")]
fn install_module_binary(code_pa: usize, module_pa: usize, module_size: usize) {
    // SAFETY: module_pa is the physical address from the DTB module-start
    // property — the hypervisor placed the binary there. code_pa points to
    // a zeroed page exclusively owned by us. module_size was validated by
    // the caller. D88: phys_to_virt converts both PAs to TTBR1 VAs.
    unsafe {
        core::ptr::copy_nonoverlapping(
            mmu::phys_to_virt(module_pa) as *const u8,
            mmu::phys_to_virt(code_pa) as *mut u8,
            module_size,
        );
    }
}

/// Copy the embedded fallback binary to the code page (D94).
#[cfg(target_os = "none")]
fn install_fallback_binary(code_pa: usize) {
    // SAFETY: code_pa points to a zeroed page exclusively owned by us.
    // FALLBACK_BINARY.len() < PAGE_SIZE. D88: phys_to_virt converts to
    // TTBR1 VA. FALLBACK_BINARY is in kernel .rodata (identity-mapped).
    unsafe {
        core::ptr::copy_nonoverlapping(
            FALLBACK_BINARY.as_ptr(),
            mmu::phys_to_virt(code_pa) as *mut u8,
            FALLBACK_BINARY.len(),
        );
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
    // fits in one 16 KiB page. D88: phys_to_virt converts to TTBR1 VA.
    unsafe {
        let rs = &mut *(mmu::phys_to_virt(rs_pa) as *mut RegisterState);

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
    asid: u16,
) -> Result<(crate::arena::ObjectId, NonNull<Observer>), AllocError> {
    let mut observers = ks.observers.acquire();
    let (obs_id, obs) = observers.allocate()?;

    obs.object_id = obs_id;
    obs.asid = asid;
    obs.register_state = RegisterStateHandle::new(
        NonNull::new(mmu::phys_to_virt(rs_pa) as *mut u8).expect("rs_pa must be non-null"),
    );
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

/// Set the current Observer on the BSP core state and enqueue it.
///
/// Maintains the scheduler invariant: the running Observer is always
/// in the scheduler queue. All dispatch paths assume this — blocking
/// dequeues, becoming runnable enqueues, Yield and timer rotate.
#[cfg(target_os = "none")]
fn set_current_observer(observer_ptr: NonNull<Observer>) {
    // SAFETY: BSP_CORE_STATE is initialized by init_bsp_per_core_data.
    // Single-threaded boot context. Raw pointer writes avoid Edition 2024's
    // prohibition on references to mutable statics.
    unsafe {
        let cs = &raw mut BSP_CORE_STATE;

        Scheduler::enqueue(&mut (*cs).scheduler, observer_ptr);

        (*cs).current = Some(observer_ptr);
    }
}

/// Enqueue an additional Observer to the BSP scheduler (Phase 2).
///
/// Unlike `set_current_observer`, this does NOT set the Observer as
/// current — it only adds it to the scheduler queue. Called during
/// multi-Observer boot to create child Observers that run after the
/// first context switch.
#[cfg(target_os = "none")]
fn enqueue_observer(observer_ptr: NonNull<Observer>) {
    // SAFETY: BSP_CORE_STATE initialized by init_bsp_per_core_data.
    // Single-threaded boot context.
    unsafe {
        let cs = &raw mut BSP_CORE_STATE;

        Scheduler::enqueue(&mut (*cs).scheduler, observer_ptr);
    }
}

// ── Child Observer binary (Phase 2.2+2.3) ──────────────────────
//
// Minimal EL0 program: IPC Send, then touch unmapped page for VmFault.
// 1. Loads test data into x0–x3 (4 data words)
// 2. Loads label 0x42 into x4
// 3. Loads Send cap handle (slot 3) into x5
// 4. Sets x6 = u64::MAX (no user cap), x7 = 0 (no reply info)
// 5. Executes SVC #1 (Send)
// 6. Loads address 0x200_0000 (child's unmapped null guard page)
// 7. Loads from that address → data abort → VmFault
//
// Expected IPC data words: 0xAA, 0xBB, 0xCC, 0xDD.
// Badge 0x99 is injected by the kernel from the cap entry.
// After Send, the child faults and enters Faulted state. It never
// reaches BRK #0x43 — the fault handler (root) receives the VmFault.

#[cfg(target_os = "none")]
const CHILD_BINARY: &[u8] = &{
    let mut buf = [0u8; 44];

    // movz x0, #0xAA        → 0xD2801540
    buf[0] = 0x40;
    buf[1] = 0x15;
    buf[2] = 0x80;
    buf[3] = 0xD2;

    // movz x1, #0xBB        → 0xD2801761
    buf[4] = 0x61;
    buf[5] = 0x17;
    buf[6] = 0x80;
    buf[7] = 0xD2;

    // movz x2, #0xCC        → 0xD2801982
    buf[8] = 0x82;
    buf[9] = 0x19;
    buf[10] = 0x80;
    buf[11] = 0xD2;

    // movz x3, #0xDD        → 0xD2801BA3
    buf[12] = 0xA3;
    buf[13] = 0x1B;
    buf[14] = 0x80;
    buf[15] = 0xD2;

    // movz x4, #0x42        → 0xD2800844
    buf[16] = 0x44;
    buf[17] = 0x08;
    buf[18] = 0x80;
    buf[19] = 0xD2;

    // movz x5, #3           → 0xD2800065
    buf[20] = 0x65;
    buf[21] = 0x00;
    buf[22] = 0x80;
    buf[23] = 0xD2;

    // movn x6, #0           → 0x92800006  (x6 = ~0 = u64::MAX = CAP_ABSENT)
    buf[24] = 0x06;
    buf[25] = 0x00;
    buf[26] = 0x80;
    buf[27] = 0x92;

    // movz x7, #0           → 0xD2800007
    buf[28] = 0x07;
    buf[29] = 0x00;
    buf[30] = 0x80;
    buf[31] = 0xD2;

    // svc #1                → 0xD4000021
    buf[32] = 0x21;
    buf[33] = 0x00;
    buf[34] = 0x00;
    buf[35] = 0xD4;

    // movz x0, #0x200, lsl #16 → 0xD2A04000  (x0 = 0x0200_0000, child null guard)
    buf[36] = 0x00;
    buf[37] = 0x40;
    buf[38] = 0xA0;
    buf[39] = 0xD2;

    // ldr x1, [x0]          → 0xF9400001  (load from unmapped → VmFault)
    buf[40] = 0x01;
    buf[41] = 0x00;
    buf[42] = 0x40;
    buf[43] = 0xF9;

    buf
};

/// Create and enqueue a child Observer for Phase 2 integration tests.
///
/// Allocates a code page (with CHILD_BINARY), stack page, L1 table,
/// RegisterState, and cap table with IPC Send + fault handler + Space caps.
/// Maps user pages via install_user_l3. The child Observer starts in
/// Runnable state in the scheduler queue.
#[cfg(target_os = "none")]
fn create_child_observer(
    ks: &KernelState,
    ipc_field_id: crate::arena::ObjectId,
    handler_field_id: crate::arena::ObjectId,
    child_space_id: crate::arena::ObjectId,
) -> Result<(crate::arena::ObjectId, NonNull<Observer>), AllocError> {
    let child_code_pa = alloc_zeroed_pages(ks, 1)?;

    // SAFETY: child_code_pa is a valid zeroed page. CHILD_BINARY fits
    // in one page. D88: phys_to_virt converts to TTBR1 VA.
    unsafe {
        core::ptr::copy_nonoverlapping(
            CHILD_BINARY.as_ptr(),
            mmu::phys_to_virt(child_code_pa) as *mut u8,
            CHILD_BINARY.len(),
        );
    }

    let child_stack_pa = alloc_zeroed_pages(ks, 1)?;
    let child_l1_pa = allocate_observer_l1(ks)?;
    let child_l3_pa = alloc_zeroed_pages(ks, 1)?;

    // SAFETY: child_l3_pa is a valid zeroed page. Write L3 page
    // descriptors for the code page (index 1) and stack page (index 2).
    unsafe {
        let l3 =
            &mut *(mmu::phys_to_virt(child_l3_pa) as *mut [u64; page_table::ENTRIES_PER_TABLE]);

        l3[1] = page_table::user_code_descriptor(child_code_pa as u64);
        l3[2] = page_table::user_data_descriptor(child_stack_pa as u64);
    }

    // Both Observers share L2_ROOT (via L1[0]). Child user L3 goes at
    // L2 index 1 to avoid collision with root's pages at index 0. The
    // nG bit + distinct ASIDs prevent cross-Observer TLB aliasing.
    mmu::install_user_l3_in_kernel_l2(1, child_l3_pa);

    // Child user VA: L2 index 1 → VA starts at 32 MiB (0x200_0000).
    // Code page at L3 index 1 → VA = 0x200_0000 + 1 * 16 KiB = 0x200_4000.
    // Stack page at L3 index 2 → VA = 0x200_0000 + 2 * 16 KiB = 0x200_8000.
    let child_code_va: usize = 0x200_4000;
    let child_stack_top: usize = 0x200_8000 + PAGE_SIZE;

    let child_rs_pa = alloc_zeroed_pages(ks, 1)?;

    // SAFETY: child_rs_pa is a valid zeroed page. RegisterState fits.
    unsafe {
        let rs = &mut *(mmu::phys_to_virt(child_rs_pa) as *mut RegisterState);

        rs.pc = child_code_va as u64;
        rs.sp = child_stack_top as u64;
    }

    let child_asid = crate::frame::cores::allocate_asid(ks);
    let child_page_table_root = mmu::make_ttbr0(child_asid, child_l1_pa as u64);

    let mut observers = ks.observers.acquire();
    let (child_id, child_obs) = observers.allocate()?;

    child_obs.object_id = child_id;
    child_obs.asid = child_asid;
    child_obs.register_state = crate::observer::RegisterStateHandle::new(
        NonNull::new(mmu::phys_to_virt(child_rs_pa) as *mut u8)
            .expect("child rs_pa must be non-null"),
    );
    child_obs.page_table_root = child_page_table_root;
    child_obs.cap_table = NonNull::dangling();
    child_obs.cap_table_capacity = 0;
    child_obs.cap_table_free_head = None;
    child_obs.cap_table_count = 0;
    child_obs.state = crate::observer::PrimaryState::Runnable;
    child_obs.suspended = false;
    child_obs.compute_aggregate = 100;
    child_obs.responsiveness = crate::observer::DEFAULT_RESPONSIVENESS;
    child_obs.throughput = crate::observer::DEFAULT_THROUGHPUT;
    child_obs.clock_access = false;
    child_obs.wait_state = crate::observer::WaitState::None;
    child_obs.refcount = 1;
    child_obs.generation = AtomicU64::new(0);

    let child_ptr = NonNull::from(&*child_obs);

    drop(observers);

    // Phase 2.2+2.3: set up child's cap table with IPC, fault handler, and Space caps.
    let child_cap_entries =
        setup_child_cap_table(ks, child_id, ipc_field_id, handler_field_id, child_space_id)?;

    // SAFETY: child_ptr was just created above and is exclusively ours.
    // Single-threaded boot context. No concurrent access.
    unsafe {
        let child = &mut *child_ptr.as_ptr();

        child.cap_table = child_cap_entries;
        child.cap_table_capacity = CHILD_CAP_TABLE_CAPACITY;
        // Freelist starts at slot 5 (slots 0-4 occupied: handler, reply, self, ipc, space).
        child.cap_table_free_head = Some(capability::SLOT_USER_START + 2);
        // 4 installed caps: handler field, self, ipc field, space.
        child.cap_table_count = 4;
    }

    enqueue_observer(child_ptr);

    crate::println!(
        "boot: child observer id={} asid={} entry={:#x}",
        child_id.0,
        child_asid,
        child_code_va,
    );

    Ok((child_id, child_ptr))
}

// ── Cap table setup (Phase 5) ────────────────────────────────────

/// Capacity of the root Observer's cap table.
///
/// 16 entries: slots 0–2 reserved (fault handler, reply, self-cap),
/// slot 3 = root Space cap, slots 4–15 = free. Matches the test helper
/// convention in frame/capabilities.rs.
#[cfg(target_os = "none")]
const ROOT_CAP_TABLE_CAPACITY: u32 = 16;

/// Allocate a cap table page and initialize it for the root Observer.
///
/// Returns `(entries_ptr, capacity)`. The page is zeroed first, then:
/// - Freelist initialized from slot 4 (SLOT_USER_START + 1) to capacity-1.
/// - Slot 2 (SLOT_SELF): self-cap pointing to the Observer.
/// - Slot 3 (first user slot): root Space cap with full rights.
///
/// The cap table page comes from the physical allocator (identity-mapped).
/// A 16 KiB page fits `16384 / size_of::<Entry>()` entries; we use 16.
#[cfg(target_os = "none")]
fn setup_root_cap_table(
    ks: &KernelState,
    observer_id: crate::arena::ObjectId,
    root_space_id: crate::arena::ObjectId,
) -> Result<NonNull<capability::Entry>, AllocError> {
    let cap_page_pa = alloc_zeroed_pages(ks, 1)?;
    // cap_page_pa is a valid physical address returned by alloc_zeroed_pages,
    // zeroed, and exclusively ours. D88: phys_to_virt converts to TTBR1 VA.
    let entries = NonNull::new(mmu::phys_to_virt(cap_page_pa) as *mut capability::Entry)
        .expect("cap_page_pa must be non-null");
    let first_free = capability::SLOT_USER_START + 1;

    crate::frame::capabilities::init_freelist(entries, ROOT_CAP_TABLE_CAPACITY, first_free);
    // Slots 0-1 must be explicitly empty. Zeroed memory is NOT equivalent
    // to Entry { object: None, .. } because Option<(ObjectType, ObjectId)>
    // represents Some((Space, ObjectId(0))) as all-zero bytes (Space = 0,
    // ObjectId(0) = 0). Without this, slot 0 looks like a valid Space cap.
    crate::frame::capabilities::write_entry(
        entries,
        ROOT_CAP_TABLE_CAPACITY,
        capability::SLOT_FAULT_HANDLER,
        capability::Entry::empty(SlotTag(0)),
    );
    crate::frame::capabilities::write_entry(
        entries,
        ROOT_CAP_TABLE_CAPACITY,
        capability::SLOT_REPLY_FIELD,
        capability::Entry::empty(SlotTag(0)),
    );
    // Slot 2 (SLOT_SELF): self-cap with full Observer rights.
    crate::frame::capabilities::write_entry(
        entries,
        ROOT_CAP_TABLE_CAPACITY,
        capability::SLOT_SELF,
        capability::Entry {
            object: Some((ObjectType::Observer, observer_id)),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );
    // Slot 3 (first user slot): root Space cap with full Space rights.
    crate::frame::capabilities::write_entry(
        entries,
        ROOT_CAP_TABLE_CAPACITY,
        capability::SLOT_USER_START,
        capability::Entry {
            object: Some((ObjectType::Space, root_space_id)),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );

    Ok(entries)
}

// ── IPC Field setup (Phase 2.2) ─────────────────────────────────

/// IPC test badge value. The child's Send cap carries this badge;
/// the kernel injects it into the message on Send. The root's IPC
/// verify handler checks it after Receive.
#[cfg(target_os = "none")]
const IPC_TEST_BADGE: u64 = 0x99;

/// Queue capacity for the boot IPC Field.
#[cfg(target_os = "none")]
const IPC_FIELD_QUEUE_CAPACITY: u32 = 4;

/// Cap table capacity for the child Observer.
#[cfg(target_os = "none")]
const CHILD_CAP_TABLE_CAPACITY: u32 = 8;

/// Create a Field in the arena for boot-time testing.
///
/// Allocates queue backing and returns the Field's ObjectId.
/// Used for both the IPC Field (Phase 2.2) and the handler Field (Phase 2.3).
#[cfg(target_os = "none")]
fn create_boot_field(
    ks: &KernelState,
    refcount: u32,
) -> Result<crate::arena::ObjectId, AllocError> {
    let queue = crate::frame::fields::allocate_field_queue(IPC_FIELD_QUEUE_CAPACITY)
        .ok_or(AllocError::OutOfMemory)?;
    let mut value = Field::new(queue, IPC_FIELD_QUEUE_CAPACITY, 0, 0);
    value.refcount = refcount;

    let mut fields = ks.fields.acquire();
    let (field_id, _) = fields.insert(value)?;

    Ok(field_id)
}

/// Create a Space in the arena representing the child Observer's VA range.
///
/// The child's user pages live at L2 index 1 (VA 0x200_0000). We create
/// a Space covering L3 indices 0–2 (3 pages) so that VmFault translation
/// can find it when the child faults on the unmapped null guard page.
#[cfg(target_os = "none")]
fn create_child_space(ks: &KernelState) -> Result<crate::arena::ObjectId, AllocError> {
    let mut spaces = ks.spaces.acquire();
    let (space_id, space) = spaces.allocate()?;

    space.va_base = 0x200_0000;
    space.size = 3 * PAGE_SIZE;
    space.l3_table_pa = 0;
    space.refcount = 1;
    space.generation = AtomicU64::new(0);

    Ok(space_id)
}

/// Set up the child Observer's cap table.
///
/// Slot 0: fault handler — Send cap to handler Field (for VmFault delivery).
/// Slot 1: reply field — empty.
/// Slot 2: self-cap — Observer.
/// Slot 3: Send cap to IPC Field (badge IPC_TEST_BADGE).
/// Slot 4: Space cap (child's VA range, for VmFault translation).
#[cfg(target_os = "none")]
fn setup_child_cap_table(
    ks: &KernelState,
    child_id: crate::arena::ObjectId,
    ipc_field_id: crate::arena::ObjectId,
    handler_field_id: crate::arena::ObjectId,
    child_space_id: crate::arena::ObjectId,
) -> Result<NonNull<capability::Entry>, AllocError> {
    let cap_page_pa = alloc_zeroed_pages(ks, 1)?;
    let entries = NonNull::new(mmu::phys_to_virt(cap_page_pa) as *mut capability::Entry)
        .expect("cap_page_pa must be non-null");
    // Freelist starts at slot 5 (slots 0-4 are all occupied).
    let first_free = capability::SLOT_USER_START + 2;

    crate::frame::capabilities::init_freelist(entries, CHILD_CAP_TABLE_CAPACITY, first_free);
    // Slot 0: handler Field Send cap for fault delivery (D21).
    crate::frame::capabilities::write_entry(
        entries,
        CHILD_CAP_TABLE_CAPACITY,
        capability::SLOT_FAULT_HANDLER,
        capability::Entry {
            object: Some((capability::ObjectType::Field, handler_field_id)),
            rights: capability::Rights::SEND,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );
    crate::frame::capabilities::write_entry(
        entries,
        CHILD_CAP_TABLE_CAPACITY,
        capability::SLOT_REPLY_FIELD,
        capability::Entry::empty(SlotTag(0)),
    );
    crate::frame::capabilities::write_entry(
        entries,
        CHILD_CAP_TABLE_CAPACITY,
        capability::SLOT_SELF,
        capability::Entry {
            object: Some((capability::ObjectType::Observer, child_id)),
            rights: capability::Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );
    // Slot 3: Send cap to IPC Field.
    crate::frame::capabilities::write_entry(
        entries,
        CHILD_CAP_TABLE_CAPACITY,
        capability::SLOT_USER_START,
        capability::Entry {
            object: Some((capability::ObjectType::Field, ipc_field_id)),
            rights: capability::Rights::SEND,
            badge: Badge(IPC_TEST_BADGE),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );
    // Slot 4: Space cap (child's VA range for VmFault translation).
    crate::frame::capabilities::write_entry(
        entries,
        CHILD_CAP_TABLE_CAPACITY,
        capability::SLOT_USER_START + 1,
        capability::Entry {
            object: Some((capability::ObjectType::Space, child_space_id)),
            rights: capability::Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );

    Ok(entries)
}

// ── Pulsar setup (Phase 2.4) ────────────────────────────────────

/// Badge value for the boot-time Pulsar (Phase 2.4).
///
/// The timer_fire message carries this badge. The root Observer's
/// BRK #0x46 verify handler checks it.
#[cfg(target_os = "none")]
const TIMER_TEST_BADGE: u64 = 0xBEEF;

/// Duration in nanoseconds for the boot-time Pulsar (Phase 2.4).
///
/// 50 ms — long enough for the IPC and fault scenarios to complete first,
/// short enough that the test finishes quickly. One-shot (period_ns = 0).
#[cfg(target_os = "none")]
const TIMER_PULSAR_DURATION_NS: u64 = 50_000_000;

/// Create a Pulsar in the arena for the timer fire test (Phase 2.4).
///
/// Arms the Pulsar immediately (D62) with a short one-shot deadline
/// targeting the given timer Field. Returns the Pulsar ObjectId and
/// the absolute deadline in counter ticks for installation in the
/// per-core deadline array.
#[cfg(target_os = "none")]
fn create_boot_pulsar(
    ks: &KernelState,
    timer_field_id: crate::arena::ObjectId,
) -> Result<(crate::arena::ObjectId, u64), AllocError> {
    let counter_freq = crate::frame::arch::cntfrq_el0();
    let now_ticks = crate::frame::arch::cntvct_el0();

    let mut pulsars = ks.pulsars.acquire();
    let (pulsar_id, pulsar) = pulsars.allocate()?;

    *pulsar = crate::pulsar::Pulsar::new(
        timer_field_id,
        crate::capability::Badge(TIMER_TEST_BADGE),
        TIMER_PULSAR_DURATION_NS,
        0,
        counter_freq,
        now_ticks,
    );

    let deadline_ticks = pulsar.next_deadline_ticks;

    Ok((pulsar_id, deadline_ticks))
}

/// Install a Pulsar deadline entry in the BSP's per-core state (Phase 2.4).
///
/// Must be called after `init_bsp_per_core_data`. Writes directly to
/// `BSP_CORE_STATE` — single-threaded boot context, no lock needed.
#[cfg(target_os = "none")]
fn install_deadline_at_boot(
    pulsar_id: crate::arena::ObjectId,
    field_id: crate::arena::ObjectId,
    deadline_ticks: u64,
) {
    use crate::core_manager::DeadlineEntry;

    // SAFETY: BSP_CORE_STATE is initialized by init_bsp_per_core_data.
    // Single-threaded boot context — no concurrent access. Raw pointer
    // write avoids Edition 2024's prohibition on references to mutable statics.
    unsafe {
        let cs = &raw mut BSP_CORE_STATE;
        let count = (*cs).deadline_count;

        (*cs).deadlines[count] = Some(DeadlineEntry {
            deadline_ticks,
            pulsar_id,
            field_id,
        });
        (*cs).deadline_count = count + 1;
    }
}

/// Create the root Space in the Space arena representing usable memory.
///
/// The root Space's va_base and size correspond to the remaining usable
/// physical memory after boot allocations. This Space is what typed
/// syscalls (CreateField, CreateObserver, SpaceSplit) consume.
#[cfg(target_os = "none")]
fn create_root_space(ks: &KernelState) -> Result<crate::arena::ObjectId, AllocError> {
    let (va_base, size, page_size) = {
        let sm = ks.space_manager.acquire();

        (
            sm.next_va_base,
            sm.root_pool.free_bytes,
            sm.root_pool.page_size,
        )
    };
    let page_count = size / page_size;
    let l3_pa = allocate_space_l3(ks, va_base as u64, page_count)?;

    let mut spaces = ks.spaces.acquire();
    let (space_id, space) = spaces.allocate()?;

    space.va_base = va_base;
    space.size = size;
    space.l3_table_pa = l3_pa;
    space.refcount = 1;
    space.generation = AtomicU64::new(0);

    Ok(space_id)
}

// ── Public boot entry point ──────────────────────────────────────

/// Boot the kernel into EL0 with a test Observer (D94, D102).
///
/// When a DTB `/chosen` module node is present (loaded by the hypervisor
/// via `--module`), uses that binary; otherwise falls back to the embedded
/// `FALLBACK_BINARY`.
///
/// Allocates physical pages, builds user mappings, creates the root
/// Observer, initializes per-core data, and context switches to EL0.
/// This function does not return.
#[cfg(target_os = "none")]
pub fn enter_first_observer(ks: &KernelState) -> ! {
    use crate::frame::arch::platform;

    crate::println!("boot: allocating root observer resources");

    let code_pa = alloc_zeroed_pages(ks, 1).expect("allocate code page");
    let module_start = platform::module_start();
    let module_size = platform::module_size();

    if module_start != 0 && module_size != 0 {
        assert!(
            module_size <= PAGE_SIZE,
            "boot: module too large ({module_size} > {PAGE_SIZE})"
        );

        install_module_binary(code_pa, module_start, module_size);

        crate::println!("boot: loaded DTB module ({} bytes)", module_size);
    } else {
        install_fallback_binary(code_pa);

        crate::println!("boot: using embedded fallback binary");
    }

    let stack_pa = alloc_zeroed_pages(ks, 1).expect("allocate stack page");

    setup_user_pages(ks, code_pa, stack_pa).expect("setup user page table");

    crate::println!(
        "boot: code={:#x} stack={:#x} entry={:#x}",
        code_pa,
        stack_pa,
        USER_CODE_VA,
    );

    let rs_pa = setup_register_state(ks).expect("allocate register state");
    let asid = crate::frame::cores::allocate_asid(ks);
    let l1_pa = allocate_observer_l1(ks).expect("allocate root observer L1");
    let page_table_root = mmu::make_ttbr0(asid, l1_pa as u64);
    let (obs_id, obs_ptr) =
        create_root_observer(ks, rs_pa, page_table_root, asid).expect("create root observer");
    // ── Root Space and cap table setup (Phase 5) ────────────────
    let root_space_id = create_root_space(ks).expect("create root space");
    let cap_entries =
        setup_root_cap_table(ks, obs_id, root_space_id).expect("setup root cap table");

    // ── Phase 2.2: IPC Field for integration tests ────────────────
    //
    // Create a Field for IPC testing. Install a Receive cap in root's
    // table (slot 4) and a Send cap in child's table (slot 3).
    let ipc_field_id = create_boot_field(ks, 2).expect("create IPC field");

    // Phase 2.3: handler Field for fault delivery + child Space for VmFault.
    let handler_field_id = create_boot_field(ks, 2).expect("create handler field");
    let child_space_id = create_child_space(ks).expect("create child space");

    // Install Receive cap at slot 4 (IPC Field).
    crate::frame::capabilities::write_entry(
        cap_entries,
        ROOT_CAP_TABLE_CAPACITY,
        capability::SLOT_USER_START + 1,
        capability::Entry {
            object: Some((capability::ObjectType::Field, ipc_field_id)),
            rights: capability::Rights::RECEIVE,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );
    // Install Receive cap at slot 5 (handler Field).
    crate::frame::capabilities::write_entry(
        cap_entries,
        ROOT_CAP_TABLE_CAPACITY,
        capability::SLOT_USER_START + 2,
        capability::Entry {
            object: Some((capability::ObjectType::Field, handler_field_id)),
            rights: capability::Rights::RECEIVE,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );

    // ── Phase 2.4: timer Field + Pulsar for timer fire test ──────
    //
    // Create a one-shot Pulsar that fires after 50 ms, delivering a
    // timer_fire message to a dedicated timer Field. Root Receives on
    // this Field after the IPC and fault scenarios complete.
    let timer_field_id = create_boot_field(ks, 1).expect("create timer field");
    let (pulsar_id, deadline_ticks) =
        create_boot_pulsar(ks, timer_field_id).expect("create boot pulsar");

    // Install Receive cap at slot 6 (timer Field).
    crate::frame::capabilities::write_entry(
        cap_entries,
        ROOT_CAP_TABLE_CAPACITY,
        capability::SLOT_USER_START + 3,
        capability::Entry {
            object: Some((capability::ObjectType::Field, timer_field_id)),
            rights: capability::Rights::RECEIVE,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );

    // SAFETY: obs_ptr was just created above and is exclusively ours.
    // Single-threaded BSP boot — no concurrent access. The &mut is
    // safe because no other references to this Observer exist.
    unsafe {
        let obs = &mut *obs_ptr.as_ptr();

        obs.cap_table = cap_entries;
        obs.cap_table_capacity = ROOT_CAP_TABLE_CAPACITY;
        // Freelist starts at slot 8 (slots 3–7 occupied).
        obs.cap_table_free_head = Some(capability::SLOT_USER_START + 5);
        obs.cap_table_count = 6;
        obs.clock_access = true;
    }

    crate::println!(
        "boot: cap_table capacity={} installed=6 (self+space+ipc+handler+timer+child)",
        ROOT_CAP_TABLE_CAPACITY,
    );

    init_bsp_per_core_data(mmu::phys_to_virt(rs_pa) as *mut RegisterState);
    set_current_observer(obs_ptr);

    // Phase 2.4: install Pulsar deadline in BSP core state.
    // Must be after init_bsp_per_core_data (BSP_CORE_STATE initialized).
    install_deadline_at_boot(pulsar_id, timer_field_id, deadline_ticks);

    crate::println!(
        "boot: pulsar id={} badge={:#x} deadline={}",
        pulsar_id.0,
        TIMER_TEST_BADGE,
        deadline_ticks,
    );

    // ── Phase 2: child Observer for integration tests ────────────
    //
    // Create a second Observer with its own address space. The child's
    // CHILD_BINARY sends an IPC message then touches an unmapped page
    // (VmFault). Root's FALLBACK_BINARY does three Receives: IPC
    // (BRK #0x44 verify), fault message (BRK #0x45 verify), and timer
    // fire (BRK #0x46 verify).
    //
    // Must be called AFTER init_bsp_per_core_data and set_current_observer
    // so that enqueue_observer operates on the initialized BSP_CORE_STATE.
    let (child_id, _child_ptr) =
        create_child_observer(ks, ipc_field_id, handler_field_id, child_space_id)
            .expect("create child observer");

    // Phase 2.5: install child Observer cap at slot 7 in root's table
    // so the root can issue Destroy. OBSERVER_ALL includes Destroy right.
    crate::frame::capabilities::write_entry(
        cap_entries,
        ROOT_CAP_TABLE_CAPACITY,
        capability::SLOT_USER_START + 4,
        capability::Entry {
            object: Some((capability::ObjectType::Observer, child_id)),
            rights: capability::Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: 0,
        },
    );

    crate::println!("boot: entering EL0 at {:#x}", USER_CODE_VA);

    let (rs_ptr, pt_root, clock_access) = crate::frame::cores::observer_restore_info(obs_ptr);

    crate::frame::cores::update_register_state_ptr(rs_ptr);

    // SAFETY: rs_ptr points to a valid, fully initialized RegisterState
    // (setup_register_state wrote PC/SP/pstate). pt_root is the current
    // TTBR0 value (identity map with user pages installed). clock_access
    // is 1 (EL0 counter access enabled for benchmarks). IRQs are
    // currently masked (hardware default from boot). PerCoreData is
    // initialized above.
    unsafe {
        crate::frame::arch::exception::__restore_observer(rs_ptr, pt_root, clock_access);
    }
}
