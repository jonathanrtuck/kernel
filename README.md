# kernel

A [Rust](<https://en.wikipedia.org/wiki/Rust_(programming_language)>)
[microkernel](https://en.wikipedia.org/wiki/Microkernel) for
[ARM64](https://en.wikipedia.org/wiki/AArch64).

## design

- [Philosophy](design/philosophy.md) · Two root principles and their
  consequences. The general thinking framework that produces decisions.
- [Landscape](design/landscape.md) · Survey of how 18+ real kernels and academic
  systems resolved each major design decision.
- [Research](design/research/) · Prior art studies prepared before major design
  decisions.
- [Journal](design/journal/) · Numbered exploration entries recording reasoning,
  rejected alternatives, and discoveries.
- [Spec](design/spec.md) · Single source of truth for settled design decisions
  with brief rationale.
- [Graph](design/graph.d2) · Structural map of components, interfaces, and
  edges. Render with [D2](https://d2lang.com/tour/install/):
  `d2 --layout elk graph.d2`.

## code

```sh
# build
cargo build

# run
cargo run

# test (runs on host)
cargo test --target aarch64-apple-darwin
```

**Requires:**

- [Rust nightly](https://rustup.rs/)
- [hypervisor](https://github.com/jonathanrtuck/hypervisor) (preferred)
  - or [QEMU](https://www.qemu.org/download/#macos) as fallback
