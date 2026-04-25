# D76 — Dispatch entry contract

**Question:** What data crosses the frame/ → safe-code boundary at exception
entry? What does safe code return to frame/?

**Rests on:** D1 (per-core hot path), D7 (IPC vs typed split), D47 (register
layout), D49 (error signaling encoding), D50 (fast-path conditions), D74
(register save on EL0), ARM64 exception model.

---

## Settles

### Register access model: Pull

Safe dispatch reads registers from RegisterState via frame/ helpers when needed.
Registers are already in RegisterState — saved by EL0 exception entry assembly
(D74) before any Rust code runs. Reading is not an extra step; it reads from the
data's natural location.

Three constraints force pull over push:

1. **D47:** IPC dispatches from ESR_EL1 alone, before reading any GPR. Push
   would read GPRs before the operation is known.
2. **D48:** Yield needs zero GPRs. Push wastes a struct construction.
3. **D50:** Fast-path x0–x3 pass through in physical registers. The kernel never
   reads x0–x3 from the sender on the fast path. Push reads them anyway.

### DispatchResult

```rust
pub enum DispatchResult {
    Resume(NonNull<Observer>),
    ResumeFastPath(NonNull<Observer>),
    Idle,
}
```

`ResumeFastPath` tells the restore assembly to skip loading x0–x3 from
RegisterState. The dispatch layer evaluates all D50 conditions and determines
fast-path applicability before returning.

Safe dispatch writes syscall results (error codes, message data) to
RegisterState via frame/ helpers _before_ returning DispatchResult. The result
carries only the scheduling decision. frame/ gets a uniform restore path: load
registers from the indicated Observer's RegisterState, eret.

### frame/ → safe code calling convention

| Exception source    | frame/ calls               | Parameters                      |
| ------------------- | -------------------------- | ------------------------------- |
| SVC #1–#5           | `core.dispatch_ipc(op)`    | `IpcOperation` (from ESR[15:0]) |
| SVC #0              | `core.dispatch_typed(op)`  | `TypedOperation` (from gprs[4]) |
| Invalid SVC / abort | fault path (D80)           | never reaches safe dispatch     |
| IRQ (timer)         | `core.handle_timer(ticks)` | `u64` (timer counter snapshot)  |
| IRQ (device)        | `core.handle_irq(intid)`   | `u32` (GIC INTID)               |

Invalid SVC numbers and invalid typed op codes are faults. frame/ classifies
them and takes the fault path (D80) — safe dispatch only receives valid, decoded
operations.

### frame/ helper interfaces

**Read (pull from RegisterState):**

- `read_ipc_registers(NonNull<Observer>) -> IpcRegisters` — x0–x7
- `read_typed_registers(NonNull<Observer>) -> TypedRegisters` — x0–x5

**Write (push results to RegisterState):**

- `write_ipc_error(NonNull<Observer>, SyscallError)` — set carry in pstate bit
  29, write error code to gprs[0]
- `clear_ipc_carry(NonNull<Observer>)` — clear carry (IPC success)
- `write_typed_result(NonNull<Observer>, u64)` — write gprs[0]; negative values
  are errors per D49
- `write_message_to_registers(NonNull<Observer>, &Message)` — write x0–x7 from
  message (slow-path receive delivery)
- `write_metadata_to_registers(NonNull<Observer>, u64, Badge, u64, u64)` — write
  x4–x7 only: label, badge, user_cap, reply_cap (fast-path receive)

### handle_timer parameter

`handle_timer(&mut self, current_ticks: u64)` — frame/ reads the timer counter
once and passes it as a consistent snapshot. The counter is volatile hardware
state (changes every cycle); unlike saved GPRs (stable in RegisterState), it
must be pushed.

---

## Rejected alternatives

**Push model (registers as parameters):** el0_exception_handler reads registers
and passes them to dispatch. Reads GPRs before knowing which are needed — wastes
work for Yield and fast-path. Forces the exception handler to understand
operation-specific register semantics (it should just forward the decoded
operation). Rejected on D47 (dispatch from ESR alone) and D50 (fast-path avoids
x0–x3 read).

**Error info in DispatchResult:** dispatch returns error, frame/ writes it.
Splits error-writing responsibility between two modules. Safe code knows the
error type; it should complete the error path before returning. Keeping
DispatchResult simple (just the scheduling decision) gives frame/ a uniform
restore path with no error-specific branching.

**Single Resume variant with fast_path: bool:** Each variant maps to a distinct
restore assembly path. Separate variants make the match exhaustive without
conditional logic. Aligns with explicit-state-transition style (Verus readiness,
src/CLAUDE.md).

**Push for handle_timer's ticks vs pull:** Pull (frame/ helper) would require
dispatch to call into frame/ for a volatile counter. Multiple reads give
inconsistent snapshots. The pushed parameter is a single consistent value — pure
and testable.

---

## D-chain gaps diagnosed

Three interface adjustments required. All are forced (single option survives
constraints). The D-chain missed them because D1–D75 settled _what_ objects
exist and what interfaces they expose, not _how data flows between them at
runtime_.

1. **ResumeFastPath variant:** D50 (fast-path conditions) and D74 (x0–x3
   pass-through) were settled separately. Neither derived the restore-side
   consequence: the dispatch result must distinguish "skip x0–x3 load."

2. **handle_timer(current_ticks):** The implementation plan decided this (Task
   1.8) but the derivation chain didn't trace it. Forced by: volatile counter,
   single-snapshot consistency, testability.

3. **Write helpers:** D49 settled error signaling encoding (carry flag for IPC,
   negative x0 for typed). The frame/ helpers that _implement_ the encoding were
   not derived because D49 focused on the ABI, not the kernel-internal bridge.

---

## Test

- `dispatch_ipc(Yield)` returns without accessing IPC registers (pull model:
  only accessed when needed).
- `DispatchResult::ResumeFastPath` is structurally distinct from `Resume` in
  pattern matching.
- Write helpers correctly modify RegisterState: carry flag set/clear in pstate,
  error codes in gprs[0], message data in gprs[0–7].
- `handle_timer(current_ticks)` receives the value passed by frame/ (pure
  function over a snapshot).
