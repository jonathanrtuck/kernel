//! Saved register context for Observer context switches.
//!
//! Distinct from [`super::exception::TrapFrame`] which captures
//! exception-specific registers (ESR, FAR) not part of the Observer's
//! persistent identity. D74: EL0 exception entry saves directly into
//! RegisterState (not via TrapFrame). TrapFrame is used only for EL1h
//! exceptions (kernel-interrupting-kernel).

/// Full saved register context for an Observer (AArch64).
///
/// Lives in the consumed Space's structural backing (D35). The Observer
/// metadata struct holds a pointer to this (D43: too large for root-Space
/// metadata per D32).
#[repr(C)]
pub struct RegisterState {
    /// General-purpose registers x0–x30.
    pub gprs: [u64; 31],
    /// User stack pointer (SP_EL0).
    pub sp: u64,
    /// Program counter (ELR_EL1 — resume address).
    pub pc: u64,
    /// Saved processor state (SPSR_EL1).
    pub pstate: u64,
    /// Thread-local storage (TPIDR_EL0).
    pub tpidr: u64,
    /// FP/SIMD registers v0–v31 (128-bit each).
    pub fp_regs: [u128; 32],
    /// Floating-point control register.
    pub fpcr: u64,
    /// Floating-point status register.
    pub fpsr: u64,
}

// ── Byte offsets for assembly access ─────────────────────────────
//
// EL0 exception entry (exception.S) and context restore (__restore_observer)
// use these offsets to save/load RegisterState fields at known positions.
// The `offset_of!` assertions below guarantee they match the actual layout.

/// Byte offset of `gprs` within `RegisterState`.
pub const RS_GPRS: usize = 0;
/// Byte offset of `sp` within `RegisterState`.
pub const RS_SP: usize = 248;
/// Byte offset of `pc` within `RegisterState`.
pub const RS_PC: usize = 256;
/// Byte offset of `pstate` within `RegisterState`.
pub const RS_PSTATE: usize = 264;
/// Byte offset of `tpidr` within `RegisterState`.
pub const RS_TPIDR: usize = 272;
/// Byte offset of `fp_regs` within `RegisterState`.
pub const RS_FP_REGS: usize = 288;
/// Byte offset of `fpcr` within `RegisterState`.
pub const RS_FPCR: usize = 800;
/// Byte offset of `fpsr` within `RegisterState`.
pub const RS_FPSR: usize = 808;

// Compile-time layout and offset assertions — these MUST match the assembly
// immediates in exception.S. If any field is reordered or padded differently,
// the assertion fires at compile time rather than producing silent context
// corruption at runtime.
const _: () = {
    assert!(core::mem::size_of::<RegisterState>() == 816);
    assert!(core::mem::offset_of!(RegisterState, gprs) == RS_GPRS);
    assert!(core::mem::offset_of!(RegisterState, sp) == RS_SP);
    assert!(core::mem::offset_of!(RegisterState, pc) == RS_PC);
    assert!(core::mem::offset_of!(RegisterState, pstate) == RS_PSTATE);
    assert!(core::mem::offset_of!(RegisterState, tpidr) == RS_TPIDR);
    assert!(core::mem::offset_of!(RegisterState, fp_regs) == RS_FP_REGS);
    assert!(core::mem::offset_of!(RegisterState, fpcr) == RS_FPCR);
    assert!(core::mem::offset_of!(RegisterState, fpsr) == RS_FPSR);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_state_gpr_stride() {
        // Assembly uses immediate offsets for individual GPRs (e.g., x19 at
        // offset 152 = 19 * 8). Verify the array element stride is 8 bytes.
        assert_eq!(
            core::mem::size_of::<u64>(),
            8,
            "GPR stride must be 8 bytes for assembly offset calculations"
        );
        // Verify specific GPR offsets used in the trampoline (x19 at 152).
        assert_eq!(RS_GPRS + 19 * 8, 152, "x19 must be at offset 152");
    }

    #[test]
    fn register_state_fp_reg_stride() {
        // Assembly uses stp/ldp q-register pairs at 32-byte stride.
        assert_eq!(
            core::mem::size_of::<u128>(),
            16,
            "FP register stride must be 16 bytes"
        );
        // Verify q0 at offset 288, q31 at 288 + 31*16 = 784.
        assert_eq!(RS_FP_REGS + 31 * 16, 784, "q31 must be at offset 784");
    }
}
