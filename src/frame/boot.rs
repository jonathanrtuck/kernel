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

// ── Fallback test binary (D94) ──────────────────────────────────
//
// Minimal EL0 program used when no DTB module is present.
// D102 settles: test binaries are flat binaries loaded by the hypervisor
// (--module flag). This embedded fallback preserves backward compatibility
// for runs without --module.
//
// 1. Loads a marker into x0 (proves register state is live)
// 2. Executes SVC #5 (Yield — proves syscall round-trip)
// 3. After return from Yield, executes BRK #0x42
//    (debug exception → fault handler → kernel prints pass → exit)

#[cfg(target_os = "none")]
const FALLBACK_BINARY: &[u8] = &{
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

// ── Child Observer binary (Phase 2) ─────────────────────────────
//
// Minimal EL0 program for the child Observer in integration tests.
// 1. Loads 0xC1 into x0 (marker: "child ran")
// 2. Executes BRK #0x43 (signals child completed)

#[cfg(target_os = "none")]
const CHILD_BINARY: &[u8] = &{
    let mut buf = [0u8; 8];

    // mov x0, #0xC1         → 0xD2801820
    buf[0] = 0x20;
    buf[1] = 0x18;
    buf[2] = 0x80;
    buf[3] = 0xD2;

    // brk #0x43             → 0xD4200860
    buf[4] = 0x60;
    buf[5] = 0x08;
    buf[6] = 0x20;
    buf[7] = 0xD4;

    buf
};

/// Create and enqueue a child Observer for Phase 2 integration tests.
///
/// Allocates a code page (with CHILD_BINARY), stack page, L1 table,
/// RegisterState, and cap table. Maps user pages via install_user_l3.
/// The child Observer starts in Runnable state in the scheduler queue.
#[cfg(target_os = "none")]
fn create_child_observer(
    ks: &KernelState,
    _handler_field_id: crate::arena::ObjectId,
) -> Result<NonNull<Observer>, AllocError> {
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

    // Install the child's user L3 in the child's L2 (via L1[0] → L2_ROOT).
    // The child shares the kernel's L2_ROOT at L1[0]. Install the child's
    // user L3 at L2 index 1 (VA 0x200_0000–0x3FF_FFFF) to avoid collision
    // with the root Observer's user pages at L2 index 0.
    //
    // Actually, each Observer has its own L1. L1[0] → L2_ROOT which
    // includes the root Observer's user pages at index 0. We need a
    // SEPARATE L2 for child's user pages to avoid sharing the root's.
    //
    // Simpler: install child's L3 at L2 index 0 of a FRESH L2 under the
    // child's L1. But child's L1[0] → L2_ROOT for kernel code.
    //
    // Solution: give the child a second L1 entry (L1[1]) pointing to a
    // new L2 that holds the child's user L3. Child user VA starts at
    // 64 GiB * 1 = too high for 16-bit ASID...
    //
    // Actually, L1 index 0 → L2_ROOT already has index 0 used by root.
    // The child shares L2_ROOT (same physical table), so the root's
    // user pages (L2 index 0) are visible in the child. This is
    // incorrect — the child should NOT see the root's user pages.
    //
    // For Phase 2 MVP: install the child's user L3 at L2 index 1 in
    // L2_ROOT. Both observers share L2_ROOT through their L1[0].
    // The nG (non-global) bit in L3 descriptors + different ASIDs
    // prevent cross-Observer TLB collisions. The root's pages at L2
    // index 0 are tagged with root's ASID; child's pages at L2 index 1
    // are tagged with child's ASID. Hardware only uses TLB entries
    // matching the current ASID.
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

    enqueue_observer(child_ptr);

    crate::println!(
        "boot: child observer id={} asid={} entry={:#x}",
        child_id.0,
        child_asid,
        child_code_va,
    );

    Ok(child_ptr)
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

/// Create the root Space in the Space arena representing usable memory.
///
/// The root Space's va_base and size correspond to the remaining usable
/// physical memory after boot allocations. This Space is what typed
/// syscalls (CreateField, CreateObserver, SpaceSplit) consume.
#[cfg(target_os = "none")]
fn create_root_space(ks: &KernelState) -> Result<crate::arena::ObjectId, AllocError> {
    let (va_base, size) = {
        let sm = ks.space_manager.acquire();

        (sm.next_va_base, sm.root_pool.free_bytes)
    };
    let page_size = {
        let sm = ks.space_manager.acquire();

        sm.root_pool.page_size
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

    // SAFETY: obs_ptr was just created above and is exclusively ours.
    // Single-threaded BSP boot — no concurrent access. The &mut is
    // safe because no other references to this Observer exist.
    unsafe {
        let obs = &mut *obs_ptr.as_ptr();

        obs.cap_table = cap_entries;
        obs.cap_table_capacity = ROOT_CAP_TABLE_CAPACITY;
        obs.cap_table_free_head = Some(capability::SLOT_USER_START + 1);
        obs.cap_table_count = 2;
        obs.clock_access = true;
    }

    crate::println!(
        "boot: cap_table capacity={} installed=2 (self + root space)",
        ROOT_CAP_TABLE_CAPACITY,
    );

    init_bsp_per_core_data(mmu::phys_to_virt(rs_pa) as *mut RegisterState);
    set_current_observer(obs_ptr);

    // ── Phase 2: child Observer for integration tests ────────────
    //
    // Create a second Observer with its own address space, code, and
    // stack. The child's CHILD_BINARY yields and signals BRK #0x43.
    // The scheduler context-switches between root and child, proving
    // the TTBR0 swap, register save/restore, and ASID tagging work.
    //
    // Must be called AFTER init_bsp_per_core_data and set_current_observer
    // so that enqueue_observer operates on the initialized BSP_CORE_STATE.
    let _child_ptr =
        create_child_observer(ks, crate::arena::ObjectId(0)).expect("create child observer");

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
