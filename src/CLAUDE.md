# src/

Kernel implementation. Design decisions live in `design/spec.md`; this directory
realizes them in Rust.

## Code style

Write for eventual Verus verification (journal 023). These preferences are free
now and expensive to retrofit:

- **Concrete types over trait objects.** Verus verifies concrete
  implementations; `dyn Trait` requires more complex specifications. Use
  generics with trait bounds when polymorphism is needed.
- **Explicit state transitions.** Prefer `fn(State, Input) -> State` over
  mutating in place. Aligns with A4 (purely reactive) and makes invariants
  visible.
- **Pure functions where feasible.** Side-effect-free logic is dramatically
  easier to verify. Isolate side effects at the edges.
- **Simple lifetimes.** Verus handles ownership well but intricate lifetime
  annotations are harder to specify. If a lifetime is getting complex,
  reconsider the data structure.
- **No abbreviations.** Spell out full words in file names, modules, types, and
  identifiers. Standard acronyms (MMU, GIC, IPC) are fine.

## Framekernel discipline (journal 023)

Confine all `unsafe` to a minimal core boundary. Everything above that boundary
is safe Rust. The core provides safe abstractions that structurally prevent
misuse — not just wrappers around unsafe functions.

`arch/` is the hardware half of the core. A kernel-primitives layer (safe
references to kernel objects, typed memory regions) may emerge as the other
half. The boundary is the trait interface between them.

Study Asterinas OSTD's API surface before committing to the core boundary
design.

## Module map

Each module corresponds to a design concept. Derivation references in module doc
comments link to `design/spec.md`.

| Module          | Design concept                   | Key derivations                                      |
| --------------- | -------------------------------- | ---------------------------------------------------- |
| `arena.rs`      | Per-type slab + generation       | D53, D67, D70                                        |
| `capability.rs` | Authority mechanism              | D4, D8, D11, D17, D51, D52, D57, D58, D67            |
| `space.rs`      | Memory object                    | D9, D25, D26, D27, D41, D60, D67                     |
| `time.rs`       | Compute allocation               | D29, D30, D31, D36–D38, D67                          |
| `field.rs`      | IPC mechanism + message format   | D13, D15–D18, D28, D45, D54, D67, D71, D73           |
| `observer.rs`   | Execution unit                   | D6, D14, D20, D21, D39, D42, D43, D56, D57, D66, D67 |
| `pulsar.rs`     | Timer mechanism                  | D44, D52, D62, D63, D67, D72                         |
| `scheduler.rs`  | Scheduler + placement traits     | D2, D50, D56, D59                                    |
| `fault.rs`      | Fault types and delivery         | D12, D40, D61                                        |
| `syscall.rs`    | Syscall ABI types                | D47, D48, D49                                        |
| `arch/`         | Hardware abstraction (core half) | A2, D1, D5, D46, D47, D49, D56                       |

## Spec drives code

Open questions in `design/spec.md` are represented as absent code, not as
guesses or `todo!()` placeholders. When a derivation settles something, the
corresponding module gains the concrete type or field. Code never quietly
settles something the spec left open.
