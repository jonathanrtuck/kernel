# kernel

ARM64 microkernel, built from first principles. See `design/principles.md` for the foundational reasoning.

## Repository layout

```text
src/        — kernel source (no_std, aarch64-unknown-none)
host/       — host-side tests (independent crate, native target)
design/     — design documents and first-principles reasoning
```

The root Cargo.toml IS the kernel crate. `host/` is an independent Cargo project (not a workspace member) that runs on the host.

## Build commands

```sh
# Build the kernel
cargo build

# Run host-side tests
cd host && cargo test
```

The prebuilt `core` for `aarch64-unknown-none` comes from the nightly toolchain (pinned in `rust-toolchain.toml`).

## Rust

- Nightly toolchain, pinned via `rust-toolchain.toml`
- Edition 2024
- `rustfmt` runs automatically on `.rs` files after edits (Claude hook)

## Design

The kernel's scope is defined by what the hardware requires at EL1:

1. Multiplex CPU and RAM (MMU, preemption timer)
2. Route interrupts and faults (exception vectors, GIC)
3. Manage the privilege boundary (register save/restore, `eret`)

Everything else must earn its way in. See `design/principles.md`.
