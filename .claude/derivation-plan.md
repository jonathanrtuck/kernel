# Derivation Plan: Runtime Flows → Implementation

From "interfaces defined" to "kernel dispatches syscalls and returns to
userspace." Organized by derivation level, not implementation task. Each
derivation settles one runtime interface before any code implements it.

Replaces the task-first structure of `implementation-plan-layers-1-3.md`. That
document remains as an engineering reference — its settled decisions (handle
encoding, max IRQs, TLB strategy, etc.) are inputs to these derivations, not
standalone truths.

---

## Why this structure

The project's method (philosophy.md §Levels of resolution):

> At each level, the work is: define the interfaces between sibling black boxes.
> Resist defining next-level internals until the current-level interfaces are as
> stable as you can make them.

D1–D75 settled what objects exist and what interfaces they expose. The next
level down is: **what data flows between those interfaces at runtime.** This is
a distinct level — not implementation, not leaf algorithms, but the connective
tissue that composes settled interfaces into operational sequences.

Evidence this level matters: tracing the IPC Send flow from first principles
(2026-04-25 session) revealed 6 interface gaps that the D-chain missed. 4 of 6
naive proposals for fixing them were wrong because the derivation hadn't been
done — the ARM64 exception model, message ownership semantics, and error
signaling boundary all constrained the answers in non-obvious ways.

The existing implementation plan has correct answers for most of these, but
without derivation trails. Correct answers without reasoning are fragile — they
can't be verified, extended, or debugged when something downstream breaks.

---

## Derivation sequence: Phase A (D76–D83)

Each derivation settles one interface between runtime components. Format follows
the D1–D75 convention: question, rests on, settles, rejected alternatives, test.

### D76: Dispatch entry contract

**Question:** What data crosses the frame/ → safe-code boundary at exception
entry? What does safe code return to frame/?

**Rests on:** D1 (per-core hot path), D7 (IPC vs typed split), D47 (register
layout), D49 (error signaling encoding), D74 (register save on EL0), ARM64
exception model.

**Must settle:**

- Whether dispatch reads registers itself via frame/ helper (pull) or receives
  them as parameters (push). The ARM64 exception sequence saves registers to
  memory before the Rust handler runs — this constrains the answer.
- What DispatchResult carries. Whether it includes error info or whether
  dispatch writes errors directly via frame/ helpers.
- The frame/ helper interfaces: read_ipc_registers, read_typed_registers,
  write_ipc_error, clear_ipc_carry, write_typed_result.
- dispatch_ipc and dispatch_typed signatures: parameters, return type.

**Reference check:** implementation-plan-layers-1-3.md Tasks 1.5, 1.9.

### D77: Cap resolution protocol

**Question:** How does a userspace handle value become a mutable reference to a
kernel object?

**Rests on:** D4 (cap-based authority), D8 (flat table), D11 (slot tags), D52
(rights), D67 (generation), D75 (KernelState).

**Must settle:**

- Observer needs cap_table_capacity (why the D-chain missed it: D8 put capacity
  on Table, hot path indexes through Observer's raw pointer).
- Handle ABI encoding (index + slot_tag packing).
- The full resolution sequence: bounds check → entry lookup → slot tag check →
  occupied check → generation check → rights check → type check → arena lookup →
  mutable reference.
- Lock acquisition: which arena lock, when acquired, when released.
- KernelState struct contents (what the resolution path needs).

**Reference check:** implementation-plan-layers-1-3.md Tasks 1.1, 1.2, Task 1.5
"DECIDED: handle encoding."

### D78: IPC message ownership

**Question:** Who holds the message at each point in the send / receive / call /
reply_recv paths? Where does it live when "in transit"?

**Rests on:** D13 (queued fields), D16 (reply via send-once), D28 (message
format), D50 (fast-path conditions), D74 (register pass-through).

**Must settle:**

- Whether send()/call() consume or borrow the Message.
- What SendOutcome::WokeReceiver / CallOutcome::DirectSwitch carry (message?
  just observer pointer?).
- How the dispatch layer delivers message data to a woken receiver's saved
  registers. The frame/ bridge function for this.
- D74 fast-path register pass-through: x0-x3 stay in physical registers, kernel
  writes only x4-x7. How this interacts with message construction — the Message
  object may not need to be constructed at all on the fast path.
- Ownership through the queue: Message moves into Field queue on Enqueued, moves
  out on dequeue. Receiver gets a copy in registers.

**Reference check:** implementation-plan-layers-1-3.md Task 2.6.

### D79: Scheduling decision matrix

**Question:** For each (IPC operation × outcome) pair, what state transitions
happen on which Observers, and who runs next?

**Rests on:** D2 (scheduling), D13 (IPC semantics), D16 (reply), D39 (Observer
state machine), D48 (5 operations), D50 (fast-path direct switch), D59
(scheduler trait).

**Must settle:**

- The full 9-row matrix: Send×Enqueued, Send×WokeReceiver, Receive×Received,
  Receive×Blocked, Call×Enqueued, Call×DirectSwitch, ReplyRecv×Received,
  ReplyRecv×Blocked, Yield.
- For each row: which Observer(s) change state, which scheduler methods are
  called, what DispatchResult is returned.
- D50 should_switch_to callback: when consulted, what happens on approval vs
  denial.
- Yield semantics: enqueue current before schedule_next, or just schedule_next?

**Reference check:** implementation-plan-layers-1-3.md Task 1.5 step h.

### D80: Error and fault delivery

**Question:** How do syscall errors reach userspace? How do hardware faults
become IPC messages sent to handler Fields?

**Rests on:** D12 (fault delegation), D49 (error encoding), D61 (fault types),
ARM64 SPSR encoding, D21 (fault handler slot).

**Must settle:**

- Syscall error path: dispatch detects error → calls frame/ helper to write
  error to RegisterState → returns Resume(current). The frame/ helpers:
  write_ipc_error (carry + x0), write_typed_result (negative x0),
  clear_ipc_carry.
- Fault delivery path: frame/ classifies exception → dispatch constructs
  FaultType → fault.to_message() builds Message → kernel-as-sender to handler
  Field (slot 0) → observer.fault() transition.
- Fault message Observer cap: 5-right subset, how it's constructed (mint from
  self-cap? kernel constructs directly?).
- Pending list path: handler Field full → D18 pending list → drain on next
  receive.

**Reference check:** implementation-plan-layers-1-3.md Task 1.9.

### D81: Hardware event protocol

**Question:** How do timer interrupts and device IRQs flow through the dispatch
path?

**Rests on:** D2 (preemption), D22 (IRQ routing), D44 (Pulsar), D62/D63 (timer
fire message).

**Must settle:**

- IRQ routing: table structure, location (KernelState), indexing (direct by
  INTID), max entries. Route contains: field_id, badge, generation. Lookup +
  generation check + message construction + send to Field.
- Timer/Pulsar: per-core deadline structure, location (CoreState), max entries,
  checking protocol (iterate on every timer tick), fire protocol (construct
  Message::timer_fire, send to delivery Field), rearm protocol (drift
  compensation for repeating Pulsars).
- handle_timer flow: check deadlines → fire expired → on_preempt →
  schedule_next.
- handle_irq flow: lookup route → construct message → send → schedule_next.

**Reference check:** implementation-plan-layers-1-3.md Tasks 1.3, 1.4, 1.7, 1.8.

### D82: Global state organization

**Question:** What is the KernelState struct, where does it live, how is it
accessed from the dispatch path?

**Rests on:** D53 (lock ordering), D70 (per-type arenas), D75 (KernelState
bundle).

**Must settle:**

- KernelState fields: 5 arenas (Field, Observer, Pulsar, Space, Time) +
  SpaceManager + IRQ routing table. Each wrapped in Lock<T> with D53 ordering.
- Location: static in frame/ (MaybeUninit is unsafe). frame/ exports safe
  accessor fn kernel_state() -> &'static KernelState.
- Access from dispatch: global function call, not CoreState field. Avoids
  inflating CoreState, avoids 'static lifetime in test setup.
- Boot initialization contract: what main.rs provides, what frame/ wraps.

**Reference check:** implementation-plan-layers-1-3.md Tasks 1.1, 3.4.

### D83: Per-core data organization

**Question:** What does each core own locally? What is the assembly-visible
PerCoreData layout?

**Rests on:** D1 (per-core hot path), D46 (core lifecycle), D56 (placement), D74
(register save target).

**Must settle:**

- PerCoreData: #[repr(C)] for assembly. Fields: register_state_ptr (offset 0,
  assembly reads), core_state_ptr (offset 8, Rust reads). Stored in TPIDR_EL1.
- CoreState additions: deadline array (per-core Pulsar deadlines), deadline
  count. Max deadlines per core.
- Relationship: TPIDR_EL1 → PerCoreData → CoreState<S>. One pointer chase from
  assembly-visible struct to generic Rust struct.

**Reference check:** implementation-plan-layers-1-3.md Tasks 1.4, 2.1.

---

## Derivation dependency order

```text
D76 (dispatch entry contract)
 ├──→ D77 (cap resolution) ──→ D78 (message ownership) ──→ D79 (scheduling matrix)
 ├──→ D80 (error/fault delivery)
 └──→ D81 (hardware events)

D82 (global state) ← feeds into D77 (cap resolution needs KernelState)
D83 (per-core data) ← feeds into D81 (deadlines live in CoreState)
```

D76 is the root — everything depends on what crosses the frame/safe boundary.
D82 and D83 are infrastructure derivations that D77 and D81 reference.

Natural order: D76 → D82 → D83 → D77 → D78 → D79 → D80 → D81.

Within each derivation, the outputs are:

1. Journal entry (design/journal/) recording the derivation
2. Test(s) expressing the settled interface (src/lib.rs integration_tests)
3. Any interface adjustments, with diagnosed derivation gap

---

## Phase B: Implement the flows

Once Phase A derivations are stable, implementation follows mechanically. The
implementation-plan-layers-1-3.md tasks map to derivations:

| Plan task              | Implements derivation(s) |
| ---------------------- | ------------------------ |
| 1.1 KernelState struct | D82                      |
| 1.2 cap_table_capacity | D77                      |
| 1.3 IRQ routing table  | D81                      |
| 1.4 Deadline data      | D83                      |
| 1.5 dispatch_ipc       | D76 + D77 + D78 + D79    |
| 1.6 dispatch_typed     | D76 + D77                |
| 1.7 handle_irq         | D81                      |
| 1.8 handle_timer       | D81                      |
| 1.9 Error helpers      | D80                      |

Leaf algorithms (run queue backing structure, deadline heap, routing table
lookup) get the simplest correct implementation. They are behind interfaces and
can be iterated later without affecting the flow.

## Phase C: Hardware integration (plan's Layer 2)

Needs its own derivation pass from the ARM64 Architecture Reference Manual:

- Exception vector layout and register save protocol
- Context switch assembly
- Fast-path register pass-through (D74 mechanical detail)
- EL0 → Rust handler bridge

These derivations are ARM64-specific research, not design decisions. They
produce frame/arch/ code.

## Phase D: Memory management (plan's Layer 3)

Page table derivation from ARM64 VMSAv8-64:

- Translation table format (granule, levels, descriptor bits)
- Per-Observer page table lifecycle
- Space cap → page table mapping/unmapping
- TTBR0/TTBR1 split

## Phase E: Boot sequence

Initialization order derivation:

- KernelState creation from DTB-discovered RAM
- Per-core PerCoreData + CoreState allocation
- First Observer creation and resume

---

## Autonomous work protocol

**At each derivation (D76–D83):**

1. **Research.** Read spec, landscape.md, ARM64 references as needed.
2. **Derive.** Trace the data flow, apply constraints, narrow to answer.
3. **Record.** Journal entry: rests on, settles, rejected alternatives.
4. **Test.** Write integration test(s) that validate the interface.
5. **Check.** Does this require changing a settled interface (D1–D75)?
   - **No** → implement, proceed to next derivation.
   - **Yes, forced** (only one option survives constraints) → record why the
     D-chain missed it, implement, proceed.
   - **Yes, genuine fork** (multiple viable options, tradeoff-dependent) →
     record options, **STOP**, present to user.

**At each implementation (Phase B tasks):**

- Write tests first (from the derivation's interface spec).
- Implement simplest correct version.
- Run scripts/verify.
- Leaf algorithms: simplest correct, iterate later.

**Cross-session state:**

- Journal entries (design/journal/) are the progress tracker.
- Each entry either settles something or records a blocked fork.
- A future session reads the latest journal entries to know the current
  derivation and what's been settled.
- This file is the map; the journal entries are the territory.

**Reference checking:** After each derivation, compare the result against
implementation-plan-layers-1-3.md. If the derivation disagrees with the plan's
engineering decision, investigate: either the derivation found something the
plan missed, or the derivation made an error the plan avoided. The plan is a
check, not an authority.
