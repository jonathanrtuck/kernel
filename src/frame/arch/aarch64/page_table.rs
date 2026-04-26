//! Per-Observer page table management (D89).
//!
//! 3-level user page tables (T0SZ=17, 16 KiB granule):
//!
//! - **L1 root**: per-Observer (2048 entries, each covers 64 GiB)
//! - **L2 tables**: per-Observer (2048 entries, each covers 32 MiB)
//! - **L3 tables**: per-Space, shared across Observers (2048 entries, 16 KiB pages)
//!
//! Spaces are aligned to L2 entry boundaries (32 MiB) so each L3 table
//! belongs to exactly one Space. D26's shared subtree model: Observers'
//! L2 entries point to the same Space-owned L3 tables.
//!
//! This module exports the testable building blocks. The full map_space /
//! unmap_space orchestration that walks physical page tables lives in
//! frame/ wrappers using unsafe PA→pointer conversion.

use super::mmu::USER_VA_END;

const PAGE_SIZE: usize = 16 * 1024;
const PAGE_SHIFT: usize = 14;
const L2_BLOCK_SHIFT: usize = 25;
const L1_BLOCK_SHIFT: usize = 36;

/// Number of entries per page table page (L1, L2, or L3).
///
/// 16 KiB / 8 bytes = 2048.
/// - L1 entry covers 64 GiB (2^36)
/// - L2 entry covers 32 MiB (2^25)
/// - L3 entry covers 16 KiB (2^14)
pub const ENTRIES_PER_TABLE: usize = PAGE_SIZE / 8;

/// VA alignment for Space VA bases (D89).
///
/// Each Space is aligned to an L2 entry boundary (32 MiB) so its L3
/// table(s) can be shared across Observers without entry conflicts.
pub const SPACE_VA_ALIGNMENT: usize = 1 << L2_BLOCK_SHIFT;

// ── Descriptor bits ────────────────────────────────────────────────

const VALID: u64 = 1 << 0;
const TABLE: u64 = 1 << 1;
const PAGE: u64 = 1 << 1;

const ATTR_NORMAL: u64 = 1 << 2;
const AP_RW_EL0: u64 = 0b01 << 6;
const SH_ISH: u64 = 0b11 << 8;
const AF: u64 = 1 << 10;
const NG: u64 = 1 << 11;
const PXN: u64 = 1 << 53;

const PA_MASK: u64 = !((PAGE_SIZE as u64) - 1) & 0x0000_FFFF_FFFF_C000;

// ── Index helpers ──────────────────────────────────────────────────

/// L1 table index for a virtual address (bits\[46:36\]).
#[inline]
pub const fn l1_index(va: usize) -> usize {
    (va >> L1_BLOCK_SHIFT) & (ENTRIES_PER_TABLE - 1)
}

/// L2 table index for a virtual address (bits\[35:25\]).
#[inline]
pub const fn l2_index(va: usize) -> usize {
    (va >> L2_BLOCK_SHIFT) & (ENTRIES_PER_TABLE - 1)
}

/// L3 table index for a virtual address (bits\[24:14\]).
#[inline]
pub const fn l3_index(va: usize) -> usize {
    (va >> PAGE_SHIFT) & (ENTRIES_PER_TABLE - 1)
}

/// Number of L3 tables needed to cover a VA range.
pub const fn l3_tables_for_range(va_base: usize, page_count: usize) -> usize {
    if page_count == 0 {
        return 0;
    }

    let va_end = va_base + (page_count - 1) * PAGE_SIZE;
    let first_l2 = l2_index(va_base);
    let last_l2 = l2_index(va_end);

    last_l2 - first_l2 + 1
}

// ── Descriptor constructors ────────────────────────────────────────

/// Build a user-space L3 page descriptor (D89).
///
/// Maps one 16 KiB page at the given physical address with:
/// - AP = RW for EL0 (permissive default — D26 access rights open)
/// - Normal memory, Inner Shareable
/// - AF = 1 (no access-flag fault on first touch)
/// - nG = 1 (ASID-tagged, per-Observer)
/// - PXN = 1 (EL1 cannot execute from user pages)
/// - UXN = 0 (EL0 CAN execute — permissive default)
pub const fn user_page_descriptor(pa: u64) -> u64 {
    (pa & !((PAGE_SIZE as u64) - 1))
        | ATTR_NORMAL
        | AP_RW_EL0
        | SH_ISH
        | AF
        | NG
        | PXN
        | PAGE
        | VALID
}

/// Build a table descriptor (L1→L2 or L2→L3 — same format on ARM64).
pub const fn table_descriptor(next_level_pa: u64) -> u64 {
    (next_level_pa & !((PAGE_SIZE as u64) - 1)) | TABLE | VALID
}

// ── Entry queries ──────────────────────────────────────────────────

/// Check whether an L2 entry is a valid table descriptor.
pub const fn is_valid_table(entry: u64) -> bool {
    (entry & (VALID | TABLE)) == (VALID | TABLE)
}

/// Extract the L3 table physical address from an L2 table descriptor.
pub const fn l2_table_address(entry: u64) -> u64 {
    entry & PA_MASK
}

/// Check whether an L3 page entry is valid.
pub const fn is_valid_page(entry: u64) -> bool {
    (entry & VALID) != 0
}

/// Extract the physical page address from an L3 page descriptor.
pub const fn page_address(entry: u64) -> u64 {
    entry & PA_MASK
}

// ── Entry-level operations ─────────────────────────────────────────

/// Write a user page descriptor at the specified L3 position.
pub fn write_page_entry(l3: &mut [u64; ENTRIES_PER_TABLE], idx: usize, pa: u64) {
    l3[idx] = user_page_descriptor(pa);
}

/// Clear a page entry (set to invalid).
pub fn clear_page_entry(l3: &mut [u64; ENTRIES_PER_TABLE], idx: usize) {
    l3[idx] = 0;
}

/// Check if an L3 table has no valid entries.
pub fn l3_is_empty(l3: &[u64; ENTRIES_PER_TABLE]) -> bool {
    l3.iter().all(|&e| e == 0)
}

// ── Range validation ───────────────────────────────────────────────

/// Errors from page table mapping operations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapError {
    OutOfMemory,
    OutOfRange,
    VaOverflow,
}

/// Validate that a VA range fits within the user address space (D88).
pub const fn validate_user_range(va_base: usize, page_count: usize) -> Result<(), MapError> {
    let byte_count = page_count * PAGE_SIZE;

    if va_base > usize::MAX - byte_count {
        return Err(MapError::VaOverflow);
    }
    if va_base + byte_count > USER_VA_END {
        return Err(MapError::OutOfRange);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Descriptor construction ────────────────────────────────────

    #[test]
    fn d89_user_page_descriptor_valid() {
        let desc = user_page_descriptor(0x4000_0000);

        assert_ne!(desc & VALID, 0, "descriptor must be valid");
        assert_ne!(desc & PAGE, 0, "descriptor must be a page");
    }

    #[test]
    fn d89_user_page_descriptor_el0_accessible() {
        let desc = user_page_descriptor(0x4000_0000);

        assert_eq!((desc >> 6) & 0b11, 0b01, "AP must be 0b01 (EL0 RW)");
    }

    #[test]
    fn d89_user_page_descriptor_pxn_set() {
        let desc = user_page_descriptor(0x4000_0000);

        assert_ne!(
            desc & PXN,
            0,
            "PXN must be set (no EL1 execute from user pages)"
        );
    }

    #[test]
    fn d89_user_page_descriptor_uxn_clear() {
        let desc = user_page_descriptor(0x4000_0000);
        let uxn = 1u64 << 54;

        assert_eq!(
            desc & uxn,
            0,
            "UXN must be 0 (EL0 can execute — permissive)"
        );
    }

    #[test]
    fn d89_user_page_descriptor_non_global() {
        let desc = user_page_descriptor(0x4000_0000);

        assert_ne!(desc & NG, 0, "nG must be set (ASID-tagged)");
    }

    #[test]
    fn d89_user_page_descriptor_af_set() {
        let desc = user_page_descriptor(0x4000_0000);

        assert_ne!(desc & AF, 0, "AF must be set (no fault on first access)");
    }

    #[test]
    fn d89_user_page_descriptor_normal_memory() {
        let desc = user_page_descriptor(0x4000_0000);

        assert_eq!(
            (desc >> 2) & 0b111,
            0b001,
            "AttrIndx must be 1 (Normal memory)"
        );
    }

    #[test]
    fn d89_user_page_descriptor_inner_shareable() {
        let desc = user_page_descriptor(0x4000_0000);

        assert_eq!((desc >> 8) & 0b11, 0b11, "SH must be Inner Shareable");
    }

    #[test]
    fn d89_user_page_descriptor_preserves_pa() {
        let pa: u64 = 0x4008_0000;
        let desc = user_page_descriptor(pa);

        assert_eq!(page_address(desc), pa, "PA must be preserved in descriptor");
    }

    #[test]
    fn d89_user_page_descriptor_masks_unaligned_pa() {
        let desc = user_page_descriptor(0x4000_1234);

        assert_eq!(
            page_address(desc),
            0x4000_0000,
            "unaligned PA bits must be masked"
        );
    }

    // ── L2 table descriptor ────────────────────────────────────────

    #[test]
    fn d89_table_descriptor_valid() {
        let desc = table_descriptor(0x5000_0000);

        assert!(is_valid_table(desc));
    }

    #[test]
    fn d89_l2_table_address_roundtrip() {
        let pa: u64 = 0x5000_0000;
        let desc = table_descriptor(pa);

        assert_eq!(l2_table_address(desc), pa);
    }

    #[test]
    fn d89_zero_entry_is_invalid() {
        assert!(!is_valid_table(0));
        assert!(!is_valid_page(0));
    }

    // ── Index calculations ─────────────────────────────────────────

    #[test]
    fn d89_l1_index_at_64gib_boundary() {
        assert_eq!(l1_index(0), 0);
        assert_eq!(l1_index(64 * 1024 * 1024 * 1024), 1);
        assert_eq!(l1_index(128 * 1024 * 1024 * 1024), 2);
    }

    #[test]
    fn d89_l1_covers_128_tib() {
        let coverage = ENTRIES_PER_TABLE * (1usize << L1_BLOCK_SHIFT);

        assert_eq!(coverage, 128 * 1024 * 1024 * 1024 * 1024);
    }

    #[test]
    fn d89_l2_index_at_region_boundary() {
        assert_eq!(l2_index(0), 0);
        assert_eq!(l2_index(32 * 1024 * 1024), 1);
        assert_eq!(l2_index(64 * 1024 * 1024), 2);
    }

    #[test]
    fn d89_l3_index_at_page_boundary() {
        assert_eq!(l3_index(0), 0);
        assert_eq!(l3_index(PAGE_SIZE), 1);
        assert_eq!(l3_index(2 * PAGE_SIZE), 2);
    }

    #[test]
    fn d89_l3_index_wraps_at_region_boundary() {
        assert_eq!(l3_index(32 * 1024 * 1024), 0);
    }

    #[test]
    fn d89_l3_tables_for_range_single_region() {
        assert_eq!(l3_tables_for_range(0, 1), 1);
        assert_eq!(l3_tables_for_range(0, 2048), 1);
    }

    #[test]
    fn d89_l3_tables_for_range_cross_boundary() {
        let boundary = 32 * 1024 * 1024;
        let va = boundary - PAGE_SIZE;

        assert_eq!(l3_tables_for_range(va, 2), 2);
    }

    #[test]
    fn d89_l3_tables_for_range_zero_pages() {
        assert_eq!(l3_tables_for_range(0, 0), 0);
    }

    // ── Entry operations ───────────────────────────────────────────

    #[test]
    fn d89_write_and_read_page_entry() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];
        let pa: u64 = 0x8000_0000;
        let idx = 42;

        write_page_entry(&mut l3, idx, pa);

        assert!(is_valid_page(l3[idx]));
        assert_eq!(page_address(l3[idx]), pa);
    }

    #[test]
    fn d89_clear_page_entry_invalidates() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        write_page_entry(&mut l3, 0, 0x8000_0000);
        clear_page_entry(&mut l3, 0);

        assert!(!is_valid_page(l3[0]));
    }

    #[test]
    fn d89_l3_is_empty_on_fresh_table() {
        let l3 = [0u64; ENTRIES_PER_TABLE];

        assert!(l3_is_empty(&l3));
    }

    #[test]
    fn d89_l3_is_not_empty_with_entry() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        write_page_entry(&mut l3, 1000, 0x8000_0000);

        assert!(!l3_is_empty(&l3));
    }

    #[test]
    fn d89_l3_becomes_empty_after_clear() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        write_page_entry(&mut l3, 5, 0x8000_0000);
        clear_page_entry(&mut l3, 5);

        assert!(l3_is_empty(&l3));
    }

    // ── Range validation ───────────────────────────────────────────

    #[test]
    fn d89_validate_user_range_accepts_valid() {
        assert!(validate_user_range(0, 1).is_ok());
        assert!(validate_user_range(0x4000, 4).is_ok());
    }

    #[test]
    fn d89_validate_user_range_rejects_past_end() {
        assert_eq!(
            validate_user_range(USER_VA_END, 1),
            Err(MapError::OutOfRange)
        );
    }

    #[test]
    fn d89_validate_user_range_rejects_overflow() {
        assert_eq!(
            validate_user_range(usize::MAX, 1),
            Err(MapError::VaOverflow)
        );
    }

    // ── Map/unmap simulation ───────────────────────────────────────

    #[test]
    fn d89_map_unmap_roundtrip() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let mut l3 = [0u64; ENTRIES_PER_TABLE];
        let va_base = 0x4000usize;
        let pa_base = 0x8000_0000u64;
        let count = 4;

        for i in 0..count {
            let va = va_base + i * PAGE_SIZE;
            let pa = pa_base + (i as u64) * (PAGE_SIZE as u64);

            l2[l2_index(va)] = table_descriptor(0x5000_0000);

            write_page_entry(&mut l3, l3_index(va), pa);
        }

        for i in 0..count {
            let va = va_base + i * PAGE_SIZE;

            assert!(is_valid_page(l3[l3_index(va)]));
        }

        for i in 0..count {
            let va = va_base + i * PAGE_SIZE;

            clear_page_entry(&mut l3, l3_index(va));
        }

        assert!(l3_is_empty(&l3));
    }

    #[test]
    fn d89_partial_unmap_preserves_other_entries() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        write_page_entry(&mut l3, 10, 0x8000_0000);
        write_page_entry(&mut l3, 20, 0x8004_0000);
        clear_page_entry(&mut l3, 10);

        assert!(!is_valid_page(l3[10]));
        assert!(is_valid_page(l3[20]));
        assert!(!l3_is_empty(&l3));
    }

    // ── Layout invariants ──────────────────────────────────────────

    #[test]
    fn d89_page_size_is_16k() {
        assert_eq!(PAGE_SIZE, 16 * 1024);
    }

    #[test]
    fn d89_entries_per_table_is_2048() {
        assert_eq!(ENTRIES_PER_TABLE, 2048);
    }

    #[test]
    fn d89_l2_covers_64_gib() {
        let coverage = ENTRIES_PER_TABLE * (1 << L2_BLOCK_SHIFT);

        assert_eq!(coverage, 64 * 1024 * 1024 * 1024);
    }

    #[test]
    fn d89_space_va_alignment_is_32_mib() {
        assert_eq!(SPACE_VA_ALIGNMENT, 32 * 1024 * 1024);
    }

    #[test]
    fn d89_space_slots_at_alignment() {
        let slots = USER_VA_END / SPACE_VA_ALIGNMENT;

        assert!(
            slots >= 4_000_000,
            "must support at least 4M Space slots (got {slots})"
        );
    }
}
