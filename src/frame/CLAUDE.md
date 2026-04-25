# frame/

The framekernel core — the kernel's unsafe boundary. Every `unsafe` block in the
kernel lives inside this module tree. The crate-level `#![deny(unsafe_code)]`
with `#[allow(unsafe_code)]` on `mod frame` enforces this at compile time.
`scripts/verify` checks it as belt-and-suspenders.

## What belongs here

Code that requires `unsafe` and cannot be expressed in safe Rust:

- **`arch/`** — system registers, MMU programming, exception vectors, inline asm
- **`firmware/`** — parsing untrusted boot-time data (DTB, future ACPI/UEFI)
- **Future additions** — page allocator internals (arena slab pages, D70),
  spinlock primitives (D53), per-core state access

## What does NOT belong here

- Kernel object types and their operations (Space, Time, Observer, Field,
  Pulsar) — these are safe Rust in `src/`
- Scheduling algorithm logic — safe Rust in `time_manager/`
- IPC orchestration logic — safe Rust in `communication.rs`
- Capability table operations — safe Rust in `capability.rs`

If you can express something in safe Rust, it goes outside `frame/`. The
boundary exists to minimize the trusted computing base — every line of unsafe is
a line that must be manually verified correct.

## Rules for unsafe code

Every `unsafe` block MUST have a `// SAFETY:` comment explaining:

1. The invariant it relies on
2. What would break if that invariant were violated

When editing an existing `unsafe` block, re-verify the SAFETY comment still
holds with the change. The comment is a proof obligation, not documentation.

## Interaction with safe code

frame/ **exports** safe abstractions that the rest of the kernel uses. The safe
modules define the types and logic; frame/ provides the mechanism:

- Safe code calls into frame/ for hardware operations (register access, MMU,
  interrupts)
- frame/ calls into safe code for kernel logic (cap resolution, IPC, scheduling)
- The entry points from frame/ to safe code are the `core_manager` dispatch
  methods and the `time_manager` scheduler trait callbacks

The dependency direction at the type level is bidirectional, but at the
implementation level: frame/ depends on safe module types, safe modules depend
on frame/ abstractions.
