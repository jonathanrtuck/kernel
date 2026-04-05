# kernel

A [Rust](<https://en.wikipedia.org/wiki/Rust_(programming_language)>) [microkernel](https://en.wikipedia.org/wiki/Microkernel) for [ARM64](https://en.wikipedia.org/wiki/AArch64).

## design

- [Claims](design/claims.toml) · Single source of truth for design decisions. Each claim has a statement, status, confidence, scope, and rationale.
- [Philosophy](design/philosophy.md) · Two root principles and their consequences. The general thinking framework that produces decisions.
- [Landscape](design/landscape.md) · Survey of how 18+ real kernels and academic systems resolved each major design decision. Organized by decision point. Lists known approaches, tradeoffs, and novelty opportunities.

## build

```sh
cargo build -r
```

- requires [Rust nightly](https://rustup.rs/)
- targets [`aarch64-unknown-none`](https://doc.rust-lang.org/beta/rustc/platform-support/aarch64-unknown-none.html)

## run

```sh
cargo run -r
```

- requires [hypervisor](https://github.com/jonathanrtuck/hypervisor)

### or in [QEMU](https://www.qemu.org/download/#macos)

```sh
qemu-system-aarch64 -M virt -cpu cortex-a72 -m 256M -nographic -kernel target/aarch64-unknown-none/release/kernel
```
