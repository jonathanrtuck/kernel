# Syscall Reference

25 operations: 5 IPC + 20 typed. This is the complete kernel-userspace
interface. The code of record is `src/syscall.rs`, `src/core_manager.rs`, and
the per-type modules; this document is a derived summary for userspace
programmers.

## Two-level syscall structure

The kernel uses ARM64 `SVC #imm16` as the trap mechanism (D47). The 16-bit
immediate in the instruction determines the syscall family:

- **Nonzero immediate (SVC #1 through SVC #5):** IPC operations. The kernel
  dispatches from ESR_EL1 alone, before reading any general-purpose register.
  Registers x0-x7 carry the IPC message per D28.

- **Zero immediate (SVC #0):** Typed kernel operations. Register x4 carries the
  operation code (0-19), x5 carries the target capability handle, and x0-x3
  carry operation-specific arguments.

The two families have different error signaling conventions (D49):

- **IPC:** Errors signaled via the ARM64 carry flag in SPSR_EL1. Carry clear =
  success; carry set = error with x0 = error code.

- **Typed:** Errors signaled via negative x0 (bit 63 set). Non-negative x0 =
  success or return value.

The cap-absent sentinel is `0xFFFF_FFFF_FFFF_FFFF` (u64::MAX) in x6 and x7 for
both send and receive sides (D49).

## Error codes

All error codes are ABI-stable negative integers for typed operations, and
positive integers delivered through x0 (with carry set) for IPC operations.

| Code | Name                 | Description                                              |
| ---- | -------------------- | -------------------------------------------------------- |
| -1   | InvalidCap           | Invalid, empty, or slot-tag-mismatched capability handle |
| -2   | StaleCap             | Revoked capability (generation mismatch)                 |
| -3   | NoRight              | Insufficient rights for this operation                   |
| -4   | WrongType            | Wrong object type for this operation                     |
| -5   | QueueFull            | Field queue is full (IPC Send/Call error-to-sender)      |
| -6   | TableFull            | Observer's capability table is full                      |
| -7   | AlreadyConsumed      | Send-once capability already used                        |
| -8   | CloneForbidden       | Clone forbidden for linear types (Time)                  |
| -9   | InvalidState         | Invalid state transition for the target Observer         |
| -10  | InvalidProfile       | Scheduling profile invalid (R + T > 128)                 |
| -11  | ZeroSize             | Zero-size split or allocation                            |
| -12  | InsufficientResource | Insufficient resource for the requested operation        |
| -13  | NotAdjacent          | Merge requires adjacent virtual address ranges           |

---

## IPC operations

Five operations, each with its own SVC immediate. All IPC register layouts
follow D28: x0-x3 = four data words, x4 = label, x5 = handle or badge, x6 = user
capability handle, x7 = reply info or flags. The fast-path optimization (D50,
D74) passes x0-x3 through physical registers without save/restore on direct
switch.

### Send (SVC #1)

Non-blocking deposit of a message into a Field queue. D13, D17, D18.

**Entry registers:**

| Register | Content                                    |
| -------- | ------------------------------------------ |
| x0-x3    | Data words (4 arbitrary u64 values)        |
| x4       | Label                                      |
| x5       | Target Field capability handle             |
| x6       | User capability handle (u64::MAX = no cap) |
| x7       | Reserved (flags)                           |

**Return registers (success, carry clear):**

No output registers. Sender continues immediately.

**Return registers (error, carry set):**

| Register | Content    |
| -------- | ---------- |
| x0       | Error code |

**Possible errors:** InvalidCap, StaleCap, NoRight, WrongType, QueueFull,
AlreadyConsumed.

**Notes:** The kernel injects the badge from the caller's capability entry into
the message (D17). If the capability is send-once (D51), it is consumed after
successful delivery. Send never blocks.

### Receive (SVC #2)

Blocking wait for a message on a Field. D13.

**Entry registers:**

| Register | Content                        |
| -------- | ------------------------------ |
| x5       | Target Field capability handle |

All other entry registers are undefined (not read by the kernel).

**Return registers (success, carry clear):**

| Register | Content                                           |
| -------- | ------------------------------------------------- |
| x0-x3    | Data words                                        |
| x4       | Label                                             |
| x5       | Badge (from sender's capability entry)            |
| x6       | User capability handle (u64::MAX = no cap)        |
| x7       | Reply capability handle (u64::MAX = no reply cap) |

**Return registers (error, carry set):**

| Register | Content    |
| -------- | ---------- |
| x0       | Error code |

**Possible errors:** InvalidCap, StaleCap, NoRight, WrongType.

**Notes:** Blocks until a message is available (D13). If the queue has messages,
dequeues the front message (FIFO) and returns immediately. If the queue is
empty, the Observer transitions to Blocked (D39) and is linked into the Field's
waiters list.

### Call (SVC #3)

Compound send + block on reply. D16, D50, D65.

**Entry registers:**

| Register | Content                                          |
| -------- | ------------------------------------------------ |
| x0-x3    | Data words                                       |
| x4       | Label                                            |
| x5       | Target Field capability handle                   |
| x6       | User capability handle (u64::MAX = no cap)       |
| x7       | Reply badge (D65: embedded in the send-once cap) |

**Return registers (success, carry clear):**

Same layout as Receive return: x0-x3 = reply data, x4 = reply label, x5 = reply
badge, x6 = reply user cap (u64::MAX if none), x7 = u64::MAX (no reply-to-reply
cap).

**Return registers (error, carry set):**

| Register | Content    |
| -------- | ---------- |
| x0       | Error code |

**Possible errors:** InvalidCap, StaleCap, NoRight, WrongType, QueueFull,
AlreadyConsumed.

**Notes:** The kernel creates a send-once reply capability pointing to the
caller's reply Field (cap-table reserved slot 1, D43) and includes it in the
outgoing message. The reply badge from x7 is embedded in that send-once
capability (D65). The caller blocks on its reply Field until the server replies.
Eligible for the direct-switch fast path when the message carries no user
capability and a receiver is waiting (D50).

### ReplyRecv (SVC #4)

Atomic reply + receive next request. D16.

**Entry registers:**

| Register | Content                                                  |
| -------- | -------------------------------------------------------- |
| x0-x3    | Reply data words                                         |
| x4       | Reply label                                              |
| x5       | Reply Field capability handle (send-once, from prior x7) |
| x6       | User capability handle for reply (u64::MAX = no cap)     |
| x7       | Receive Field capability handle (next receive target)    |

**Return registers (success, carry clear):**

Same layout as Receive return.

**Return registers (error, carry set):**

| Register | Content    |
| -------- | ---------- |
| x0       | Error code |

**Possible errors:** InvalidCap, StaleCap, NoRight, WrongType.

**Notes:** Server fast path. Sends the reply to the reply Field (x5, typically
the send-once cap from the previous Receive's x7), then receives the next
message from the receive Field (x7). Atomic: no scheduling gap between reply
delivery and next-request pickup. The reply Field and receive Field must be
different objects.

### Yield (SVC #5)

Voluntary CPU relinquishment. D48.

**Entry registers:** None. All registers are undefined.

**Return registers:** Carry clear (always succeeds). All registers undefined.

**Possible errors:** None.

**Notes:** Scheduling hint. The yielding Observer remains Runnable (D39) and is
rotated to the tail of the run queue. The kernel calls `scheduler.pick_next()`
to select the next Observer. Does not target any capability.

---

## Typed kernel operations

All typed operations use SVC #0. Common register layout on entry: x4 = operation
code, x5 = target capability handle. Arguments in x0-x3 are operation-specific.
Return in x0 (non-negative = success, negative = error).

The kernel resolves the target capability handle (D77), checks the object type,
and verifies the required right before dispatching to per-operation logic (D52).

### Observer operations

Operations on Observer objects. Target must be an Observer capability.

#### ObserverResume (opcode 0)

Transition a stopped Observer to Runnable. D14, D35, D39.

| Register | Direction | Content                    |
| -------- | --------- | -------------------------- |
| x5       | in        | Observer capability handle |
| x4       | in        | 0 (opcode)                 |
| x0       | out       | 0 on success               |

**Required right:** RESUME. **Possible errors:** InvalidCap, StaleCap, NoRight,
WrongType, InvalidState.

Valid from Inert (first start) or Faulted (after handler resolves). Also clears
the suspension flag. InvalidState if the Observer is Runnable or Blocked.

#### ObserverInstallCap (opcode 1)

Install a capability into a target Observer's capability table. D97.

| Register | Direction | Content                                      |
| -------- | --------- | -------------------------------------------- |
| x0       | in        | Source capability handle (in caller's table) |
| x5       | in        | Target Observer capability handle            |
| x4       | in        | 1 (opcode)                                   |
| x0       | out       | New handle in target's table, or error       |

**Required right:** INSTALL_CAP. **Possible errors:** InvalidCap, StaleCap,
NoRight, WrongType, TableFull.

The source capability is copied (not moved) into the target Observer's table.
Returns the encoded handle in the target's table on success.

#### ObserverWriteRegisters (opcode 2)

Write registers into a stopped Observer's saved state. D103.

| Register | Direction | Content                           |
| -------- | --------- | --------------------------------- |
| x0       | in        | New PC value                      |
| x1       | in        | New SP value                      |
| x2       | in        | New x0 value                      |
| x3       | in        | New PSTATE (masked to NZCV only)  |
| x5       | in        | Target Observer capability handle |
| x4       | in        | 2 (opcode)                        |
| x0       | out       | 0 on success                      |

**Required right:** WRITE_REGISTERS. **Possible errors:** InvalidCap, StaleCap,
NoRight, WrongType, InvalidState.

Inline transfer of four register values. PSTATE is masked to bits 31:28 (N, Z,
C, V condition flags only); all other PSTATE bits are cleared for security
(prevents EL escalation via SPSR_EL1.M). Target must be in a stopped state
(Inert or Faulted). All other registers in the target's saved state are
unchanged.

#### ObserverReadRegisters (opcode 3)

Read registers from a stopped Observer's saved state. D103.

| Register | Direction | Content                           |
| -------- | --------- | --------------------------------- |
| x5       | in        | Target Observer capability handle |
| x4       | in        | 3 (opcode)                        |
| x0       | out       | Target's PC                       |
| x1       | out       | Target's SP                       |
| x2       | out       | Target's x0                       |
| x3       | out       | Target's PSTATE (NZCV bits only)  |

**Required right:** READ_REGISTERS. **Possible errors:** InvalidCap, StaleCap,
NoRight, WrongType, InvalidState.

Returns four register values inline. Target must be in a stopped state (Inert or
Faulted). On success, x0 contains the target's PC (not a zero-for-success
value), which is a non-negative integer; this is the one typed operation where
x0 on success is not zero.

#### ObserverSuspend (opcode 4)

Set the external suspension overlay on an Observer. D39.

| Register | Direction | Content                    |
| -------- | --------- | -------------------------- |
| x5       | in        | Observer capability handle |
| x4       | in        | 4 (opcode)                 |
| x0       | out       | 0 on success               |

**Required right:** SUSPEND. **Possible errors:** InvalidCap, StaleCap, NoRight,
WrongType.

Suspension is orthogonal to the primary state (D39). The Observer is removed
from the run queue if Runnable; if Blocked or Faulted, the suspension overlay
co-occurs. Resume clears the suspension. Always succeeds if the capability is
valid.

#### ObserverChangeHandler (opcode 5)

Replace the fault handler Field in a target Observer's capability table. D97.

| Register | Direction | Content                                        |
| -------- | --------- | ---------------------------------------------- |
| x0       | in        | New handler Field capability handle (caller's) |
| x1       | in        | Handler badge value                            |
| x5       | in        | Target Observer capability handle              |
| x4       | in        | 5 (opcode)                                     |
| x0       | out       | 0 on success                                   |

**Required right:** CHANGE_HANDLER. **Possible errors:** InvalidCap, StaleCap,
NoRight, WrongType.

Overwrites the fault handler entry at reserved slot 0 (D21) in the target
Observer's capability table. The new handler must be a Field capability. The
handler badge is embedded in the new entry and delivered in fault messages
(D17).

#### ObserverSetScheduling (opcode 6)

Update the three-value scheduling profile. D42, D57.

| Register | Direction | Content                    |
| -------- | --------- | -------------------------- |
| x0       | in        | Responsiveness (u8, 0-128) |
| x1       | in        | Throughput (u8, 0-128)     |
| x5       | in        | Observer capability handle |
| x4       | in        | 6 (opcode)                 |
| x0       | out       | 0 on success               |

**Required right:** MODIFY_SCHEDULING. **Possible errors:** InvalidCap,
StaleCap, NoRight, WrongType, InvalidProfile.

Budget = 128. Precision is derived: P = 128 - R - T. InvalidProfile if R +
T > 128. The per-core scheduler reads the new values on the next scheduling
decision.

### Generic capability operations

Cross-type operations. Target can be any object type.

#### Destroy (opcode 7)

Authoritative destruction of a kernel object. D11, D33, D98.

| Register | Direction | Content                   |
| -------- | --------- | ------------------------- |
| x5       | in        | Target capability handle  |
| x4       | in        | 7 (opcode)                |
| x0       | out       | Return Space handle, or 0 |

**Required right:** DESTROY. **Possible errors:** InvalidCap, StaleCap, NoRight,
TableFull.

Behavior is type-specific:

- **Observer:** Revokes all capabilities, runs a preemptible cascade (D33) to
  close all entries in the target's capability table, frees the arena slot, and
  returns a Space capability for the structural backing to the caller. TableFull
  if the caller has no free slot for the return Space.
- **Field:** Revokes, frees the arena slot, returns the backing Space.
- **Pulsar:** Removes from the per-core deadline array, revokes, frees, returns
  the backing Space.
- **Space:** Revokes, returns pages to the kernel's root pool, frees the arena
  slot. Returns 0 (no Space returned).
- **Time:** Revokes, frees the arena slot. Returns 0.

#### Clone (opcode 8)

Duplicate a capability with equal or reduced rights. D23.

| Register | Direction | Content                                |
| -------- | --------- | -------------------------------------- |
| x5       | in        | Source capability handle               |
| x4       | in        | 8 (opcode)                             |
| x0       | out       | New handle (encoded) in caller's table |

**Required right:** CLONE. **Possible errors:** InvalidCap, StaleCap, NoRight,
WrongType, CloneForbidden, TableFull.

Creates a duplicate capability entry in the caller's own table with the same
rights, badge, and generation. CloneForbidden for Time capabilities (D38:
linear, conservation invariant). No generation check on the object; the cloned
entry inherits the stored generation and is validated when used.

#### Close (opcode 9)

Relinquish a capability. D11, D107.

| Register | Direction | Content                  |
| -------- | --------- | ------------------------ |
| x5       | in        | Target capability handle |
| x4       | in        | 9 (opcode)               |
| x0       | out       | 0 on success             |

**Required right:** None (always permitted). **Possible errors:** InvalidCap.

Frees the capability table slot and decrements the object's reference count. If
the refcount reaches zero, the kernel auto-destroys the object inline (D107):
structural backing returns to the root Space (D31), not to the closer. For
Observers, auto-destroy triggers a preemptible cascade (D33). Users wanting the
backing Space returned to them should call Destroy (code 7) instead. InvalidCap
if the slot is already empty.

#### Mint (opcode 10)

Create an attenuated, optionally badged copy of a capability. D17.

| Register | Direction | Content                                    |
| -------- | --------- | ------------------------------------------ |
| x0       | in        | Requested rights mask (u16)                |
| x1       | in        | Badge value (u64::MAX = keep source badge) |
| x5       | in        | Source capability handle                   |
| x4       | in        | 10 (opcode)                                |
| x0       | out       | New handle (encoded) in caller's table     |

**Required right:** MINT. **Possible errors:** InvalidCap, StaleCap, NoRight,
WrongType, TableFull.

The new capability's rights are the intersection of the source rights and the
requested mask (attenuation only, never escalation). If x1 = u64::MAX, the
source badge is preserved; otherwise, the badge is set to x1. Primarily used to
create badged Field send capabilities for sender identification (D17).

### Space operations

Operations on Space memory objects. Target must be a Space capability.

#### SpaceSplit (opcode 11)

Extract a portion of a Space into a new Space object. D41, D60.

| Register | Direction | Content                                      |
| -------- | --------- | -------------------------------------------- |
| x0       | in        | Size in bytes (rounded up to page size)      |
| x5       | in        | Source Space capability handle               |
| x4       | in        | 11 (opcode)                                  |
| x0       | out       | New Space handle (encoded) in caller's table |

**Required right:** SPLIT. **Possible errors:** InvalidCap, StaleCap, NoRight,
WrongType, ZeroSize, InsufficientResource, TableFull.

The source Space shrinks by the rounded size. The new Space receives a
kernel-assigned virtual address base at the high end of the source range (D26).
ZeroSize if the rounded size is zero. InsufficientResource if the requested size
is greater than or equal to the source size (a Space cannot be emptied by split;
use Destroy). Conservation: total page count unchanged (D32).

#### SpaceMerge (opcode 12)

Absorb a source Space into the target Space. D41.

| Register | Direction | Content                                       |
| -------- | --------- | --------------------------------------------- |
| x0       | in        | Source Space capability handle (to be merged) |
| x5       | in        | Target Space capability handle (absorber)     |
| x4       | in        | 12 (opcode)                                   |
| x0       | out       | 0 on success                                  |

**Required right:** MERGE (on target). MERGE (on source, checked separately).
**Possible errors:** InvalidCap, StaleCap, NoRight, WrongType, NotAdjacent,
InvalidState.

The source Space is absorbed into the target. The target's virtual address range
extends upward. The source ceases to exist as an independent object. NotAdjacent
if the source's virtual address base does not immediately follow the target's
range. InvalidState if target and source are the same object.

### Field operations

Operations that create Fields or install routing rules.

#### CreateField (opcode 13)

Create a new Field by consuming a Space (type conversion). D32, D45.

| Register | Direction | Content                            |
| -------- | --------- | ---------------------------------- |
| x5       | in        | Space capability handle (consumed) |
| x4       | in        | 13 (opcode)                        |
| x0       | out       | 0 on success                       |

**Required right:** SPLIT (on the Space capability). **Possible errors:**
InvalidCap, StaleCap, NoRight, WrongType, InsufficientResource.

The Space is consumed and replaced in-place: the capability slot that held the
Space capability now holds the new Field capability with full Field rights. The
queue capacity is determined by the consumed Space's size divided by the message
struct size. InsufficientResource if the Space is too small for at least one
message slot.

#### FieldSplit (opcode 14)

Install a badge-range routing rule on a Field and create a sub-Field. D45, D54.

| Register | Direction | Content                                          |
| -------- | --------- | ------------------------------------------------ |
| x0       | in        | Space capability handle (consumed for sub-Field) |
| x1       | in        | Badge range low (inclusive)                      |
| x2       | in        | Badge range high (inclusive)                     |
| x5       | in        | Source Field capability handle                   |
| x4       | in        | 14 (opcode)                                      |
| x0       | out       | 0 on success                                     |

**Required right:** SPLIT (on the Field capability). **Possible errors:**
InvalidCap, StaleCap, NoRight, WrongType, InsufficientResource, InvalidState.

Creates a new sub-Field backed by the consumed Space and installs a routing rule
on the source Field: messages with badges in [low, high] (D71 closed range) are
routed to the new sub-Field. Senders are oblivious to the routing. The Space
capability slot is replaced with the new Field capability. InvalidState if
badge_low > badge_high.

### Time operations

Operations on Time compute-allocation objects. Target must be a Time capability.

#### TimeSplit (opcode 15)

Split compute units from a Time object into a new Time object. D38.

| Register | Direction | Content                                     |
| -------- | --------- | ------------------------------------------- |
| x0       | in        | Amount of compute units to split off        |
| x5       | in        | Source Time capability handle               |
| x4       | in        | 15 (opcode)                                 |
| x0       | out       | New Time handle (encoded) in caller's table |

**Required right:** SPLIT. **Possible errors:** InvalidCap, StaleCap, NoRight,
WrongType, ZeroSize, InsufficientResource, TableFull.

The source Time shrinks by the requested amount. A new Time object is created
with that amount. ZeroSize if amount is zero. InsufficientResource if amount
exceeds the source's compute units. Conservation: total compute units unchanged
(D36). Time is linear (D38): Clone is forbidden, so Split is the only way to
delegate compute authority.

### Pulsar operations

Operations on Pulsar timer objects.

#### CreatePulsar (opcode 16)

Create a new Pulsar timer by consuming a Space. D44, D62, D72.

| Register | Direction | Content                                   |
| -------- | --------- | ----------------------------------------- |
| x0       | in        | Delivery Field capability handle          |
| x1       | in        | Badge value (injected into fire messages) |
| x2       | in        | Duration in nanoseconds (relative)        |
| x3       | in        | Period in nanoseconds (0 = one-shot)      |
| x5       | in        | Space capability handle (consumed)        |
| x4       | in        | 16 (opcode)                               |
| x0       | out       | 0 on success                              |

**Required right:** SPLIT (on the Space capability). **Possible errors:**
InvalidCap, StaleCap, NoRight, WrongType, InsufficientResource.

Armed on creation (D62). No separate arm, configure, or modify operation. Cancel
= Destroy. Modify = Destroy + CreatePulsar. The Space is consumed and its slot
is replaced with the Pulsar capability. Duration is converted from nanoseconds
to counter ticks by the kernel (D72, A5). Period = 0 creates a one-shot timer;
period > 0 creates a repeating timer with drift-compensated re-arm (D44).
InsufficientResource if the per-core deadline array is full (maximum 32 per
core).

Fire messages are delivered to the specified Field with the format: badge +
LABEL_TIMER_FIRE + data[0] = fire time (raw CNTVCT_EL0 ticks) + data[1] =
overrun count (D63).

#### ClockRead (opcode 17)

Read the current counter value and enable direct EL0 counter access. D66.

| Register | Direction | Content                                  |
| -------- | --------- | ---------------------------------------- |
| x5       | in        | Any valid capability handle              |
| x4       | in        | 17 (opcode)                              |
| x0       | out       | Current counter ticks (CNTVCT_EL0 value) |

**Required right:** None. **Possible errors:** InvalidCap, StaleCap.

Returns the current hardware counter value. As a side effect, enables direct EL0
counter access for the calling Observer (sets CNTKCTL_EL1.EL0VCTEN on the next
context switch). After the first ClockRead, subsequent counter reads can use
CNTVCT_EL0 directly from userspace (~1 cycle) without a syscall. The target
handle is resolved but not meaningfully used beyond validation.

### Observer creation

#### CreateObserver (opcode 18)

Create a new Observer by consuming a Space (type conversion). D35, D32, D95.

| Register | Direction | Content                                              |
| -------- | --------- | ---------------------------------------------------- |
| x0       | in        | Handler Field capability handle (for fault delivery) |
| x1       | in        | Handler badge value                                  |
| x5       | in        | Space capability handle (consumed)                   |
| x4       | in        | 18 (opcode)                                          |
| x0       | out       | 0 on success                                         |

**Required right:** SPLIT (on the Space capability). **Possible errors:**
InvalidCap, StaleCap, NoRight, WrongType, InsufficientResource.

The consumed Space provides structural backing for the Observer's register save
area, L1 page table root, and capability table (D95). The Space must be large
enough for these structures plus at least 4 capability table entries (3 reserved
slots + 1 user slot). The Space capability slot is replaced with the new
Observer capability (full Observer rights).

The new Observer starts in Inert state (D39). Its capability table is populated
with: slot 0 = fault handler Field (SEND right + handler badge), slot 1 = empty
(reply Field, installed by userspace via SetReplyField, D106), slot 2 =
self-reference (full Observer rights). Configuration follows the composable
setup sequence (D35): CreateObserver, then ObserverInstallCap (for Space, Time,
and other capabilities), ObserverWriteRegisters (for PC, SP, x0),
ObserverSetReplyField (for RPC Observers), and ObserverResume.

#### ObserverSetReplyField (opcode 20)

Install a Field capability as the Observer's reply Field (slot 1). D106.

| Register | Direction | Content                                     |
| -------- | --------- | ------------------------------------------- |
| x0       | in        | Field capability handle (in caller's table) |
| x5       | in        | Observer capability handle (target)         |
| x4       | in        | 20 (opcode)                                 |
| x0       | out       | 0 on success                                |

**Required right:** INSTALL_CAP (on the Observer capability). **Possible
errors:** InvalidCap, StaleCap, NoRight, WrongType.

Installs the Field at the target Observer's reserved slot 1. The kernel
auto-enables badge_tracking (D73) on the installed Field and closes any existing
entry at slot 1 (D11). The installed capability entry carries RECEIVE right with
badge 0.

Non-RPC Observers skip SetReplyField entirely — no waste. Call() checks slot 1
and returns an error if empty, preventing silent caller zombification (blocked
on nothing, no badge-closure signal, permanently stuck).

### Resource acquisition

#### ResourceRequest (opcode 19)

Request additional Space or Time resources. D31, D104.

| Register | Direction | Content                                      |
| -------- | --------- | -------------------------------------------- |
| x0       | in        | Resource type (0 = Space, 1 = Time)          |
| x1       | in        | Quantity (pages for Space, units for Time)   |
| x5       | in        | Space capability handle (context)            |
| x4       | in        | 19 (opcode)                                  |
| x0       | out       | New handle (encoded) or 0, depending on path |

**Required right:** DESTROY (on the Space capability). **Possible errors:**
InvalidCap, StaleCap, NoRight, WrongType, InvalidState, ZeroSize,
InsufficientResource, TableFull.

Dual-path dispatch (D104):

- **Non-root Observer** (valid handler at slot 0): Treated as a fault. The
  kernel constructs a ResourceRequest fault message (D61) and routes it to the
  Observer's fault handler Field. The Observer blocks until the handler resolves
  the request (via ObserverInstallCap + ObserverResume) or denies it.

- **Root Observer** (empty or invalid handler at slot 0): The kernel allocates
  directly from the Space by splitting it. A new Space object is created and its
  capability is installed in the caller's table. Only Space requests are
  supported on the root path. InvalidState for Time requests on the root path.

---

## Derivation cross-reference

| Derivation | Topic                                      | Operations affected                           |
| ---------- | ------------------------------------------ | --------------------------------------------- |
| D7         | Split interaction model                    | All (IPC vs typed families)                   |
| D13        | Queued fields, direct-switch fast path     | Send, Receive, Call, ReplyRecv                |
| D16        | Reply via send-once cap                    | Call, ReplyRecv                               |
| D17        | Badge semantics                            | Send, Call, ReplyRecv, Mint                   |
| D28        | Fixed-size message format                  | All IPC                                       |
| D31        | Resource acquisition                       | ResourceRequest                               |
| D32        | Type conversion (Space consumed)           | CreateField, CreatePulsar, CreateObserver     |
| D35        | Observer creation sequence                 | CreateObserver                                |
| D38        | Time linearity                             | TimeSplit, Clone (forbidden)                  |
| D39        | Observer rights and state machine          | All Observer operations                       |
| D41        | Space topology operations                  | SpaceSplit, SpaceMerge                        |
| D42        | Scheduling profile                         | ObserverSetScheduling                         |
| D44        | Pulsar timers                              | CreatePulsar, ClockRead                       |
| D45        | Field split (badge-range routing)          | FieldSplit                                    |
| D47        | Syscall ABI framework                      | All (SVC immediate, register convention)      |
| D48        | Syscall enumeration (25 operations)        | All                                           |
| D49        | Error signaling, encoding details          | All (error codes, cap-absent sentinel)        |
| D50        | Direct-switch fast-path conditions         | Call, ReplyRecv                               |
| D51        | Send-once flag                             | Send, Call                                    |
| D52        | Per-type rights masks                      | All typed (rights checking)                   |
| D57        | Scheduling budget (R + T <= 128)           | ObserverSetScheduling                         |
| D62        | Pulsar creation API                        | CreatePulsar                                  |
| D65        | Reply badge                                | Call                                          |
| D66        | Per-Observer clock access                  | ClockRead                                     |
| D67        | Generation counter for revocation          | All typed (generation check on object access) |
| D72        | Duration parameter (relative nanoseconds)  | CreatePulsar                                  |
| D74        | Fast-path x0-x3 register pass-through      | Call, ReplyRecv                               |
| D77        | Handle encoding and cap resolution         | All typed (handle decode, bounds check)       |
| D95        | Structural backing layout                  | CreateObserver                                |
| D97        | Cap table self-mutation                    | ObserverInstallCap, ObserverChangeHandler     |
| D98        | Destroy cascade                            | Destroy                                       |
| D103       | Inline register transfer                   | ObserverWriteRegisters, ObserverReadRegisters |
| D104       | ResourceRequest dual-path dispatch         | ResourceRequest                               |
| D106       | Reply Field allocation (userspace-created) | ObserverSetReplyField, CreateObserver         |
| D107       | Auto-destroy on zero refcount              | Close (inline destroy on refcount zero)       |

## Operation summary table

| #   | Name                   | Encoding      | Target type      | Required right          |
| --- | ---------------------- | ------------- | ---------------- | ----------------------- |
| 1   | Send                   | SVC #1        | Field            | SEND                    |
| 2   | Receive                | SVC #2        | Field            | RECEIVE                 |
| 3   | Call                   | SVC #3        | Field            | SEND                    |
| 4   | ReplyRecv              | SVC #4        | Field (x5+x7)    | SEND (x5), RECEIVE (x7) |
| 5   | Yield                  | SVC #5        | none             | none                    |
| 6   | ObserverResume         | SVC #0, x4=0  | Observer         | RESUME                  |
| 7   | ObserverInstallCap     | SVC #0, x4=1  | Observer         | INSTALL_CAP             |
| 8   | ObserverWriteRegisters | SVC #0, x4=2  | Observer         | WRITE_REGISTERS         |
| 9   | ObserverReadRegisters  | SVC #0, x4=3  | Observer         | READ_REGISTERS          |
| 10  | ObserverSuspend        | SVC #0, x4=4  | Observer         | SUSPEND                 |
| 11  | ObserverChangeHandler  | SVC #0, x4=5  | Observer         | CHANGE_HANDLER          |
| 12  | ObserverSetScheduling  | SVC #0, x4=6  | Observer         | MODIFY_SCHEDULING       |
| 13  | Destroy                | SVC #0, x4=7  | any              | DESTROY                 |
| 14  | Clone                  | SVC #0, x4=8  | any (not Time)   | CLONE                   |
| 15  | Close                  | SVC #0, x4=9  | any              | none                    |
| 16  | Mint                   | SVC #0, x4=10 | any              | MINT                    |
| 17  | SpaceSplit             | SVC #0, x4=11 | Space            | SPLIT                   |
| 18  | SpaceMerge             | SVC #0, x4=12 | Space            | MERGE                   |
| 19  | CreateField            | SVC #0, x4=13 | Space (consumed) | SPLIT                   |
| 20  | FieldSplit             | SVC #0, x4=14 | Field            | SPLIT                   |
| 21  | TimeSplit              | SVC #0, x4=15 | Time             | SPLIT                   |
| 22  | CreatePulsar           | SVC #0, x4=16 | Space (consumed) | SPLIT                   |
| 23  | ClockRead              | SVC #0, x4=17 | any              | none                    |
| 24  | CreateObserver         | SVC #0, x4=18 | Space (consumed) | SPLIT                   |
| 25  | ResourceRequest        | SVC #0, x4=19 | Space            | DESTROY                 |
| 26  | ObserverSetReplyField  | SVC #0, x4=20 | Observer         | INSTALL_CAP             |
