//! Page table descriptor construction tests.
//!
//! Verifies the bit-level math for AArch64 16 KiB granule page tables.
//! Constants here must stay in sync with `src/arch/aarch64/mmu.rs`.

// Descriptor bits (duplicated from kernel for host-side testing)
const VALID: u64 = 1 << 0;
const TABLE_OR_PAGE: u64 = 1 << 1;
const AF: u64 = 1 << 10;
const SH_ISH: u64 = 0b11 << 8;
const AP_RW_EL1: u64 = 0b00 << 6;
const AP_RO_EL1: u64 = 0b10 << 6;
const PXN: u64 = 1 << 53;
const UXN: u64 = 1 << 54;
const ATTR_DEVICE: u64 = 0 << 2;
const ATTR_NORMAL: u64 = 1 << 2;

const PAGE_SHIFT: usize = 14;
const PAGE_SIZE: usize = 1 << PAGE_SHIFT;
const L2_BLOCK_SHIFT: usize = 25;
const L2_BLOCK_SIZE: usize = 1 << L2_BLOCK_SHIFT;
const ENTRIES: usize = 2048;

// --- Descriptor builders (same logic as kernel) ---

fn l2_block(pa: usize, attrs: u64) -> u64 {
    (pa as u64 & !((L2_BLOCK_SIZE as u64) - 1)) | attrs | SH_ISH | AF | VALID
}

fn l3_page(pa: usize, attrs: u64) -> u64 {
    (pa as u64 & !((PAGE_SIZE as u64) - 1)) | attrs | SH_ISH | AF | TABLE_OR_PAGE | VALID
}

fn l2_table(table_pa: usize) -> u64 {
    (table_pa as u64 & !((PAGE_SIZE as u64) - 1)) | TABLE_OR_PAGE | VALID
}

fn l2_index(va: usize) -> usize {
    (va >> L2_BLOCK_SHIFT) & (ENTRIES - 1)
}

fn l3_index(va: usize) -> usize {
    (va >> PAGE_SHIFT) & (ENTRIES - 1)
}

// --- Index math ---

#[test]
fn l2_indices_for_device_mmio() {
    assert_eq!(l2_index(0x0800_0000), 4); // GIC
    assert_eq!(l2_index(0x0900_0000), 4); // UART (same 32 MiB block)
    assert_eq!(l2_index(0x080A_0000), 4); // GIC redist
    assert_eq!(l2_index(0x0A00_0000), 5); // Virtio
}

#[test]
fn l2_indices_for_ram() {
    assert_eq!(l2_index(0x4000_0000), 32); // RAM base
    assert_eq!(l2_index(0x4008_0000), 32); // Kernel (same block)
    assert_eq!(l2_index(0x4200_0000), 33);
    assert_eq!(l2_index(0x4FFF_FFFF), 39); // End of 256 MiB
}

#[test]
fn l3_indices_within_kernel_block() {
    assert_eq!(l3_index(0x4000_0000), 0);
    assert_eq!(l3_index(0x4000_4000), 1); // +16 KiB
    assert_eq!(l3_index(0x4008_0000), 32); // Kernel base
    assert_eq!(l3_index(0x41FF_C000), 2047); // Last page in block
}

// --- Descriptor bit patterns ---

#[test]
fn device_block_is_valid_non_executable() {
    let desc = l2_block(0x0800_0000, ATTR_DEVICE | AP_RW_EL1 | PXN | UXN);

    assert_ne!(desc & VALID, 0, "must be valid");
    assert_eq!(desc & TABLE_OR_PAGE, 0, "must be block (bit 1 = 0)");
    assert_eq!(desc & 0x0000_FFFF_FE00_0000, 0x0800_0000, "output address");
    assert_eq!((desc >> 2) & 0x7, 0, "MAIR index 0 (device)");
    assert_ne!(desc & AF, 0, "access flag");
    assert_ne!(desc & PXN, 0, "no execute (EL1)");
    assert_ne!(desc & UXN, 0, "no execute (EL0)");
}

#[test]
fn normal_rx_page_is_readonly_executable() {
    let desc = l3_page(0x4008_0000, ATTR_NORMAL | AP_RO_EL1 | UXN);

    assert_ne!(desc & VALID, 0);
    assert_ne!(desc & TABLE_OR_PAGE, 0, "L3 page bit");
    assert_eq!(desc & 0x0000_FFFF_FFFF_C000, 0x4008_0000, "output address");
    assert_eq!((desc >> 2) & 0x7, 1, "MAIR index 1 (normal)");
    assert_eq!((desc >> 6) & 0x3, 0b10, "AP = RO at EL1");
    assert_eq!(desc & PXN, 0, "executable at EL1");
    assert_ne!(desc & UXN, 0, "not executable at EL0");
}

#[test]
fn normal_rw_page_is_not_executable() {
    let desc = l3_page(0x4009_0000, ATTR_NORMAL | AP_RW_EL1 | PXN | UXN);

    assert_eq!((desc >> 6) & 0x3, 0b00, "AP = RW at EL1");
    assert_ne!(desc & PXN, 0);
    assert_ne!(desc & UXN, 0);
}

#[test]
fn table_descriptor_format() {
    let desc = l2_table(0x4010_0000);

    assert_eq!(desc & (VALID | TABLE_OR_PAGE), VALID | TABLE_OR_PAGE);
    assert_eq!(desc & 0x0000_FFFF_FFFF_C000, 0x4010_0000);
    // Table descriptors should NOT have attribute bits
    assert_eq!(desc & AF, 0);
}

#[test]
fn alignment_constants() {
    assert_eq!(1usize << PAGE_SHIFT, PAGE_SIZE);
    assert_eq!(PAGE_SIZE, 16384);
    assert_eq!(1usize << L2_BLOCK_SHIFT, L2_BLOCK_SIZE);
    assert_eq!(L2_BLOCK_SIZE, 32 * 1024 * 1024);
    assert_eq!(ENTRIES * 8, PAGE_SIZE, "one table = one page");
}

#[test]
fn block_address_alignment() {
    // Block descriptor must mask low bits
    let desc = l2_block(0x0800_1234, ATTR_DEVICE | AP_RW_EL1 | PXN | UXN);
    assert_eq!(
        desc & 0x0000_FFFF_FE00_0000,
        0x0800_0000,
        "low bits of PA must be masked"
    );
}
