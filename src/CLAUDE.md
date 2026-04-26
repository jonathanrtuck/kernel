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

All `unsafe` code lives inside `frame/`. The crate-level `#![deny(unsafe_code)]`
with `#[allow(unsafe_code)]` on `mod frame` enforces this at compile time.
Everything outside `frame/` is safe Rust built against the abstractions it
exports.

`frame/arch/` is the hardware half (system registers, MMU, MMIO, exceptions).
`frame/firmware/` parses boot-time data (DTB). Future unsafe additions — page
allocator, arena internals, sync primitives — go in `frame/` as well. The
`scripts/verify` gate checks this boundary.

## Module map

Each module corresponds to a design concept. Derivation references in module doc
comments link to `design/spec.md`.

| Module             | Design concept (graph.d2 name)               | Key derivations                                      |
| ------------------ | -------------------------------------------- | ---------------------------------------------------- |
| `core_manager.rs`  | Per-core hot path (`core-manager`)           | D1, D7, D46, D74, D79, D81, D83, D99, A4             |
| `time_manager/`    | Scheduling + placement (`time-manager`)      | D2, D29, D50, D56, D59                               |
| `space_manager.rs` | Physical memory allocation (`space-manager`) | D3, D31, D32, D70                                    |
| `communication.rs` | IPC orchestration                            | D7, D13, D16, D28, D50, D69                          |
| `arena.rs`         | Per-type slab + generation                   | D53, D67, D70, D75                                   |
| `capability.rs`    | Authority mechanism                          | D4, D8, D11, D17, D51, D52, D57, D58, D67            |
| `space.rs`         | Memory object (Space)                        | D9, D25, D26, D27, D41, D60, D67                     |
| `time.rs`          | Compute allocation (Time)                    | D29, D30, D31, D36–D38, D67                          |
| `field.rs`         | IPC mechanism + message format (Field)       | D13, D15–D18, D28, D45, D54, D67, D71, D73           |
| `observer.rs`      | Execution unit (Observer)                    | D6, D14, D20, D21, D39, D42, D43, D56, D57, D66, D67 |
| `pulsar.rs`        | Timer mechanism (Pulsar)                     | D44, D52, D62, D63, D67, D72                         |
| `kernel_state.rs`  | Global state bundle + IRQ routing            | D75, D81, D82, D99                                   |
| `fault.rs`         | Fault types and delivery                     | D12, D40, D61, D100                                  |
| `syscall.rs`       | Syscall ABI types                            | D47, D48, D49                                        |
| `frame/`           | Framekernel core (all unsafe)                | A1, A2, D75, D83, journal 023                        |
| `frame/arch/`      | Hardware abstraction (incl. PAGE_SIZE)       | D1, D5, D25, D46, D47, D49, D56, D74                 |
| `frame/firmware/`  | Boot-time data parsing                       |                                                      |

## Interface status

Each module now has both **types** (structs, enums) and **method signatures**
(the inter-module interfaces). Method bodies that are `todo!()` represent
settled interfaces whose implementations are the next pass — they are not open
questions. The derivation references in each method's doc comment trace why the
interface exists and why it has that particular shape.

### Per-module interface summary

| Module             | Types defined                                                                                                         | Methods defined (settled by)                                                                                                                                                                                                                                                                   |
| ------------------ | --------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `arena.rs`         | `ObjectId`, `AllocError`, `Arena<T>`                                                                                  | allocate, get, get_mut, free (D53, D70)                                                                                                                                                                                                                                                        |
| `capability.rs`    | `Entry`, `Table`, `Rights`, `Badge`, `Handle`, `SlotTag`, `TransferredCap`, `CapError`, `CloseResult`, `CascadeState` | Entry: is_occupied, check_generation, check_rights, check_type, is_send_once, empty (D8, D11, D51, D67); Table: resolve, resolve_mut, allocate_slot, free_slot, install_at, install, close, begin_cascade, cascade_step (D4, D8, D11, D17, D33)                                                |
| `space.rs`         | `Space`, `SpaceError`                                                                                                 | split, merge, contains_offset, page_count (D41, D60)                                                                                                                                                                                                                                           |
| `time.rs`          | `Time`, `TimeError`                                                                                                   | split, revoke (D38, D67)                                                                                                                                                                                                                                                                       |
| `field.rs`         | `Message`, `RoutingEntry`, `RoutingTable`, `Field`, `FieldError`                                                      | Message: timer_fire, badge_closure (D63, D64); Field: enqueue, dequeue, is_empty, is_full, add_waiter, remove_waiter, pop_waiter, resolve_route, add_route, revoke (D13, D18, D45, D54, D67)                                                                                                   |
| `observer.rs`      | `Observer`, `PrimaryState`, `WaitState`, `WaitEntry`, `ObserverError`                                                 | validate_profile, precision, resume, suspend, block, unblock, fault, set_scheduling, add_compute, remove_compute, revoke (D14, D39, D42, D57, D67); PrimaryState: is_stopped                                                                                                                   |
| `pulsar.rs`        | `Pulsar`                                                                                                              | new, is_repeating, fire_message, rearm, record_overrun, revoke (D44, D62, D63, D67, D72)                                                                                                                                                                                                       |
| `time_manager/`    | `CoreId`, `CoreSnapshot`, `PlacementDecision`                                                                         | traits: Scheduler (5 methods), Placement (1 method) (D2, D50, D56, D59); RoundRobin: new (D59); ScoredPlacement: new (D56)                                                                                                                                                                     |
| `core_manager.rs`  | `CoreState<S>`, `DispatchResult`, `DeadlineEntry`, `MAX_DEADLINES_PER_CORE`                                           | dispatch_ipc(IpcOperation, &KernelState), dispatch_typed(TypedOperation), handle_timer, handle_irq, schedule_next (D1, D7, D22, D46, D79, D83); dispatch_send_outcome, dispatch_receive_outcome, dispatch_call_outcome, dispatch_reply_recv_outcome (D79); current_core[_mut] (bare-metal, D1) |
| `space_manager.rs` | `RootPool`, `VaAssignment`, `SpaceManager`                                                                            | allocate_pages, return_pages, assign_va, type_conversion_overhead (D3, D31, D32, D70)                                                                                                                                                                                                          |
| `communication.rs` | `SendOutcome`, `ReceiveOutcome`, `CallOutcome`, `ReplyRecvOutcome`, `ReplyDelivery`                                   | send, receive, call, reply_recv, yield_cpu (D7, D13, D16, D50, D78)                                                                                                                                                                                                                            |
| `fault.rs`         | `FaultType`, `AccessType`, `ResourceType`, `FaultDeliveryOutcome`                                                     | label, data_words, to_message (D12, D61); make_observer_fault_cap, deliver_fault, validate_handler_cap (D80, D100)                                                                                                                                                                             |
| `syscall.rs`       | `IpcOperation`, `TypedOperation`, `IpcRegisters`, `TypedRegisters`, `SyscallError`                                    | IpcOperation: from_svc, is_fast_path_eligible (D47, D49, D50); TypedOperation: from_code, target_type (D49)                                                                                                                                                                                    |

### Not yet represented (identified gaps)

These are implementation-level concerns that do not yet have settled interfaces.
Documented here rather than guessed at in code:

- ~~**Destroy cascade driver.**~~ Settled: preemptible cascade via
  `CascadeContinuation` in `CoreState`, driven by `continue_cascade()` in
  `handle_timer`. Destroyer is blocked while cascade runs; unblocked with return
  Space cap on completion.
- **Boot/init sequence.** Root Observer creation, initial Space/Time pool setup,
  per-core scheduler initialization. Partially unsettled (D31, D46).
- **Badge tracking map.** D17 opt-in per-badge refcount tracking on Fields. The
  `badge_tracking: bool` flag exists; the internal map data structure is
  deferred.
- **Cross-core scheduling infrastructure.** D56 idle bitmap, mailboxes, IPI
  send/receive handlers. The `Placement` trait defines the interface; the
  backing data structures are deferred.
- ~~**CoreState arena references.**~~ Settled by D75: arenas live in a global
  `KernelState` struct (not in CoreState). Lock<T> refactored to own data
  (UnsafeCell). Cold-path dispatch accesses arenas through the global.
- ~~**Observer cap table capacity.**~~ Settled: `Observer::cap_table_capacity`
  field added. Dispatch path uses it for bounds-checking handle resolution.
- ~~**IRQ-to-Field routing.**~~ Settled by D81: `IrqRoute` and `IrqRoutingTable`
  in `kernel_state.rs`, `irq_routes: Lock<IrqRoutingTable>` in KernelState.
  Direct-indexed by INTID, max 1024. `handle_irq` looks up route, checks
  generation, constructs `Message::device_irq`, enqueues to target Field.
- ~~**Pulsar deadline queue.**~~ Settled by D83: `DeadlineEntry` array
  (32-element hard cap) and `deadline_count` added to `CoreState<S>`. Per-core,
  no lock needed.
- ~~**WriteRegisters/ReadRegisters.**~~ Settled by D103: inline in syscall args
  (PC, SP, x0, PSTATE masked to NZCV). Full buffer transfer deferred.
- ~~**ResourceRequest.**~~ Settled by D104: dual-path dispatch. Non-root
  fault-routes to handler Field. Root allocates from SpaceManager pool.

## Spec drives code

Open questions in `design/spec.md` are represented as absent code, not as
guesses or `todo!()` placeholders. When a derivation settles something, the
corresponding module gains the concrete type or field. Code never quietly
settles something the spec left open.
