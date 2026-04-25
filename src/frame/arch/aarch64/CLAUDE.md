# frame/arch/aarch64/

ARM64 hardware implementation. The ARM Architecture Reference Manual (ARM ARM)
is the spec — never guess an instruction encoding, system register field, or
side effect.

## Modules

| File                | What it owns                                       |
| ------------------- | -------------------------------------------------- |
| `boot.S`            | EL2→EL1 drop, BSS clear, stack setup, jump to Rust |
| `secondary_entry.S` | Secondary core entry (PSCI CPU_ON target)          |
| `cpu.rs`            | Core discovery, MPIDR, multicore bring-up          |
| `entropy.rs`        | Hardware RNG (RNDR/RNDRRS)                         |
| `exception.rs`      | Vector table, exception entry/exit                 |
| `gic.rs`            | GICv3 distributor + redistributor + CPU interface  |
| `mmio.rs`           | Volatile MMIO read/write primitives                |
| `mmu.rs`            | Page table construction, TTBR, TLB invalidation    |
| `platform.rs`       | Device base addresses, RAM layout                  |
| `psci.rs`           | PSCI calls (CPU_ON, CPU_OFF, SYSTEM_RESET)         |
| `register_state.rs` | Observer register save/restore layout (816 bytes)  |
| `serial.rs`         | UART driver (PL011)                                |
| `sysreg.rs`         | System register accessors (MRS/MSR wrappers)       |
| `timer.rs`          | Generic timer (CNTFRQ, CNTVCT, CNTV_CVAL/CTL)      |

## Inline assembly rules

These are from the root CLAUDE.md but they apply here specifically:

**`nomem` — never use by default.** Only add `nomem` with explicit justification
citing the instruction's side effects from the ARM ARM. `nomem` tells LLVM the
instruction doesn't access memory — if that's wrong, LLVM reorders memory
accesses past it, creating races that only manifest at higher optimization
levels or under SMP.

- **Safe to use `nomem`:** `mrs` of truly immutable registers (MPIDR_EL1,
  CNTFRQ_EL0), `wfe`/`wfi` hints.
- **Never use `nomem`:** `msr` to any system register (DAIF, TTBR, TPIDR, timer
  registers), `dsb`/`isb` barriers, `hvc`/`smc` calls, `tlbi` instructions, any
  `ldr`/`str`.

**SAFETY comments** are mandatory on every `unsafe` block. For inline asm, the
comment must cite the ARM ARM section that describes the instruction's behavior,
or name the invariant the instruction maintains.

## Key constants

- **PAGE_SIZE**: defined in `mmu.rs` from hardware granule configuration. This
  is the authoritative value — safe code receives it as a parameter.
- **CNTFRQ**: read from `CNTFRQ_EL0` at boot. Used by Pulsar (D72) for ns→ticks
  conversion.
- **GIC base addresses**: from DTB via `platform.rs`.

## Testing

Most code here is `#[cfg(target_os = "none")]` — it only compiles for the
bare-metal target. Tests that can run on the host (layout assertions, pure
computation) should be `#[cfg(test)]` and work under
`cargo test --target aarch64-apple-darwin`. Hardware-dependent behavior is
tested via the hypervisor runner.
