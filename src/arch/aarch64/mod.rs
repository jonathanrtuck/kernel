//! AArch64 architecture implementation.

core::arch::global_asm!(include_str!("boot.S"));

pub mod mmio;
pub mod serial;

/// Halt the CPU until an event or interrupt arrives.
#[inline(always)]
pub fn halt() {
    // SAFETY: `wfe` is a hint instruction with no side effects beyond pausing
    // the core until the next event (interrupt, SEV from another core, etc.).
    // It does not modify memory or registers.
    unsafe {
        core::arch::asm!("wfe", options(nomem, nostack));
    }
}
