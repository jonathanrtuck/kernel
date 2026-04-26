//! Exception handling for AArch64.
//!
//! The assembly vector table (`exception.S`) currently saves full register
//! context into a [`TrapFrame`] on the stack for all exception sources. D74
//! settles the target design: EL0 exceptions save directly into the current
//! Observer's RegisterState (via TPIDR_EL1 per-core state pointer); EL1h
//! exceptions continue to use the stack TrapFrame. The current implementation
//! predates D74 and uses TrapFrame universally — the EL0 path will be updated
//! to save to RegisterState when context switching is implemented.

#[cfg(target_os = "none")]
core::arch::global_asm!(include_str!("exception.S"));

use super::sysreg;

// ── Assembly entry points for EL0 context switch (Phase C) ───────
//
// These functions are defined in exception.S and called from the Rust
// el0_exception_handler (noreturn). They perform the final context switch:
// either restoring an Observer's register state and eret-ing to EL0, or
// entering the idle loop waiting for interrupts.

#[cfg(target_os = "none")]
unsafe extern "C" {
    /// Restore an Observer's full register context and eret to EL0.
    ///
    /// # Parameters
    /// - `register_state_ptr`: pointer to the Observer's RegisterState
    /// - `page_table_root`: TTBR0_EL1 value for the Observer's address space
    /// - `clock_access`: 0 or 1 — whether to allow EL0 access to the
    ///   virtual counter (CNTKCTL_EL1 bit 1)
    ///
    /// # Safety
    /// - `register_state_ptr` must point to a valid, fully-populated
    ///   RegisterState. The assembly loads all fields unconditionally.
    /// - `page_table_root` must be a valid, complete TTBR0_EL1 value
    ///   (physical address with ASID). Invalid values cause translation
    ///   faults after eret.
    /// - The caller must be in EL1 with IRQs masked (DAIF.I set).
    ///   The Observer's SPSR will restore the EL0 interrupt state.
    pub fn __restore_observer(
        register_state_ptr: *mut super::register_state::RegisterState,
        page_table_root: u64,
        clock_access: u64,
    ) -> !;

    /// Enter the idle loop: unmask IRQs and execute WFI in a loop.
    ///
    /// Called when no Observer is runnable. IRQs arrive through the EL1h
    /// vector (source 5), are handled by the existing TrapFrame-based
    /// irq_handler, and return via eret back into the WFI loop.
    ///
    /// # Safety
    /// - Must be called from EL1 with a valid kernel stack.
    /// - The caller must not hold any locks (IRQs will be unmasked).
    pub fn __enter_idle() -> !;
}

// ---------------------------------------------------------------------------
// TrapFrame — must match the assembly layout in exception.S exactly.
// ---------------------------------------------------------------------------

/// Saved CPU state at the point of an exception.
///
/// Created by the assembly vector entry, passed to [`exception_handler`] as a
/// stack pointer. 816 bytes, 16-byte aligned. Includes full FP/SIMD state
/// so that interrupts cannot corrupt the interrupted code's float registers.
#[repr(C)]
pub struct TrapFrame {
    /// General-purpose registers x0–x30.
    pub gprs: [u64; 31],
    /// Exception Link Register — address to return to.
    pub elr: u64,
    /// Saved Processor State Register — PSTATE before the exception.
    pub spsr: u64,
    /// Exception Syndrome Register — exception class and details.
    pub esr: u64,
    /// Fault Address Register — address that caused a data/instruction abort.
    pub far: u64,
    /// Padding for 16-byte alignment of FP register block. The assembly stores
    /// the source ID here temporarily, but it is passed to Rust via the
    /// `source` parameter.
    _pad: u64,
    /// FP/SIMD registers q0–q31 (128-bit each).
    pub fp_regs: [u128; 32],
    /// Floating-point control register.
    pub fpcr: u64,
    /// Floating-point status register.
    pub fpsr: u64,
}

// Offsets must match exception.S — the assembly uses hard-coded immediates for
// STP/LDP/STR/LDR. If any field is reordered, these assertions catch it at
// compile time rather than producing silent context corruption at runtime.
const _: () = {
    assert!(core::mem::offset_of!(TrapFrame, gprs) == 0);
    assert!(core::mem::offset_of!(TrapFrame, elr) == 248);
    assert!(core::mem::offset_of!(TrapFrame, spsr) == 256);
    assert!(core::mem::offset_of!(TrapFrame, esr) == 264);
    assert!(core::mem::offset_of!(TrapFrame, far) == 272);
    assert!(core::mem::offset_of!(TrapFrame, fp_regs) == 288);
    assert!(core::mem::offset_of!(TrapFrame, fpcr) == 800);
    assert!(core::mem::offset_of!(TrapFrame, fpsr) == 808);
    assert!(core::mem::size_of::<TrapFrame>() == 816); // sub sp, sp, #816
};

// ---------------------------------------------------------------------------
// Initialization
// ---------------------------------------------------------------------------

/// Install the exception vector table by writing VBAR_EL1.
pub fn init() {
    unsafe extern "C" {
        static __vectors: u8;
    }

    // __vectors is the assembly vector table, 2KB-aligned by `.align 11`
    // in exception.S. The `unsafe extern` block above covers the access.
    let vbar = (&raw const __vectors) as u64;

    sysreg::set_vbar_el1(vbar);
    sysreg::isb();
}

// ---------------------------------------------------------------------------
// Exception handler entry point (called from assembly)
// ---------------------------------------------------------------------------

/// Main exception dispatch, called from the assembly common handler.
///
/// `source` identifies which of the 16 vector entries was taken (0–15).
/// The assembly performs full context save/restore around this call, so
/// returning normally resumes the interrupted code via `eret`.
///
/// SPECULATION MITIGATION PLUG POINT — BHB clearing (Spectre-BHB)
///
/// On hardware without CSV3 or CLRBHB, a BHB clearing sequence (~32
/// unconditional branches) must execute in the `exception.S` vector
/// preamble BEFORE this function is called. The Branch History Buffer
/// can otherwise steer kernel indirect branches using attacker-trained
/// EL0 history.
///
/// On this hardware (Apple Silicon, FEAT_CSV3): not needed. CSV3
/// guarantees speculative reads from another context cannot be disclosed.
/// See `speculation.rs` for the full porting guide.
#[unsafe(no_mangle)]
extern "C" fn exception_handler(frame: &mut TrapFrame, source: u64) {
    match source {
        // EL1h IRQ — timer and device interrupts.
        // Returns to let assembly eret resume the interrupted code.
        5 => irq_handler(frame),

        // Everything else is unhandled — print diagnostics and halt.
        _ => fatal_exception(frame, source),
    }
}

// ---------------------------------------------------------------------------
// EL0 exception handler entry point (called from assembly, D74)
// ---------------------------------------------------------------------------

/// EL0 exception dispatch, called from the EL0 assembly common handler (D86).
///
/// Unlike `exception_handler` (which returns and lets assembly eret), this
/// function is divergent — it calls `__restore_observer` to resume an
/// Observer or `__enter_idle` if no Observer is runnable.
///
/// The assembly saves the full register context to RegisterState before
/// calling, resets SP to the kernel stack top, and passes exception info
/// as parameters rather than through RegisterState.
///
/// D86: source 8 = Sync (SVC, faults), 9 = IRQ, 10 = FIQ, 11 = SError.
#[cfg(target_os = "none")]
#[unsafe(no_mangle)]
extern "C" fn el0_exception_handler(source: u64, esr: u64, far: u64) -> ! {
    use crate::time_manager::round_robin::RoundRobin;

    let result = match source {
        8 => handle_el0_sync::<RoundRobin>(source, esr, far),
        9 => handle_el0_irq::<RoundRobin>(),
        _ => fatal_exception_el0(source, esr, far),
    };

    restore_or_idle(result)
}

/// Decode and dispatch an EL0 synchronous exception (D86).
///
/// EC 0x15 = SVC: route to dispatch_ipc or dispatch_typed.
/// EC 0x20/0x24 = Instruction/Data abort: fault delivery.
/// All others: fault delivery.
#[cfg(target_os = "none")]
fn handle_el0_sync<S: crate::time_manager::Scheduler + 'static>(
    source: u64,
    esr: u64,
    far: u64,
) -> crate::core_manager::DispatchResult {
    use crate::core_manager::{self, DispatchResult};
    use crate::syscall::{IpcOperation, TypedOperation};

    let ec = esr_ec(esr);

    match ec {
        0x15 => {
            let imm = esr_svc_imm(esr);
            let core = core_manager::current_core_mut::<S>();
            let ks = crate::frame::kernel_state();
            let observer = core.current.unwrap();

            if imm == 0 {
                let regs = crate::frame::cores::read_typed_registers(observer);

                match TypedOperation::from_code(regs.op_code) {
                    Some(op) => core.dispatch_typed(op, ks),
                    None => {
                        crate::frame::cores::write_typed_result(observer, (-1i64) as u64);
                        DispatchResult::Resume(observer)
                    }
                }
            } else {
                match IpcOperation::from_svc(imm) {
                    Some(op) => core.dispatch_ipc(op, ks),
                    None => {
                        crate::frame::cores::write_ipc_error(
                            observer,
                            crate::syscall::SyscallError::InvalidCap,
                        );
                        DispatchResult::Resume(observer)
                    }
                }
            }
        }
        _ => {
            // Proper fault delivery (D80) requires page table infrastructure (Phase D).
            fatal_exception_el0(source, esr, far)
        }
    }
}

/// Handle an IRQ that arrived while in EL0 (D81, D86).
#[cfg(target_os = "none")]
fn handle_el0_irq<S: crate::time_manager::Scheduler + 'static>()
-> crate::core_manager::DispatchResult {
    use crate::core_manager::{self, DispatchResult};

    let intid = super::gic::acknowledge();
    let core = core_manager::current_core_mut::<S>();

    if intid == super::gic::INTID_SPURIOUS {
        return DispatchResult::Resume(core.current.unwrap());
    }

    let ks = crate::frame::kernel_state();
    let result = match intid {
        super::gic::INTID_VTIMER => {
            super::timer::tick();

            let current_ticks = sysreg::cntvct_el0();
            let counter_freq = sysreg::cntfrq_el0();

            core.handle_timer(current_ticks, ks, counter_freq)
        }
        _ => core.handle_irq(intid, ks),
    };

    super::gic::end_of_interrupt(intid);

    result
}

/// Convert a DispatchResult into an assembly restore call (D76, D85).
#[cfg(target_os = "none")]
fn restore_or_idle(result: crate::core_manager::DispatchResult) -> ! {
    use crate::core_manager::DispatchResult;

    match result {
        DispatchResult::Resume(observer_ptr) | DispatchResult::ResumeFastPath(observer_ptr) => {
            let (rs_ptr, pt_root, clock_access) =
                crate::frame::cores::observer_restore_info(observer_ptr);

            crate::frame::cores::update_register_state_ptr(rs_ptr);

            // SAFETY: rs_ptr points to a valid RegisterState (extracted from
            // a live Observer). pt_root is the Observer's page_table_root.
            // clock_access is 0 or 1. IRQs are masked (hardware masks on
            // exception entry, and we have not unmasked). The assembly loads
            // the full RegisterState and erets to EL0.
            unsafe { __restore_observer(rs_ptr, pt_root, clock_access) }
        }
        DispatchResult::Idle => {
            // SAFETY: kernel stack is valid, no locks held. The assembly
            // unmasks IRQs and enters a WFI loop. IRQs arrive through the
            // EL1h vector and are handled by the existing TrapFrame path.
            unsafe { __enter_idle() }
        }
    }
}

/// Fatal EL0 exception — dump state and halt.
#[cfg(target_os = "none")]
fn fatal_exception_el0(source: u64, esr: u64, far: u64) -> ! {
    let ec = esr_ec(esr);

    sysreg::disable_irqs();

    crate::println!();
    crate::println!(
        "EL0 EXCEPTION: {} — {} (EC 0x{ec:02x})",
        source_name(source),
        ec_name(ec),
    );
    crate::println!("  ESR:  0x{esr:016x}");
    crate::println!("  FAR:  0x{far:016x}");
    crate::println!();

    super::signal_panic();

    loop {
        crate::frame::arch::halt();
    }
}

// ---------------------------------------------------------------------------
// IRQ handler
// ---------------------------------------------------------------------------

fn irq_handler(_frame: &mut TrapFrame) {
    let intid = super::gic::acknowledge();

    if intid == super::gic::INTID_SPURIOUS {
        return;
    }

    match intid {
        super::gic::INTID_VTIMER => {
            super::timer::tick();
        }
        // BUG: println! here will deadlock if this IRQ preempted a println!
        // on the same core (serial lock is not interrupt-aware). Acceptable
        // for now — unhandled IRQs during serial output are unlikely. Fix
        // when the serial driver gains interrupt-safe locking.
        _ => {
            crate::println!("IRQ: unhandled INTID {intid}");
        }
    }

    super::gic::end_of_interrupt(intid);
}

// ---------------------------------------------------------------------------
// Fatal exception — dump state and halt
// ---------------------------------------------------------------------------

fn fatal_exception(frame: &TrapFrame, source: u64) -> ! {
    // Mask IRQs to prevent timer ticks from interleaving diagnostic output.
    sysreg::disable_irqs();

    let ec = esr_ec(frame.esr);

    crate::println!();
    crate::println!(
        "EXCEPTION: {} — {} (EC 0x{ec:02x})",
        source_name(source),
        ec_name(ec),
    );
    crate::println!("  ELR:  0x{:016x}", frame.elr);
    crate::println!("  ESR:  0x{:016x}", frame.esr);
    crate::println!("  FAR:  0x{:016x}", frame.far);
    crate::println!("  SPSR: 0x{:016x}", frame.spsr);
    crate::println!();

    // Print GPRs, two per line.
    for i in (0..31).step_by(2) {
        if i + 1 < 31 {
            crate::println!(
                "  x{i:<2} = 0x{:016x}  x{:<2} = 0x{:016x}",
                frame.gprs[i],
                i + 1,
                frame.gprs[i + 1],
            );
        } else {
            crate::println!("  x{i:<2} = 0x{:016x}", frame.gprs[i]);
        }
    }

    crate::println!();

    // Signal the hypervisor so it knows the kernel crashed (same as panic).
    super::signal_panic();

    loop {
        crate::frame::arch::halt();
    }
}

// ---------------------------------------------------------------------------
// ESR field extraction
// ---------------------------------------------------------------------------

/// Extract the Exception Class from ESR_EL1 (bits [31:26]).
#[inline(always)]
fn esr_ec(esr: u64) -> u64 {
    (esr >> 26) & 0x3F
}

/// Extract the SVC immediate from ESR_EL1 (bits [15:0], valid when EC = 0x15).
#[inline(always)]
fn esr_svc_imm(esr: u64) -> u16 {
    (esr & 0xFFFF) as u16
}

// ---------------------------------------------------------------------------
// ESR exception class decoding
// ---------------------------------------------------------------------------

fn ec_name(ec: u64) -> &'static str {
    match ec {
        0x00 => "Unknown",
        0x01 => "WFI/WFE trap",
        0x0E => "Illegal execution state",
        0x15 => "SVC (AArch64)",
        0x18 => "MSR/MRS trap",
        0x20 => "Instruction abort (lower EL)",
        0x21 => "Instruction abort (same EL)",
        0x22 => "PC alignment fault",
        0x24 => "Data abort (lower EL)",
        0x25 => "Data abort (same EL)",
        0x26 => "SP alignment fault",
        0x2C => "FP/SIMD exception",
        0x2F => "SError",
        0x30 => "Breakpoint (lower EL)",
        0x31 => "Breakpoint (same EL)",
        0x32 => "Software step (lower EL)",
        0x33 => "Software step (same EL)",
        0x34 => "Watchpoint (lower EL)",
        0x35 => "Watchpoint (same EL)",
        0x3C => "BRK (AArch64)",
        _ => "Reserved",
    }
}

fn source_name(source: u64) -> &'static str {
    match source {
        0 => "EL1t Sync",
        1 => "EL1t IRQ",
        2 => "EL1t FIQ",
        3 => "EL1t SError",
        4 => "EL1h Sync",
        5 => "EL1h IRQ",
        6 => "EL1h FIQ",
        7 => "EL1h SError",
        8 => "EL0/64 Sync",
        9 => "EL0/64 IRQ",
        10 => "EL0/64 FIQ",
        11 => "EL0/64 SError",
        12 => "EL0/32 Sync",
        13 => "EL0/32 IRQ",
        14 => "EL0/32 FIQ",
        15 => "EL0/32 SError",
        _ => "Unknown",
    }
}
