//! Speculative execution mitigation — detection and barriers.
//!
//! ## What this module provides
//!
//! 1. **Speculation barrier** ([`speculation_barrier`]) — the `SB` instruction,
//!    used after bounds checks on user-provided indices (Spectre v1 mitigation).
//!
//! 2. **Feature detection** ([`detect`], [`parse_features`]) — reads ARM64 ID
//!    registers to determine which mitigations are hardware-provided vs. need
//!    software fallbacks.
//!
//! ## Mitigation landscape for ARM64
//!
//! | Vulnerability          | Hardware feature       | Software fallback         | Plug point                    |
//! |------------------------|------------------------|---------------------------|-------------------------------|
//! | Meltdown               | E0PD (TCR_EL1 bit)     | KPTI (separate page tables)| `mmu.rs` TCR config           |
//! | Spectre v1 (bounds)    | None — always software  | SB after bounds checks    | `capabilities.rs` via this module |
//! | Spectre v2 (branch)    | CSV2 / CSV3            | Branch predictor flush    | Context switch path           |
//! | Spectre-BHB            | CSV3 / CLRBHB          | BHB clearing loop         | `exception.S` preamble        |
//! | Store bypass (SSB)     | SSBS                   | Per-Observer SSBS mgmt    | Register save/restore         |
//!
//! ## Porting guide
//!
//! A porter targeting different ARM64 hardware should call [`detect`] at boot
//! and check which features are absent:
//!
//! - **No CSV2/CSV3:** Add branch predictor invalidation to the context
//!   switch path. On Cortex-A cores, this is typically `IC IALLU` or a
//!   firmware-assisted call via SMCCC. Location: `register_state.rs` or
//!   the switch sequence in `exception.S`.
//!
//! - **No CSV3 and no CLRBHB:** Add a BHB clearing loop (~32 unconditional
//!   branches) to the exception vector preamble in `exception.S`, before
//!   the call to `exception_handler`. See ARM white paper "Cache Speculation
//!   Side-channels" (version 19+) for the canonical clearing sequence.
//!
//! - **No E0PD:** Implement KPTI — maintain separate user/kernel page table
//!   configurations and swap between them on every exception entry/exit.
//!   The trampoline goes in `exception.S`; page table management in `mmu.rs`.
//!   E0PD is available on ARMv8.5+ (all Apple Silicon, Cortex-A77+).
//!
//! - **No FEAT_SB:** The SB instruction is in the ARM Hints space and
//!   executes as NOP on pre-ARMv8.5 hardware (ARM ARM §C6.2.229), so
//!   [`speculation_barrier`] is safe to call on any ARM64. On pre-ARMv8.5
//!   cores where NOP is insufficient, replace with `CSDB` (available since
//!   ARMv8.0) for equivalent Spectre v1 barrier semantics.

#[cfg(target_os = "none")]
use super::sysreg;

// ── Speculation barrier ─────────────────────────────────────────────

/// Speculation Barrier (SB, FEAT_SB / ARMv8.5).
///
/// Prevents speculative execution of subsequent instructions until all
/// prior conditional branches are resolved. Used between a bounds check
/// and the dependent pointer dereference to prevent Spectre v1 (bounds
/// check bypass).
///
/// The canonical pattern:
/// ```text
///     if index >= capacity { return None; }
///     speculation_barrier();       // <── branch must resolve before load
///     unsafe { ptr.add(index) }
/// ```
///
/// ARM ARM §C6.2.229: SB is in the Hints space. On hardware without
/// FEAT_SB, it executes as NOP. This makes it safe to emit
/// unconditionally — a porter targeting pre-ARMv8.5 can rely on it as a
/// no-worse-than-NOP baseline, upgrading to CSDB if actual barrier
/// semantics are needed.
#[inline(always)]
pub fn speculation_barrier() {
    // SAFETY: SB (Speculation Barrier) is a barrier hint instruction that
    // does not access memory or modify registers beyond preventing
    // speculative instruction fetch. Encoded as raw `.inst` because the
    // bare-metal target may not enable the `sb` assembler feature.
    // ARM ARM §C6.2.229: encoding 0xD503_30FF, Hints space — executes as
    // NOP on hardware without FEAT_SB.
    // No `nomem` — the barrier's purpose is to prevent LLVM from reordering
    // memory accesses past a resolved branch. Reordering past SB would
    // defeat the Spectre v1 mitigation.
    unsafe {
        core::arch::asm!(".inst 0xd50330ff", options(nostack));
    }
}

// ── Feature detection ───────────────────────────────────────────────

/// Hardware speculation mitigation features detected from ID registers.
///
/// All fields default to `false` (feature absent). A porter checks these
/// at boot to decide which software fallbacks to enable.
#[derive(Clone, Copy, Debug)]
pub struct SpeculationFeatures {
    /// FEAT_CSV2 — branch predictor entries are tagged per-context.
    /// Cross-Observer branch target injection is hardware-mitigated.
    pub csv2: bool,
    /// FEAT_CSV3 — speculative reads from another context cannot be
    /// disclosed by any side channel. Strongest speculation isolation;
    /// makes Spectre v2 AND Spectre-BHB hardware-mitigated.
    pub csv3: bool,
    /// FEAT_SB — Speculation Barrier instruction is functional (not NOP).
    pub sb: bool,
    /// FEAT_E0PD — EL0 speculative accesses to EL1 pages return a fixed
    /// value. Hardware Meltdown mitigation; makes KPTI unnecessary.
    pub e0pd: bool,
}

/// Parse speculation features from raw ID register values.
///
/// Pure function — testable on the host with mock values.
///
/// Bit field references (ARM ARM §D19):
/// - `ID_AA64PFR0_EL1[59:56]` → CSV2 (≥1 = implemented)
/// - `ID_AA64PFR0_EL1[63:60]` → CSV3 (≥1 = implemented)
/// - `ID_AA64ISAR1_EL1[39:36]` → SB (≥1 = implemented)
/// - `ID_AA64MMFR2_EL1[63:60]` → E0PD (≥1 = implemented)
pub fn parse_features(pfr0: u64, isar1: u64, mmfr2: u64) -> SpeculationFeatures {
    SpeculationFeatures {
        csv2: ((pfr0 >> 56) & 0xF) >= 1,
        csv3: ((pfr0 >> 60) & 0xF) >= 1,
        sb: ((isar1 >> 36) & 0xF) >= 1,
        e0pd: ((mmfr2 >> 60) & 0xF) >= 1,
    }
}

/// Detect speculation features from hardware ID registers.
///
/// Reads `ID_AA64PFR0_EL1`, `ID_AA64ISAR1_EL1`, and `ID_AA64MMFR2_EL1`.
/// These are EL1-accessible read-only registers — only callable from
/// bare-metal (EL1), not from host userspace tests.
#[cfg(target_os = "none")]
pub fn detect() -> SpeculationFeatures {
    parse_features(
        sysreg::id_aa64pfr0_el1(),
        sysreg::id_aa64isar1_el1(),
        sysreg::id_aa64mmfr2_el1(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_zero_registers_detects_nothing() {
        let f = parse_features(0, 0, 0);

        assert!(!f.csv2, "CSV2 must be absent with zeroed registers");
        assert!(!f.csv3, "CSV3 must be absent with zeroed registers");
        assert!(!f.sb, "SB must be absent with zeroed registers");
        assert!(!f.e0pd, "E0PD must be absent with zeroed registers");
    }

    #[test]
    fn parse_csv2_at_bits_59_56() {
        let pfr0 = 0x1u64 << 56;
        let f = parse_features(pfr0, 0, 0);

        assert!(f.csv2, "CSV2=1 at PFR0[59:56] must be detected");
        assert!(!f.csv3, "CSV3 must not be set by CSV2-only PFR0");
    }

    #[test]
    fn parse_csv2_level_2_detected() {
        let pfr0 = 0x2u64 << 56;
        let f = parse_features(pfr0, 0, 0);

        assert!(
            f.csv2,
            "CSV2=2 (SCXTNUM support) must still report csv2=true"
        );
    }

    #[test]
    fn parse_csv3_at_bits_63_60() {
        let pfr0 = 0x1u64 << 60;
        let f = parse_features(pfr0, 0, 0);

        assert!(f.csv3, "CSV3=1 at PFR0[63:60] must be detected");
    }

    #[test]
    fn parse_sb_at_isar1_bits_39_36() {
        let isar1 = 0x1u64 << 36;
        let f = parse_features(0, isar1, 0);

        assert!(f.sb, "SB=1 at ISAR1[39:36] must be detected");
    }

    #[test]
    fn parse_e0pd_at_mmfr2_bits_63_60() {
        let mmfr2 = 0x1u64 << 60;
        let f = parse_features(0, 0, mmfr2);

        assert!(f.e0pd, "E0PD=1 at MMFR2[63:60] must be detected");
    }

    #[test]
    fn parse_all_features_present() {
        let pfr0 = (0x1u64 << 56) | (0x1u64 << 60);
        let isar1 = 0x1u64 << 36;
        let mmfr2 = 0x1u64 << 60;
        let f = parse_features(pfr0, isar1, mmfr2);

        assert!(f.csv2);
        assert!(f.csv3);
        assert!(f.sb);
        assert!(f.e0pd);
    }

    #[test]
    fn parse_unrelated_bits_do_not_trigger_false_positive() {
        // Zero out CSV2 [59:56] and CSV3 [63:60] nibbles, set everything else.
        let pfr0 = 0x00FF_FFFF_FFFF_FFFFu64;
        let f = parse_features(pfr0, 0, 0);

        assert!(!f.csv2, "bits outside [59:56] must not trigger CSV2");
        assert!(!f.csv3, "bits outside [63:60] must not trigger CSV3");
    }

    #[test]
    fn speculation_barrier_executes_without_trapping() {
        speculation_barrier();
    }
}
