//! MMU setup — identity map with W^X permissions.
//!
//! Builds a two-level page table (L2 root → L3 for kernel block) using the
//! 16 KiB granule with 36-bit virtual addresses (T0SZ = 28).
//!
//! ## Table structure
//!
//! - **L2 root** (2048 entries, each covers 32 MiB):
//!   - Indices 4–5: device MMIO blocks (GIC, UART, virtio)
//!   - Index 32: table descriptor → L3 table for kernel's 32 MiB block
//!   - Indices 33–39: RAM blocks (normal, RW)
//!
//! - **L3 kernel** (2048 entries, each covers 16 KiB):
//!   - Pages before kernel: RW (DTB, pre-kernel RAM)
//!   - Kernel text: RO + executable (W^X: writable ⊕ executable)
//!   - Kernel rodata: RO, no execute
//!   - Kernel data/bss/stack: RW, no execute
//!
//! ## Memory attributes (MAIR_EL1)
//!
//! - Index 0: Device-nGnRnE (0x00)
//! - Index 1: Normal, Inner/Outer Write-Back Write-Allocate (0xFF)

use super::{platform, sysreg};
use core::cell::UnsafeCell;

#[cfg(target_os = "none")]
core::arch::global_asm!(include_str!("mmu.S"));

// ---------------------------------------------------------------------------
// Descriptor constants
// ---------------------------------------------------------------------------

const VALID: u64 = 1 << 0;
const TABLE: u64 = 1 << 1; // L2 table descriptor
const PAGE: u64 = 1 << 1; // L3 page descriptor (same bit position)
const AF: u64 = 1 << 10;
const SH_ISH: u64 = 0b11 << 8;
const AP_RW_EL1: u64 = 0b00 << 6;
const AP_RO_EL1: u64 = 0b10 << 6;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const ATTR_DEVICE: u64 = 0 << 2; // MAIR index 0
const ATTR_NORMAL: u64 = 1 << 2; // MAIR index 1

// MAIR encodings
const MAIR_DEVICE_NGNRNE: u64 = 0x00;
const MAIR_NORMAL_WB: u64 = 0xFF; // Inner/Outer Write-Back, Write-Allocate

// Page geometry — 16 KiB granule (Apple Silicon native). These stay private
// to the MMU module; the page size does not leak into the kernel interface.
const PAGE_SIZE: usize = 16 * 1024;
const PAGE_SHIFT: usize = 14;

// L2 geometry
const L2_BLOCK_SHIFT: usize = 25;
const L2_BLOCK_SIZE: usize = 1 << L2_BLOCK_SHIFT;
const ENTRIES_PER_TABLE: usize = PAGE_SIZE / 8;

// ── TTBR split contract (D88) ─────────────────────────────────────
//
// Kernel in TTBR1 (upper half), per-Observer user in TTBR0 (lower half).
// Asymmetric VA sizes: user T0SZ=17 (47-bit, 128 TiB, 3-level L1→L2→L3),
// kernel T1SZ=28 (36-bit, 64 GiB, 2-level L2→L3).

pub use crate::frame::KERNEL_VIRT_OFFSET;

/// Maximum user virtual address (exclusive).
///
/// TTBR0 range with T0SZ=17: 0 to 2^47 − 1 = 128 TiB.
/// 3-level walk (L1→L2→L3) enables D26's shared L3 subtrees with
/// 32 MiB VA alignment per Space (~4 million Space slots).
pub const USER_VA_END: usize = 1 << 47;

/// Mask for extracting the page table physical address from a TTBR value.
///
/// 16 KiB granule: table must be 16 KiB aligned (bits\[13:0\] = 0).
/// TTBR BADDR occupies bits\[47:14\]. Bits\[63:48\] hold the ASID,
/// bit\[0\] is CnP. This mask isolates the PA.
const TTBR_BADDR_MASK: u64 = 0x0000_FFFF_FFFF_C000;

// ---------------------------------------------------------------------------
// Static page tables
// ---------------------------------------------------------------------------

#[repr(C, align(16384))]
struct PageTablePage(UnsafeCell<[u64; ENTRIES_PER_TABLE]>);

// SAFETY: Page tables are only written during single-threaded init before the
// MMU is enabled. After init, they are read-only (the MMU walker reads them
// via the hardware page table walk, not through Rust references).
unsafe impl Sync for PageTablePage {}

static L1_ROOT: PageTablePage = PageTablePage(UnsafeCell::new([0; ENTRIES_PER_TABLE]));
static L2_ROOT: PageTablePage = PageTablePage(UnsafeCell::new([0; ENTRIES_PER_TABLE]));
static L3_KERNEL: PageTablePage = PageTablePage(UnsafeCell::new([0; ENTRIES_PER_TABLE]));

// ---------------------------------------------------------------------------
// Descriptor builders
// ---------------------------------------------------------------------------

fn l2_block(pa: usize, attrs: u64) -> u64 {
    (pa as u64 & !((L2_BLOCK_SIZE as u64) - 1)) | attrs | SH_ISH | AF | VALID
}

fn l2_table_desc(table_pa: usize) -> u64 {
    (table_pa as u64 & !((PAGE_SIZE as u64) - 1)) | TABLE | VALID
}

fn l3_page(pa: usize, attrs: u64) -> u64 {
    (pa as u64 & !((PAGE_SIZE as u64) - 1)) | attrs | SH_ISH | AF | PAGE | VALID
}

#[inline]
fn l2_index(va: usize) -> usize {
    (va >> L2_BLOCK_SHIFT) & (ENTRIES_PER_TABLE - 1)
}

#[allow(dead_code)]
#[inline]
fn l3_index(va: usize) -> usize {
    (va >> PAGE_SHIFT) & (ENTRIES_PER_TABLE - 1)
}

// ---------------------------------------------------------------------------
// Linker symbols (defined in link.ld)
// ---------------------------------------------------------------------------

// SAFETY: Linker-provided section boundary symbols from link.ld. We only
// take their addresses (via linker_addr) for page table permission mapping;
// never dereference them.
unsafe extern "C" {
    static __text_start: u8;
    static __text_end: u8;
    static __rodata_start: u8;
    static __rodata_end: u8;
    static __data_start: u8;
    static __kernel_end: u8;
}

fn linker_addr(sym: *const u8) -> usize {
    sym as usize
}

// ---------------------------------------------------------------------------
// Page table builders (safe — no global state, testable on host)
// ---------------------------------------------------------------------------

/// Physical address ranges of the kernel's ELF sections.
struct KernelLayout {
    text_start: usize,
    text_end: usize,
    rodata_start: usize,
    rodata_end: usize,
    data_start: usize,
    kernel_end: usize,
}

/// W^X permission policy: map a physical address to page attributes.
///
/// Every mapped page is either writable or executable, never both.
/// Pages beyond `kernel_end` are mapped RW for the root pool (D93).
#[allow(clippy::if_same_then_else)]
fn page_attrs(pa: usize, layout: &KernelLayout) -> Option<u64> {
    if pa >= layout.text_start && pa < layout.text_end {
        Some(ATTR_NORMAL | AP_RO_EL1 | UXN)
    } else if pa >= layout.rodata_start && pa < layout.rodata_end {
        Some(ATTR_NORMAL | AP_RO_EL1 | PXN | UXN)
    } else if pa >= layout.data_start && pa < layout.kernel_end {
        Some(ATTR_NORMAL | AP_RW_EL1 | PXN | UXN)
    } else if pa < layout.text_start {
        Some(ATTR_NORMAL | AP_RW_EL1 | PXN | UXN)
    } else {
        // Beyond kernel_end: root pool memory (D93). Map RW for slab
        // pages, user page tables, RegisterState, and other kernel
        // allocations. EL1-only, no execute.
        Some(ATTR_NORMAL | AP_RW_EL1 | PXN | UXN)
    }
}

/// Populate the L2 root table: device MMIO blocks, kernel L3 table descriptor,
/// and remaining RAM blocks.
#[allow(clippy::needless_range_loop)]
fn build_l2(table: &mut [u64; ENTRIES_PER_TABLE], l3_pa: usize, ram_base: usize, ram_size: usize) {
    let device_attrs = ATTR_DEVICE | AP_RW_EL1 | PXN | UXN;

    for idx in l2_index(platform::GIC_DIST_BASE)..=l2_index(0x0BFF_FFFF) {
        table[idx] = l2_block(idx * L2_BLOCK_SIZE, device_attrs);
    }

    table[l2_index(ram_base)] = l2_table_desc(l3_pa);

    let ram_rw = ATTR_NORMAL | AP_RW_EL1 | PXN | UXN;
    let ram_start_idx = l2_index(ram_base) + 1;
    let ram_end_idx = l2_index(ram_base + ram_size - 1).min(ENTRIES_PER_TABLE - 1);

    for idx in ram_start_idx..=ram_end_idx {
        table[idx] = l2_block(idx * L2_BLOCK_SIZE, ram_rw);
    }
}

/// Populate the L3 table for the kernel's 32 MiB block using [`page_attrs`].
#[allow(clippy::needless_range_loop)]
fn build_l3(table: &mut [u64; ENTRIES_PER_TABLE], block_base: usize, layout: &KernelLayout) {
    for i in 0..ENTRIES_PER_TABLE {
        let pa = block_base + i * PAGE_SIZE;

        if let Some(attrs) = page_attrs(pa, layout) {
            table[i] = l3_page(pa, attrs);
        }
    }
}

// ---------------------------------------------------------------------------
// TTBR split helpers (D88)
// ---------------------------------------------------------------------------

pub use crate::frame::phys_to_virt;
pub use crate::frame::virt_to_phys;

/// Construct a TTBR0 value from an ASID and L1 root physical address.
///
/// TTBR0\_EL1 format for 16 KiB granule (D88):
/// - Bits\[63:48\]: ASID (when `TCR_EL1.A1=0`)
/// - Bits\[47:14\]: L1 root table PA (with T0SZ=17, root is L1)
/// - Bits\[13:1\]: RES0 (16 KiB alignment)
/// - Bit\[0\]: CnP = 0 (per-Observer, not shared across cores)
#[inline(always)]
pub const fn make_ttbr0(asid: u16, l1_root_pa: u64) -> u64 {
    asid_field(asid) | (l1_root_pa & TTBR_BADDR_MASK)
}

/// Construct a TTBR1 value for the kernel page table.
///
/// D88: `CnP=1` because all cores share the same kernel tables. No ASID —
/// `TCR_EL1.A1=0` means the ASID comes from TTBR0, not TTBR1.
#[inline(always)]
pub const fn make_ttbr1(l2_root_pa: u64) -> u64 {
    (l2_root_pa & TTBR_BADDR_MASK) | 1
}

/// Place an ASID into bits\[63:48\] for TTBR or TLBI operand encoding.
#[inline(always)]
pub const fn asid_field(asid: u16) -> u64 {
    (asid as u64) << 48
}

/// Extract the base physical address from a TTBR value (strip ASID and CnP).
#[inline(always)]
pub const fn ttbr_base_address(ttbr: u64) -> u64 {
    ttbr & TTBR_BADDR_MASK
}

/// Build the TCR\_EL1 value for the TTBR0/TTBR1 split (D88).
///
/// Asymmetric VA sizes: user T0SZ=17 (47-bit, 3-level L1→L2→L3),
/// kernel T1SZ=28 (36-bit, 2-level L2→L3). `EPD1=0` enables TTBR1
/// walks; `E0PD1=1` provides Meltdown mitigation.
#[allow(clippy::identity_op)]
pub const fn build_tcr_split(pa_range: u64) -> u64 {
    (17 << 0) // T0SZ = 17: 47-bit VA (128 TiB) for TTBR0 — 3-level walk
        | (0b01 << 8) // IRGN0: Inner Write-Back Write-Allocate
        | (0b01 << 10) // ORGN0: Outer Write-Back Write-Allocate
        | (0b11 << 12) // SH0: Inner Shareable
        | (0b10 << 14) // TG0: 16 KiB granule
        | (28 << 16) // T1SZ = 28: 36-bit VA (64 GiB) for TTBR1 — 2-level walk
        // EPD1 (bit 23) = 0: enable TTBR1 walks
        | (0b01 << 24) // IRGN1: Inner Write-Back Write-Allocate
        | (0b01 << 26) // ORGN1: Outer Write-Back Write-Allocate
        | (0b11 << 28) // SH1: Inner Shareable
        | (0b01 << 30) // TG1: 16 KiB granule
        | (pa_range << 32) // IPS: from hardware
        | (1 << 56) // E0PD1: prevent EL0 speculative access to TTBR1
}

/// Detect ASID width from `ID_AA64MMFR0_EL1`.
///
/// ARM ARM: `ASIDBits` at bits\[7:4\]. `0b0000` = 8-bit, `0b0010` = 16-bit.
pub const fn asid_width_from_mmfr0(mmfr0: u64) -> u8 {
    if ((mmfr0 >> 4) & 0xF) >= 2 { 16 } else { 8 }
}

/// Read the hardware ASID width (D101).
///
/// Reads `ID_AA64MMFR0_EL1` and returns 8 or 16. Called at boot to
/// configure the `AsidAllocator` in `KernelState`.
#[cfg(target_os = "none")]
pub fn asid_width() -> u8 {
    asid_width_from_mmfr0(sysreg::id_aa64mmfr0_el1())
}

/// Invalidate all user-space TLB entries across all cores (D101 wrap flush).
///
/// Called when the ASID counter wraps to flush stale entries from the
/// previous generation. No preceding page-table stores to drain, so no
/// pre-barrier (`DSB ISHST`) is needed — unlike the per-VA/per-ASID
/// unmap paths which must drain page-table stores before the TLBI.
///
/// Sequence: `TLBI VMALLE1IS; DSB ISH; ISB` — the minimum ARM ARM
/// requirement for recycled-identifier flush. `DSB ISH` ensures the TLBI
/// completes on all cores before the new ASID enters TTBR0. `ISB`
/// synchronizes the instruction stream.
#[cfg(target_os = "none")]
pub fn tlb_flush_all_user() {
    sysreg::tlbi_vmalle1is();
    sysreg::dsb_ish();
    sysreg::isb();
}

// ---------------------------------------------------------------------------
// Boot-time page table modification (D94)
// ---------------------------------------------------------------------------

/// Install a user L3 table at a given L2 index in the kernel's identity-map
/// L2 root table.
///
/// Phase E strategy: user pages are added to the existing kernel identity map
/// at L2 index 0 (VA 0x0–0x01FF_FFFF, currently unmapped). The kernel's
/// RAM mappings at L2 indices 32+ are EL1-only. User pages at index 0 have
/// EL0 access (set in the L3 descriptors). __restore_observer skips the
/// TTBR switch since TTBR0 doesn't change.
///
/// `l3_pa` must be page-aligned (16 KiB).
///
/// ARM ARM D5.10.1: installing a new valid entry where an invalid entry
/// previously existed does not require TLB invalidation — the hardware
/// cannot have cached an invalid descriptor as valid. DSB + ISB ensures
/// the table walker sees the store before subsequent accesses.
#[cfg(target_os = "none")]
pub fn install_user_l3_in_kernel_l2(l2_index: usize, l3_pa: usize) {
    debug_assert!(l2_index < ENTRIES_PER_TABLE);

    // SAFETY: L2_ROOT is the active TTBR0 page table. Writing a table
    // descriptor at an unmapped index followed by DSB+ISB ensures the
    // hardware walker sees the new entry. No TLBI needed — the previous
    // entry was invalid. Single-core BSP boot context.
    unsafe {
        let l2 = &mut *L2_ROOT.0.get();

        l2[l2_index] = l2_table_desc(l3_pa);
    }

    sysreg::dsb_ish();
    sysreg::isb();
}

/// Read the current TTBR0_EL1 value.
///
/// D88: after the split, TTBR0 points to L1_ROOT (identity map with
/// 3-level walk). The root Observer uses this L1 table. Phase 1.2
/// allocates per-Observer L1 tables.
#[cfg(target_os = "none")]
pub fn current_ttbr0() -> u64 {
    sysreg::ttbr0_el1()
}

// ---------------------------------------------------------------------------
// Exported constants (D25, D93)
// ---------------------------------------------------------------------------

/// Page size in bytes (D25).
///
/// 16 KiB granule — the native granule for Apple Silicon and the project's
/// design target. Callers outside frame/ receive this as a parameter via
/// SpaceManager; this function is for boot-time construction of the
/// SpaceManager itself.
pub const fn page_size() -> usize {
    PAGE_SIZE
}

/// Physical address of the first byte after the kernel image.
///
/// Page-aligned (link.ld: `__kernel_end = ALIGN(16384)`). Used by the boot
/// sequence (D93) to compute the root pool: usable memory runs from
/// `kernel_end_address()` to `ram_base() + ram_size()`.
#[cfg(target_os = "none")]
pub fn kernel_end_address() -> usize {
    linker_addr(&raw const __kernel_end)
}

/// Physical address of the kernel's L2 root table (D88, D89).
///
/// Per-Observer L1 tables chain to this via L1[0] → L2_ROOT to provide
/// identity-mapped kernel code access. The L2 table covers VA 0..64 GiB.
pub fn kernel_l2_root_pa() -> usize {
    L2_ROOT.0.get() as usize
}

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Build identity-mapped page tables and enable the MMU.
///
/// After this returns, VA == PA for all mapped regions. The kernel's text is
/// RX, rodata is RO, and data/bss/stack is RW. Device memory is mapped as
/// Device-nGnRnE. SCTLR.WXN enforces W^X in hardware.
pub fn init() {
    // SAFETY: Single-threaded init, tables are written before MMU enable.
    let l2 = unsafe { &mut *L2_ROOT.0.get() };
    let l3 = unsafe { &mut *L3_KERNEL.0.get() };
    let layout = KernelLayout {
        text_start: linker_addr(&raw const __text_start),
        text_end: linker_addr(&raw const __text_end),
        rodata_start: linker_addr(&raw const __rodata_start),
        rodata_end: linker_addr(&raw const __rodata_end),
        data_start: linker_addr(&raw const __data_start),
        kernel_end: linker_addr(&raw const __kernel_end),
    };
    let l3_pa = L3_KERNEL.0.get() as usize;

    build_l2(l2, l3_pa, platform::ram_base(), platform::ram_size());
    build_l3(l3, platform::ram_base(), &layout);

    // D88: build L1 root for TTBR0 (3-level walk with T0SZ=17).
    // L1[0] → L2_ROOT covers VA 0..64 GiB — the kernel identity map.
    // SAFETY: Single-threaded init, L1_ROOT is written before MMU enable.
    let l1 = unsafe { &mut *L1_ROOT.0.get() };
    let l2_pa = L2_ROOT.0.get() as usize;

    l1[0] = l2_table_desc(l2_pa);

    configure_and_enable();
}

/// Enable the MMU on a secondary core using the BSP's page tables.
///
/// The BSP must have called [`init`] first — this function does NOT build
/// page tables. It configures this core's system registers to share the
/// existing L2/L3 tables and enables the MMU.
pub fn init_secondary() {
    configure_and_enable();
}

/// Program MAIR/TCR/TTBR0/TTBR1, invalidate TLBs, and enable the MMU.
///
/// Shared by both BSP ([`init`]) and secondary cores ([`init_secondary`]).
/// Page tables must already exist before this is called.
///
/// D88: TTBR0/TTBR1 split. TTBR0 (L1_ROOT, 3-level walk, T0SZ=17)
/// provides identity-mapped kernel code execution. TTBR1 (L2_ROOT,
/// 2-level walk, T1SZ=28) provides the kernel linear map at high VAs.
/// E0PD1 prevents EL0 speculative access to TTBR1 (Meltdown mitigation).
fn configure_and_enable() {
    // -----------------------------------------------------------------------
    // MAIR_EL1: memory attribute definitions
    // -----------------------------------------------------------------------
    let mair = MAIR_DEVICE_NGNRNE | (MAIR_NORMAL_WB << 8);

    sysreg::set_mair_el1(mair);

    // -----------------------------------------------------------------------
    // TCR_EL1: translation control (D88 split)
    // -----------------------------------------------------------------------
    let pa_range = sysreg::id_aa64mmfr0_el1() & 0xF;
    let tcr = build_tcr_split(pa_range);

    sysreg::set_tcr_el1(tcr);

    // -----------------------------------------------------------------------
    // TTBR0_EL1: L1 root for identity map (3-level walk, T0SZ=17)
    // -----------------------------------------------------------------------
    let l1_pa = L1_ROOT.0.get() as u64;

    sysreg::set_ttbr0_el1(l1_pa);

    // -----------------------------------------------------------------------
    // TTBR1_EL1: L2 root for kernel linear map (2-level walk, T1SZ=28)
    // -----------------------------------------------------------------------
    let l2_pa = L2_ROOT.0.get() as u64;

    sysreg::set_ttbr1_el1(make_ttbr1(l2_pa));

    // -----------------------------------------------------------------------
    // Invalidate TLBs and enable MMU
    // -----------------------------------------------------------------------

    // ARM ARM D5.10: ISB required after MAIR/TCR/TTBR writes to ensure
    // system register changes are visible before the TLBI sequence.
    sysreg::isb();

    // ARM ARM D5.10: DSB ISHST drains page-table stores before TLBI so
    // hardware walkers on other cores observe updated descriptors.
    sysreg::dsb_ishst();

    // IS broadcast: PSCI leaves TLB state IMPLEMENTATION DEFINED.
    sysreg::tlbi_vmalle1is();
    sysreg::dsb_ish();
    sysreg::isb();

    let mut sctlr = sysreg::sctlr_el1();

    sctlr |= 1 << 0; // M: MMU enable
    sctlr |= 1 << 2; // C: data cache enable
    sctlr |= 1 << 12; // I: instruction cache enable
    sctlr |= 1 << 19; // WXN: write-implies-XN (hardware W^X)

    // SAFETY: __mmu_enable is defined in mmu.S. It writes SCTLR_EL1 and
    // returns via the identity-mapped trampoline. C calling convention.
    unsafe extern "C" {
        fn __mmu_enable(sctlr: u64);
    }
    // SAFETY: Page tables are populated. MAIR/TCR/TTBR0/TTBR1 configured
    // above. TLBs invalidated. L1_ROOT[0] → L2_ROOT preserves the identity
    // map (VA == PA for low addresses), so the trampoline returns safely.
    unsafe { __mmu_enable(sctlr) };
}

// ── D91: TLB invalidation sequences ──────────────────────────────

/// Invalidate TLB entries for a Space's pages after unmap (D91).
///
/// Uses per-VA invalidation (`TLBI VAE1IS`) for each page, followed by
/// `DSB ISH; ISB` to ensure completion. The IS suffix broadcasts across
/// all cores in the inner-shareable domain (SMP correctness).
///
/// ARM ARM D5.10.2: TLBI by VA requires the VA shifted right by 12 and
/// the ASID in bits[63:48]. For 16 KiB granule, pages are 16 KiB aligned
/// so `VA >> 12` gives bits[47:12] >> 12 = bits[35:0].
///
/// D91/D101: per-VA last-level TLB invalidation across all cores.
///
/// Uses `TLBI VALE1IS` (last-level only) rather than `VAE1IS` because
/// Spaces always map at L3 granularity — avoids unnecessary invalidation
/// of intermediate walk-cache entries.
///
/// Prefer per-VA over per-ASID when page count is small. For bulk
/// removal (large Spaces), callers should use `tlb_flush_all_user()`
/// or `tlb_invalidate_asid()` instead.
#[cfg(target_os = "none")]
pub fn tlb_invalidate_space_pages(asid: u16, va_base: usize, page_count: usize) {
    let asid_bits = asid_field(asid);

    sysreg::dsb_ishst();

    for i in 0..page_count {
        let va = va_base + i * PAGE_SIZE;
        let va_shifted = (va >> 12) as u64;

        sysreg::tlbi_vale1is(asid_bits | va_shifted);
    }

    sysreg::dsb_ish();
    sysreg::isb();
}

/// Invalidate all TLB entries for a given ASID (D91 bulk unmap, D89 destroy).
///
/// Used when destroying an Observer's page table — invalidates all user
/// mappings for that ASID in one operation.
#[cfg(target_os = "none")]
pub fn tlb_invalidate_asid(asid: u16) {
    sysreg::dsb_ishst();
    sysreg::tlbi_aside1is(asid_field(asid));
    sysreg::dsb_ish();
    sysreg::isb();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_layout() -> KernelLayout {
        KernelLayout {
            text_start: 0x4008_0000,
            text_end: 0x400C_0000,
            rodata_start: 0x400C_0000,
            rodata_end: 0x400E_0000,
            data_start: 0x400E_0000,
            kernel_end: 0x4012_0000,
        }
    }

    // -- W^X policy: the central safety property --

    #[test]
    fn wxn_no_page_is_writable_and_executable() {
        let layout = test_layout();
        let block_base = 0x4000_0000;

        for i in 0..ENTRIES_PER_TABLE {
            let pa = block_base + i * PAGE_SIZE;

            if let Some(attrs) = page_attrs(pa, &layout) {
                let writable = (attrs & AP_RO_EL1) == 0;
                let el1_executable = (attrs & PXN) == 0;

                assert!(!(writable && el1_executable), "W^X violation at PA {pa:#x}",);
            }
        }
    }

    // -- Region classification --

    #[test]
    fn text_is_readonly_executable() {
        let layout = test_layout();
        let attrs = page_attrs(layout.text_start, &layout).unwrap();

        assert_ne!(attrs & AP_RO_EL1, 0);
        assert_eq!(attrs & PXN, 0);
        assert_ne!(attrs & UXN, 0);
    }

    #[test]
    fn rodata_is_readonly_noexec() {
        let layout = test_layout();
        let attrs = page_attrs(layout.rodata_start, &layout).unwrap();

        assert_ne!(attrs & AP_RO_EL1, 0);
        assert_ne!(attrs & PXN, 0);
    }

    #[test]
    fn data_is_readwrite_noexec() {
        let layout = test_layout();
        let attrs = page_attrs(layout.data_start, &layout).unwrap();

        assert_eq!(attrs & AP_RO_EL1, 0);
        assert_ne!(attrs & PXN, 0);
    }

    #[test]
    fn pre_kernel_is_readwrite_noexec() {
        let layout = test_layout();
        let attrs = page_attrs(0x4000_0000, &layout).unwrap();

        assert_eq!(attrs & AP_RO_EL1, 0);
        assert_ne!(attrs & PXN, 0);
    }

    #[test]
    fn beyond_kernel_end_is_rw_root_pool() {
        let layout = test_layout();
        let attrs = page_attrs(layout.kernel_end, &layout).unwrap();

        assert_eq!(attrs & AP_RO_EL1, 0, "root pool must be writable");
        assert_ne!(attrs & PXN, 0, "root pool must not be EL1-executable");
        assert_ne!(attrs & UXN, 0, "root pool must not be EL0-executable");
    }

    // -- Boundary precision --

    #[test]
    fn text_end_is_exclusive() {
        let layout = test_layout();
        let last_text = page_attrs(layout.text_end - PAGE_SIZE, &layout).unwrap();
        let first_rodata = page_attrs(layout.text_end, &layout).unwrap();

        assert_eq!(last_text & PXN, 0, "last text page is executable");
        assert_ne!(first_rodata & PXN, 0, "first rodata page is not executable");
    }

    // -- L3 builder --

    #[test]
    fn build_l3_maps_kernel_and_root_pool() {
        let mut table = [0u64; ENTRIES_PER_TABLE];
        let layout = test_layout();

        build_l3(&mut table, 0x4000_0000, &layout);

        let text_idx = (layout.text_start - 0x4000_0000) / PAGE_SIZE;

        assert_ne!(table[text_idx], 0, "kernel text must be mapped");

        let pool_idx = (layout.kernel_end - 0x4000_0000) / PAGE_SIZE;

        assert_ne!(table[pool_idx], 0, "root pool pages must be mapped (D93)");
    }

    // -- L2 builder --

    #[test]
    fn build_l2_device_region_is_mapped() {
        let mut table = [0u64; ENTRIES_PER_TABLE];

        build_l2(&mut table, 0x4100_0000, 0x4000_0000, 256 * 1024 * 1024);

        let gic_idx = l2_index(platform::GIC_DIST_BASE);

        assert_ne!(table[gic_idx], 0);
    }

    #[test]
    fn build_l2_kernel_block_is_table_descriptor() {
        let mut table = [0u64; ENTRIES_PER_TABLE];

        build_l2(&mut table, 0x4100_0000, 0x4000_0000, 256 * 1024 * 1024);

        let kernel_idx = l2_index(0x4000_0000);
        let entry = table[kernel_idx];

        assert_ne!(entry & TABLE, 0);
        assert_ne!(entry & VALID, 0);
    }

    // -- D88: TTBR split contract --

    #[test]
    fn d88_phys_to_virt_roundtrip() {
        let pa = 0x4008_0000usize;
        let va = phys_to_virt(pa);

        assert_eq!(virt_to_phys(va), pa);
    }

    #[test]
    fn d88_phys_to_virt_known_addresses() {
        assert_eq!(phys_to_virt(0x0800_0000), 0xFFFF_FFF0_0800_0000);
        assert_eq!(phys_to_virt(0x4000_0000), 0xFFFF_FFF0_4000_0000);
        assert_eq!(phys_to_virt(0x4008_0000), 0xFFFF_FFF0_4008_0000);
    }

    #[test]
    fn d88_kernel_and_user_ranges_disjoint() {
        let user_max = USER_VA_END - 1;
        let kernel_min = KERNEL_VIRT_OFFSET;

        assert!(
            user_max < kernel_min,
            "user range [{:#x}] must not overlap kernel range [{:#x}]",
            user_max,
            kernel_min
        );
    }

    #[test]
    fn d88_make_ttbr0_asid_in_upper_bits() {
        let ttbr = make_ttbr0(42, 0x4_0000);

        assert_eq!(ttbr >> 48, 42);
    }

    #[test]
    fn d88_make_ttbr0_pa_in_baddr_field() {
        let pa: u64 = 0x4_0000;
        let ttbr = make_ttbr0(0, pa);

        assert_eq!(ttbr & TTBR_BADDR_MASK, pa);
    }

    #[test]
    fn d88_make_ttbr0_cnp_is_zero() {
        let ttbr = make_ttbr0(1, 0x4_0000);

        assert_eq!(ttbr & 1, 0, "TTBR0 CnP must be 0 (per-Observer)");
    }

    #[test]
    fn d88_make_ttbr0_masks_unaligned_bits() {
        let ttbr = make_ttbr0(0, 0x4_1234);

        assert_eq!(
            ttbr & TTBR_BADDR_MASK,
            0x4_0000,
            "unaligned PA bits must be masked"
        );
    }

    #[test]
    fn d88_make_ttbr1_cnp_is_one() {
        let ttbr = make_ttbr1(0x4_0000);

        assert_eq!(ttbr & 1, 1, "TTBR1 CnP must be 1 (shared kernel tables)");
    }

    #[test]
    fn d88_make_ttbr1_pa_preserved() {
        let pa: u64 = 0x4_0000;
        let ttbr = make_ttbr1(pa);

        assert_eq!(ttbr & TTBR_BADDR_MASK, pa);
    }

    #[test]
    fn d88_tcr_split_epd1_clear() {
        let tcr = build_tcr_split(0);

        assert_eq!(tcr & (1 << 23), 0, "EPD1 must be 0 to enable TTBR1 walks");
    }

    #[test]
    fn d88_tcr_split_e0pd1_set() {
        let tcr = build_tcr_split(0);

        assert_ne!(
            tcr & (1 << 56),
            0,
            "E0PD1 must be 1 for Meltdown mitigation"
        );
    }

    #[test]
    fn d88_tcr_split_granule_16k() {
        let tcr = build_tcr_split(0);

        assert_eq!((tcr >> 14) & 0b11, 0b10, "TG0 must encode 16 KiB");
        assert_eq!((tcr >> 30) & 0b11, 0b01, "TG1 must encode 16 KiB");
    }

    #[test]
    fn d88_tcr_split_asymmetric_va_sizes() {
        let tcr = build_tcr_split(0);

        assert_eq!(tcr & 0x3F, 17, "T0SZ must be 17 (47-bit user VA, 3-level)");
        assert_eq!(
            (tcr >> 16) & 0x3F,
            28,
            "T1SZ must be 28 (36-bit kernel VA, 2-level)"
        );
    }

    #[test]
    fn d88_tcr_split_pa_range_propagated() {
        let tcr = build_tcr_split(0b0101);

        assert_eq!((tcr >> 32) & 0x7, 0b101, "IPS must carry pa_range");
    }

    #[test]
    fn d88_asid_width_8bit() {
        let mmfr0 = 0u64;

        assert_eq!(asid_width_from_mmfr0(mmfr0), 8);
    }

    #[test]
    fn d88_asid_width_16bit() {
        let mmfr0 = 0x2u64 << 4;

        assert_eq!(asid_width_from_mmfr0(mmfr0), 16);
    }

    #[test]
    fn d88_user_va_end_is_128_tib() {
        assert_eq!(USER_VA_END, 128 * 1024 * 1024 * 1024 * 1024);
    }
}
