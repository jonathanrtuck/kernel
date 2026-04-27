# Error Code Reference

This document is a lookup reference for all error codes returned by kernel
syscalls. The source of truth is `src/syscall.rs` (SyscallError enum and
`error_code()` method); this document presents the same information in tabular
form with cross-references to operations and recovery actions.

## Error Signaling Mechanisms

The kernel uses two distinct error signaling conventions, one per syscall family
(D49, journal 049).

**IPC operations (SVC #1 through #5):** errors are signaled via the ARM64 carry
flag in SPSR_EL1. The kernel modifies the saved SPSR before `eret` -- carry
clear means success, carry set means error. On error, x0 contains the error
code; x1 through x7 are undefined. On success, all eight registers (x0 through
x7) carry normal IPC payload. This is the only convention that preserves all
eight registers for message data (D28). Cost: one BIC or ORR on the saved SPSR
value (~1 cycle).

**Typed operations (SVC #0, operation code in x4):** errors are signaled via
negative x0. If x0 is negative (bit 63 set), it contains the error code. If x0
is non-negative, the operation succeeded and x0 carries the return value (a
cap-table slot index, a timestamp, or zero for void operations). This is
unambiguous because typed operation return values are bounded non-negative
integers.

The two families having different conventions is consistent with the split
interaction model (D7) -- they already have different register semantics.

## Error Code Table

The two syscall families use different numeric encodings for the same error
variants. **Typed operations** use `SyscallError::error_code()` (negative
values, bit 63 set). **IPC operations** use the bare enum discriminant cast to
u64 (`error as u64`, non-negative values) delivered in x0 with the carry flag
set. The variant identity is the same; only the numeric encoding differs.

### Typed operation error codes (negative x0)

| Code | Name                 | Description                                                                                                                                             | Derivation |
| ---- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------- |
| -1   | InvalidCap           | Invalid or empty capability handle. Covers slot-tag mismatch (D11 stale handle to reused slot) -- userspace cannot distinguish ABA from "never existed" | D11        |
| -2   | StaleCap             | Revoked capability (generation mismatch). The object still exists but the caller's access was explicitly revoked                                        | D67        |
| -3   | NoRight              | Insufficient rights for this operation                                                                                                                  | D52        |
| -4   | WrongType            | Wrong object type for this operation                                                                                                                    | D49        |
| -5   | QueueFull            | Field queue is full (IPC send error-to-sender)                                                                                                          | D18        |
| -6   | TableFull            | Observer's cap table is full                                                                                                                            | D8         |
| -7   | AlreadyConsumed      | Send-once cap already consumed                                                                                                                          | D51        |
| -8   | CloneForbidden       | Clone forbidden for linear types (Time)                                                                                                                 | D38        |
| -9   | InvalidState         | Invalid state transition for the Observer, or invalid operation arguments                                                                               | D39        |
| -10  | InvalidProfile       | Invalid scheduling profile (responsiveness + throughput exceeds 128)                                                                                    | D57        |
| -11  | ZeroSize             | Zero-size split requested                                                                                                                               | D60        |
| -12  | InsufficientResource | Insufficient resource for the requested operation (arena full, Space too small, Time units exceeded)                                                    | D31        |
| -13  | NotAdjacent          | Merge requires adjacent virtual address space                                                                                                           | D41        |

### IPC error codes (carry set, x0 = enum discriminant)

IPC errors are signaled via `error as u64` -- the Rust enum discriminant. These
are non-negative values; the carry flag in SPSR_EL1 distinguishes error from
success (not the sign of x0).

| Code | Name                 |
| ---- | -------------------- |
| 0    | InvalidCap           |
| 1    | StaleCap             |
| 2    | NoRight              |
| 3    | WrongType            |
| 4    | QueueFull            |
| 5    | TableFull            |
| 6    | AlreadyConsumed      |
| 7    | CloneForbidden       |
| 8    | InvalidState         |
| 9    | InvalidProfile       |
| 10   | ZeroSize             |
| 11   | InsufficientResource |
| 12   | NotAdjacent          |

In practice, only InvalidCap, StaleCap, NoRight, WrongType, QueueFull, and
AlreadyConsumed can arise from IPC operations. The remaining codes are listed
for completeness since the encoding function does not restrict by operation
class.

## CapError to SyscallError Mapping

Capability resolution (D77) produces CapError variants. The
`From<CapError> for SyscallError` implementation in `src/syscall.rs` translates
these to user-visible error codes. The mapping is injective -- no information is
lost except where two CapError variants produce the same SyscallError by design.

| CapError           | SyscallError    | Typed code | IPC code | Rationale                                                                                               |
| ------------------ | --------------- | ---------- | -------- | ------------------------------------------------------------------------------------------------------- |
| InvalidHandle      | InvalidCap      | -1         | 0        | Handle index out of bounds or slot is empty                                                             |
| SlotTagMismatch    | InvalidCap      | -1         | 0        | D11 ABA defense -- stale handle to reused slot. Same recovery as InvalidHandle (re-acquire through IPC) |
| StaleGeneration    | StaleCap        | -2         | 1        | D67 revocation -- object exists but access was revoked                                                  |
| InsufficientRights | NoRight         | -3         | 2        | D52 rights check failed                                                                                 |
| TypeMismatch       | WrongType       | -4         | 3        | Operation targets the wrong object type                                                                 |
| TableFull          | TableFull       | -6         | 5        | D8 cap table full -- triggers cap-table-full fault (D40)                                                |
| SendOnceConsumed   | AlreadyConsumed | -7         | 6        | D51 send-once cap already used                                                                          |
| CloneForbidden     | CloneForbidden  | -8         | 7        | D38 Time linearity -- cannot clone                                                                      |

## Success Signaling

**IPC operations:** carry flag clear. All eight registers carry the message
payload (x0 through x3 = data words, x4 = label, x5 = badge, x6 = user cap
handle or `u64::MAX`, x7 = reply cap handle or `u64::MAX`). For Send, success
means the message was enqueued or delivered. For Receive, success means a
message was dequeued. For Yield, success is unconditional.

**Typed operations:** x0 contains a non-negative return value. Zero means void
success. Positive values are encoded cap handles (for operations that create or
clone capabilities) or counter values (ClockRead).

## Per-Operation Error Listing

### IPC Operations

#### Send (SVC #1)

| Error      | Code | Condition                                                                         |
| ---------- | ---- | --------------------------------------------------------------------------------- |
| InvalidCap | -1   | Target handle invalid, slot empty, slot-tag mismatch, or Field not found in arena |
| StaleCap   | -2   | Target Field cap's stored generation does not match live generation (D67)         |
| NoRight    | -3   | Cap does not carry SEND right                                                     |
| WrongType  | -4   | Cap targets a non-Field object                                                    |
| QueueFull  | -5   | Field queue full and no receiver waiting (D18)                                    |

On success (carry clear): sender continues execution. The message was either
enqueued or delivered directly to a waiting receiver.

#### Receive (SVC #2)

| Error      | Code | Condition                                                                         |
| ---------- | ---- | --------------------------------------------------------------------------------- |
| InvalidCap | -1   | Target handle invalid, slot empty, slot-tag mismatch, or Field not found in arena |
| StaleCap   | -2   | Target Field cap's stored generation does not match live generation               |
| NoRight    | -3   | Cap does not carry RECEIVE right                                                  |
| WrongType  | -4   | Cap targets a non-Field object                                                    |

On success (carry clear): x0 through x7 carry the received message. If the queue
was empty, the Observer blocks until a message arrives -- the success return
happens after unblocking, not immediately. There is no error for "queue empty";
blocking is the normal path.

#### Call (SVC #3)

| Error      | Code | Condition                                                                                                  |
| ---------- | ---- | ---------------------------------------------------------------------------------------------------------- |
| InvalidCap | -1   | Target handle invalid, slot empty, slot-tag mismatch, Field not found in arena, or user-cap handle invalid |
| StaleCap   | -2   | Target Field cap's stored generation does not match live generation                                        |
| NoRight    | -3   | Cap does not carry SEND right                                                                              |
| WrongType  | -4   | Cap targets a non-Field object                                                                             |
| QueueFull  | -5   | Field queue full and no receiver waiting                                                                   |

On success (carry clear): the caller blocks on its reply Field (slot 1). When
the reply arrives, x0 through x7 carry the reply message. The kernel creates a
send-once reply cap pointing to the caller's reply Field and includes it in the
sent message (D16).

#### ReplyRecv (SVC #4)

ReplyRecv resolves two caps: x5 = reply Field handle (SEND right), x7 = receive
Field handle (RECEIVE right). Either resolution can fail.

| Error      | Code | Condition                                                                                                                          |
| ---------- | ---- | ---------------------------------------------------------------------------------------------------------------------------------- |
| InvalidCap | -1   | Reply handle or receive handle invalid, slot empty, slot-tag mismatch, Field not found, or reply and receive target the same Field |
| StaleCap   | -2   | Either Field cap's stored generation does not match live generation                                                                |
| NoRight    | -3   | Reply cap lacks SEND right, or receive cap lacks RECEIVE right                                                                     |
| WrongType  | -4   | Either cap targets a non-Field object                                                                                              |

On success (carry clear): the reply is sent (to the reply Field), then the
server receives the next message from the receive Field. If the receive queue is
empty, the server blocks. Return format is the same as Receive.

Note: the reply phase never produces QueueFull to the caller -- if the reply
Field queue is full and no client is waiting, the reply is silently dropped. The
receive phase always proceeds regardless of reply outcome.

#### Yield (SVC #5)

Yield takes no arguments and performs no cap resolution.

| Error  | Code | Condition         |
| ------ | ---- | ----------------- |
| (none) |      | Yield never fails |

On success (carry clear, unconditional): all registers are undefined. The
Observer remains Runnable and is rotated to the tail of the scheduler queue.

### Typed Operations

All typed operations signal errors via negative x0 and success via non-negative
x0.

#### ObserverResume (code 0)

| Error        | Code | Condition                                                        |
| ------------ | ---- | ---------------------------------------------------------------- |
| InvalidCap   | -1   | Handle resolution failed, or Observer not found in arena         |
| StaleCap     | -2   | Cap generation mismatch                                          |
| NoRight      | -3   | Cap lacks RESUME right                                           |
| WrongType    | -4   | Cap does not target an Observer                                  |
| InvalidState | -9   | Observer is not in Inert or Faulted state (D39 transition rules) |

On success: x0 = 0. Observer transitions to Runnable and is enqueued in the
scheduler.

#### ObserverInstallCap (code 1)

| Error      | Code | Condition                                                                                  |
| ---------- | ---- | ------------------------------------------------------------------------------------------ |
| InvalidCap | -1   | Target handle, source handle, or Observer arena lookup failed                              |
| StaleCap   | -2   | Target Observer cap generation mismatch, or source cap resolution produced StaleGeneration |
| NoRight    | -3   | Target cap lacks INSTALL_CAP right, or source cap resolution produced InsufficientRights   |
| WrongType  | -4   | Target cap does not target an Observer, or source cap resolution produced TypeMismatch     |
| TableFull  | -6   | Target Observer's cap table is full                                                        |

On success: x0 = encoded handle of the installed cap in the target Observer's
table.

#### ObserverWriteRegisters (code 2)

| Error        | Code | Condition                                                |
| ------------ | ---- | -------------------------------------------------------- |
| InvalidCap   | -1   | Handle resolution failed, or Observer not found in arena |
| StaleCap     | -2   | Cap generation mismatch                                  |
| NoRight      | -3   | Cap lacks WRITE_REGISTERS right                          |
| WrongType    | -4   | Cap does not target an Observer                          |
| InvalidState | -9   | Observer is not in a stopped state (Inert or Faulted)    |

On success: x0 = 0. The target Observer's PC, SP, x0, and PSTATE (masked to
NZCV) are updated from the caller's arguments (D103).

#### ObserverReadRegisters (code 3)

| Error        | Code | Condition                                                |
| ------------ | ---- | -------------------------------------------------------- |
| InvalidCap   | -1   | Handle resolution failed, or Observer not found in arena |
| StaleCap     | -2   | Cap generation mismatch                                  |
| NoRight      | -3   | Cap lacks READ_REGISTERS right                           |
| WrongType    | -4   | Cap does not target an Observer                          |
| InvalidState | -9   | Observer is not in a stopped state (Inert or Faulted)    |

On success: x0 = PC, x1 = SP, x2 = target's x0, x3 = PSTATE (D103).

#### ObserverSuspend (code 4)

| Error      | Code | Condition                                                |
| ---------- | ---- | -------------------------------------------------------- |
| InvalidCap | -1   | Handle resolution failed, or Observer not found in arena |
| StaleCap   | -2   | Cap generation mismatch                                  |
| NoRight    | -3   | Cap lacks SUSPEND right                                  |
| WrongType  | -4   | Cap does not target an Observer                          |

On success: x0 = 0. Suspension overlay is set. Idempotent -- suspending an
already-suspended Observer succeeds.

#### ObserverChangeHandler (code 5)

| Error      | Code | Condition                                                                                    |
| ---------- | ---- | -------------------------------------------------------------------------------------------- |
| InvalidCap | -1   | Target handle, handler Field handle, or Observer arena lookup failed                         |
| StaleCap   | -2   | Target Observer cap generation mismatch, or handler cap resolution produced StaleGeneration  |
| NoRight    | -3   | Target cap lacks CHANGE_HANDLER right, or handler cap resolution produced InsufficientRights |
| WrongType  | -4   | Target cap does not target an Observer, or handler cap does not target a Field               |

On success: x0 = 0. The target Observer's fault handler cap at slot 0 is
replaced with the new handler Field cap.

#### ObserverSetScheduling (code 6)

| Error          | Code | Condition                                                |
| -------------- | ---- | -------------------------------------------------------- |
| InvalidCap     | -1   | Handle resolution failed, or Observer not found in arena |
| StaleCap       | -2   | Cap generation mismatch                                  |
| NoRight        | -3   | Cap lacks MODIFY_SCHEDULING right                        |
| WrongType      | -4   | Cap does not target an Observer                          |
| InvalidProfile | -10  | Responsiveness + throughput exceeds 128 (D57)            |

On success: x0 = 0. Observer's scheduling profile is updated.

#### Destroy (code 7)

| Error      | Code | Condition                                                                                                                      |
| ---------- | ---- | ------------------------------------------------------------------------------------------------------------------------------ |
| InvalidCap | -1   | Handle resolution failed, or object not found in arena                                                                         |
| StaleCap   | -2   | Cap generation mismatch                                                                                                        |
| NoRight    | -3   | Cap lacks DESTROY right                                                                                                        |
| TableFull  | -6   | Caller's cap table has no free slot for the return Space cap (Observer, Field, or Pulsar destroy when backing size is nonzero) |

On success: x0 = encoded handle of the returned Space cap (for Observer, Field,
Pulsar with nonzero backing), or 0 (for Space, Time, or zero-backing objects).
Observer destroy may block the caller during cascade (D98); the return value is
written when the cascade completes.

#### Clone (code 8)

| Error          | Code | Condition                                 |
| -------------- | ---- | ----------------------------------------- |
| InvalidCap     | -1   | Handle resolution failed                  |
| StaleCap       | -2   | Cap generation mismatch (from resolution) |
| NoRight        | -3   | Cap lacks CLONE right                     |
| CloneForbidden | -8   | Target is a Time object (D38 linearity)   |
| TableFull      | -6   | Caller's cap table is full                |

On success: x0 = encoded handle of the new cap entry (duplicate of the original
with identical rights, badge, and generation).

#### Close (code 9)

| Error      | Code | Condition                                       |
| ---------- | ---- | ----------------------------------------------- |
| InvalidCap | -1   | Handle resolution failed, or slot already empty |

On success: x0 = 0. The cap-table slot is freed. No rights check -- Close is
always permitted (D11).

#### Mint (code 10)

| Error      | Code | Condition                                 |
| ---------- | ---- | ----------------------------------------- |
| InvalidCap | -1   | Handle resolution failed                  |
| StaleCap   | -2   | Cap generation mismatch (from resolution) |
| NoRight    | -3   | Cap lacks MINT right                      |
| TableFull  | -6   | Caller's cap table is full                |

On success: x0 = encoded handle of the new attenuated cap. Arguments: x0 =
requested rights mask (intersection with source rights), x1 = badge value (or
`u64::MAX` to keep source badge).

#### SpaceSplit (code 11)

| Error                | Code | Condition                                                          |
| -------------------- | ---- | ------------------------------------------------------------------ |
| InvalidCap           | -1   | Handle resolution failed, or Space not found in arena              |
| StaleCap             | -2   | Cap generation mismatch                                            |
| NoRight              | -3   | Cap lacks SPLIT right                                              |
| WrongType            | -4   | Cap does not target a Space                                        |
| ZeroSize             | -11  | Requested split size rounds to zero pages (D60)                    |
| InsufficientResource | -12  | Requested size exceeds available Space, or arena allocation failed |
| TableFull            | -6   | Caller's cap table is full (rollback: source Space restored)       |

On success: x0 = encoded handle of the new Space cap. The source Space shrinks
by the rounded split amount.

#### SpaceMerge (code 12)

| Error        | Code | Condition                                                                         |
| ------------ | ---- | --------------------------------------------------------------------------------- |
| InvalidCap   | -1   | Target handle, source handle, or arena lookup failed                              |
| StaleCap     | -2   | Target or source cap generation mismatch                                          |
| NoRight      | -3   | Target cap lacks MERGE right, or source cap lacks MERGE right                     |
| WrongType    | -4   | Target cap does not target a Space, or source cap does not target a Space         |
| InvalidState | -9   | Source and target are the same Space                                              |
| NotAdjacent  | -13  | Source Space's virtual address base is not immediately after target's range (D41) |

On success: x0 = 0. The target Space extends to absorb the source Space.

#### CreateField (code 13)

| Error                | Code | Condition                                                                                         |
| -------------------- | ---- | ------------------------------------------------------------------------------------------------- |
| InvalidCap           | -1   | Handle resolution failed, or Space not found in arena                                             |
| StaleCap             | -2   | Space cap generation mismatch                                                                     |
| NoRight              | -3   | Cap lacks SPLIT right                                                                             |
| WrongType            | -4   | Cap does not target a Space                                                                       |
| InsufficientResource | -12  | Space too small for at least one message queue slot, queue allocation failed, or Field arena full |

On success: x0 = 0. The Space cap in the caller's table is replaced in-place
with a Field cap (type conversion, D32). The consumed Space backs the new
Field's message queue.

#### FieldSplit (code 14)

| Error                | Code | Condition                                                                         |
| -------------------- | ---- | --------------------------------------------------------------------------------- |
| InvalidCap           | -1   | Source Field handle, Space argument handle, or arena lookups failed               |
| StaleCap             | -2   | Source Field or Space cap generation mismatch                                     |
| NoRight              | -3   | Source Field cap lacks SPLIT right, or Space argument lacks required rights       |
| WrongType            | -4   | Source cap does not target a Field, or Space argument does not target a Space     |
| InvalidState         | -9   | Badge range low exceeds high                                                      |
| InsufficientResource | -12  | Space too small, queue allocation failed, Field arena full, or routing table full |

On success: x0 = 0. A new sub-Field is created, backed by the consumed Space. A
routing rule is added to the source Field for the specified badge range. The
Space cap slot is replaced with the new Field cap.

#### TimeSplit (code 15)

| Error                | Code | Condition                                                   |
| -------------------- | ---- | ----------------------------------------------------------- |
| InvalidCap           | -1   | Handle resolution failed, or Time not found in arena        |
| StaleCap             | -2   | Cap generation mismatch                                     |
| NoRight              | -3   | Cap lacks SPLIT right                                       |
| WrongType            | -4   | Cap does not target a Time                                  |
| ZeroSize             | -11  | Split amount is zero                                        |
| InsufficientResource | -12  | Amount exceeds available compute units, or Time arena full  |
| TableFull            | -6   | Caller's cap table is full (rollback: source Time restored) |

On success: x0 = encoded handle of the new Time cap. The source Time's compute
units decrease by the split amount.

#### CreatePulsar (code 16)

| Error                | Code | Condition                                                                              |
| -------------------- | ---- | -------------------------------------------------------------------------------------- |
| InvalidCap           | -1   | Space handle, delivery Field handle, or arena lookups failed                           |
| StaleCap             | -2   | Space or delivery Field cap generation mismatch                                        |
| NoRight              | -3   | Space cap lacks SPLIT right                                                            |
| WrongType            | -4   | Target cap does not target a Space, or delivery argument does not target a Field       |
| InsufficientResource | -12  | Per-core deadline array full (32 max), Pulsar arena full, or Space verification failed |

On success: x0 = 0. The Space cap is replaced in-place with a Pulsar cap (type
conversion). Arguments: x0 = delivery Field handle, x1 = badge, x2 = duration in
nanoseconds, x3 = period in nanoseconds (0 = one-shot).

#### ClockRead (code 17)

| Error      | Code | Condition                                 |
| ---------- | ---- | ----------------------------------------- |
| InvalidCap | -1   | Handle resolution failed                  |
| StaleCap   | -2   | Cap generation mismatch (from resolution) |

ClockRead requires no specific right (D66) and accepts any object type as the
target cap. It enables direct EL0 counter access on the calling Observer and
returns the current counter value.

On success: x0 = current CNTVCT_EL0 counter value in ticks.

#### CreateObserver (code 18)

| Error                | Code | Condition                                                                                                                                                                                 |
| -------------------- | ---- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| InvalidCap           | -1   | Space handle, handler Field handle, or arena lookups failed                                                                                                                               |
| StaleCap             | -2   | Space or handler Field cap generation mismatch                                                                                                                                            |
| NoRight              | -3   | Space cap lacks SPLIT right                                                                                                                                                               |
| WrongType            | -4   | Target cap does not target a Space, or handler argument does not target a Field                                                                                                           |
| InsufficientResource | -12  | Space too small for register state + L1 table + minimum cap table, register state allocation failed, cap table allocation failed, Observer arena full, or L1 page table allocation failed |

On success: x0 = 0. The Space cap is replaced in-place with an Observer cap
(type conversion, D32). The new Observer starts in Inert state with the
specified fault handler and a self-cap at slot 2.

#### ResourceRequest (code 19)

ResourceRequest has dual-path dispatch (D104): root Observers allocate directly
from the kernel pool; non-root Observers generate a fault message to their
handler Field.

**Root path (no valid handler at slot 0):**

| Error                | Code | Condition                                                           |
| -------------------- | ---- | ------------------------------------------------------------------- |
| InvalidCap           | -1   | Handle resolution failed, or Space not found in arena               |
| StaleCap             | -2   | Cap generation mismatch (from resolution)                           |
| NoRight              | -3   | Cap lacks DESTROY right                                             |
| InvalidState         | -9   | Requested resource type is not Space (root can only allocate Space) |
| ZeroSize             | -11  | Requested page count is zero                                        |
| InsufficientResource | -12  | Space split failed, or Space arena full                             |
| TableFull            | -6   | Caller's cap table is full                                          |

On success: x0 = encoded handle of the new Space cap.

**Non-root path (valid handler at slot 0):** the kernel constructs a
ResourceRequest fault message (D61) and delivers it to the handler Field. The
calling Observer transitions to Faulted state. No error is returned to the
caller directly -- the handler resolves the fault via ObserverResume.
