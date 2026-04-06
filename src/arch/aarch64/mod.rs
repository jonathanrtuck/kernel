//! AArch64 architecture implementation.

core::arch::global_asm!(include_str!("boot.S"));

mod dtb;
pub mod entropy;
pub mod exception;
pub mod gic;
pub use gic as interrupts;
mod mmio;
pub mod mmu;
pub mod platform;
pub mod serial;
mod sysreg;
pub mod timer;

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

/// Mask all maskable interrupts.
///
/// Prevents async hardware events (timer ticks, device IRQs) from
/// interrupting the current execution.
pub fn disable_interrupts() {
    sysreg::disable_irqs();
}

/// Print a register dump to the console for crash diagnostics.
///
/// Reads exception-related registers and the link register, printing
/// them for post-mortem debugging. ELR/SPSR/ESR reflect the most recent
/// exception (likely the last timer IRQ), not the panic site — the Rust
/// panic message has the precise source location.
pub fn dump_panic_registers() {
    let lr: u64;
    // SAFETY: Reading x30 (link register) for crash diagnostics.
    unsafe { core::arch::asm!("mov {lr}, x30", lr = out(reg) lr, options(nomem, nostack)) };
    let elr = sysreg::elr_el1();
    let spsr = sysreg::spsr_el1();
    let esr = sysreg::esr_el1();
    crate::println!("  LR:   0x{lr:016x}");
    crate::println!("  ELR:  0x{elr:016x}");
    crate::println!("  SPSR: 0x{spsr:016x}");
    crate::println!("  ESR:  0x{esr:016x}");
}

/// Signal a fatal crash to the hypervisor via the pvpanic device.
///
/// Writes 0x01 to the pvpanic MMIO register, which tells QEMU/HVF that
/// the guest has panicked.
pub fn signal_panic() {
    mmio::write32(platform::PVPANIC_BASE, 1);
}
