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

| Module          | Design concept                   | Key derivations         |
| --------------- | -------------------------------- | ----------------------- |
| `capability.rs` | Authority mechanism              | D4, D8, D11, D17        |
| `space.rs`      | Memory object                    | D9, D25, D26, D27       |
| `time.rs`       | Compute allocation               | D29, D30, D31, D36, D37 |
| `field.rs`      | IPC mechanism + message format   | D13, D15, D16, D28, D37 |
| `observer.rs`   | Execution unit                   | D6, D14, D20, D21       |
| `arch/`         | Hardware abstraction (core half) | A2, D1, D5              |

Management modules (core manager, scheduler, space manager) will emerge during
implementation — their shapes depend on implementation strategy more than
settled design.

## Spec drives code

Open questions in `design/spec.md` are represented as absent code, not as
guesses or `todo!()` placeholders. When a derivation settles something, the
corresponding module gains the concrete type or field. Code never quietly
settles something the spec left open.
