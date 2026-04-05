//! Memory-mapped I/O helpers.
//!
//! Centralizes all volatile hardware access so `unsafe` lives in one place.
//! Every other module goes through these instead of raw pointer casts.
//!
//! Uses inline assembly instead of `core::ptr::{read,write}_volatile` to
//! guarantee simple LDR/STR instructions without writeback or pair modes.
//! This is required for HVF (Apple Hypervisor.framework) compatibility:
//! data aborts from complex addressing modes have ISV=0, which HVF cannot
//! decode.

#[inline(always)]
pub fn read32(addr: usize) -> u32 {
    debug_assert!(
        addr.is_multiple_of(4),
        "read32: addr must be 4-byte aligned"
    );

    let val: u32;

    // SAFETY: LDR w from an MMIO address. Inline asm guarantees a plain
    // `ldr` without writeback or pair modes (HVF-compatible). The address
    // is passed as an input register, so LLVM cannot fold it into a
    // pre/post-index addressing mode. Caller must ensure `addr` is valid.
    unsafe {
        core::arch::asm!(
            "ldr {val:w}, [{addr}]",
            addr = in(reg) addr,
            val = out(reg) val,
            options(nostack),
        );
    }

    val
}

#[inline(always)]
pub fn write32(addr: usize, val: u32) {
    debug_assert!(
        addr.is_multiple_of(4),
        "write32: addr must be 4-byte aligned"
    );

    // SAFETY: STR w to an MMIO address. Inline asm guarantees a plain
    // `str` without writeback or pair modes (HVF-compatible). The address
    // is passed as an input register, so LLVM cannot fold it into a
    // pre/post-index addressing mode. Caller must ensure `addr` is valid.
    unsafe {
        core::arch::asm!(
            "str {val:w}, [{addr}]",
            addr = in(reg) addr,
            val = in(reg) val,
            options(nostack),
        );
    }
}
