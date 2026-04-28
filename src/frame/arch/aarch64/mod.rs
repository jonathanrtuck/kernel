//! AArch64 architecture implementation.

#[cfg(target_os = "none")]
core::arch::global_asm!(include_str!("boot.S"));
#[cfg(target_os = "none")]
core::arch::global_asm!(include_str!("secondary_entry.S"));

pub mod cpu;
pub mod entropy;
pub mod exception;
pub mod gic;
pub use gic as interrupts;
mod mmio;
pub mod mmu;
pub mod page_table;
pub mod platform;
pub mod psci;
pub mod register_state;
pub mod serial;
pub mod speculation;
mod sysreg;
pub mod timer;

/// Mask all maskable interrupts.
pub fn disable_interrupts() {
    sysreg::disable_irqs();
}

/// Mask all maskable interrupts and return the previous state.
pub fn disable_interrupts_save() -> u64 {
    let daif = sysreg::daif();

    sysreg::disable_irqs();

    daif
}

/// Restore interrupts to a previously saved state.
pub fn restore_interrupts(daif: u64) {
    sysreg::set_daif(daif);
}

/// Print a register dump to the console for crash diagnostics.
///
/// Reads exception-related registers and the link register, printing
/// them for post-mortem debugging. ELR/SPSR/ESR reflect the most recent
/// exception (likely the last timer IRQ), not the panic site — the Rust
/// panic message has the precise source location.
pub fn dump_panic_registers() {
    let lr: u64;

    // SAFETY: Copies the link register (x30) into a general-purpose register.
    // Pure register-to-register move — no memory or system side effects. No
    // `nomem` because the project policy restricts it to an explicit approved
    // list (immutable `mrs`, hint instructions).
    unsafe { core::arch::asm!("mov {lr}, x30", lr = out(reg) lr, options(nostack)) };

    let elr = sysreg::elr_el1();
    let spsr = sysreg::spsr_el1();
    let esr = sysreg::esr_el1();

    crate::println!("  LR:   0x{lr:016x}");
    crate::println!("  ELR:  0x{elr:016x}");
    crate::println!("  SPSR: 0x{spsr:016x}");
    crate::println!("  ESR:  0x{esr:016x}");
}

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

/// Read the counter frequency register (CNTFRQ_EL0).
///
/// D72: used for ns→ticks conversion (Pulsar creation, D95).
/// Immutable per-core — value set by firmware at boot.
pub fn cntfrq_el0() -> u64 {
    sysreg::cntfrq_el0()
}

/// Read the virtual counter register (CNTVCT_EL0).
///
/// Returns the current timer count in hardware ticks. Used by Pulsar
/// creation (D95) to compute absolute deadlines from relative durations.
pub fn cntvct_el0() -> u64 {
    sysreg::cntvct_el0()
}

/// Enable the PMU cycle counter for EL0 access.
///
/// Configures the Performance Monitors:
/// - PMCR_EL0.E = 1 (enable counting)
/// - PMCNTENSET_EL0 bit 31 (enable cycle counter)
/// - PMUSERENR_EL0.EN = 1 (allow EL0 access)
///
/// On the hypervisor, each of these traps to EL2 where the PMU is
/// emulated. PMCCNTR_EL0 reads return clock_gettime_nsec * 3 (~3 GHz
/// virtual cycles). On bare metal, this enables the real CPU cycle counter.
pub fn enable_pmu_el0() {
    // SAFETY: MSR to PMU control registers. All three writes configure
    // the PMU to enable the cycle counter and allow EL0 read access.
    // On the hypervisor these trap to EL2 for emulation.
    sysreg::set_pmcr_el0(1);
    sysreg::set_pmcntenset_el0(1 << 31);
    sysreg::set_pmuserenr_el0(1);
}

/// Read the per-core data pointer (TPIDR_EL1).
///
/// D83: each core stores a pointer to its `PerCoreData` in this register
/// at boot. Used by `frame::cores::read_per_core_data` and
/// `frame::cores::read_core_state` to find per-core kernel state.
/// TPIDR_EL1 → PerCoreData → CoreState<S>.
pub fn tpidr_el1() -> u64 {
    sysreg::tpidr_el1()
}

/// Write the per-core data pointer (TPIDR_EL1).
///
/// D83: called once per core during boot to store the PerCoreData pointer.
/// TPIDR_EL1 is per-core writable state — no nomem.
pub fn set_tpidr_el1(value: u64) {
    sysreg::set_tpidr_el1(value);
}

/// Signal a fatal crash to the hypervisor via the pvpanic device.
///
/// Writes 0x01 to the pvpanic MMIO register, which tells QEMU/HVF that
/// the guest has panicked.
pub fn signal_panic() {
    mmio::write32(platform::PVPANIC_BASE, 1);
}
