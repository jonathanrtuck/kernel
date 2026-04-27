//! Page table mapping orchestration (D91).
//!
//! Bridge between the pure page_table building blocks (testable on host)
//! and the hardware page tables (PA→pointer conversion, TLB invalidation).
//!
//! Functions here are unsafe because they dereference physical addresses
//! via the TTBR1 linear map (D88). The linear map must be active (post-
//! boot transition, Phase E).
//!
//! On host builds (`cfg(test)`), these functions are not available — the
//! page_table module's pure functions are tested directly with stack arrays.

#[cfg(target_os = "none")]
use crate::frame::arch::mmu;
#[cfg(target_os = "none")]
use crate::frame::arch::page_table::{
    self, ENTRIES_PER_TABLE, MapError, install_l2_in_l1, install_space_in_l2, l1_l2_table_pa,
    l2_maps_space, remove_l2_from_l1, remove_space_from_l2,
};

/// Convert a physical address to a mutable reference to a page table array.
///
/// Uses the D88 TTBR1 linear map: `VA = PA + KERNEL_VIRT_OFFSET`.
///
/// # Safety
///
/// - `pa` must be a valid, 16 KiB-aligned physical address within RAM.
/// - The caller must ensure exclusive access (no other references to this page).
/// - The TTBR1 linear map must be active (post-boot transition).
#[cfg(target_os = "none")]
unsafe fn pa_to_table(pa: u64) -> &'static mut [u64; ENTRIES_PER_TABLE] {
    // SAFETY: The caller guarantees `pa` is valid RAM and exclusively owned.
    // phys_to_virt adds KERNEL_VIRT_OFFSET, producing the linear-map VA.
    unsafe { &mut *(mmu::phys_to_virt(pa as usize) as *mut [u64; ENTRIES_PER_TABLE]) }
}

/// Map a Space's L3 table into an Observer's page table (D91 install).
///
/// Called when a Space cap is installed into an Observer's cap table.
/// Handles L2 allocation from the root pool if needed.
///
/// Returns `Ok(())` if the mapping was created or already existed (duplicate cap).
/// Returns `Err(MapError::OutOfMemory)` if L2 allocation fails.
///
/// # Safety
///
/// - `observer_page_table_root` must be a valid TTBR0 value with L1 root PA.
/// - `space_va_base` must be 32 MiB aligned (D89).
/// - `space_l3_table_pa` must point to a valid, populated L3 table.
/// - The TTBR1 linear map must be active.
#[cfg(target_os = "none")]
pub unsafe fn map_space_in_observer(
    observer_page_table_root: u64,
    space_va_base: usize,
    space_l3_table_pa: u64,
    space_manager: &mut crate::space_manager::SpaceManager,
) -> Result<(), MapError> {
    let l1_pa = mmu::ttbr_base_address(observer_page_table_root);
    // SAFETY: L1 table PA comes from Observer's page_table_root, set at creation.
    let l1 = unsafe { pa_to_table(l1_pa) };
    let l2_pa = match l1_l2_table_pa(l1, space_va_base) {
        Some(pa) => pa,
        None => {
            let new_l2_pa = space_manager
                .allocate_pages(1)
                .map_err(|_| MapError::OutOfMemory)? as u64;
            // SAFETY: Freshly allocated page from root pool, exclusively ours.
            let new_l2 = unsafe { pa_to_table(new_l2_pa) };

            *new_l2 = [0u64; ENTRIES_PER_TABLE];

            install_l2_in_l1(l1, space_va_base, new_l2_pa);

            new_l2_pa
        }
    };
    // SAFETY: L2 PA either from existing L1 entry or freshly allocated above.
    let l2 = unsafe { pa_to_table(l2_pa) };

    if l2_maps_space(l2, space_va_base, space_l3_table_pa) {
        return Ok(());
    }

    install_space_in_l2(l2, space_va_base, space_l3_table_pa);

    Ok(())
}

/// Unmap a Space from an Observer's page table (D91 close, D101 invalidation).
///
/// Called when the last cap to a Space is closed in an Observer's cap table.
/// Frees the L2 table back to the root pool if it becomes empty.
///
/// D101 threshold-based TLB invalidation:
/// - `page_count <= ASID_TLBI_THRESHOLD`: per-VA (`TLBI VAE1IS`) for each page
/// - `page_count > ASID_TLBI_THRESHOLD`: per-ASID (`TLBI ASIDE1IS`) in one shot
///
/// # Safety
///
/// Same preconditions as [`map_space_in_observer`].
#[cfg(target_os = "none")]
pub unsafe fn unmap_space_from_observer(
    observer_page_table_root: u64,
    space_va_base: usize,
    space_l3_table_pa: u64,
    space_page_count: usize,
    asid: u16,
    space_manager: &mut crate::space_manager::SpaceManager,
) {
    let l1_pa = mmu::ttbr_base_address(observer_page_table_root);
    // SAFETY: L1 table PA from Observer's page_table_root.
    let l1 = unsafe { pa_to_table(l1_pa) };
    let l2_pa = match l1_l2_table_pa(l1, space_va_base) {
        Some(pa) => pa,
        None => return,
    };
    // SAFETY: L2 PA from L1 table descriptor.
    let l2 = unsafe { pa_to_table(l2_pa) };

    if !l2_maps_space(l2, space_va_base, space_l3_table_pa) {
        return;
    }

    let l2_empty = remove_space_from_l2(l2, space_va_base);

    if l2_empty {
        remove_l2_from_l1(l1, space_va_base);
        space_manager.return_pages(l2_pa as usize, 1);
    }

    // D101: threshold-based invalidation strategy.
    if space_page_count <= crate::kernel_state::ASID_TLBI_THRESHOLD {
        mmu::tlb_invalidate_space_pages(asid, space_va_base, space_page_count);
    } else {
        mmu::tlb_invalidate_asid(asid);
    }
}

// ── Safe wrappers for dispatch (D91) ────────────────────────────

/// Wire a Space's L3 table into an Observer's page table (D91).
///
/// Safe wrapper around `map_space_in_observer` for the dispatch layer.
/// Called when a Space cap is installed into an Observer's cap table.
#[cfg(target_os = "none")]
pub fn wire_space_mapping(
    observer_page_table_root: u64,
    space_va_base: usize,
    space_l3_table_pa: u64,
    kernel_state: &crate::kernel_state::KernelState,
) -> Result<(), MapError> {
    let mut sm = kernel_state.space_manager.acquire();

    // SAFETY: observer_page_table_root is a valid TTBR0 value from the Observer.
    // space_va_base and space_l3_table_pa are from the Space arena. The TTBR1
    // linear map is active post-boot.
    unsafe {
        map_space_in_observer(
            observer_page_table_root,
            space_va_base,
            space_l3_table_pa,
            &mut sm,
        )
    }
}

/// Remove a Space's L3 table from an Observer's page table (D91).
///
/// Safe wrapper around `unmap_space_from_observer` for the dispatch layer.
/// Called when the last cap to a Space is closed in an Observer.
#[cfg(target_os = "none")]
pub fn unwire_space_mapping(
    observer_page_table_root: u64,
    space_va_base: usize,
    space_l3_table_pa: u64,
    space_page_count: usize,
    asid: u16,
    kernel_state: &crate::kernel_state::KernelState,
) {
    let mut sm = kernel_state.space_manager.acquire();

    // SAFETY: same preconditions as wire_space_mapping.
    unsafe {
        unmap_space_from_observer(
            observer_page_table_root,
            space_va_base,
            space_l3_table_pa,
            space_page_count,
            asid,
            &mut sm,
        );
    }
}

/// Populate a Space's L3 table at a physical address (D90).
///
/// Called during Space creation (type conversion) to eagerly fill L3
/// entries with page descriptors for the Space's physical pages.
///
/// # Safety
///
/// - `l3_table_pa` must be a valid, 16 KiB-aligned physical address.
/// - The TTBR1 linear map must be active.
#[cfg(target_os = "none")]
pub unsafe fn populate_l3_at_pa(l3_table_pa: u64, content_pa: u64, page_count: usize) {
    // SAFETY: L3 table PA from Space creation, exclusively ours.
    let l3 = unsafe { pa_to_table(l3_table_pa) };

    *l3 = [0u64; ENTRIES_PER_TABLE];

    page_table::populate_l3(l3, content_pa, page_count);
}
