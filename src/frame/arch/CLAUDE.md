# frame/arch/

Architecture abstraction layer. The portability boundary.

## Structure

`mod.rs` re-exports the current target architecture module (`aarch64/`). Code
outside `frame/arch/` uses `frame::arch::*` and never names a specific
architecture. When a second architecture is added, it gets a sibling directory
and the `mod.rs` `cfg` gates select the right one.

**A2 is an input here but not elsewhere.** Architecture-specific details (GIC,
generic timer, EL0/EL1, PSCI) are confined to this directory. The rest of the
kernel sees trait interfaces and opaque types. If a change in this directory
forces a change outside `frame/arch/`, the abstraction boundary has leaked — fix
the boundary, not the caller.

## What this directory exports

- **Page size** (D25): discovered from hardware at boot, exported as a constant
  or function. Safe code receives it as a parameter — never imports it directly
  from an architecture module.
- **Interrupt control** (D22, D69): mask/unmask, GIC programming, IPI send
- **Timer programming** (D44): set comparator, read counter, frequency
- **MMU operations** (D5, D26): page table manipulation, TTBR load, TLB flush
- **Register state** (D6, D43): save/restore, read/write for Observer contexts
- **Exception entry** (D1, A4): vector table, dispatch to core_manager
- **Core lifecycle** (D46): PSCI CPU_ON/CPU_OFF/CPU_SUSPEND, WFI
- **Platform constants**: device base addresses, RAM layout (from DTB via
  `frame/firmware/`)

## What does NOT belong here

Kernel logic. This layer programs hardware mechanisms — it does not make policy
decisions. "The kernel's job at this boundary is to _program_ the mechanism, not
to _be_ the mechanism" (philosophy.md: use what the hardware provides).
