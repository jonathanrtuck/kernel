# Message Shape — 2026-04-12

Tenth exploration. Defined the concrete message format: what goes in an Endpoint
queue, how capability transfers are encoded, and how the payload size was
determined.

## Starting point

Journal 002 established: all information delivery is one mechanism. A message
has source, type/metadata, and payload. Messages are small (register-sized).
Journals 006-009 established capability transfer via IPC, Endpoint queues, and
object types. Open question: what is the concrete message format?

## Message structure

A message consists of kernel-managed metadata (outside the payload) and a
fixed-size payload:

```text
Message:
  badge           integer, from sender's capability (set at clone time, unforgeable)
  type            IPC | fault | interrupt | system
  cap_mask        bitmask — which payload slots contain capability handles
  payload         4 slots × 8 bytes = 32 bytes
```

### Badge

An integer baked into the sender's capability at clone time. The sender can't
see, choose, or modify it — the kernel attaches it to the message automatically
based on which capability was used to send.

The badge identifies the **capability**, not the Context. Whoever mints (clones)
the send capability to an Endpoint chooses the badge value. Typically the
server, who uses it as a key into per-client state.

Badges are unforgeable: the sender can't lie about their badge. This is the
structural prevention of impersonation in the IPC model.

The badge is a capability property stored in the capability table:

```text
Capability: (object reference, rights, badge)
```

Badge is set at clone time, immutable after creation. The clone operation is
extended: `clone(handle, badge: N)` — creates a copy with the specified badge.

### Type

Set by the kernel. The sender doesn't choose the type — the kernel knows whether
a message is IPC (from a Context), a fault (from the kernel's fault delivery),
an interrupt (from hardware), or a system signal (endpoint destroyed, admission
failure, etc.). The receiver uses the type to dispatch.

### Payload: 4 slots (32 bytes)

The payload is 4 slots of 8 bytes each (one 64-bit register per slot). Some
slots carry data, some carry capability handles to transfer. The cap_mask
bitmask indicates which.

## Capability transfer encoding

Capability transfers are encoded directly in payload slots, not as a separate
sideband. The cap_mask bitmask tells the kernel which slots contain handles to
transfer.

On send: the kernel reads cap_mask. For each flagged slot, it reads the handle
from the sender's capability table, removes it (or clones it based on flags),
and stores the (object, rights, badge) for delivery.

On receive: the kernel adds entries to the receiver's capability table for each
transferred capability, and replaces the slot values with the receiver's new
local handle numbers. The same cap_mask is delivered so the receiver knows which
slots are handles.

```text
Sender's view:          Receiver's view:
  slot 0: 0x0001 (opcode)    slot 0: 0x0001 (opcode)      — data, unchanged
  slot 1: 0x00FF (arg)       slot 1: 0x00FF (arg)          — data, unchanged
  slot 2: 7 (my handle)      slot 2: 3 (their new handle)  — cap, remapped
  slot 3: 4 (my handle)      slot 3: 9 (their new handle)  — cap, remapped
  cap_mask: 0b1100           cap_mask: 0b1100
```

This means the 4-slot payload is a shared budget between data and capabilities.
A message with no transfers uses all 4 for data. A message with 3 transfers has
only 1 data slot.

## Payload size derivation

### Step 1: hardware ceiling

The message payload must fit entirely in registers to avoid memory spillover on
all target architectures:

| Arch   | Syscall arg registers | Overhead (handle + flags) | Payload ceiling |
| ------ | --------------------- | ------------------------- | --------------- |
| ARM64  | 8                     | 2                         | 6 (48 bytes)    |
| x86-64 | 6                     | 2                         | 4 (32 bytes)    |
| RISC-V | 7                     | 2                         | 5 (40 bytes)    |

The bottleneck is x86-64 at 6 argument registers (hardware constraint: `syscall`
clobbers rcx and r11). After 2 registers of syscall overhead (endpoint handle +
cap_mask/flags), 4 remain for payload. **Ceiling: 32 bytes.**

The message interface is defined in bytes, not registers. It is
arch-independent. The arch layer maps logical payload slots to CPU registers
(all in registers on ARM64, all in registers on x86-64 at 4 slots). An
architecture with fewer registers would spill overflow to a per-Context memory
buffer — same interface, lower performance.

### Step 2: requirements

| Message type             | Data slots | Cap slots        | Total |
| ------------------------ | ---------- | ---------------- | ----- |
| IPC RPC request          | 2          | 2 (reply + Time) | 4     |
| IPC RPC reply            | 2          | 1 (Time return)  | 3     |
| IPC simple (no reply)    | 3-4        | 0-1              | 3-4   |
| Fault (kernel → handler) | 3          | 0-1 (resume?)    | 3-4   |
| Interrupt                | 1          | 0                | 1     |
| System signal            | 2          | 0                | 2     |

The largest messages are RPC requests (2 data + 2 caps = 4) and faults (3 data +
0-1 cap = 3-4). Both fit exactly in 4 slots.

**4 slots (32 bytes):** matches the hardware ceiling and satisfies all
identified message types with no wasted space.

## Message patterns

### IPC RPC (Context → Context, with reply)

```text
Request:  [opcode, arg, reply_endpoint(cap), time(cap)]    cap_mask: 0b1100
Reply:    [status, result, time_return(cap), —]             cap_mask: 0b0100
```

The client transfers a send capability to its own reply Endpoint and a Time
capability. The server replies to the client's Endpoint, returning Time. No
special reply primitive — uses existing Endpoints and capability transfer.

For requests needing more data than 2 slots: pass a handle to shared Memory
(takes one cap slot), read the data from there.

### Fault (kernel → handler)

```text
Fault:    [fault_type, faulting_va, flags, resume?(cap)]    cap_mask: 0b0000 or 0b1000
```

3 data slots for fault information. The fourth slot may carry a resume
capability (open question — see below).

### Interrupt (kernel → driver)

```text
Interrupt: [interrupt_number, —, —, —]                      cap_mask: 0b0000
```

Minimal. The interrupt number is all the driver needs to begin processing.

## Open questions

- **Reply routing mechanism.** The RPC pattern uses explicit reply Endpoints
  (Option A from the exploration). The client transfers a send capability to its
  reply Endpoint. This uses existing mechanisms but costs one cap slot in every
  request. Alternative: kernel-managed one-shot reply capabilities (Option B)
  would free one slot but add a new kernel primitive. Leaning toward Option A
  but not settled.

- **Fault resume mechanism.** Same structural question as reply routing: how
  does the fault handler tell the kernel to resume the faulting Context?
  Options: a resume capability in the fault message (1 cap slot), or a
  badge-based resume syscall (0 cap slots). Connected to the reply routing
  decision — the answer likely applies to both.

- **Badge assignment.** The minter (whoever clones the capability) sets the
  badge. Typically the server. Kernel auto-assignment is an alternative but
  gives servers less control. Leaning toward minter-assigned.

- **Payload slot semantics.** Are slots untyped (just bytes) or does the kernel
  interpret slot 0 as an opcode? Probably untyped — the kernel delivers bytes,
  the receiver interprets them. The type field distinguishes kernel messages
  from IPC. Within IPC, interpretation is the receiver's business.

## Status

**Tentatively accepted:**

- Messages are: badge + type + cap_mask + 4 payload slots (32 bytes)
- Badge is a capability property (set at clone, unforgeable, kernel-attached)
- Type is kernel-set (IPC, fault, interrupt, system)
- Capability transfers are encoded in payload slots via cap_mask bitmask
- 4 slots = 32 bytes, derived from: x86-64 ceiling (4 registers after overhead)
  AND largest message requirements (RPC request: 2 data + 2 caps)
- Payload slots are a shared budget between data and capability transfers
- The message interface (bytes) is arch-independent; the register mapping is
  arch-specific (ABI layer)

**Open:** reply routing, fault resume, badge assignment, slot interpretation.
