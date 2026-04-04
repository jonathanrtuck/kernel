# kernel

ARM64 microkernel, built from first principles. Design decisions live in `design/claims.toml` (loaded automatically at session start). See `design/philosophy.md` for the general thinking framework.

## Working Protocol (MANDATORY)

These rules govern how you work on this project. They are not preferences — they are requirements.

### 1. Understand before acting

- Read every file you will modify, AND every file that depends on it
- Trace all downstream effects of the change before writing code
- If the problem has known algorithms or prior art, research them from authoritative sources (specs, papers, reference implementations) — never improvise when a solution exists
- Never guess an API, syscall, instruction encoding, or wire format — look it up in the actual source or documentation. Wrong assumptions cascade silently.

### 2. Verify everything yourself

- Write or identify tests BEFORE implementing. Watch them fail. Implement. Watch them pass.
- Run the FULL test suite, not just tests you think are relevant
- Trace every affected code path. Finding A bug is not the same as finding THE bug.
- Never declare "done" without evidence.
- If verification tooling doesn't exist for a change, STOP. Building the tooling becomes the immediate priority. Push the original task onto the stack, build what's needed to verify, then resume. Unverifiable work does not ship — no exceptions.

### 3. Fix root causes, not symptoms

- When something breaks, diagnose the actual cause — don't patch the surface
- When fixing a bug, check for the same class of bug in related code
- If an interface is confusing enough to cause a bug, STOP and flag it — interfaces are architectural decisions in this project. Propose the fix, don't silently apply it.

### 4. Present the design space, not a default answer

- When suggesting kernel design approaches, never default to a single system's patterns (especially Zircon/Fuchsia). Present the design space across multiple systems.
- Name which systems you're drawing from and why. "Zircon does it this way" is not a justification — "This approach is correct because [reason], and [system] chose it for [their reason]" is.
- Reference landscape: seL4, L4 family, EROS/Coyotos, Genode, QNX, Plan 9, Barrelfish, Redox, Minix 3 — not just Zircon.

### 5. Update reference docs at milestone boundaries

When completing a milestone, verify design documents reflect the current architecture. These files don't change during daily work but drift across milestones. A quick scan before tagging catches stale references.

## Working Mode

This project produces two deliverables: the **kernel source** and the **design** (`design/claims.toml`). Both are first-class outputs. A session that explores architecture for two hours and records one claim is as productive as a session that implements a subsystem. Every session should advance one or both.

This is a long-running exploration project with no deadline. Sessions may be days or months apart. The designer wants a **thinking partner**, not a project manager:

- **Explore, don't push.** Help think through ideas, poke holes, surface tradeoffs. Don't rush toward decisions or implementation.
- **Hold context across sessions.** Use MEMORY.md and design claims to resume seamlessly.
- **Connect the dots.** Flag similarities, inconsistencies, or connections to previous discussions. Remind when something was already explored or rejected.
- **Guide gently.** Suggest topics that would address gaps in the emerging design. Ask for clarity when needed. Flag dead ends or common traps.
- **Respect the pace.** The designer may want to deep-dive a topic, switch to coding, or just chat loosely. Follow their energy.

## Repository Layout

```text
src/        — kernel source (no_std, aarch64-unknown-none)
host/       — host-side tests (independent crate, native target)
design/     — claims.toml (decisions SSOT), philosophy, derivations
```

The root Cargo.toml IS the kernel crate. `host/` is an independent Cargo project (not a workspace member) that runs on the host.

## Build Commands

```sh
# Build the kernel
cargo build

# Run the kernel (native Apple Hypervisor.framework — preferred)
hypervisor target/aarch64-unknown-none/debug/kernel --no-gpu --timeout 5

# Run host-side tests
cd host && cargo test
```

Use `hypervisor` (installed at `~/.local/bin/hypervisor`, source at `~/Sites/hypervisor/`) for all kernel testing. QEMU is a fallback only. Key flags:

- `--no-gpu` — serial-only mode (no Metal window)
- `--timeout SECS` — exit after N seconds (for automated runs)
- `--capture N PATH` — capture frame N as PNG, then exit
- `--events FILE` — run scripted input + captures

The prebuilt `core` for `aarch64-unknown-none` comes from the nightly toolchain (pinned in `rust-toolchain.toml`).

## Kernel Change Protocol (MANDATORY)

**Every change to the kernel MUST follow this protocol.** The kernel is the foundation; a bug here corrupts everything above.

### Unsafe code and inline assembly

- Every `unsafe` block MUST have a `// SAFETY:` comment explaining the invariant it relies on and what would break if violated.
- Inline asm `options()`: **never use `nomem` by default.** Only add `nomem` with explicit justification citing the instruction's side effects from the ARM architecture manual. `nomem` tells LLVM the instruction doesn't access memory — if that's a lie, LLVM will reorder memory accesses past it, creating races that only manifest at higher optimization levels or under SMP load.
  - **Safe to use `nomem`:** `mrs` of truly immutable registers (MPIDR_EL1, CNTFRQ_EL0), `wfe`/`wfi` hints.
  - **Never use `nomem`:** `msr` to any system register (DAIF, TTBR, TPIDR, timer registers), `dsb`/`isb` barriers, `hvc`/`smc` calls, `tlbi` instructions, any `ldr`/`str` (obviously reads/writes memory).
- When editing existing `unsafe` blocks, re-verify the SAFETY comment still holds with the change.

### Anomaly tracking

- Any unexplained kernel behavior (spurious wakeups, unexpected fault codes, timing anomalies) MUST be documented with `Status: open-bug`.
- Workarounds (retry loops, defensive checks) are acceptable as defense-in-depth but do NOT close the bug. The root cause investigation continues.

## Rust

- Nightly toolchain, pinned via `rust-toolchain.toml`
- Edition 2024
- `rustfmt` runs automatically on `.rs` files after edits (Claude hook)

### File layout convention

Every `.rs` file follows this order:

1. **Module doc comment** (`//!`)
2. **Imports** (`use` statements, grouped by rustfmt)
3. **Constants and statics**
4. **Types in dependency order, each co-located with its `impl` blocks** — define a type, then immediately its `impl` block(s), before the next type. Within `impl` blocks: constructors first (`new`, `from_*`), then public methods, then private methods.
5. **Free functions**
6. **Tests** (`#[cfg(test)]` module)

**Co-located, not types-first.** Do NOT group all type definitions at the top with all `impl` blocks below. Each type lives next to its implementation. Types appear in dependency order: if type B uses type A, define A first.

## Design

The kernel's scope is defined by what the hardware requires at EL1:

1. Multiplex CPU and RAM (MMU, preemption timer)
2. Route interrupts and faults (exception vectors, GIC)
3. Manage the privilege boundary (register save/restore, `eret`)

Everything else must earn its way in. See `design/claims.toml`.

## Design Documents

- `design/claims.toml` — **Single source of truth for design decisions.** Each claim has a statement, status, confidence, scope, and rationale. Loaded automatically at session start via hook. To add a decision, add a `[[claim]]` entry.
- `design/philosophy.md` — **Read first.** Two root principles and their consequences. The general thinking framework that produces decisions.
