# Autonomous Implementation Plan

How to transition from design derivations to working kernel code while
maintaining top-down discipline. The core insight: design and implementation are
the same activity (defining a graph of interfaces and components), but the
medium shifts from prose to Rust's type system at the point where the compiler
becomes a better verifier than logical coherence checking.

Created: 2026-04-24. Agreed between designer and Claude.

---

## Current state

**Design:** D1-D55 settled. Five kernel object types (Space, Time, Field,
Observer, Pulsar), IPC semantics, fault handling, scheduling, syscall ABI, fast
path, routing tables, arena lock ordering, per-type rights masks, send-once
encoding — all derived from axioms A1-A5.

**Code:** Architecture layer complete (~2,100 lines: boot, MMU, GIC, exceptions,
timer, serial, entropy, SMP). Kernel domain is type-level sketches only
(Observer metadata struct, capability ObjectType enum, stubs for Space/Field/
Time/Pulsar). No behavioral code.

**Gap:** ~80% of the designed system has no implementation. But the design is
thorough enough that most implementation is constrained by the derivation graph.

---

## Remaining open questions

### Mechanical — graph constrains the answer

These can be derived autonomously via `/explore` loops. Each is a self-contained
derivation with clear inputs and a narrow solution space.

| #   | Question                                                                                                 | Constraining derivations                      | Status      |
| --- | -------------------------------------------------------------------------------------------------------- | --------------------------------------------- | ----------- |
| M1  | Badge size                                                                                               | D17 (64-bit default stated)                   | not started |
| M2  | Scheduler callback signature                                                                             | D2, D42, D50, D53                             | not started |
| M3  | Send-to-waiting-receiver optimization                                                                    | D13, D50 (probably no separate mechanism)     | not started |
| M4  | Page-addressed vs byte-addressed                                                                         | A5, D25 (kernel absorbs → byte with rounding) | not started |
| M5  | Observer schema downstream (register save layout, budget encoding, default profile, self-reference caps) | D43, D39, ARM64 ABI                           | not started |
| M6  | Fault message content and enqueue mechanism                                                              | D12, D18, D20, D28, D7                        | not started |
| M7  | Pulsar creation API shape                                                                                | D32, D44, D35 (composable pattern)            | not started |
| M8  | Pulsar message content layout                                                                            | D28, D44                                      | not started |
| M9  | Badge-closure message format                                                                             | D17, D28                                      | not started |
| M10 | Badge on kernel-created send-once caps                                                                   | D16, D17                                      | not started |
| M11 | Clock access authority                                                                                   | D44 (CNTKCTL_EL1 per-Observer)                | not started |

### Genuine choices — multiple valid paths, need designer input

These require the designer to make a judgment call. Claude presents the full
design space; the designer decides.

| #   | Question                                                                                      | Core tension                                                                                                                                                     | Status      |
| --- | --------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------- |
| G1  | Revocation add-ons (generation vs CDT vs both)                                                | Speed vs granularity; cross-core prompt-effect                                                                                                                   | not started |
| G2  | Shared capability tables                                                                      | D8 per-Observer sufficiency vs sharing pressure                                                                                                                  | not started |
| G3  | Interrupt priority and routing                                                                | A5 (absorb) vs A3 (expose for RT workloads)                                                                                                                      | not started |
| G4  | Pager unavailability protocol                                                                 | Timeout/watchdog vs "let it hang" vs cleanup chain                                                                                                               | not started |
| G5  | Interrupt masking during fast path                                                            | Worst-case latency vs scheduling consistency                                                                                                                     | not started |
| G6  | Sub-page packing under D24                                                                    | Waste memory vs copy vs accept no cleanup                                                                                                                        | not started |
| G7  | Badge condition form                                                                          | Range vs bitmask vs predicate (IPC perf impact)                                                                                                                  | not started |
| G8  | Split-to-new vs split-to-existing                                                             | One syscall or two                                                                                                                                               | not started |
| G9  | Pulsar: duration vs absolute deadline                                                         | API shape and drift compensation                                                                                                                                 | not started |
| G10 | Send-once exemption encoding                                                                  | Consumed-by-use vs closed-without-use                                                                                                                            | not started |
| G11 | Cross-core kernel logic (migration, sleep/wake, placement)                                    | A4 (reactive) vs need for rebalancing; D46 (cores kernel-internal) means no userspace policy lever; migration policy must be embedded in existing event handlers | settled (D56) |
| G12 | Implementation hardening checklist (KASLR, guard pages, freed-memory zeroing, stack canaries) | Not derivable from design graph — leaf-node techniques that don't interact with the object model; track as a checklist, not derivations                          | not started |

---

## Phases

### Phase A — Derive mechanical questions (autonomous)

**Protocol:** Spawn parallel `/explore` loops, one per question. Each produces a
journal entry and a spec.md update (new derivation D56+).

**Parallelism:** Questions M1-M11 are independent. All can run concurrently as
subagents.

**Output:** Journal entries + spec.md additions. Designer reviews the batch.

**Completion gate:** All 11 questions settled or reclassified as genuine
choices.

### Phase B — Settle genuine choices (collaborative)

**Protocol:** For each question, Claude presents the design space across the
reference landscape (seL4, L4, EROS, Genode, QNX, etc.), names tradeoffs, and
maps each option to the axiom/derivation it best serves. Designer decides.

**Output:** Journal entries + spec.md derivations.

**Completion gate:** All genuine choices settled. The open questions section of
spec.md is empty (all struck through).

### Phase C — Interface layer (autonomous, then review)

**Protocol:** Define all Rust type signatures, method signatures, error types,
and trait bounds for the five kernel object types and their interactions. No
implementations — just the contract surface.

This is the **critical top-down gate.** The interface layer is the derivation
graph rendered in Rust's type system. Once the designer approves it,
implementation agents are constrained by the compiler. Bottom-up drift becomes a
type error.

**Key principle:** Interfaces are architectural decisions. The borrow checker,
lifetime system, and ownership model will surface constraints that prose
couldn't express. Expect 2-3 iterations as implementation attempts (Phase D)
reveal interface gaps.

**Output:** Complete type signatures across all kernel domain modules. No
`todo!()` bodies — absent code for unsettled interfaces, concrete types for
settled ones.

**Completion gate:** Designer reviews the interface layer as a coherent whole.
`cargo check` passes. The interface surface matches spec.md derivations.

**Reference:** The implementation-v1 branch has layer design docs
(design/layers/01-09.md) that can be consulted but are not binding. The
interface layer should be derived fresh from the settled spec, not copied from
the branch.

### Phase D — Leaf implementations (autonomous)

**Protocol:** TDD subagents (everything-claude-code:tdd skill). For each module:

1. Write tests from spec.md derivations (RED)
2. Implement to pass tests (GREEN)
3. Run `scripts/verify`
4. Refactor if needed (IMPROVE)

**Parallelism:** Modules behind settled interfaces can be implemented
concurrently. The arena lock ordering (D53) defines the dependency graph for
cross-module interactions.

**No `/explore` loops here.** Implementation of a settled interface behind a
settled spec is engineering, not design exploration. The design already
happened.

**Output:** Working kernel domain code with tests. Each module committed
independently.

**Completion gate:** `scripts/verify` passes. Test coverage >= 80%. All kernel
object lifecycles exercised.

**Reference:** The implementation-v1 branch has ~22K lines of implementation
that can be consulted as a reference. Treat it as "one possible implementation"
— useful for seeing how someone solved a specific problem, not as code to copy.

---

## Subagent briefing template

Every subagent spawned for this plan should receive:

1. The specific phase and task identifier (e.g., "Phase A, M3")
2. The relevant derivations from spec.md (cited by number)
3. The axioms and observations that constrain the answer
4. What the output should be (journal entry, type signature, implementation)
5. What to consult (spec.md sections, landscape.md, research/ docs)
6. What NOT to do (don't settle things outside scope, don't import patterns from
   a single system without naming alternatives)

---

## Anti-drift safeguards

These protect against the specific risk of bottom-up structure dictating the
design:

1. **Interfaces before implementations.** Phase C must complete before Phase D
   starts. The compiler enforces the contracts.
2. **Derivation traceability.** Every interface decision should cite which
   derivation(s) it implements. If a type signature can't be traced to a
   derivation, it's an undeclared design decision — stop and derive it first.
3. **Borrow-checker feedback loop.** When the borrow checker forces a structural
   change (split a struct, add a lifetime, change ownership), that's the medium
   surfacing a design constraint. Record it as a derivation, not a silent code
   fix.
4. **Reference, don't copy.** The implementation-v1 branch is a reference, not a
   template. Each module should be derived from spec.md, with the branch
   consulted for "how did they solve this specific Rust problem" — not for
   structure or architecture.
5. **scripts/verify at every step.** Clippy, build, test, unsafe count, module
   boundary containment. The verification gate catches structural drift.
