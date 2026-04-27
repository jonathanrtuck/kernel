# Capability Rights Reference

Technical reference for the kernel's capability rights model. The code in
`src/capability.rs` and `src/core_manager.rs` is the source of truth; this
document is a lookup aid derived from it.

## Capability model overview

Every Observer holds a kernel-managed flat capability table (D8). Each occupied
slot is an `Entry` containing:

- **Object identity:** type (`ObjectType`) and arena identifier (`ObjectId`)
- **Rights:** 14-bit bitmask (`Rights`) governing permitted operations
- **Badge:** minter-assigned `u64`, immutable after creation (D17)
- **Slot tag:** generational counter for ABA prevention (D11)
- **Send-once flag:** boolean, outside the rights mask (D51)
- **Stored generation:** snapshot of the object's generation at cap creation,
  checked against the live generation on every use (D67 revocation)

Handles presented by userspace encode a 16-bit slot index and 48-bit slot tag
into a single `u64` register value (D77).

Resolution sequence (D77): decode, bounds check, occupancy, slot tag,
generation, rights, type. Failure at any step returns an error to userspace
without performing the operation.

## Rights bits

Fourteen bits across all types. Shared rights occupy fixed positions;
type-specific rights occupy non-overlapping positions.

| Bit | Constant          | Applies to         | Description                                                              |
| --: | ----------------- | ------------------ | ------------------------------------------------------------------------ |
|   0 | SEND              | Field              | Enqueue a message (Send, Call, ReplyRecv reply phase)                    |
|   1 | DESTROY           | all types          | Destroy the object (D33 cascade for Observer)                            |
|   2 | RECEIVE           | Field              | Dequeue a message or block waiting (Receive, ReplyRecv receive phase)    |
|   3 | RESUME            | Observer           | Transition stopped Observer to runnable                                  |
|   4 | CLONE             | all except Time    | Duplicate the capability entry in the caller's table                     |
|   5 | INSTALL_CAP       | Observer           | Install a capability into the target Observer's table                    |
|   6 | WRITE_REGISTERS   | Observer           | Write PC, SP, x0, PSTATE to the target Observer                          |
|   7 | READ_REGISTERS    | Observer           | Read PC, SP, x0, PSTATE from the target Observer                         |
|   8 | SUSPEND           | Observer           | Set external suspension overlay on the target Observer                   |
|   9 | CHANGE_HANDLER    | Observer           | Replace the fault handler Field at slot 0                                |
|  10 | MODIFY_SCHEDULING | Observer           | Change the target Observer's scheduling profile                          |
|  11 | MINT              | Field              | Create attenuated cap with caller-chosen badge                           |
|  12 | SPLIT             | Space, Time, Field | Partition the object (Space/Time topology) or add a routing rule (Field) |
|  13 | MERGE             | Space              | Absorb an adjacent Space into this Space                                 |

## Per-type rights masks (D52)

Each object type has a fixed set of valid rights. Rights outside the mask are
meaningless for that type and are never checked.

| Type     | Valid rights                                                                                                     | Bit count | Constant     |
| -------- | ---------------------------------------------------------------------------------------------------------------- | --------: | ------------ |
| Space    | SPLIT, MERGE, DESTROY, CLONE                                                                                     |         4 | SPACE_ALL    |
| Time     | SPLIT, DESTROY                                                                                                   |         2 | TIME_ALL     |
| Field    | SEND, RECEIVE, MINT, SPLIT, DESTROY, CLONE                                                                       |         6 | FIELD_ALL    |
| Observer | RESUME, DESTROY, INSTALL_CAP, WRITE_REGISTERS, CLONE, READ_REGISTERS, SUSPEND, CHANGE_HANDLER, MODIFY_SCHEDULING |         9 | OBSERVER_ALL |
| Pulsar   | DESTROY, CLONE                                                                                                   |         2 | PULSAR_ALL   |

Special mask: `FAULT_OBSERVER` (D61) is a subset of Observer rights granted in
fault messages: RESUME, DESTROY, INSTALL_CAP, WRITE_REGISTERS, READ_REGISTERS (5
of 9).

## Operation rights matrix

The `required_rights` function in `core_manager.rs` is the single point of truth
for which right each typed operation requires. IPC operations check rights
inline in `dispatch_ipc`.

### IPC operations (SVC #1--#5)

All IPC operations target a Field capability. Rights are checked after cap
resolution.

| Operation | SVC | Target type | Required right | Notes                                                                                         |
| --------- | --: | ----------- | -------------- | --------------------------------------------------------------------------------------------- |
| Send      |   1 | Field       | SEND           | Fire-and-forget. Send-once consumed after success (D51).                                      |
| Receive   |   2 | Field       | RECEIVE        | Blocks if queue empty.                                                                        |
| Call      |   3 | Field       | SEND           | Caller blocks. Reply cap minted from slot 1. Send-once consumed after success.                |
| ReplyRecv |   4 | Field (x5)  | SEND           | x5 = reply field (SEND). x7 = receive field (RECEIVE, checked separately). Two caps resolved. |
| Yield     |   5 | none        | none           | No cap resolution. Rotates caller to tail of run queue.                                       |

ReplyRecv resolves two caps: the reply field handle (x5, requires SEND) and the
receive field handle (x7, requires RECEIVE). Both undergo the full resolution
sequence independently.

### Typed operations (SVC #0, code in x4)

#### Observer operations

| Operation              | Code | Target type | Required right    | Notes                                                                       |
| ---------------------- | ---: | ----------- | ----------------- | --------------------------------------------------------------------------- |
| ObserverResume         |    0 | Observer    | RESUME            | Inert/Faulted/Suspended to Runnable.                                        |
| ObserverInstallCap     |    1 | Observer    | INSTALL_CAP       | Installs a cap from caller's table into target Observer's table.            |
| ObserverWriteRegisters |    2 | Observer    | WRITE_REGISTERS   | Inline: PC, SP, x0, PSTATE (NZCV masked). Target must be stopped.           |
| ObserverReadRegisters  |    3 | Observer    | READ_REGISTERS    | Inline: returns PC, SP, x0, PSTATE. Target must be stopped.                 |
| ObserverSuspend        |    4 | Observer    | SUSPEND           | Sets external suspension overlay.                                           |
| ObserverChangeHandler  |    5 | Observer    | CHANGE_HANDLER    | Replaces fault handler at slot 0. Secondary arg: Field cap with SEND right. |
| ObserverSetScheduling  |    6 | Observer    | MODIFY_SCHEDULING | Sets responsiveness and throughput (D57: R + T <= 128).                     |

#### Generic operations (cross-type)

These operations accept any object type. The required right is type-appropriate.

| Operation | Code | Target type | Required right | Notes                                                                                                                                     |
| --------- | ---: | ----------- | -------------- | ----------------------------------------------------------------------------------------------------------------------------------------- |
| Destroy   |    7 | any         | DESTROY        | Revokes all caps, frees arena slot. Observer: preemptible cascade (D98). Returns backing Space for types created via D32 type conversion. |
| Clone     |    8 | any         | CLONE          | Duplicates the entry in the caller's table. Time is forbidden (D38 linear): returns CloneForbidden.                                       |
| Close     |    9 | any         | none           | Frees the slot. Always permitted -- no right required. Decrements refcount; may trigger badge-closure (D17).                              |
| Mint      |   10 | Field       | MINT           | Creates an attenuated copy with caller-chosen badge and rights mask. Rights can only be narrowed (intersection with source).              |

Close requires no right because the holder already possesses the capability. The
operation relinquishes authority; it does not exercise it. This matches seL4 and
L4 family convention.

Mint is Field-only in practice because only Field capabilities carry
operationally meaningful badges (D17). The rights check requires MINT on the
source cap. The operation creates a new entry with
`rights = source.rights AND requested_rights` and the caller-supplied badge.

#### Space operations

| Operation  | Code | Target type | Required right | Notes                                                                                                                    |
| ---------- | ---: | ----------- | -------------- | ------------------------------------------------------------------------------------------------------------------------ |
| SpaceSplit |   11 | Space       | SPLIT          | Partitions bytes from the target Space into a new Space. New cap gets SPACE_ALL rights.                                  |
| SpaceMerge |   12 | Space       | MERGE          | Absorbs a source Space into the target. Source cap (secondary argument) also requires MERGE. Spaces must be VA-adjacent. |

SpaceMerge checks MERGE on both the primary target cap and the secondary source
cap (resolved from args[0]).

#### Field operations

| Operation   | Code | Target type | Required right | Notes                                                                                                                                  |
| ----------- | ---: | ----------- | -------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| CreateField |   13 | Space       | SPLIT          | D32 type conversion. Consumes the Space, creates a Field. Cap slot is overwritten in-place. New cap gets FIELD_ALL.                    |
| FieldSplit  |   14 | Field       | SPLIT          | D45 badge-range routing. Secondary arg: Space cap (requires SPLIT on the Space). Creates a new sub-Field backed by the consumed Space. |

CreateField targets a Space capability (not a Field). The Space is consumed and
the cap entry is transformed in-place from Space to Field.

FieldSplit resolves a secondary Space cap from args[0] and checks SPLIT on it
via `resolve_space_argument`.

#### Time operations

| Operation | Code | Target type | Required right | Notes                                                                        |
| --------- | ---: | ----------- | -------------- | ---------------------------------------------------------------------------- |
| TimeSplit |   15 | Time        | SPLIT          | Transfers compute units from this Time to a new Time. New cap gets TIME_ALL. |

#### Pulsar operations

| Operation    | Code | Target type | Required right | Notes                                                                                                               |
| ------------ | ---: | ----------- | -------------- | ------------------------------------------------------------------------------------------------------------------- |
| CreatePulsar |   16 | Space       | SPLIT          | D32 type conversion. Consumes the Space, creates a Pulsar. Secondary arg: Field cap (requires SEND).                |
| ClockRead    |   17 | self        | none           | Reads the counter and enables direct EL0 counter access (D66). No cap resolved -- operates on the calling Observer. |

CreatePulsar targets a Space capability. The Space is consumed and the cap entry
is transformed in-place. A secondary Field cap for delivery is resolved via
`resolve_field_argument`, which checks SEND on that Field cap.

ClockRead does not resolve any capability. The rights check is `Rights::empty()`
(always passes). The operation targets the calling Observer implicitly.

#### Observer creation

| Operation      | Code | Target type | Required right | Notes                                                                                                                       |
| -------------- | ---: | ----------- | -------------- | --------------------------------------------------------------------------------------------------------------------------- |
| CreateObserver |   18 | Space       | SPLIT          | D32 type conversion. Consumes the Space for structural backing. Secondary arg: Field cap for fault handler (requires SEND). |

The consumed Space must be large enough for register save area, L1 page table
root, and minimum cap table (SLOT_USER_START + 1 entries). A secondary Field cap
is resolved via `resolve_field_argument` (requires SEND).

#### Resource acquisition

| Operation       | Code | Target type | Required right | Notes                                                                                         |
| --------------- | ---: | ----------- | -------------- | --------------------------------------------------------------------------------------------- |
| ResourceRequest |   19 | Space       | DESTROY        | D104 dual-path. Root Observer: kernel allocates from pool. Non-root: fault-routed to handler. |

DESTROY is used as the privilege gate -- only root-level Space capabilities
should carry it.

### Secondary cap rights summary

Several operations resolve additional capabilities beyond the primary target.
These secondary caps undergo independent rights checks.

| Operation             | Secondary cap  | Required right on secondary                                           |
| --------------------- | -------------- | --------------------------------------------------------------------- |
| SpaceMerge            | Space (source) | MERGE                                                                 |
| FieldSplit            | Space          | SPLIT                                                                 |
| CreatePulsar          | Field          | SEND                                                                  |
| CreateObserver        | Field          | SEND                                                                  |
| ObserverInstallCap    | any (source)   | none (entry copied as-is)                                             |
| ObserverChangeHandler | Field          | none (resolved, type-checked, but no explicit right beyond existence) |
| ReplyRecv             | Field (x7)     | RECEIVE                                                               |

## Send-once capabilities (D51)

Send-once is a boolean flag on the Entry, not a rights bit. Attenuation cannot
clear it -- this prevents defeating the use-limit guarantee by narrowing rights.

- Set at creation time, immutable thereafter.
- Checked on Send and Call. After successful delivery, the kernel removes the
  cap from the holder's table.
- Reply caps minted by Call are send-once by construction (D16).
- The `send_once` flag is copied through Clone and Mint (it is outside the
  rights mask and not subject to attenuation).

## Badge semantics (D17)

A badge is a `u64` value (D58) stored in the capability Entry, set by the minter
at Mint time, immutable after creation.

- The kernel injects the badge into every message sent through that capability.
- The sender cannot read, choose, or forge its badge.
- MINT right controls who can assign badges when creating derived caps.
- Badge-closure tracking is opt-in per Field. When the last send cap with badge
  B to a tracked Field is closed, the kernel enqueues a closure notification
  (LABEL_CLOSURE, D64).
- Reply Fields are always-tracked (D73).

## Minting and rights attenuation

Mint (code 10) creates a new entry derived from an existing cap:

1. The source cap must carry the MINT right (D17).
2. The new cap's rights = `source.rights AND requested_rights` (intersection).
   Rights can only be narrowed, never widened.
3. The caller supplies a badge value. If the badge argument is CAP_ABSENT, the
   source badge is preserved.
4. The `send_once` flag is copied from the source (not attenuatable).
5. The `stored_generation` is copied from the source.

The result is installed in a new slot in the caller's table. The source entry is
unmodified.

Clone (code 8) duplicates the entry exactly, including rights, badge, and
send-once flag. It requires CLONE on the source. Time capabilities cannot be
cloned (D38 linear; returns CloneForbidden).

## D32 type conversion pattern

CreateField, CreatePulsar, and CreateObserver all follow the same pattern:

1. Target cap must be a Space with SPLIT right.
2. The Space's generation is verified.
3. The Space is consumed (generation bumped, arena slot freed).
4. A new object is allocated in the appropriate arena.
5. The cap slot is overwritten in-place, changing from Space to the new type.
6. The new cap receives full rights for its type (FIELD_ALL, PULSAR_ALL,
   OBSERVER_ALL).

The consumed Space provides the structural backing for the new object. On
Destroy, the kernel performs reverse type conversion, returning a Space cap to
the destroyer.

## Reserved cap table slots (D21, D43, D57)

| Slot | Constant           | Contents                                   |
| ---: | ------------------ | ------------------------------------------ |
|    0 | SLOT_FAULT_HANDLER | Fault handler Field cap (SEND right only)  |
|    1 | SLOT_REPLY_FIELD   | Reply Field cap                            |
|    2 | SLOT_SELF          | Self-reference Observer cap (OBSERVER_ALL) |
|   3+ | user slots         | Available for application use              |
|  MAX | SLOT_GROWTH        | Sentinel for cap table growth (D40)        |
