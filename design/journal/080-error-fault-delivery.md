# 080 — Error and fault delivery

Date: 2025-04-25

## Starting point

D76 settled the dispatch entry contract: safe dispatch writes syscall results to
RegisterState via frame/ helpers before returning DispatchResult. D61 settled
fault message content and delivery mechanism. D49 settled error signaling
encoding. The question: how do these compose into the complete error and fault
delivery protocol?

Two distinct paths exist:

1. **Syscall error path:** dispatch detects error, writes to RegisterState,
   returns Resume(current).
2. **Fault delivery path:** frame/ classifies exception, dispatch constructs
   FaultType, delivers as IPC to handler Field.

## Syscall error path

Fully constrained by D49 + D76. No new code needed — the protocol is the
composition of existing primitives:

1. dispatch_ipc detects error (invalid cap, queue full, etc.)
2. Calls `frame::cores::write_ipc_error(current, error)` — sets carry flag in
   SPSR_EL1 bit 29, writes error code to gprs[0]
3. Returns `DispatchResult::Resume(current)` — the faulting Observer resumes
   with carry set, x0 = error code

For typed operations:

1. dispatch_typed detects error
2. Calls `frame::cores::write_typed_result(current, negative_value)` — writes
   negative x0 per D49
3. Returns `DispatchResult::Resume(current)`

Key insight: the error path does NOT transition Observer state. The Observer
stays Runnable — it simply resumes with error information in its registers. This
is fundamentally different from the fault path.

## Fault delivery path

Composes D12 + D21 + D61 + D18 + D39:

1. frame/ classifies exception (ESR_EL1 exception class, FAR_EL1)
2. dispatch constructs `FaultType` variant
3. Transition faulting Observer to Faulted state (`observer.fault()`, D39)
4. Read handler cap at reserved slot 0 (D21)
5. Validate handler cap: occupied, Field type, SEND right, generation match
6. Construct fault message via `fault.to_message(handler_badge, observer_cap)`
7. Attempt delivery to handler Field:
   - Receiver waiting: direct delivery (D13)
   - Queue has space: enqueue
   - Queue full: D18 deferred delivery via pending list
8. Return scheduling decision (not Resume(faulting) — the faulting Observer is
   descheduled)

### Observer handle cap construction (D61)

The kernel constructs a TransferredCap with FAULT_OBSERVER rights (5 of 9:
resume, destroy, install_cap, write_registers, read_registers) directly from the
arena. This is NOT minting from the self-cap at slot 2 — the kernel is the
sender and creates authority directly. Same pattern as D16 reply cap
construction.

Three approaches considered:

1. **Mint from self-cap (slot 2).** Attenuate OBSERVER_ALL to FAULT_OBSERVER.
   Unnecessary complexity — the kernel has direct access to the Observer's
   ObjectId and generation. Minting adds a cap-table read + rights computation
   for information the kernel already has.

2. **Kernel constructs directly.** The kernel knows the observer_id and
   observer_generation from the arena. It constructs a TransferredCap with the
   exact FAULT_OBSERVER rights. No cap-table access needed. Clean — the kernel
   IS the authority source for kernel-as-sender messages.

3. **Pre-allocated fault cap.** Store a pre-made fault cap alongside the handler
   entry. Eliminates per-fault construction. Over-optimization for cold path —
   adds struct complexity for negligible gain.

Option 2 chosen. The kernel-as-sender pattern means the kernel IS the authority
source. Constructing directly is honest about what's happening.

### Handler unavailability (D68)

When the handler cap is invalid (empty, wrong type, stale generation), the
delivery function returns `HandlerUnavailable`. The caller (core_manager
dispatch) must then:

- D68 Case A: send supervision notification to the faulting Observer's
  supervisor
- D68 Case C: kernel-autonomous destroy at chain terminus

The `validate_handler_cap` function checks: occupied, Field type, SEND right,
generation match. All four checks are necessary — any failure means the handler
Field is not reachable.

### D18 pending list

When the handler Field queue is full and no receiver is waiting, deliver_fault
returns `Deferred`. The caller links the faulting Observer into the handler
Field's pending_head list using the Observer's wait_state (D43 linkage reuse,
D61). The next receive() that frees a slot drains the pending entry.

## What settles

1. **Syscall error protocol:** dispatch writes error to RegisterState via frame/
   helpers, returns Resume(current). No state transition. Already fully
   constrained by D49 + D76 — this derivation confirms.

2. **Fault delivery protocol:** `deliver_fault` composes FaultType -> Message ->
   enqueue to handler Field. Three outcomes: Delivered, Deferred,
   HandlerUnavailable.

3. **Observer handle cap construction:** kernel constructs TransferredCap
   directly with FAULT_OBSERVER rights. No self-cap minting.

4. **Handler validation:** `validate_handler_cap` checks occupied, Field type,
   SEND right, generation. Returns (field_id, badge) or None.

5. **Pending list path:** deliver_fault returns Deferred when handler Field is
   full. Caller handles linkage.

## Interface additions

New public items in `fault.rs`:

- `FaultDeliveryOutcome` enum (Delivered, Deferred, HandlerUnavailable)
- `make_observer_fault_cap(observer_id, observer_generation) -> TransferredCap`
- `deliver_fault(fault, handler_field, handler_badge, observer_id, observer_generation) -> FaultDeliveryOutcome`
- `validate_handler_cap(handler_entry, handler_field_generation) -> Option<(ObjectId, Badge)>`

No changes to settled interfaces. No new unsafe code. All new code is safe Rust
composing existing primitives.

## Rejected alternatives

**Fault delivery inside core_manager.** The delivery protocol could live in
core_manager.rs as part of the dispatch path. Rejected: the protocol is
independent of per-core state. It composes FaultType + Field + capability
operations. Placing it in fault.rs keeps fault-related logic cohesive and
testable without CoreState<S> generic parameters.

**Single deliver_fault that also does observer.fault() transition.** Considered
having deliver_fault take &mut Observer and call fault() internally. Rejected:
the Observer state transition and the message delivery are separate concerns.
The caller (core_manager) needs to handle both the state transition and the
scheduling decision. Keeping them separate gives the caller explicit control
over the sequence.

**FieldError instead of FaultDeliveryOutcome.** Could reuse
FieldError::QueueFull directly. Rejected: the caller needs to distinguish three
outcomes (delivered, deferred, handler unavailable). FieldError does not capture
handler unavailability. A purpose-built enum is clearer.

## Test

30 tests covering:

- Fault cap rights (5 included, 4 excluded, exact match)
- Delivery to empty field, direct delivery to waiter
- Deferred delivery on full queue
- Full queue + waiter = direct delivery (waiter bypasses queue)
- Zero-capacity field edge cases
- Handler cap validation (valid, empty, wrong type, no SEND, stale generation)
- All four fault type data words
- All four fault types deliver with correct label, Observer cap, no reply cap

## Status

**Settled.** The error and fault delivery protocol is fully constrained by D49 +
D76 + D12 + D21 + D61 + D18 + D39. This derivation confirms the composition and
provides the implementation.
