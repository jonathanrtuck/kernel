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

/// Extract the next-level table physical address from a table descriptor.
///
/// Works at any level (L1→L2 or L2→L3) — the ARM64 table descriptor
/// format uses the same PA mask at all levels.
pub const fn table_address(entry: u64) -> u64 {
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

// ── Eager L3 population ──────────────────────────────────────────

/// Eagerly populate an L3 table with user page descriptors (D90).
///
/// Writes `page_count` entries starting from index 0, each mapping
/// `pa_base + i * PAGE_SIZE`. If `page_count > ENTRIES_PER_TABLE`,
/// it is clamped to `ENTRIES_PER_TABLE`. Remaining entries are untouched
/// (caller is expected to provide a zeroed table).
///
/// Precondition: the Space's VA base is 32 MiB aligned (D89), so the
/// Space's first page maps to L3 index 0.
pub fn populate_l3(l3: &mut [u64; ENTRIES_PER_TABLE], pa_base: u64, page_count: usize) {
    let count = page_count.min(ENTRIES_PER_TABLE);

    for i in 0..count {
        write_page_entry(l3, i, pa_base + (i as u64) * (PAGE_SIZE as u64));
    }
}

// ── Cap-to-mapping protocol helpers ──────────────────────────────

/// Check whether L1 has an L2 table for the given VA.
///
/// Returns `Some(l2_pa)` if the L1 entry at the VA's L1 index is a
/// valid table descriptor, `None` otherwise.
pub fn l1_l2_table_pa(l1: &[u64; ENTRIES_PER_TABLE], va: usize) -> Option<u64> {
    let entry = l1[l1_index(va)];

    if is_valid_table(entry) {
        Some(table_address(entry))
    } else {
        None
    }
}

/// Install an L2 table descriptor in L1 at the VA's L1 index.
pub fn install_l2_in_l1(l1: &mut [u64; ENTRIES_PER_TABLE], va: usize, l2_pa: u64) {
    l1[l1_index(va)] = table_descriptor(l2_pa);
}

/// Remove the L2 table descriptor from L1 at the VA's L1 index.
pub fn remove_l2_from_l1(l1: &mut [u64; ENTRIES_PER_TABLE], va: usize) {
    l1[l1_index(va)] = 0;
}

/// Install a Space's L3 table descriptor in L2 at the VA's L2 index.
pub fn install_space_in_l2(l2: &mut [u64; ENTRIES_PER_TABLE], va: usize, l3_pa: u64) {
    l2[l2_index(va)] = table_descriptor(l3_pa);
}

/// Remove a Space's L3 table descriptor from L2 at the VA's L2 index.
///
/// Returns `true` if the L2 table is now completely empty (all entries
/// invalid), `false` otherwise. The caller uses this to decide whether
/// to free the L2 table and clear the L1 entry.
pub fn remove_space_from_l2(l2: &mut [u64; ENTRIES_PER_TABLE], va: usize) -> bool {
    l2[l2_index(va)] = 0;

    l2_is_empty(l2)
}

/// Check whether an L2 entry already maps to a specific L3 table (D91 duplicate detection).
///
/// Returns `true` if the L2 entry at the VA's L2 index is a valid table
/// descriptor pointing to `l3_pa`.
pub fn l2_maps_space(l2: &[u64; ENTRIES_PER_TABLE], va: usize, l3_pa: u64) -> bool {
    let entry = l2[l2_index(va)];

    is_valid_table(entry) && table_address(entry) == l3_pa
}

/// Check if an L2 table has no valid entries.
pub fn l2_is_empty(l2: &[u64; ENTRIES_PER_TABLE]) -> bool {
    l2.iter().all(|&e| e == 0)
}

/// Count valid table descriptors in an L2 table.
///
/// Useful for assertions that verify the number of Spaces mapped
/// into an Observer's address space.
pub fn count_valid_l2_entries(l2: &[u64; ENTRIES_PER_TABLE]) -> usize {
    l2.iter().filter(|&&e| is_valid_table(e)).count()
}

/// Count valid page descriptors in an L3 table.
///
/// Useful for assertions that verify how many pages a Space has
/// mapped after populate, split, or partial unmap operations.
pub fn count_valid_l3_pages(l3: &[u64; ENTRIES_PER_TABLE]) -> usize {
    l3.iter().filter(|&&e| is_valid_page(e)).count()
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
    let byte_count = match page_count.checked_mul(PAGE_SIZE) {
        Some(n) => n,
        None => return Err(MapError::VaOverflow),
    };

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
    fn d89_table_address_roundtrip() {
        let pa: u64 = 0x5000_0000;
        let desc = table_descriptor(pa);

        assert_eq!(table_address(desc), pa);
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

    #[test]
    fn d89_validate_user_range_rejects_page_count_overflow() {
        // page_count * PAGE_SIZE wraps to zero without checked_mul,
        // which would silently pass both guards.
        let huge_page_count = (usize::MAX / PAGE_SIZE) + 2;

        assert_eq!(
            validate_user_range(0, huge_page_count),
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

    // ── D90: populate_l3 ──────────────────────────────────────────

    #[test]
    fn d90_populate_l3_correct_count() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        populate_l3(&mut l3, 0x8000_0000, 10);

        assert_eq!(count_valid_l3_pages(&l3), 10);
    }

    #[test]
    fn d90_populate_l3_each_entry_is_valid_page() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        populate_l3(&mut l3, 0x8000_0000, 5);

        for i in 0..5 {
            assert!(is_valid_page(l3[i]), "entry {i} must be a valid page");
            assert_ne!(l3[i] & PAGE, 0, "entry {i} must have PAGE bit set");
            assert_ne!(l3[i] & VALID, 0, "entry {i} must have VALID bit set");
        }
    }

    #[test]
    fn d90_populate_l3_pa_offset_per_entry() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];
        let pa_base: u64 = 0x8000_0000;

        populate_l3(&mut l3, pa_base, 8);

        for i in 0..8 {
            let expected_pa = pa_base + (i as u64) * (PAGE_SIZE as u64);

            assert_eq!(page_address(l3[i]), expected_pa, "entry {i} PA mismatch");
        }
    }

    #[test]
    fn d90_populate_l3_zero_count_leaves_empty() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        populate_l3(&mut l3, 0x8000_0000, 0);

        assert!(l3_is_empty(&l3));
    }

    #[test]
    fn d90_populate_l3_clamps_above_entries_per_table() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        populate_l3(&mut l3, 0x8000_0000, ENTRIES_PER_TABLE + 100);

        assert_eq!(
            count_valid_l3_pages(&l3),
            ENTRIES_PER_TABLE,
            "page_count exceeding ENTRIES_PER_TABLE must be clamped"
        );
    }

    #[test]
    fn d90_populate_l3_exact_table_fills_all() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        populate_l3(&mut l3, 0x8000_0000, ENTRIES_PER_TABLE);

        for i in 0..ENTRIES_PER_TABLE {
            assert!(
                is_valid_page(l3[i]),
                "entry {i} must be valid when fully populated"
            );
        }
    }

    #[test]
    fn d90_populate_l3_remaining_entries_stay_zero() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];
        let count = 100;

        populate_l3(&mut l3, 0x8000_0000, count);

        for i in count..ENTRIES_PER_TABLE {
            assert_eq!(l3[i], 0, "entry {i} beyond page_count must remain zero");
        }
    }

    #[test]
    fn d90_populate_l3_pa_alignment_mask() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];
        // Unaligned PA base — low bits should be masked in the descriptor
        let unaligned_pa: u64 = 0x8000_1234;

        populate_l3(&mut l3, unaligned_pa, 1);

        assert_eq!(
            page_address(l3[0]),
            0x8000_0000,
            "unaligned PA bits must be masked off"
        );
    }

    #[test]
    fn d90_populate_l3_pa_alignment_mask_per_entry() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];
        // Unaligned base: low bits masked, but offset arithmetic still works
        let unaligned_pa: u64 = 0x8000_0FFF;

        populate_l3(&mut l3, unaligned_pa, 3);

        // Each entry's PA is (unaligned_pa + i * PAGE_SIZE) masked to page boundary
        for i in 0..3 {
            let raw_pa = unaligned_pa + (i as u64) * (PAGE_SIZE as u64);
            let expected_pa = raw_pa & !((PAGE_SIZE as u64) - 1);

            assert_eq!(
                page_address(l3[i]),
                expected_pa,
                "entry {i}: PA alignment mask must be applied"
            );
        }
    }

    #[test]
    fn d90_populate_l3_descriptor_has_correct_attributes() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        populate_l3(&mut l3, 0x8000_0000, 1);

        let desc = l3[0];

        // Each entry should be a full user_page_descriptor
        assert_ne!(desc & AF, 0, "AF must be set");
        assert_ne!(desc & NG, 0, "nG must be set");
        assert_ne!(desc & PXN, 0, "PXN must be set");
        assert_eq!((desc >> 6) & 0b11, 0b01, "AP must be EL0 RW");
        assert_eq!((desc >> 8) & 0b11, 0b11, "SH must be Inner Shareable");
    }

    #[test]
    fn d90_populate_l3_single_entry() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        populate_l3(&mut l3, 0xA000_0000, 1);

        assert!(is_valid_page(l3[0]));
        assert_eq!(page_address(l3[0]), 0xA000_0000);
        assert!(!is_valid_page(l3[1]));
    }

    // ── D91: install_l2_in_l1 ─────────────────────────────────────

    #[test]
    fn d91_install_l2_in_l1_creates_valid_table() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let l2_pa: u64 = 0x5000_0000;
        let va = 0usize; // L1 index 0

        install_l2_in_l1(&mut l1, va, l2_pa);

        assert!(is_valid_table(l1[l1_index(va)]));
    }

    #[test]
    fn d91_install_l2_in_l1_correct_index() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let l2_pa: u64 = 0x5000_0000;
        // VA at 64 GiB boundary -> L1 index 1
        let va = 64 * 1024 * 1024 * 1024usize;

        install_l2_in_l1(&mut l1, va, l2_pa);

        assert_eq!(l1_index(va), 1);
        assert!(is_valid_table(l1[1]));
        assert_eq!(l1[0], 0, "other L1 entries must be untouched");
    }

    #[test]
    fn d91_install_l2_in_l1_preserves_pa() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let l2_pa: u64 = 0x5000_0000;

        install_l2_in_l1(&mut l1, 0, l2_pa);

        assert_eq!(table_address(l1[0]), l2_pa);
    }

    // ── D91: l1_l2_table_pa ───────────────────────────────────────

    #[test]
    fn d91_l1_l2_table_pa_returns_some_for_valid() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let l2_pa: u64 = 0x5000_0000;

        install_l2_in_l1(&mut l1, 0, l2_pa);

        assert_eq!(l1_l2_table_pa(&l1, 0), Some(l2_pa));
    }

    #[test]
    fn d91_l1_l2_table_pa_returns_none_for_invalid() {
        let l1 = [0u64; ENTRIES_PER_TABLE];

        assert_eq!(l1_l2_table_pa(&l1, 0), None);
    }

    #[test]
    fn d91_l1_l2_table_pa_uses_correct_l1_index() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let l2_pa: u64 = 0x6000_0000;
        let va = 128 * 1024 * 1024 * 1024usize; // L1 index 2

        install_l2_in_l1(&mut l1, va, l2_pa);

        assert_eq!(l1_l2_table_pa(&l1, va), Some(l2_pa));
        // Different VA in index 0 should return None
        assert_eq!(l1_l2_table_pa(&l1, 0), None);
    }

    // ── D91: remove_l2_from_l1 ────────────────────────────────────

    #[test]
    fn d91_remove_l2_from_l1_clears_entry() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];

        install_l2_in_l1(&mut l1, 0, 0x5000_0000);
        remove_l2_from_l1(&mut l1, 0);

        assert_eq!(l1_l2_table_pa(&l1, 0), None);
        assert_eq!(l1[0], 0);
    }

    #[test]
    fn d91_remove_l2_from_l1_only_affects_target_index() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let va_0 = 0usize;
        let va_1 = 64 * 1024 * 1024 * 1024usize;

        install_l2_in_l1(&mut l1, va_0, 0x5000_0000);
        install_l2_in_l1(&mut l1, va_1, 0x6000_0000);
        remove_l2_from_l1(&mut l1, va_0);

        assert_eq!(l1_l2_table_pa(&l1, va_0), None);
        assert_eq!(l1_l2_table_pa(&l1, va_1), Some(0x6000_0000));
    }

    // ── D91: install_space_in_l2 ──────────────────────────────────

    #[test]
    fn d91_install_space_in_l2_creates_valid_table() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let l3_pa: u64 = 0x7000_0000;
        let va = 0usize;

        install_space_in_l2(&mut l2, va, l3_pa);

        assert!(is_valid_table(l2[l2_index(va)]));
    }

    #[test]
    fn d91_install_space_in_l2_correct_index() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let l3_pa: u64 = 0x7000_0000;
        // VA at 32 MiB boundary -> L2 index 1
        let va = SPACE_VA_ALIGNMENT;

        install_space_in_l2(&mut l2, va, l3_pa);

        assert_eq!(l2_index(va), 1);
        assert!(is_valid_table(l2[1]));
        assert_eq!(l2[0], 0, "other L2 entries must be untouched");
    }

    #[test]
    fn d91_install_space_in_l2_preserves_l3_pa() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let l3_pa: u64 = 0x7000_0000;

        install_space_in_l2(&mut l2, 0, l3_pa);

        assert_eq!(table_address(l2[0]), l3_pa);
    }

    // ── D91: remove_space_from_l2 ─────────────────────────────────

    #[test]
    fn d91_remove_space_from_l2_clears_entry() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];

        install_space_in_l2(&mut l2, 0, 0x7000_0000);

        let _ = remove_space_from_l2(&mut l2, 0);

        assert_eq!(l2[0], 0);
    }

    #[test]
    fn d91_remove_space_from_l2_returns_true_when_empty() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];

        install_space_in_l2(&mut l2, 0, 0x7000_0000);

        let is_empty = remove_space_from_l2(&mut l2, 0);

        assert!(is_empty, "removing the only entry must yield true (empty)");
    }

    #[test]
    fn d91_remove_space_from_l2_returns_false_when_not_empty() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];

        install_space_in_l2(&mut l2, 0, 0x7000_0000);
        install_space_in_l2(&mut l2, SPACE_VA_ALIGNMENT, 0x8000_0000);

        let is_empty = remove_space_from_l2(&mut l2, 0);

        assert!(!is_empty, "other entries remain — must not be empty");
    }

    // ── D91: l2_maps_space ────────────────────────────────────────

    #[test]
    fn d91_l2_maps_space_true_for_matching() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let l3_pa: u64 = 0x7000_0000;

        install_space_in_l2(&mut l2, 0, l3_pa);

        assert!(l2_maps_space(&l2, 0, l3_pa));
    }

    #[test]
    fn d91_l2_maps_space_false_for_wrong_pa() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];

        install_space_in_l2(&mut l2, 0, 0x7000_0000);

        assert!(
            !l2_maps_space(&l2, 0, 0x8000_0000),
            "different L3 PA must not match"
        );
    }

    #[test]
    fn d91_l2_maps_space_false_for_empty_entry() {
        let l2 = [0u64; ENTRIES_PER_TABLE];

        assert!(!l2_maps_space(&l2, 0, 0x7000_0000));
    }

    #[test]
    fn d91_l2_maps_space_false_for_different_index() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let l3_pa: u64 = 0x7000_0000;

        install_space_in_l2(&mut l2, 0, l3_pa);

        // Check at a different VA (different L2 index)
        assert!(!l2_maps_space(&l2, SPACE_VA_ALIGNMENT, l3_pa));
    }

    // ── D91: l2_is_empty ──────────────────────────────────────────

    #[test]
    fn d91_l2_is_empty_on_fresh_table() {
        let l2 = [0u64; ENTRIES_PER_TABLE];

        assert!(l2_is_empty(&l2));
    }

    #[test]
    fn d91_l2_is_not_empty_with_entry() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];

        install_space_in_l2(&mut l2, 0, 0x7000_0000);

        assert!(!l2_is_empty(&l2));
    }

    // ── D91: Full roundtrip ───────────────────────────────────────

    #[test]
    fn d91_full_roundtrip_install_verify_remove() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let l2_pa: u64 = 0x5000_0000;
        let l3_pa: u64 = 0x7000_0000;
        let va = SPACE_VA_ALIGNMENT; // 32 MiB — L1 index 0, L2 index 1

        // Step 1: Install L2 in L1
        install_l2_in_l1(&mut l1, va, l2_pa);

        assert_eq!(l1_l2_table_pa(&l1, va), Some(l2_pa));

        // Step 2: Install Space (L3) in L2
        install_space_in_l2(&mut l2, va, l3_pa);

        assert!(l2_maps_space(&l2, va, l3_pa));

        // Step 3: Verify both levels
        assert!(is_valid_table(l1[l1_index(va)]));
        assert!(is_valid_table(l2[l2_index(va)]));

        // Step 4: Remove Space from L2
        let l2_empty = remove_space_from_l2(&mut l2, va);

        assert!(l2_empty, "L2 had only one entry, should be empty now");
        assert!(!l2_maps_space(&l2, va, l3_pa));

        // Step 5: Remove L2 from L1 (since L2 is empty)
        remove_l2_from_l1(&mut l1, va);

        assert_eq!(l1_l2_table_pa(&l1, va), None);
    }

    // ── D89 integration: multiple Spaces in same L2 ───────────────

    #[test]
    fn d89_multiple_spaces_in_same_l2() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let l3_pa_a: u64 = 0x7000_0000;
        let l3_pa_b: u64 = 0x8000_0000;
        let l3_pa_c: u64 = 0x9000_0000;
        let va_a = 0usize;
        let va_b = SPACE_VA_ALIGNMENT;
        let va_c = 2 * SPACE_VA_ALIGNMENT;

        install_space_in_l2(&mut l2, va_a, l3_pa_a);
        install_space_in_l2(&mut l2, va_b, l3_pa_b);
        install_space_in_l2(&mut l2, va_c, l3_pa_c);

        assert!(l2_maps_space(&l2, va_a, l3_pa_a));
        assert!(l2_maps_space(&l2, va_b, l3_pa_b));
        assert!(l2_maps_space(&l2, va_c, l3_pa_c));
    }

    #[test]
    fn d89_removing_one_space_preserves_others() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let l3_pa_a: u64 = 0x7000_0000;
        let l3_pa_b: u64 = 0x8000_0000;
        let va_a = 0usize;
        let va_b = SPACE_VA_ALIGNMENT;

        install_space_in_l2(&mut l2, va_a, l3_pa_a);
        install_space_in_l2(&mut l2, va_b, l3_pa_b);

        let is_empty = remove_space_from_l2(&mut l2, va_a);

        assert!(!is_empty, "L2 still has entry for va_b");
        assert!(!l2_maps_space(&l2, va_a, l3_pa_a), "va_a must be removed");
        assert!(l2_maps_space(&l2, va_b, l3_pa_b), "va_b must be preserved");
    }

    #[test]
    fn d89_two_observers_share_l3_pa() {
        // Two L1 tables (two Observers) both install L2 entries pointing
        // to the same L3 PA (shared Space, D26/D89).
        let mut l1_observer_a = [0u64; ENTRIES_PER_TABLE];
        let mut l1_observer_b = [0u64; ENTRIES_PER_TABLE];
        let mut l2_observer_a = [0u64; ENTRIES_PER_TABLE];
        let mut l2_observer_b = [0u64; ENTRIES_PER_TABLE];
        let l2_pa_a: u64 = 0x5000_0000;
        let l2_pa_b: u64 = 0x5000_4000;
        let shared_l3_pa: u64 = 0x7000_0000;
        let va = SPACE_VA_ALIGNMENT;

        // Observer A
        install_l2_in_l1(&mut l1_observer_a, va, l2_pa_a);
        install_space_in_l2(&mut l2_observer_a, va, shared_l3_pa);
        // Observer B
        install_l2_in_l1(&mut l1_observer_b, va, l2_pa_b);
        install_space_in_l2(&mut l2_observer_b, va, shared_l3_pa);

        // Both point to the same L3 table
        assert!(l2_maps_space(&l2_observer_a, va, shared_l3_pa));
        assert!(l2_maps_space(&l2_observer_b, va, shared_l3_pa));

        // Removing from Observer A does not affect Observer B
        let _ = remove_space_from_l2(&mut l2_observer_a, va);

        assert!(!l2_maps_space(&l2_observer_a, va, shared_l3_pa));
        assert!(l2_maps_space(&l2_observer_b, va, shared_l3_pa));
    }

    #[test]
    fn d89_space_va_bases_at_l2_boundaries() {
        // Verify that Space VA bases at 32 MiB boundaries produce the correct
        // L2 indices, covering several boundary values.
        for i in 0..8 {
            let va = i * SPACE_VA_ALIGNMENT;

            assert_eq!(l2_index(va), i, "VA {:#x} must map to L2 index {i}", va);
        }
    }

    #[test]
    fn d89_space_va_base_l1_l2_decomposition() {
        // A VA at 64 GiB + 32 MiB should map to L1 index 1, L2 index 1
        let va = 64 * 1024 * 1024 * 1024 + SPACE_VA_ALIGNMENT;

        assert_eq!(l1_index(va), 1);
        assert_eq!(l2_index(va), 1);
    }

    // ── Adversarial tests ─────────────────────────────────────────

    #[test]
    fn d91_va_at_maximum_user_range_boundary() {
        // Last valid L2 slot in user range: USER_VA_END - SPACE_VA_ALIGNMENT
        let va = USER_VA_END - SPACE_VA_ALIGNMENT;
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let l3_pa: u64 = 0x7000_0000;

        install_space_in_l2(&mut l2, va, l3_pa);

        assert!(l2_maps_space(&l2, va, l3_pa));

        // Verify the index is what we expect
        let idx = l2_index(va);

        assert!(
            idx < ENTRIES_PER_TABLE,
            "L2 index must be within table bounds"
        );
    }

    #[test]
    fn d91_l2_all_entries_filled_remove_last_returns_true() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];

        // Fill all 2048 entries
        for i in 0..ENTRIES_PER_TABLE {
            let va = i * SPACE_VA_ALIGNMENT;
            let l3_pa = 0x7000_0000u64 + (i as u64) * (PAGE_SIZE as u64);

            install_space_in_l2(&mut l2, va, l3_pa);
        }
        // Remove all but the last
        for i in 0..(ENTRIES_PER_TABLE - 1) {
            let va = i * SPACE_VA_ALIGNMENT;
            let is_empty = remove_space_from_l2(&mut l2, va);

            assert!(!is_empty, "table should not be empty yet (removed {i})");
        }

        // Remove the last one
        let last_va = (ENTRIES_PER_TABLE - 1) * SPACE_VA_ALIGNMENT;
        let is_empty = remove_space_from_l2(&mut l2, last_va);

        assert!(is_empty, "removing the last entry must return true (empty)");
    }

    #[test]
    fn d91_duplicate_mapping_detection_wrong_l3_pa() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let correct_l3_pa: u64 = 0x7000_0000;
        let wrong_l3_pa: u64 = 0x8000_0000;

        install_space_in_l2(&mut l2, 0, correct_l3_pa);

        assert!(
            l2_maps_space(&l2, 0, correct_l3_pa),
            "correct PA must match"
        );
        assert!(
            !l2_maps_space(&l2, 0, wrong_l3_pa),
            "wrong PA must not match — duplicate detection"
        );
    }

    #[test]
    fn d91_l1_l2_table_pa_after_double_install() {
        // Installing a second L2 PA at the same L1 index overwrites
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let first_l2_pa: u64 = 0x5000_0000;
        let second_l2_pa: u64 = 0x6000_0000;

        install_l2_in_l1(&mut l1, 0, first_l2_pa);
        install_l2_in_l1(&mut l1, 0, second_l2_pa);

        assert_eq!(
            l1_l2_table_pa(&l1, 0),
            Some(second_l2_pa),
            "second install must overwrite"
        );
    }

    #[test]
    fn d90_populate_l3_then_verify_with_entry_helpers() {
        // Integration: populate via D90 then verify entries with D89 helpers
        let mut l3 = [0u64; ENTRIES_PER_TABLE];
        let pa_base: u64 = 0xA000_0000;
        let count = 64;

        populate_l3(&mut l3, pa_base, count);

        for i in 0..count {
            assert!(is_valid_page(l3[i]), "populated entry {i} must be valid");

            let expected_pa = pa_base + (i as u64) * (PAGE_SIZE as u64);

            assert_eq!(page_address(l3[i]), expected_pa);
        }

        // Can clear individual entries
        clear_page_entry(&mut l3, 0);

        assert!(!is_valid_page(l3[0]));
        assert!(is_valid_page(l3[1]));
    }

    #[test]
    fn d91_remove_l2_from_l1_on_empty_is_noop() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];

        // Removing from an already-empty entry should not panic or corrupt
        remove_l2_from_l1(&mut l1, 0);

        assert_eq!(l1[0], 0);
        assert_eq!(l1_l2_table_pa(&l1, 0), None);
    }

    #[test]
    fn d91_remove_space_from_l2_on_empty_returns_true() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];

        // Removing from an already-empty slot — table was already empty
        let is_empty = remove_space_from_l2(&mut l2, 0);

        assert!(is_empty, "removing from empty table must return true");
    }

    // ── D91: count helpers ────────────────────────────────────────

    #[test]
    fn d91_count_valid_l2_entries_empty() {
        let l2 = [0u64; ENTRIES_PER_TABLE];

        assert_eq!(count_valid_l2_entries(&l2), 0);
    }

    #[test]
    fn d91_count_valid_l2_entries_after_installs() {
        let mut l2 = [0u64; ENTRIES_PER_TABLE];

        install_space_in_l2(&mut l2, 0, 0x7000_0000);
        install_space_in_l2(&mut l2, SPACE_VA_ALIGNMENT, 0x8000_0000);
        install_space_in_l2(&mut l2, 2 * SPACE_VA_ALIGNMENT, 0x9000_0000);

        assert_eq!(count_valid_l2_entries(&l2), 3);
    }

    #[test]
    fn d91_count_valid_l3_pages_empty() {
        let l3 = [0u64; ENTRIES_PER_TABLE];

        assert_eq!(count_valid_l3_pages(&l3), 0);
    }

    #[test]
    fn d91_count_valid_l3_pages_after_populate() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        populate_l3(&mut l3, 0x8000_0000, 512);

        assert_eq!(count_valid_l3_pages(&l3), 512);
    }

    // ── D91 Integration: full cap-to-mapping lifecycle ────────────

    /// Scenario (a): Single Observer, single Space — full lifecycle.
    ///
    /// Simulates the complete D91 protocol: allocate tables, populate
    /// L3, check L1 → install L2, check duplicate → install Space,
    /// verify, unmap, free, verify clean.
    #[test]
    fn d91_integration_single_observer_single_space_full_lifecycle() {
        // Allocate all page tables (stack-allocated, zeroed)
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let mut l3 = [0u64; ENTRIES_PER_TABLE];
        // Fake PAs — building blocks store these in descriptors but never
        // dereference them, so any page-aligned value works for testing.
        let l2_pa: u64 = 0x0010_0000;
        let l3_pa: u64 = 0x0020_0000;
        let space_pa_base: u64 = 0x4000_0000;
        let space_page_count = 256;
        let va = 0usize; // L1 index 0, L2 index 0

        // ── Step 1: Populate L3 with the Space's physical pages (D90) ──
        populate_l3(&mut l3, space_pa_base, space_page_count);

        assert_eq!(count_valid_l3_pages(&l3), space_page_count);
        // Spot-check first and last page
        assert_eq!(page_address(l3[0]), space_pa_base);
        assert_eq!(
            page_address(l3[space_page_count - 1]),
            space_pa_base + ((space_page_count - 1) as u64) * (PAGE_SIZE as u64)
        );
        // ── Step 2: Check L1 — no L2 yet ──
        assert_eq!(l1_l2_table_pa(&l1, va), None, "L1 must start empty");

        // ── Step 3: Allocate and install L2 in L1 ──
        install_l2_in_l1(&mut l1, va, l2_pa);

        assert_eq!(l1_l2_table_pa(&l1, va), Some(l2_pa));
        // ── Step 4: Check for duplicate (D91) — no existing mapping ──
        assert!(
            !l2_maps_space(&l2, va, l3_pa),
            "Space not yet installed — duplicate check must return false"
        );

        // ── Step 5: Install Space's L3 table in L2 ──
        install_space_in_l2(&mut l2, va, l3_pa);

        assert!(
            l2_maps_space(&l2, va, l3_pa),
            "Observer must now see the Space"
        );
        assert_eq!(count_valid_l2_entries(&l2), 1);

        // ── Step 6: Unmap — remove Space from L2 ──
        let l2_empty = remove_space_from_l2(&mut l2, va);

        assert!(l2_empty, "L2 had one entry, must be empty after removal");
        assert!(
            !l2_maps_space(&l2, va, l3_pa),
            "Space must no longer be visible"
        );

        // ── Step 7: Free L2 — remove from L1 ──
        remove_l2_from_l1(&mut l1, va);

        assert_eq!(l1_l2_table_pa(&l1, va), None, "L1 must be clean");
        // ── Step 8: Verify fully clean state ──
        assert!(l2_is_empty(&l2));
        assert_eq!(l1[l1_index(va)], 0);
        // L3 retains its pages (Space still exists, just unmapped from this Observer)
        assert_eq!(count_valid_l3_pages(&l3), space_page_count);
    }

    /// Scenario (b): Single Observer, three Spaces — install and selective unmap.
    ///
    /// Three Spaces at 0, 32 MiB, 64 MiB share the same L2 table.
    /// Unmap the middle one: L2 not empty. Unmap remaining: last returns true.
    #[test]
    fn d91_integration_three_spaces_selective_unmap() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let mut l3_a = [0u64; ENTRIES_PER_TABLE];
        let mut l3_b = [0u64; ENTRIES_PER_TABLE];
        let mut l3_c = [0u64; ENTRIES_PER_TABLE];
        let l2_pa: u64 = 0x0010_0000;
        let l3_pa_a: u64 = 0x0020_0000;
        let l3_pa_b: u64 = 0x0030_0000;
        let l3_pa_c: u64 = 0x0040_0000;
        let va_a = 0usize;
        let va_b = SPACE_VA_ALIGNMENT; // 32 MiB
        let va_c = 2 * SPACE_VA_ALIGNMENT; // 64 MiB

        // All three VAs have L1 index 0 — they share the same L2
        assert_eq!(l1_index(va_a), 0);
        assert_eq!(l1_index(va_b), 0);
        assert_eq!(l1_index(va_c), 0);

        // Populate each Space's L3 table
        populate_l3(&mut l3_a, 0x4000_0000, 100);
        populate_l3(&mut l3_b, 0x5000_0000, 200);
        populate_l3(&mut l3_c, 0x6000_0000, 50);
        // Install L2 in L1 (once, since all three share the same L1 index)
        install_l2_in_l1(&mut l1, va_a, l2_pa);
        // Install all three Spaces in L2
        install_space_in_l2(&mut l2, va_a, l3_pa_a);
        install_space_in_l2(&mut l2, va_b, l3_pa_b);
        install_space_in_l2(&mut l2, va_c, l3_pa_c);

        assert_eq!(count_valid_l2_entries(&l2), 3);
        assert!(l2_maps_space(&l2, va_a, l3_pa_a));
        assert!(l2_maps_space(&l2, va_b, l3_pa_b));
        assert!(l2_maps_space(&l2, va_c, l3_pa_c));

        // ── Unmap the middle Space (B) ──
        let l2_empty = remove_space_from_l2(&mut l2, va_b);

        assert!(!l2_empty, "two Spaces remain — L2 must not be empty");
        assert!(!l2_maps_space(&l2, va_b, l3_pa_b), "B must be gone");
        assert!(l2_maps_space(&l2, va_a, l3_pa_a), "A must survive");
        assert!(l2_maps_space(&l2, va_c, l3_pa_c), "C must survive");
        assert_eq!(count_valid_l2_entries(&l2), 2);

        // ── Unmap Space A ──
        let l2_empty = remove_space_from_l2(&mut l2, va_a);

        assert!(!l2_empty, "one Space remains — L2 must not be empty");
        assert_eq!(count_valid_l2_entries(&l2), 1);

        // ── Unmap Space C (last one) ──
        let l2_empty = remove_space_from_l2(&mut l2, va_c);

        assert!(l2_empty, "last Space removed — L2 must be empty");
        assert_eq!(count_valid_l2_entries(&l2), 0);

        // Clean up L1
        remove_l2_from_l1(&mut l1, va_a);

        assert_eq!(l1_l2_table_pa(&l1, va_a), None);
        // L3 tables still have their pages (Spaces still exist as objects)
        assert_eq!(count_valid_l3_pages(&l3_a), 100);
        assert_eq!(count_valid_l3_pages(&l3_b), 200);
        assert_eq!(count_valid_l3_pages(&l3_c), 50);
    }

    /// Scenario (c): Two Observers sharing a Space (D26/D89).
    ///
    /// Two L1 tables, two L2 tables, ONE shared L3 table. Both Observers
    /// install the same Space. Unmapping from Observer A does not affect B.
    #[test]
    fn d91_integration_two_observers_shared_space() {
        // Observer A's page tables
        let mut l1_a = [0u64; ENTRIES_PER_TABLE];
        let mut l2_a = [0u64; ENTRIES_PER_TABLE];
        // Observer B's page tables
        let mut l1_b = [0u64; ENTRIES_PER_TABLE];
        let mut l2_b = [0u64; ENTRIES_PER_TABLE];
        // Shared Space's L3 table
        let mut l3_shared = [0u64; ENTRIES_PER_TABLE];
        let l2_pa_a: u64 = 0x0010_0000;
        let l2_pa_b: u64 = 0x0014_0000;
        let shared_l3_pa: u64 = 0x0020_0000;
        let va = SPACE_VA_ALIGNMENT; // Both Observers map the Space at the same VA

        // Populate the shared L3 table (D90)
        populate_l3(&mut l3_shared, 0x8000_0000, 1024);

        assert_eq!(count_valid_l3_pages(&l3_shared), 1024);
        // ── Observer A installs the Space ──
        assert_eq!(l1_l2_table_pa(&l1_a, va), None);

        install_l2_in_l1(&mut l1_a, va, l2_pa_a);

        assert!(!l2_maps_space(&l2_a, va, shared_l3_pa));

        install_space_in_l2(&mut l2_a, va, shared_l3_pa);

        // ── Observer B installs the same Space ──
        assert_eq!(l1_l2_table_pa(&l1_b, va), None);

        install_l2_in_l1(&mut l1_b, va, l2_pa_b);

        assert!(!l2_maps_space(&l2_b, va, shared_l3_pa));

        install_space_in_l2(&mut l2_b, va, shared_l3_pa);

        // ── Both Observers can see the Space ──
        assert!(l2_maps_space(&l2_a, va, shared_l3_pa));
        assert!(l2_maps_space(&l2_b, va, shared_l3_pa));
        // Both L2 entries point to the SAME L3 PA — the subtree sharing model
        assert_eq!(table_address(l2_a[l2_index(va)]), shared_l3_pa);
        assert_eq!(table_address(l2_b[l2_index(va)]), shared_l3_pa);

        // ── Unmap from Observer A ──
        let l2_a_empty = remove_space_from_l2(&mut l2_a, va);

        assert!(l2_a_empty);

        remove_l2_from_l1(&mut l1_a, va);

        // Observer A is clean
        assert!(!l2_maps_space(&l2_a, va, shared_l3_pa));
        assert_eq!(l1_l2_table_pa(&l1_a, va), None);
        // Observer B still has the mapping — completely unaffected
        assert!(l2_maps_space(&l2_b, va, shared_l3_pa));
        assert_eq!(l1_l2_table_pa(&l1_b, va), Some(l2_pa_b));
        // Shared L3 table is untouched (Space is still alive)
        assert_eq!(count_valid_l3_pages(&l3_shared), 1024);

        // ── Unmap from Observer B ──
        let l2_b_empty = remove_space_from_l2(&mut l2_b, va);

        assert!(l2_b_empty);

        remove_l2_from_l1(&mut l1_b, va);

        // Both Observers are now clean
        assert_eq!(l1_l2_table_pa(&l1_a, va), None);
        assert_eq!(l1_l2_table_pa(&l1_b, va), None);
        assert!(l2_is_empty(&l2_a));
        assert!(l2_is_empty(&l2_b));
        // L3 still exists (Space object isn't destroyed by unmapping)
        assert_eq!(count_valid_l3_pages(&l3_shared), 1024);
    }

    /// Scenario (d): Duplicate cap detection (D91).
    ///
    /// Observer holds two caps to the same Space. On second install,
    /// l2_maps_space detects the duplicate. On close, only the last
    /// cap triggers unmap.
    #[test]
    fn d91_integration_duplicate_cap_detection() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let mut l3 = [0u64; ENTRIES_PER_TABLE];
        let l2_pa: u64 = 0x0010_0000;
        let l3_pa: u64 = 0x0020_0000;
        let va = 0usize;

        populate_l3(&mut l3, 0x8000_0000, 512);
        // ── First cap: install ──
        install_l2_in_l1(&mut l1, va, l2_pa);

        assert!(!l2_maps_space(&l2, va, l3_pa), "no mapping yet");

        install_space_in_l2(&mut l2, va, l3_pa);

        assert!(l2_maps_space(&l2, va, l3_pa));

        // ── Second cap (e.g., attenuated clone per D52): ──
        // The kernel checks l2_maps_space BEFORE installing. Since the
        // Space is already mapped at this VA, no page table work is needed.
        let already_mapped = l2_maps_space(&l2, va, l3_pa);

        assert!(
            already_mapped,
            "duplicate cap must be detected — no page table work needed"
        );
        // No install_space_in_l2 call — the mapping already exists.
        // The L2 table still has exactly one valid entry (not two).
        assert_eq!(count_valid_l2_entries(&l2), 1);
        // ── Close first cap: another cap still references this Space ──
        // Kernel scans cap table, finds the second cap — no unmap.
        assert!(
            l2_maps_space(&l2, va, l3_pa),
            "mapping must survive while any cap to this Space exists"
        );

        // ── Close second cap: last reference, trigger unmap ──
        let l2_empty = remove_space_from_l2(&mut l2, va);

        assert!(l2_empty);

        remove_l2_from_l1(&mut l1, va);

        // Fully clean
        assert_eq!(l1_l2_table_pa(&l1, va), None);
        assert!(l2_is_empty(&l2));
    }

    /// Scenario (e): Space split (D41) — L3 table update.
    ///
    /// Original Space has a full L3 table. Split moves the upper half
    /// to a new Space. The original's cleared entries become invalid.
    /// The new Space gets its own L3 table with the moved pages.
    #[test]
    fn d91_integration_space_split_l3_update() {
        // Original Space: full L3 table
        let mut l3_original = [0u64; ENTRIES_PER_TABLE];
        let pa_base: u64 = 0x8000_0000;

        populate_l3(&mut l3_original, pa_base, ENTRIES_PER_TABLE);

        assert_eq!(count_valid_l3_pages(&l3_original), ENTRIES_PER_TABLE);

        // ── Split at the midpoint ──
        // Pages [0..1024) stay in original, pages [1024..2048) move to new Space.
        let split_index = ENTRIES_PER_TABLE / 2;
        // New Space's L3 table: populated with the moved pages
        let mut l3_new = [0u64; ENTRIES_PER_TABLE];
        let new_pa_base = pa_base + (split_index as u64) * (PAGE_SIZE as u64);

        populate_l3(&mut l3_new, new_pa_base, ENTRIES_PER_TABLE - split_index);

        // Clear moved pages from original L3
        for i in split_index..ENTRIES_PER_TABLE {
            clear_page_entry(&mut l3_original, i);
        }

        // ── Verify original: only lower half remains ──
        assert_eq!(count_valid_l3_pages(&l3_original), split_index);

        for i in 0..split_index {
            assert!(
                is_valid_page(l3_original[i]),
                "original entry {i} must remain valid"
            );

            let expected_pa = pa_base + (i as u64) * (PAGE_SIZE as u64);

            assert_eq!(page_address(l3_original[i]), expected_pa);
        }
        for i in split_index..ENTRIES_PER_TABLE {
            assert!(
                !is_valid_page(l3_original[i]),
                "moved entry {i} must be invalid"
            );
        }

        // ── Verify new Space: upper half pages ──
        assert_eq!(
            count_valid_l3_pages(&l3_new),
            ENTRIES_PER_TABLE - split_index
        );

        for i in 0..(ENTRIES_PER_TABLE - split_index) {
            assert!(
                is_valid_page(l3_new[i]),
                "new Space entry {i} must be valid"
            );

            let expected_pa = new_pa_base + (i as u64) * (PAGE_SIZE as u64);

            assert_eq!(page_address(l3_new[i]), expected_pa);
        }

        // The two L3 tables together account for all original pages
        assert_eq!(
            count_valid_l3_pages(&l3_original) + count_valid_l3_pages(&l3_new),
            ENTRIES_PER_TABLE,
            "split must preserve total page count"
        );
    }

    /// Scenario (f): Cross-region mapping (two different L1 indices).
    ///
    /// Space A at VA 0 (L1 index 0), Space B at VA 64 GiB (L1 index 1).
    /// Each needs its own L2 table. Unmapping A leaves B intact.
    #[test]
    fn d91_integration_cross_region_mapping() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let mut l2_region0 = [0u64; ENTRIES_PER_TABLE];
        let mut l2_region1 = [0u64; ENTRIES_PER_TABLE];
        let mut l3_a = [0u64; ENTRIES_PER_TABLE];
        let mut l3_b = [0u64; ENTRIES_PER_TABLE];
        let l2_pa_region0: u64 = 0x0010_0000;
        let l2_pa_region1: u64 = 0x0014_0000;
        let l3_pa_a: u64 = 0x0020_0000;
        let l3_pa_b: u64 = 0x0030_0000;
        let va_a = 0usize; // L1 index 0
        let va_b = 64 * 1024 * 1024 * 1024usize; // L1 index 1

        assert_eq!(l1_index(va_a), 0);
        assert_eq!(l1_index(va_b), 1);

        // Populate both Spaces
        populate_l3(&mut l3_a, 0x4000_0000, 100);
        populate_l3(&mut l3_b, 0x5000_0000, 200);
        // ── Install Space A (region 0) ──
        install_l2_in_l1(&mut l1, va_a, l2_pa_region0);
        install_space_in_l2(&mut l2_region0, va_a, l3_pa_a);
        // ── Install Space B (region 1) ──
        install_l2_in_l1(&mut l1, va_b, l2_pa_region1);
        install_space_in_l2(&mut l2_region1, va_b, l3_pa_b);

        // Both L1 entries point to different L2 tables
        assert_eq!(l1_l2_table_pa(&l1, va_a), Some(l2_pa_region0));
        assert_eq!(l1_l2_table_pa(&l1, va_b), Some(l2_pa_region1));
        assert_ne!(
            table_address(l1[l1_index(va_a)]),
            table_address(l1[l1_index(va_b)]),
            "L1 entries must point to different L2 tables"
        );
        // Both Spaces visible
        assert!(l2_maps_space(&l2_region0, va_a, l3_pa_a));
        assert!(l2_maps_space(&l2_region1, va_b, l3_pa_b));

        // ── Unmap Space A (region 0 only) ──
        let l2_empty = remove_space_from_l2(&mut l2_region0, va_a);

        assert!(l2_empty);

        remove_l2_from_l1(&mut l1, va_a);

        // L1[0] cleared, L1[1] still valid
        assert_eq!(l1_l2_table_pa(&l1, va_a), None, "region 0 must be cleared");
        assert_eq!(
            l1_l2_table_pa(&l1, va_b),
            Some(l2_pa_region1),
            "region 1 must survive"
        );
        // Space B still mapped
        assert!(l2_maps_space(&l2_region1, va_b, l3_pa_b));

        // ── Clean up Space B ──
        let l2_empty = remove_space_from_l2(&mut l2_region1, va_b);

        assert!(l2_empty);

        remove_l2_from_l1(&mut l1, va_b);

        assert_eq!(l1_l2_table_pa(&l1, va_b), None);
    }

    /// Scenario: Space split at non-midpoint boundary with Observer verification.
    ///
    /// Combines D41 split with D91 Observer mapping — after split, the
    /// Observer's L2 must reference updated L3 tables.
    #[test]
    fn d91_integration_split_with_observer_remapping() {
        let mut l1 = [0u64; ENTRIES_PER_TABLE];
        let mut l2 = [0u64; ENTRIES_PER_TABLE];
        let mut l3_original = [0u64; ENTRIES_PER_TABLE];
        let l2_pa: u64 = 0x0010_0000;
        let l3_pa_original: u64 = 0x0020_0000;
        let l3_pa_split: u64 = 0x0030_0000;
        let va = 0usize;
        let pa_base: u64 = 0x8000_0000;
        let total_pages = 1500;

        // Populate original Space and map it
        populate_l3(&mut l3_original, pa_base, total_pages);
        install_l2_in_l1(&mut l1, va, l2_pa);
        install_space_in_l2(&mut l2, va, l3_pa_original);

        assert!(l2_maps_space(&l2, va, l3_pa_original));
        assert_eq!(count_valid_l3_pages(&l3_original), total_pages);

        // ── Split: pages [1000..1500) move to new Space ──
        let split_at = 1000;
        let moved_count = total_pages - split_at;
        let mut l3_split = [0u64; ENTRIES_PER_TABLE];
        let split_pa_base = pa_base + (split_at as u64) * (PAGE_SIZE as u64);

        populate_l3(&mut l3_split, split_pa_base, moved_count);

        // Clear moved pages from original
        for i in split_at..total_pages {
            clear_page_entry(&mut l3_original, i);
        }

        assert_eq!(count_valid_l3_pages(&l3_original), split_at);
        assert_eq!(count_valid_l3_pages(&l3_split), moved_count);
        // Original mapping still valid (same L3 PA, just fewer entries)
        assert!(l2_maps_space(&l2, va, l3_pa_original));

        // Install new Space at the next L2 slot
        let va_split = SPACE_VA_ALIGNMENT;

        install_space_in_l2(&mut l2, va_split, l3_pa_split);

        // Both Spaces now visible to Observer
        assert!(l2_maps_space(&l2, va, l3_pa_original));
        assert!(l2_maps_space(&l2, va_split, l3_pa_split));
        assert_eq!(count_valid_l2_entries(&l2), 2);
    }

    /// Scenario: Multiple Observers, each with multiple Spaces, selective teardown.
    ///
    /// Exercises the full protocol across two Observers with overlapping
    /// and non-overlapping Space sets.
    #[test]
    fn d91_integration_multi_observer_multi_space_teardown() {
        // Observer A: Spaces at VA 0, 32 MiB, 64 MiB
        let mut l1_a = [0u64; ENTRIES_PER_TABLE];
        let mut l2_a = [0u64; ENTRIES_PER_TABLE];
        // Observer B: Spaces at VA 0 (shared!), 32 MiB (different Space)
        let mut l1_b = [0u64; ENTRIES_PER_TABLE];
        let mut l2_b = [0u64; ENTRIES_PER_TABLE];
        // Shared Space (S1) and Observer-specific Spaces (S2, S3, S4)
        let mut l3_s1 = [0u64; ENTRIES_PER_TABLE]; // shared between A and B
        let mut l3_s2 = [0u64; ENTRIES_PER_TABLE]; // A only
        let mut l3_s3 = [0u64; ENTRIES_PER_TABLE]; // A only
        let mut l3_s4 = [0u64; ENTRIES_PER_TABLE]; // B only
        let l2_pa_a: u64 = 0x0010_0000;
        let l2_pa_b: u64 = 0x0014_0000;
        let l3_pa_s1: u64 = 0x0020_0000; // shared
        let l3_pa_s2: u64 = 0x0024_0000;
        let l3_pa_s3: u64 = 0x0028_0000;
        let l3_pa_s4: u64 = 0x002C_0000;
        let va_0 = 0usize;
        let va_32m = SPACE_VA_ALIGNMENT;
        let va_64m = 2 * SPACE_VA_ALIGNMENT;

        // Populate all Spaces
        populate_l3(&mut l3_s1, 0x4000_0000, 500);
        populate_l3(&mut l3_s2, 0x5000_0000, 300);
        populate_l3(&mut l3_s3, 0x6000_0000, 100);
        populate_l3(&mut l3_s4, 0x7000_0000, 400);
        // ── Install for Observer A ──
        install_l2_in_l1(&mut l1_a, va_0, l2_pa_a);
        install_space_in_l2(&mut l2_a, va_0, l3_pa_s1); // S1 shared
        install_space_in_l2(&mut l2_a, va_32m, l3_pa_s2); // S2
        install_space_in_l2(&mut l2_a, va_64m, l3_pa_s3); // S3
        // ── Install for Observer B ──
        install_l2_in_l1(&mut l1_b, va_0, l2_pa_b);
        install_space_in_l2(&mut l2_b, va_0, l3_pa_s1); // S1 shared (same L3!)
        install_space_in_l2(&mut l2_b, va_32m, l3_pa_s4); // S4 (different from A)

        assert_eq!(count_valid_l2_entries(&l2_a), 3);
        assert_eq!(count_valid_l2_entries(&l2_b), 2);
        // Both A and B point to same L3 for S1
        assert!(l2_maps_space(&l2_a, va_0, l3_pa_s1));
        assert!(l2_maps_space(&l2_b, va_0, l3_pa_s1));
        // But different L3 at va_32m
        assert!(l2_maps_space(&l2_a, va_32m, l3_pa_s2));
        assert!(l2_maps_space(&l2_b, va_32m, l3_pa_s4));

        // ── Tear down Observer A completely ──
        let _ = remove_space_from_l2(&mut l2_a, va_0);
        let _ = remove_space_from_l2(&mut l2_a, va_32m);
        let l2_a_empty = remove_space_from_l2(&mut l2_a, va_64m);

        assert!(l2_a_empty);

        remove_l2_from_l1(&mut l1_a, va_0);

        // Observer A is fully clean
        assert!(l2_is_empty(&l2_a));
        assert_eq!(l1_l2_table_pa(&l1_a, va_0), None);
        // Observer B is completely unaffected
        assert_eq!(count_valid_l2_entries(&l2_b), 2);
        assert!(l2_maps_space(&l2_b, va_0, l3_pa_s1));
        assert!(l2_maps_space(&l2_b, va_32m, l3_pa_s4));
        assert_eq!(l1_l2_table_pa(&l1_b, va_0), Some(l2_pa_b));
        // All L3 tables remain intact
        assert_eq!(count_valid_l3_pages(&l3_s1), 500);
        assert_eq!(count_valid_l3_pages(&l3_s2), 300);
        assert_eq!(count_valid_l3_pages(&l3_s3), 100);
        assert_eq!(count_valid_l3_pages(&l3_s4), 400);
    }

    #[test]
    fn d89_l1_index_at_user_va_end_boundary() {
        // Just below USER_VA_END: L1 index should be valid
        let va = USER_VA_END - 1;
        let idx = l1_index(va);

        assert!(
            idx < ENTRIES_PER_TABLE,
            "L1 index at boundary must be within table"
        );
    }

    #[test]
    fn d89_l2_index_at_user_va_end_boundary() {
        let va = USER_VA_END - SPACE_VA_ALIGNMENT;
        let idx = l2_index(va);

        assert!(
            idx < ENTRIES_PER_TABLE,
            "L2 index at boundary must be within table"
        );
    }

    #[test]
    fn d90_populate_l3_with_usize_max_page_count() {
        let mut l3 = [0u64; ENTRIES_PER_TABLE];

        // usize::MAX should clamp to ENTRIES_PER_TABLE without overflow
        populate_l3(&mut l3, 0x8000_0000, usize::MAX);

        assert_eq!(
            count_valid_l3_pages(&l3),
            ENTRIES_PER_TABLE,
            "usize::MAX page_count must clamp to ENTRIES_PER_TABLE"
        );
    }
}
