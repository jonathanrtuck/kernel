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

core::arch::global_asm!(include_str!("mmu.S"));

// ---------------------------------------------------------------------------
// Descriptor constants (sync with host/tests/mmu.rs)
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

// ---------------------------------------------------------------------------
// Static page tables
// ---------------------------------------------------------------------------

#[repr(C, align(16384))]
struct PageTablePage(UnsafeCell<[u64; ENTRIES_PER_TABLE]>);

// SAFETY: Page tables are only written during single-threaded init before the
// MMU is enabled. After init, they are read-only (the MMU walker reads them
// via the hardware page table walk, not through Rust references).
unsafe impl Sync for PageTablePage {}

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

#[inline]
fn l3_index(va: usize) -> usize {
    (va >> PAGE_SHIFT) & (ENTRIES_PER_TABLE - 1)
}

// ---------------------------------------------------------------------------
// Linker symbols (defined in link.ld)
// ---------------------------------------------------------------------------

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

    // -----------------------------------------------------------------------
    // MAIR_EL1: memory attribute definitions
    // -----------------------------------------------------------------------
    let mair = MAIR_DEVICE_NGNRNE | (MAIR_NORMAL_WB << 8);
    sysreg::set_mair_el1(mair);

    // -----------------------------------------------------------------------
    // L2 root table
    // -----------------------------------------------------------------------

    // Device MMIO: 0x08000000–0x0BFFFFFF (L2 indices 4–5).
    // Device-nGnRnE, RW, no execute.
    let device_attrs = ATTR_DEVICE | AP_RW_EL1 | PXN | UXN;
    for idx in l2_index(platform::GIC_DIST_BASE)..=l2_index(0x0BFF_FFFF) {
        l2[idx] = l2_block(idx * L2_BLOCK_SIZE, device_attrs);
    }

    // RAM block containing the kernel (0x40000000–0x41FFFFFF, L2 index 32):
    // table descriptor pointing to L3 for fine-grained W^X.
    let l3_pa = L3_KERNEL.0.get() as usize;
    l2[l2_index(platform::ram_base())] = l2_table_desc(l3_pa);

    // Remaining RAM: 0x42000000–0x4FFFFFFF (L2 indices 33–39).
    // Normal cacheable, RW, no execute.
    let ram_rw = ATTR_NORMAL | AP_RW_EL1 | PXN | UXN;
    let ram_start_idx = l2_index(platform::ram_base()) + 1;
    let ram_end_idx = l2_index(platform::ram_base() + platform::ram_size() - 1);
    for idx in ram_start_idx..=ram_end_idx {
        l2[idx] = l2_block(idx * L2_BLOCK_SIZE, ram_rw);
    }

    // -----------------------------------------------------------------------
    // L3 table for kernel block (0x40000000–0x41FFFFFF)
    // -----------------------------------------------------------------------

    let text_start = linker_addr(&raw const __text_start);
    let text_end = linker_addr(&raw const __text_end);
    let rodata_start = linker_addr(&raw const __rodata_start);
    let rodata_end = linker_addr(&raw const __rodata_end);
    let data_start = linker_addr(&raw const __data_start);
    let kernel_end = linker_addr(&raw const __kernel_end);

    let block_base = platform::ram_base();
    for i in 0..ENTRIES_PER_TABLE {
        let pa = block_base + i * PAGE_SIZE;

        let attrs = if pa >= text_start && pa < text_end {
            // Kernel text: read-only, executable at EL1.
            ATTR_NORMAL | AP_RO_EL1 | UXN
        } else if pa >= rodata_start && pa < rodata_end {
            // Kernel rodata: read-only, no execute.
            ATTR_NORMAL | AP_RO_EL1 | PXN | UXN
        } else if pa >= data_start && pa < kernel_end {
            // Kernel data/bss/stack: read-write, no execute.
            ATTR_NORMAL | AP_RW_EL1 | PXN | UXN
        } else if pa < text_start {
            // Pre-kernel (DTB, gap): read-write, no execute.
            ATTR_NORMAL | AP_RW_EL1 | PXN | UXN
        } else {
            // Beyond kernel_end but within this 32 MiB block: leave unmapped.
            continue;
        };

        l3[i] = l3_page(pa, attrs);
    }

    // -----------------------------------------------------------------------
    // TCR_EL1: translation control
    // -----------------------------------------------------------------------
    // Read the hardware's physical address size from ID_AA64MMFR0_EL1[3:0].
    // The PARange field encodes the supported PA width (32, 36, 40, 42, 44,
    // 48, or 52 bits). We use this directly as TCR_EL1.IPS — the encodings
    // are identical by design.
    let pa_range = sysreg::id_aa64mmfr0_el1() & 0xF;

    #[rustfmt::skip]
    let tcr: u64 =
          (28      <<  0)  // T0SZ = 28: 36-bit VA (64 GiB)
        | (0b01    <<  8)  // IRGN0: Inner Write-Back Write-Allocate
        | (0b01    << 10)  // ORGN0: Outer Write-Back Write-Allocate
        | (0b11    << 12)  // SH0: Inner Shareable
        | (0b10    << 14)  // TG0: 16 KiB granule
        | (28      << 16)  // T1SZ = 28
        | (1       << 23)  // EPD1: disable TTBR1 walks
        | (0b01    << 24)  // IRGN1: Inner Write-Back Write-Allocate
        | (0b01    << 26)  // ORGN1: Outer Write-Back Write-Allocate
        | (0b11    << 28)  // SH1: Inner Shareable
        | (0b01    << 30)  // TG1: 16 KiB granule
        | (pa_range << 32); // IPS: from hardware (ID_AA64MMFR0_EL1.PARange)
    sysreg::set_tcr_el1(tcr);

    // -----------------------------------------------------------------------
    // TTBR0_EL1: point to L2 root table
    // -----------------------------------------------------------------------
    let l2_pa = L2_ROOT.0.get() as u64;
    sysreg::set_ttbr0_el1(l2_pa);

    // -----------------------------------------------------------------------
    // Invalidate TLBs and enable MMU
    // -----------------------------------------------------------------------

    // Ensure system register writes (MAIR, TCR, TTBR) take effect.
    sysreg::isb();

    // Ensure page table stores are visible to hardware walkers before TLBI.
    // DSB ISHST (store-only) is the ARM ARM D5.10 recommended pre-TLBI barrier.
    sysreg::dsb_ishst();

    // Invalidate stale TLB entries (defensive — none should exist before
    // first enable, but firmware or EL2 may have populated speculative entries).
    sysreg::tlbi_vmalle1is();
    sysreg::dsb_ish();
    sysreg::isb();

    // Enable MMU, caches, and W^X enforcement via assembly trampoline.
    // The trampoline is the single transition point from physical to virtual
    // addressing — see mmu.S for why this must be in assembly.
    let mut sctlr = sysreg::sctlr_el1();
    sctlr |= 1 << 0; // M: MMU enable
    sctlr |= 1 << 2; // C: data cache enable
    sctlr |= 1 << 12; // I: instruction cache enable
    sctlr |= 1 << 19; // WXN: write-implies-XN (hardware W^X)

    unsafe extern "C" {
        fn __mmu_enable(sctlr: u64);
    }
    // SAFETY: Page tables are fully populated, MAIR/TCR/TTBR0 are configured,
    // and TLBs are invalidated. The identity map ensures VA == PA, so the
    // trampoline can return to us after enabling.
    unsafe { __mmu_enable(sctlr) };
}
