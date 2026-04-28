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
// SAFETY: These functions are defined in exception.S with C calling
// convention. Caller obligations are documented in the # Safety
// sections below. Misuse causes EL1 faults (bad register state) or
// hangs (idle without interrupt capability).
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
    // SAFETY: __vectors is defined in exception.S, 2KB-aligned by `.align 11`.
    // We only take its address for VBAR_EL1; never dereference it.
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
    use crate::time_manager::SchedulerAlgorithm;

    let result = match source {
        8 => handle_el0_sync::<SchedulerAlgorithm>(source, esr, far),
        9 => handle_el0_irq::<SchedulerAlgorithm>(),
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
    _source: u64, // D26: needed once VmFault translation is wired
    esr: u64,
    far: u64,
) -> crate::core_manager::DispatchResult {
    use crate::core_manager::{self, DispatchResult};
    use crate::syscall::{IpcOperation, TypedOperation};

    let ec = esr_ec(esr);

    match ec {
        0x07 => {
            // FP/SIMD access trapped by CPACR_EL1.FPEN=0b01 (lazy FP).
            // Save previous owner's FP state, load current Observer's,
            // set FPEN=0b11 to allow subsequent FP access without trapping.
            handle_fp_trap();

            let core = core_manager::current_core_mut::<S>();

            DispatchResult::Resume(core.current.unwrap())
        }
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
        0x3C => {
            // BRK from EL0 — software breakpoint (debug exception).
            // D102: BRK #0x42 is the Phase E test-pass signal.
            // ARM ARM: ESR_EL1.ISS[15:0] = imm16 (direct, not shifted).
            let imm = esr & 0xFFFF;

            if imm == 0x42 {
                test_passed()
            } else if imm == 0x43 {
                child_scenario_passed::<S>()
            } else if imm == 0x44 {
                verify_ipc_roundtrip::<S>()
            } else if imm == 0x45 {
                verify_fault_handling::<S>()
            } else if imm == 0x46 {
                verify_timer_fire::<S>()
            } else if imm == 0x47 {
                verify_observer_destroy()
            } else if imm == 0x48 {
                bench_emit::<S>()
            } else if imm == 0x49 {
                space_info::<S>()
            } else if imm == 0x4A {
                install_reply_field::<S>()
            } else if imm == 0x4B {
                trace_control::<S>()
            } else {
                handle_el0_fault::<S>(esr, far)
            }
        }
        // D100: all other EL0 exceptions → fault delivery.
        // EC 0x20 = Instruction abort, EC 0x24 = Data abort, plus
        // alignment faults, FP exceptions, etc.
        _ => handle_el0_fault::<S>(esr, far),
    }
}

/// Handle an FP/SIMD access trap from EL0 (lazy FP restore).
///
/// CPACR_EL1.FPEN is set to 0b01 on context switch to a different
/// Observer. When that Observer first touches an FP register, this trap
/// fires. We load the Observer's FP state from RegisterState (which was
/// saved on the previous EL0 entry — entry always saves FP eagerly
/// because the kernel uses NEON for memset/memcpy), update fp_owner,
/// and set FPEN=0b11. Subsequent FP accesses proceed without trapping
/// until the next context switch to a different Observer.
#[cfg(target_os = "none")]
fn handle_fp_trap() {
    // SAFETY: TPIDR_EL1 was set during boot to a valid PerCoreData.
    // register_state_ptr points to the current Observer's RegisterState
    // (updated by update_register_state_ptr before __restore_observer).
    // A4 non-reentrancy guarantees exclusive access.
    let current_rs_ptr = unsafe {
        let pcd = sysreg::tpidr_el1() as *mut crate::frame::cores::PerCoreData;

        (*pcd).register_state_ptr
    };

    // SAFETY: current_rs_ptr points to the current Observer's valid
    // RegisterState. Load the saved FP state into hardware.
    unsafe { load_fp_state(current_rs_ptr) }

    // SAFETY: Update fp_owner so __restore_observer can detect same-
    // Observer restore and skip the FPEN=0b01 trap next time.
    unsafe {
        let pcd = sysreg::tpidr_el1() as *mut crate::frame::cores::PerCoreData;

        (*pcd).fp_owner = current_rs_ptr;
    }

    // SAFETY: CPACR_EL1 is a per-core system register. Setting FPEN=0b11
    // enables FP/SIMD for EL0. ISB ensures the pipeline sees the change
    // before eret re-executes the trapping instruction.
    unsafe {
        let mut cpacr: u64;

        core::arch::asm!("mrs {0}, cpacr_el1", out(reg) cpacr);

        cpacr |= 0b11 << 20;

        core::arch::asm!("msr cpacr_el1, {0}", "isb", in(reg) cpacr);
    }
}

/// Load FP/SIMD state from a RegisterState into hardware.
///
/// # Safety
/// `rs` must point to a valid, readable RegisterState.
#[cfg(target_os = "none")]
unsafe fn load_fp_state(rs: *mut crate::frame::arch::register_state::RegisterState) {
    // SAFETY: Caller guarantees rs is valid. The ldp instructions load
    // 128-bit q-register pairs from the RegisterState's fp_regs offsets.
    unsafe {
        core::arch::asm!(
            "ldr {tmp}, [{rs}, #800]",
            "msr fpcr, {tmp}",
            "ldr {tmp}, [{rs}, #808]",
            "msr fpsr, {tmp}",
            "ldp q0, q1, [{rs}, #288]",
            "ldp q2, q3, [{rs}, #320]",
            "ldp q4, q5, [{rs}, #352]",
            "ldp q6, q7, [{rs}, #384]",
            "ldp q8, q9, [{rs}, #416]",
            "ldp q10, q11, [{rs}, #448]",
            "ldp q12, q13, [{rs}, #480]",
            "ldp q14, q15, [{rs}, #512]",
            "ldp q16, q17, [{rs}, #544]",
            "ldp q18, q19, [{rs}, #576]",
            "ldp q20, q21, [{rs}, #608]",
            "ldp q22, q23, [{rs}, #640]",
            "ldp q24, q25, [{rs}, #672]",
            "ldp q26, q27, [{rs}, #704]",
            "ldp q28, q29, [{rs}, #736]",
            "ldp q30, q31, [{rs}, #768]",
            rs = in(reg) rs,
            tmp = out(reg) _,
            options(nostack),
        );
    }
}

/// D61: data/instruction aborts (EC 0x20, 0x24) are translated to VmFault.
/// All other EL0 faults are delivered as HardwareException.
#[cfg(target_os = "none")]
fn handle_el0_fault<S: crate::time_manager::Scheduler + 'static>(
    esr: u64,
    far: u64,
) -> crate::core_manager::DispatchResult {
    use crate::core_manager;
    use crate::fault::{AccessType, FaultType};

    let ec = esr_ec(esr);
    let core = core_manager::current_core_mut::<S>();
    let ks = crate::frame::kernel_state();
    let observer = core.current.unwrap();

    if (ec == 0x20 || ec == 0x24)
        && let Some((space_slot, byte_offset)) =
            crate::frame::cores::translate_vm_fault(observer, far, ks)
    {
        let access = if ec == 0x20 {
            AccessType::Execute
        } else if (esr >> 6) & 1 == 1 {
            AccessType::Write
        } else {
            AccessType::Read
        };

        return core.dispatch_fault(
            FaultType::VmFault {
                space_slot,
                byte_offset,
                access,
            },
            ks,
        );
    }

    let elr = crate::frame::cores::observer_read_pc(observer);
    let fault = FaultType::HardwareException {
        esr_el1: esr,
        elr_el1: elr,
        far_el1: far,
    };

    core.dispatch_fault(fault, ks)
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
        // D56: SGI received — cross-core IPI. Drain the local core's
        // mailbox and process each request via handle_ipi.
        intid if super::gic::is_sgi(intid) => core.handle_ipi(ks),
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
            crate::frame::cores::refresh_observer_asid(observer_ptr);

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
        DispatchResult::FatalFault => {
            // D100: root Observer faulted with no handler. Diagnostics
            // already printed by dispatch_fault. Terminate the system.
            super::psci::system_off()
        }
    }
}

/// Phase E test-pass handler (D102).
///
/// Called when the test binary signals success via BRK #0x42. Prints
/// the result and exits the VM via PSCI SYSTEM_OFF.
#[cfg(target_os = "none")]
fn test_passed() -> ! {
    crate::println!();
    crate::println!("TEST PASSED");
    crate::println!();

    super::psci::system_off()
}

#[cfg(target_os = "none")]
fn child_scenario_passed<S: crate::time_manager::Scheduler + 'static>()
-> crate::core_manager::DispatchResult {
    use crate::core_manager;
    use crate::time_manager::Scheduler;

    crate::println!("scenario: child IPC send + own address space — PASS");

    let core = core_manager::current_core_mut::<S>();

    if let Some(child) = core.current {
        Scheduler::dequeue(&mut core.scheduler, child);
    }

    core.schedule_next()
}

#[cfg(target_os = "none")]
fn verify_ipc_roundtrip<S: crate::time_manager::Scheduler + 'static>()
-> crate::core_manager::DispatchResult {
    let core = crate::core_manager::current_core::<S>();
    let observer = core.current.expect("must have current observer");
    let regs = crate::frame::cores::read_ipc_registers(observer);
    let data_ok = regs.data == [0xAA, 0xBB, 0xCC, 0xDD];
    let label_ok = regs.label == 0x42;
    let badge_ok = regs.handle_or_badge == 0x99;

    if data_ok && label_ok && badge_ok {
        crate::println!("scenario: IPC roundtrip (4 data + label + badge) — PASS");
    } else {
        crate::println!("scenario: IPC roundtrip — FAIL");
        crate::println!(
            "  data:  [{:#x}, {:#x}, {:#x}, {:#x}]",
            regs.data[0],
            regs.data[1],
            regs.data[2],
            regs.data[3]
        );
        crate::println!("  label: {:#x} (expected 0x42)", regs.label);
        crate::println!("  badge: {:#x} (expected 0x99)", regs.handle_or_badge);
        super::psci::system_off()
    }

    crate::frame::cores::observer_advance_pc(observer);
    crate::core_manager::DispatchResult::Resume(observer)
}

#[cfg(target_os = "none")]
fn verify_fault_handling<S: crate::time_manager::Scheduler + 'static>()
-> crate::core_manager::DispatchResult {
    use crate::field::LABEL_VM_FAULT;

    let core = crate::core_manager::current_core::<S>();
    let observer = core.current.expect("must have current observer");
    let regs = crate::frame::cores::read_ipc_registers(observer);
    let label_ok = regs.label == LABEL_VM_FAULT;
    let space_slot_ok = regs.data[0] == 4;
    let offset_ok = regs.data[1] == 0;
    let access_ok = regs.data[2] == 0;

    if label_ok && space_slot_ok && offset_ok && access_ok {
        crate::println!("scenario: VmFault delivery (space slot + offset + access) — PASS");
    } else {
        crate::println!("scenario: VmFault delivery — FAIL");
        crate::println!(
            "  label: {:#x} (expected {:#x})",
            regs.label,
            LABEL_VM_FAULT
        );
        crate::println!("  space_slot: {} (expected 4)", regs.data[0]);
        super::psci::system_off()
    }

    crate::frame::cores::observer_advance_pc(observer);
    crate::core_manager::DispatchResult::Resume(observer)
}

#[cfg(target_os = "none")]
fn verify_timer_fire<S: crate::time_manager::Scheduler + 'static>()
-> crate::core_manager::DispatchResult {
    use crate::field::LABEL_TIMER_FIRE;

    let core = crate::core_manager::current_core::<S>();
    let observer = core.current.expect("must have current observer");
    let regs = crate::frame::cores::read_ipc_registers(observer);
    let label_ok = regs.label == LABEL_TIMER_FIRE;
    let badge_ok = regs.handle_or_badge == 0xBEEF;
    let fire_time_ok = regs.data[0] > 0;

    if label_ok && badge_ok && fire_time_ok {
        crate::println!(
            "scenario: timer fire (badge + fire_time={}) — PASS",
            regs.data[0]
        );
    } else {
        crate::println!("scenario: timer fire — FAIL");
        crate::println!(
            "  label: {:#x} (expected {:#x})",
            regs.label,
            LABEL_TIMER_FIRE
        );
        crate::println!("  badge: {:#x} (expected 0xBEEF)", regs.handle_or_badge);
        super::psci::system_off()
    }

    crate::frame::cores::observer_advance_pc(observer);
    crate::core_manager::DispatchResult::Resume(observer)
}

#[cfg(target_os = "none")]
fn verify_observer_destroy() -> ! {
    let core = crate::core_manager::current_core::<crate::time_manager::SchedulerAlgorithm>();
    let observer = core.current.expect("must have current observer");
    let regs = crate::frame::cores::read_typed_registers(observer);

    if regs.args[0] == 0 {
        crate::println!("scenario: Observer destroy + cascade (x0=0, no backing) — PASS");
        crate::println!();
        crate::println!("TEST PASSED");
        crate::println!();
    } else {
        crate::println!(
            "scenario: Observer destroy — FAIL (x0={})",
            regs.args[0] as i64
        );
        crate::println!();
        crate::println!("TEST FAILED");
        crate::println!();
    }

    super::psci::system_off()
}

/// Benchmark data point emission handler.
///
/// Called when a benchmark binary executes BRK #0x48. Reads x0–x3 from
/// the Observer's saved registers, prints a structured BENCH line to
/// serial, advances PC past the BRK, and resumes the Observer.
///
/// The kernel does not interpret the register semantics — it just prints
/// them as zero-padded hex for the host benchmark runner to parse.
#[cfg(target_os = "none")]
fn bench_emit<S: crate::time_manager::Scheduler + 'static>() -> crate::core_manager::DispatchResult
{
    let core = crate::core_manager::current_core::<S>();
    let observer = core.current.expect("must have current observer");
    let regs = crate::frame::cores::read_typed_registers(observer);

    crate::println!(
        "    BENCH {:016x} {:016x} {:016x} {:016x}",
        regs.args[0],
        regs.args[1],
        regs.args[2],
        regs.args[3],
    );

    crate::frame::cores::observer_advance_pc(observer);
    crate::core_manager::DispatchResult::Resume(observer)
}

/// BRK #0x49: read Space metadata (test infrastructure).
///
/// x0 = Space cap handle. Returns va_base in x0, size in x1,
/// then resumes execution. On error (bad handle or wrong type),
/// writes u64::MAX to x0.
#[cfg(target_os = "none")]
fn space_info<S: crate::time_manager::Scheduler + 'static>() -> crate::core_manager::DispatchResult
{
    let core = crate::core_manager::current_core::<S>();
    let observer = core.current.expect("must have current observer");
    let regs = crate::frame::cores::read_typed_registers(observer);
    let handle = crate::capability::Handle::decode(regs.args[0]);
    let ks = crate::frame::kernel_state();
    let (va_base, size) = match crate::frame::cores::observer_read_cap_entry(observer, handle.index)
    {
        Some((crate::capability::ObjectType::Space, space_id, _badge)) => {
            let spaces = ks.spaces.acquire();

            match spaces.get(space_id) {
                Some(space) => (space.va_base as u64, space.size as u64),
                None => (u64::MAX, 0),
            }
        }
        _ => (u64::MAX, 0),
    };

    // SAFETY: observer points to a live Observer. A4 non-reentrancy.
    // We write va_base to x0 and size to x1 in the saved RegisterState.
    unsafe {
        let obs = observer.as_ref();
        let rs = &mut *(obs.register_state.as_ptr().as_ptr()
            as *mut crate::frame::arch::register_state::RegisterState);

        rs.gprs[0] = va_base;
        rs.gprs[1] = size;
    }

    crate::frame::cores::observer_advance_pc(observer);
    crate::core_manager::DispatchResult::Resume(observer)
}

/// BRK #0x4A: install reply Field at SLOT_REPLY_FIELD (test infrastructure).
///
/// x0 = Field cap handle. Copies the Field cap entry to slot 1 in the
/// caller's cap table, enabling Call (SVC #3) for this Observer.
/// Returns 0 in x0 on success, u64::MAX on error.
#[cfg(target_os = "none")]
fn install_reply_field<S: crate::time_manager::Scheduler + 'static>()
-> crate::core_manager::DispatchResult {
    let core = crate::core_manager::current_core::<S>();
    let observer = core.current.expect("must have current observer");
    let regs = crate::frame::cores::read_typed_registers(observer);
    let handle = crate::capability::Handle::decode(regs.args[0]);
    let result = match crate::frame::cores::observer_read_full_cap_entry(observer, handle.index) {
        Some(entry) if entry.object.is_some_and(|(_ty, _id)| true) => {
            // SAFETY: observer points to a live Observer. A4 non-reentrancy.
            // Write the Field cap entry to SLOT_REPLY_FIELD (slot 1).
            unsafe {
                let obs = observer.as_ref();
                let wrote = crate::frame::capabilities::write_entry(
                    obs.cap_table,
                    obs.cap_table_capacity,
                    crate::capability::SLOT_REPLY_FIELD,
                    crate::capability::Entry {
                        object: entry.object,
                        rights: crate::capability::Rights::RECEIVE,
                        badge: entry.badge,
                        slot_tag: entry.slot_tag,
                        send_once: false,
                        stored_generation: entry.stored_generation,
                    },
                );

                if wrote { 0u64 } else { u64::MAX }
            }
        }
        _ => u64::MAX,
    };

    // SAFETY: same as space_info handler.
    unsafe {
        let obs = observer.as_ref();
        let rs = &mut *(obs.register_state.as_ptr().as_ptr()
            as *mut crate::frame::arch::register_state::RegisterState);

        rs.gprs[0] = result;
    }

    crate::frame::cores::observer_advance_pc(observer);
    crate::core_manager::DispatchResult::Resume(observer)
}

/// BRK #0x4B: dispatch trace control (benchmark infrastructure).
///
/// x0 = 0: stop tracing, emit buffer as BENCH lines, return count in x0.
/// x0 = 1: start tracing, clear buffer.
///
/// Trace points in the dispatch path record CNTVCT timestamps at key
/// stages. Tags 0xF000..0xF00F encode the stage ID; values are raw
/// counter ticks. scripts/bench parses these alongside regular stats.
#[cfg(target_os = "none")]
fn trace_control<S: crate::time_manager::Scheduler + 'static>()
-> crate::core_manager::DispatchResult {
    let core = crate::core_manager::current_core_mut::<S>();
    let observer = core.current.expect("must have current observer");
    let regs = crate::frame::cores::read_typed_registers(observer);

    if regs.args[0] == 1 {
        core.trace_active = true;
        core.trace_count = 0;
    } else {
        core.trace_active = false;

        let count = core.trace_count;

        for i in 0..count as usize {
            let entry = core.trace_buffer[i];

            crate::println!(
                "    BENCH {:016x} {:016x} {:016x} {:016x}",
                0xF000u64 + entry.stage as u64,
                entry.timestamp,
                0u64,
                0u64,
            );
        }

        core.trace_count = 0;

        // SAFETY: observer points to a live Observer. A4 non-reentrancy.
        unsafe {
            let obs = observer.as_ref();
            let rs = &mut *(obs.register_state.as_ptr().as_ptr()
                as *mut crate::frame::arch::register_state::RegisterState);

            rs.gprs[0] = count as u64;
        }
    }

    crate::frame::cores::observer_advance_pc(observer);
    crate::core_manager::DispatchResult::Resume(observer)
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
            super::gic::end_of_interrupt(intid);

            idle_wakeup_check();

            return;
        }
        // D56: SGI received — cross-core IPI. Drain the local core's
        // mailbox and process each request. The EL1h handler cannot do
        // full dispatch (no DispatchResult return path), so it processes
        // fire-and-forget requests (TLB invalidation, routing cleanup)
        // and defers scheduling-affecting requests (migration, work-steal)
        // to the next EL0 IRQ or timer tick.
        intid if super::gic::is_sgi(intid) => {
            super::gic::end_of_interrupt(intid);
            idle_ipi_check();

            return;
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

/// Scan Pulsar deadlines during idle and wake Observers if needed.
///
/// Called from the EL1h IRQ handler when the timer fires while the core
/// is idle (WFI). If a deadline has passed and the fire message wakes an
/// Observer, diverge into restore_or_idle to context-switch to it.
#[cfg(target_os = "none")]
fn idle_wakeup_check() {
    use crate::core_manager::{self, DispatchResult};
    use crate::time_manager::SchedulerAlgorithm;

    let core = core_manager::current_core_mut::<SchedulerAlgorithm>();

    if core.current.is_some() {
        return;
    }

    let ks = crate::frame::kernel_state();
    let current_ticks = sysreg::cntvct_el0();
    let counter_freq = sysreg::cntfrq_el0();
    let result = core.handle_timer(current_ticks, ks, counter_freq);

    match result {
        DispatchResult::Resume(_) | DispatchResult::ResumeFastPath(_) => {
            restore_or_idle(result);
        }
        _ => {}
    }
}

#[cfg(not(target_os = "none"))]
fn idle_wakeup_check() {}

/// Drain IPI mailbox when an SGI arrives at an idle core.
///
/// The EL1h IRQ handler cannot do full dispatch (no DispatchResult return
/// path back to EL0). But when the core is idle (current == None), an IPI
/// may carry an ObserverMigration that gives this core work. Drain the
/// mailbox via handle_ipi and diverge into restore_or_idle if an Observer
/// became runnable.
#[cfg(target_os = "none")]
fn idle_ipi_check() {
    use crate::core_manager::{self, DispatchResult};
    use crate::time_manager::SchedulerAlgorithm;

    let core = core_manager::current_core_mut::<SchedulerAlgorithm>();

    if core.current.is_some() {
        return;
    }

    let ks = crate::frame::kernel_state();
    let result = core.handle_ipi(ks);

    match result {
        DispatchResult::Resume(_) | DispatchResult::ResumeFastPath(_) => {
            restore_or_idle(result);
        }
        _ => {}
    }
}

#[cfg(not(target_os = "none"))]
fn idle_ipi_check() {}

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
#[cfg(target_os = "none")]
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
