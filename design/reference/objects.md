# Kernel Object Reference

This document describes the six kernel object types: their purpose, creation,
lifecycle, operations, destruction, revocation, and invariants. The code in
`src/` is the source of truth; this reference summarizes it for userspace
programmers.

All kernel objects are accessed exclusively through capabilities. Holding a
capability is both necessary and sufficient to operate on the designated object,
subject to the rights carried in that capability (D4). Every object lives in a
per-type arena (`Arena<T>`, D53/D70) and carries a generation counter for
revocation (D67).

---

## Common Mechanisms

### Generation-Based Revocation (D67)

Every kernel object carries an `AtomicU64` generation counter. When an object
owner wishes to revoke all outstanding capabilities to that object, it calls
`revoke()`, which atomically increments the counter. Capability entries store
the generation value at the time they were created. On every use, the kernel
compares the entry's stored generation against the object's live generation. A
mismatch means the capability has been revoked; the kernel returns `StaleCap`
and lazily rewrites the entry to empty (Coyotos pattern).

This is O(1) revocation with no scanning. Stale capabilities are detected at use
time. ARM64 cache coherence ensures the generation bump is eventually visible on
all cores without requiring an inter-processor interrupt.

### Reference Counting (D11)

Every kernel object has a `refcount: u32` field tracking how many capability
entries reference it. The refcount is incremented when a capability to the
object is installed in any Observer's table and decremented when a capability is
closed. When the refcount reaches zero, the object is eligible for destruction
(D33).

### Type Conversion (D32)

Objects that require memory backing (Observer, Field, Pulsar) are created by
consuming a Space capability. The consumed Space becomes the structural backing
for the new object. This is type conversion: Space in, new object out. On
destruction, the backing Space is returned to the destroyer as a Space
capability (D98 reverse type conversion). Space and Time are not created through
type conversion; they originate from the root pool or are split from existing
objects.

### Capability Table Slots

Capabilities are held in a per-Observer flat array (D8). Three slots are
reserved:

| Slot | Purpose                                        | Derivation |
| ---- | ---------------------------------------------- | ---------- |
| 0    | Fault handler Field                            | D21        |
| 1    | Reply Field                                    | D43        |
| 2    | Self-reference (Observer cap with full rights) | D57        |
| 3+   | User-available slots                           | D8         |

---

## Observer

An Observer is the kernel's execution unit -- the condition under which compute
(Time) executes instructions within specific memory (Space). Each Observer has
one register state, one program counter, and one execution stream. "Thread" is
the closest conventional analogy, but an Observer is not bound to a process or
address space; those are userspace conventions built from capability
distribution (D6, D14).

### Creation

**Syscall:** `CreateObserver` (typed operation, code 18).

**Inputs consumed:** one Space capability with sufficient size for structural
backing (register save area, capability table pages, page table root). The Space
is consumed by type conversion (D32); it ceases to exist as an independent Space
and becomes the new Observer's backing memory.

**Parameters:**

- Target Space capability handle (consumed)

**Result:** a capability to the new Observer, installed in the caller's table.
The new Observer starts in the `Inert` state with default scheduling profile
(responsiveness = 43, throughput = 43, precision = 42, budget = 128 per D57).
Clock access is disabled by default (D66). The capability table is allocated
from the structural backing with a freelist initialized from slot 3 onward.

An ASID is assigned sequentially from the kernel's allocator (D101).

### Lifecycle States (D39)

Observers have a five-state machine with an orthogonal suspension overlay:

```text
                  resume
    Inert ────────────────────> Runnable
                                  │  ▲
                          block() │  │ unblock()
                  (receive on     │  │ (message arrives)
                   empty queue)   ▼  │
                                Blocked
                                  │
                                  ▼
    Faulted <────────────────── Runnable
       │           fault()          ▲
       │        (hardware fault)    │
       │                            │
       └────────────────────────────┘
                  resume
```

**Primary states:**

| State    | Meaning                                                            |
| -------- | ------------------------------------------------------------------ |
| Inert    | Created but never started. Not schedulable.                        |
| Runnable | Eligible for scheduling. May be on a core's run queue.             |
| Blocked  | Waiting on a Field receive. Removed from run queue.                |
| Faulted  | Hardware fault occurred. Descheduled, awaiting handler resolution. |

**Suspension overlay:** `suspended` is a boolean flag orthogonal to the primary
state (D39). Suspension co-occurs with any primary state. When a blocked and
suspended Observer receives a message, the blocking condition is resolved
(primary state transitions to Runnable), but the Observer remains off the run
queue until explicitly resumed. Resume clears the suspension flag.

**Valid transitions:**

| From     | To       | Trigger                                        |
| -------- | -------- | ---------------------------------------------- |
| Inert    | Runnable | `resume()` -- first start                      |
| Runnable | Blocked  | `block()` -- receive on empty queue            |
| Blocked  | Runnable | `unblock()` -- message arrives on waited Field |
| Runnable | Faulted  | `fault()` -- hardware fault                    |
| Faulted  | Runnable | `resume()` -- handler resolved the fault       |

All other transitions return `InvalidTransition`.

### Operations

| Operation          | Syscall                           | Required Right      | Description                                                                                                                             |
| ------------------ | --------------------------------- | ------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| Resume             | `ObserverResume` (code 0)         | `RESUME`            | Transition from Inert or Faulted to Runnable. Clears suspension.                                                                        |
| Suspend            | `ObserverSuspend` (code 4)        | `SUSPEND`           | Set suspension overlay. Observer removed from run queue if Runnable.                                                                    |
| Install capability | `ObserverInstallCap` (code 1)     | `INSTALL_CAP`       | Install a capability into the target Observer's table.                                                                                  |
| Write registers    | `ObserverWriteRegisters` (code 2) | `WRITE_REGISTERS`   | Write program counter, stack pointer, x0, and PSTATE (NZCV masked) into the Observer's register state. Observer must be stopped (D103). |
| Read registers     | `ObserverReadRegisters` (code 3)  | `READ_REGISTERS`    | Read program counter, stack pointer, x0, and PSTATE from the Observer's register state. Observer must be stopped (D103).                |
| Change handler     | `ObserverChangeHandler` (code 5)  | `CHANGE_HANDLER`    | Replace the fault handler Field at reserved slot 0.                                                                                     |
| Set scheduling     | `ObserverSetScheduling` (code 6)  | `MODIFY_SCHEDULING` | Set the three-value scheduling profile (responsiveness, throughput). Precision is derived: P = 128 - R - T. Rejects if R + T > 128.     |
| Clone              | `Clone` (code 8)                  | `CLONE`             | Create a new capability to this Observer with attenuated rights.                                                                        |
| Destroy            | `Destroy` (code 7)                | `DESTROY`           | Destroy the Observer and cascade through its capability table (D33).                                                                    |
| Close              | `Close` (code 9)                  | (none)              | Drop this capability reference. Decrements refcount.                                                                                    |

### Scheduling Profile (D42, D57)

Each Observer carries a three-value scheduling profile with a fixed budget of
128:

- **Responsiveness (R):** how quickly the Observer should be scheduled after
  becoming runnable. Higher values favor latency.
- **Throughput (T):** how much continuous execution time the Observer prefers.
  Higher values favor batch processing.
- **Precision (P):** derived as `128 - R - T`. Higher values indicate hard
  real-time needs (EDF admission input).

Default: R = 43, T = 43, P = 42 (closest equal three-way distribution). The
kernel reads these values for core placement decisions (D56) and scheduling
quantum computation.

### Compute Aggregate (D30, D36)

The Observer maintains a cached `compute_aggregate: u32` summing the compute
units of all Time capabilities in its table. This is the hot-path input to the
scheduler. Updated on Time capability installation and removal.

### Clock Access (D66)

The `clock_access: bool` field controls whether the Observer can read the
hardware counter register (`CNTVCT_EL0`) directly. The kernel writes
`CNTKCTL_EL1.EL0VCTEN` on every context switch based on this flag. When false,
the Observer must use the `ClockRead` typed operation (code 17, D48).

### Destruction

**Syscall:** `Destroy` (code 7). Requires `DESTROY` right.

Destroying an Observer initiates a preemptible cascade (D33, D98). The cascade
iterates the Observer's capability table, closing each entry. Closed
capabilities decrement target objects' refcounts; objects reaching zero refcount
are themselves destroyed, potentially pushing nested cascade levels (up to
`MAX_CASCADE_DEPTH` = 4). The cascade processes entries in bounded steps
(`CASCADE_STEP_SIZE` = 16 entries per step), yielding to the scheduler between
steps so higher-priority Observers can run.

The destroying Observer is blocked for the duration of the cascade (D98). On
completion, the backing Space is returned to the destroyer as a new Space
capability (reverse type conversion, D98).

Only Observers cascade. Space, Time, Field, and Pulsar destruction is O(1).

### Key Invariants

- One register state, one program counter, one execution stream per Observer
  (D6).
- R + T <= 128 enforced at creation and on every `set_scheduling` call (D57).
- `compute_aggregate` equals the sum of compute units across all Time caps in
  the table (D30).
- Cap table capacity is bounded; growth via table-full fault to the handler (D8,
  D40).
- Self-reference capability at slot 2 with full rights (D57).
- Struct size: 128 bytes.

---

## Space

A Space is a claim to a portion of the system's bounded memory resource. It
represents a contiguous region of kernel-assigned virtual address space backed
by physical pages. Which physical pages back a Space is a kernel-internal
concern. Space is the memory primitive: all kernel objects that require backing
memory are created by consuming a Space (D9, D25, D26).

### Creation

Spaces originate in two ways:

1. **Root pool allocation:** at boot, the kernel creates the initial root Space
   from all usable physical memory. The root Observer receives a capability to
   this Space. New Spaces are obtained by splitting existing ones.

2. **Split:** the `SpaceSplit` operation (code 11) divides an existing Space
   into two. The original shrinks; a new Space is created from the extracted
   region.

3. **Reverse type conversion (D98):** when an Observer, Field, or Pulsar is
   destroyed, the backing Space is returned as a new Space capability.

There is no `CreateSpace` syscall. All Spaces ultimately derive from the root
pool through split and reverse type conversion.

### Operations

| Operation | Syscall                | Required Right | Description                                                                                                                                                                                                                                  |
| --------- | ---------------------- | -------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Split     | `SpaceSplit` (code 11) | `SPLIT`        | Extract a region from this Space into a new Space. Size is in bytes, rounded up to page size (D60). The original contracts; the new Space gets a fresh kernel-assigned VA base (D26). Cannot empty a Space -- at least one page must remain. |
| Merge     | `SpaceMerge` (code 12) | `MERGE`        | Absorb an adjacent Space into this one. The source ceases to exist; this Space's VA range extends. Requires exact VA adjacency (source VA base = target VA base + target size).                                                              |
| Clone     | `Clone` (code 8)       | `CLONE`        | Create a new capability to this Space with attenuated rights.                                                                                                                                                                                |
| Destroy   | `Destroy` (code 7)     | `DESTROY`      | Return the Space's pages to the root pool.                                                                                                                                                                                                   |
| Close     | `Close` (code 9)       | (none)         | Drop this capability reference.                                                                                                                                                                                                              |

**Split details (D41):** the new Space is carved from the high end of the
original. If a Space at VA base 0x1000 with size 0x4000 is split with a request
for 0x1000 bytes, the new Space gets VA base 0x4000 and size 0x1000, while the
original contracts to size 0x3000.

**Merge details (D41):** only upward-adjacent Spaces can merge. The source's VA
base must equal `target.va_base + target.size`. After merge, the target's size
increases by the source's size. The source Space is consumed.

**Conservation (D32):** split and merge preserve total page count. Pages change
membership, not quantity.

### Destruction

**Syscall:** `Destroy` (code 7). Requires `DESTROY` right.

Space destruction is O(1). The physical pages and page table subtree memory are
returned to the root pool (D3, D31). Cross-core TLB invalidation may be required
for Spaces shared by multiple Observers (O2).

### Rights (D52)

| Right   | Bit | Description               |
| ------- | --- | ------------------------- |
| DESTROY | 1   | Destroy the Space         |
| CLONE   | 4   | Create an attenuated copy |
| SPLIT   | 12  | Split into two Spaces     |
| MERGE   | 13  | Absorb an adjacent Space  |

Full mask: `SPACE_ALL` = SPLIT | MERGE | DESTROY | CLONE (4 bits).

### Key Invariants

- VA base is kernel-assigned and stable for the Space's lifetime (D26).
- Size is always page-aligned (D25, D60).
- Split cannot empty a Space; at least one page must remain.
- Merge requires exact VA adjacency.
- L3 page table physical address is set at creation and immutable for the
  Space's lifetime (D89). Split and merge do not alter a Space's L3 table.
- Struct size: 40 bytes.

---

## Time

A Time object represents a claim to a portion of the system's compute capacity,
denominated in normalized compute units (D36). The unit is calibrated to
hardware core capacity factors so that a given quantity represents approximately
the same work on any core. The kernel translates compute units to per-core
scheduling time internally (D29, D36).

Time is linear: at most one capability reference per Time object (D38). Clone is
structurally forbidden. Authority delegation uses split, not clone.

### Creation

Time objects originate from the root pool at boot and are obtained by splitting
existing Time objects. There is no `CreateTime` syscall.

**Split (D38):** the `TimeSplit` operation (code 15) extracts a specified number
of compute units from an existing Time into a new Time object. The original's
quantity decreases by the extracted amount. Unlike Space, a Time can be fully
exhausted (split all units), leaving the original with zero compute units.

### Operations

| Operation | Syscall               | Required Right | Description                                                                        |
| --------- | --------------------- | -------------- | ---------------------------------------------------------------------------------- |
| Split     | `TimeSplit` (code 15) | `SPLIT`        | Extract compute units into a new Time. Amount must be > 0 and <= current quantity. |
| Destroy   | `Destroy` (code 7)    | `DESTROY`      | Destroy the Time, returning compute units to the system pool.                      |
| Close     | `Close` (code 9)      | (none)         | Drop this capability reference.                                                    |

Clone and Mint are not available for Time (D38 linearity). Attempting to clone a
Time capability returns `CloneForbidden`.

### Destruction

**Syscall:** `Destroy` (code 7). Requires `DESTROY` right.

Time destruction is O(1). The compute units are returned to the kernel's system
capacity pool. If the Time was held by an Observer, the Observer's
`compute_aggregate` is decremented by the destroyed Time's quantity.

### Rights (D52)

| Right   | Bit | Description          |
| ------- | --- | -------------------- |
| DESTROY | 1   | Destroy the Time     |
| SPLIT   | 12  | Split into two Times |

Full mask: `TIME_ALL` = SPLIT | DESTROY (2 bits). No CLONE right exists for
Time.

### Key Invariants

- At most one capability reference per Time object (D38 linearity, refcount is
  always 0 or 1).
- `compute_units` is a `u32`. Split preserves conservation: the sum of all Time
  compute units plus the kernel pool equals total system capacity (D36).
- Split allows full exhaustion (amount == compute_units), unlike Space split.
- Zero amount split is rejected (`ZeroAmount`).
- Struct size: 16 bytes.

---

## Field

A Field is a queued, unidirectional, many-to-many IPC endpoint (D13, D15). All
information delivery in the kernel flows through Fields: peer messages, fault
notifications (D12), interrupt signals (D22), timer fires (D44), and
badge-closure events (D17, D64). The metaphor is from physics: a field mediates
interaction between observers.

### Creation

**Syscall:** `CreateField` (typed operation, code 13).

**Inputs consumed:** one Space capability (type conversion, D32). The consumed
Space becomes the Field's structural backing (message queue buffer).

**Parameters:**

- Target Space capability handle (consumed)
- Queue capacity
- Badge tracking enabled (boolean, D17)

**Result:** a capability to the new Field, installed in the caller's table. The
Field starts with an empty queue, no waiters, no routing table, and no pending
messages.

### Message Format (D28)

All messages use a fixed-size format:

| Component    | Size     | Description                                              |
| ------------ | -------- | -------------------------------------------------------- |
| `data[0..3]` | 4 x u64  | Untyped data words. Arbitrary 64-bit values.             |
| `label`      | u64      | Pass-through label. Kernel does not dispatch on it.      |
| `badge`      | u64      | Sender's badge, injected by kernel from cap entry (D17). |
| `user_cap`   | optional | 0 or 1 transferred capability (D28).                     |
| `reply_cap`  | optional | Kernel-created send-once cap for Call (D16).             |

Kernel-reserved labels occupy the range `0xFFFF_FFFF_FFFF_0000` and above:

| Label                   | Constant                   | Purpose                         |
| ----------------------- | -------------------------- | ------------------------------- |
| `0xFFFF_FFFF_FFFF_0001` | `LABEL_TIMER_FIRE`         | Pulsar fire (D63)               |
| `0xFFFF_FFFF_FFFF_0002` | `LABEL_CLOSURE`            | Badge closure (D64)             |
| `0xFFFF_FFFF_FFFF_0003` | `LABEL_VM_FAULT`           | Virtual memory page fault (D61) |
| `0xFFFF_FFFF_FFFF_0004` | `LABEL_RESOURCE_REQUEST`   | Resource request (D31, D61)     |
| `0xFFFF_FFFF_FFFF_0005` | `LABEL_CAP_TABLE_FULL`     | Cap table full fault (D8, D61)  |
| `0xFFFF_FFFF_FFFF_0006` | `LABEL_HARDWARE_EXCEPTION` | Hardware exception (D61)        |
| `0xFFFF_FFFF_FFFF_0007` | `LABEL_DEVICE_IRQ`         | Device interrupt (D22, D81)     |

### IPC Operations

Field operations are invoked through IPC syscalls (SVC #1 through #5):

| Operation | SVC Immediate | Description                                                                                                                                                                 |
| --------- | ------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Send      | SVC #1        | Enqueue a message to the target Field. Requires `SEND` right. Fire-and-forget: the sender continues. Returns `QueueFull` if the queue is at capacity (D18 error-to-sender). |
| Receive   | SVC #2        | Dequeue the front message from a Field. Requires `RECEIVE` right. If the queue is empty, the calling Observer blocks until a message arrives (D13).                         |
| Call      | SVC #3        | Atomic send + receive. Sends a message with a kernel-minted reply cap (D16), then blocks on the reply Field. Fast-path eligible (D50).                                      |
| ReplyRecv | SVC #4        | Atomic reply + receive. Sends a reply via a send-once cap, then blocks on the next receive. Fast-path eligible (D50).                                                       |
| Yield     | SVC #5        | Voluntarily relinquish the current scheduling quantum.                                                                                                                      |

### Typed Operations on Fields

| Operation   | Syscall                | Required Right | Description                                                                                                                                         |
| ----------- | ---------------------- | -------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| Field split | `FieldSplit` (code 14) | `SPLIT`        | Create a child Field with badge-range routing. Messages matching the badge range are routed to the child; non-matching messages stay on the parent. |
| Mint        | `Mint` (code 10)       | `MINT`         | Create a new send capability with a minter-assigned badge value (D17).                                                                              |
| Clone       | `Clone` (code 8)       | `CLONE`        | Create a new capability with attenuated rights.                                                                                                     |
| Destroy     | `Destroy` (code 7)     | `DESTROY`      | Destroy the Field.                                                                                                                                  |
| Close       | `Close` (code 9)       | (none)         | Drop this capability reference.                                                                                                                     |

### Badge-Range Routing (D45, D54, D71)

Fields support badge-range routing for server decomposition. Each routing rule
specifies a closed range `[low, high]` of badge values and a destination Field.
When a message is sent to a Field with routing rules, the kernel performs a
binary search over the sorted routing table:

- If the message's badge falls within a range, the message is routed to the
  destination Field.
- If no range matches, the message is delivered to the source Field (fallback).
- Stale routing entries (destination generation mismatch) are treated as absent;
  the message falls back to the source.

Unsplit Fields have no routing table (null pointer). The null check costs zero
on the hot path for the common case.

### Badge Tracking (D17)

When badge tracking is enabled on a Field, the kernel maintains a per-badge
reference count map. Each distinct badge value tracks how many send capabilities
with that badge exist. When the last send capability with a given badge is
closed, the kernel enqueues a `LABEL_CLOSURE` notification to the Field (D64).
This enables servers to detect client disconnection.

Reply Fields are always-tracked (D73).

### Overflow Handling (D18)

When a sender enqueues a message to a full queue:

- **Userspace sender:** `Send` returns `QueueFull` error. The sender handles
  overflow.
- **Kernel-as-sender** (faults, interrupts, timer fires): the message is
  deferred via the pending list. On the next receive that frees a queue slot,
  deferred messages are delivered.

### Destruction

**Syscall:** `Destroy` (code 7). Requires `DESTROY` right.

Field destruction is O(1) for the Field itself. If the Field is a routing
destination for other Fields, all routing entries in source Fields pointing to
this destination are cleaned up (D55). The backing Space is returned to the
destroyer via reverse type conversion (D98).

### Rights (D52)

| Right   | Bit | Description                                    |
| ------- | --- | ---------------------------------------------- |
| SEND    | 0   | Send messages to this Field                    |
| DESTROY | 1   | Destroy the Field                              |
| RECEIVE | 2   | Receive messages from this Field               |
| CLONE   | 4   | Create an attenuated copy                      |
| MINT    | 11  | Create a send cap with a minter-assigned badge |
| SPLIT   | 12  | Create a child Field with badge-range routing  |

Full mask: `FIELD_ALL` = SEND | RECEIVE | MINT | SPLIT | DESTROY | CLONE (6
bits).

### Key Invariants

- Queue is bounded circular buffer. FIFO ordering preserved across wrap-around.
- `queue_length` never exceeds `queue_capacity` (enqueue on full returns error).
- Waiters list and pending list are distinct: waiters are Observers blocked on
  receive; pending entries are deferred kernel-as-sender messages.
- Routing table is sorted by `badge_low` for binary search. Overlapping badge
  ranges are rejected.
- Badge tracking is opt-in per Field; reply Fields are always-tracked (D73).
- Struct size: 200 bytes.

---

## Pulsar

A Pulsar is a timer object that the kernel programs on behalf of an Observer and
delivers as a Field message when it fires (D44). The metaphor is from
astrophysics: a pulsar emits regular, precisely-timed signals that an Observer
listens for.

### Creation

**Syscall:** `CreatePulsar` (typed operation, code 16).

**Inputs consumed:** one Space capability (type conversion, D32).

**Parameters:**

- Target Space capability handle (consumed)
- Delivery Field identifier (where fire messages are sent)
- Badge value (injected into fire messages, D17)
- Duration in nanoseconds (D72, relative)
- Period in nanoseconds (0 = one-shot, > 0 = repeating)

**Result:** a capability to the new Pulsar, installed in the caller's table. The
Pulsar is armed immediately on creation (D62). There is no separate arm,
configure, or modify call.

The kernel converts the nanosecond duration to absolute counter ticks using the
hardware frequency (`CNTFRQ_EL0`) at creation time (D72). This conversion is
absorbed by the kernel so callers express intent in human-meaningful units (A5).

### Behavior

**One-shot (period = 0):** the Pulsar fires once after the specified duration.
After firing, the Pulsar remains in the arena until explicitly destroyed.

**Repeating (period > 0):** after firing, the kernel re-arms the Pulsar with
drift-compensated timing: `next_deadline = scheduled_deadline + period_ticks`,
not `next_deadline = actual_fire_time + period_ticks`. This prevents systematic
drift from interrupt latency accumulation (D44).

**Fire message (D63):** delivered to the designated Field with:

- `label` = `LABEL_TIMER_FIRE`
- `badge` = the Pulsar's badge
- `data[0]` = actual fire time in raw `CNTVCT_EL0` ticks
- `data[1]` = overrun count
- No capability attached (satisfies D50 fast-path 0-cap condition)

**Overrun handling (D44):** when the delivery Field is full at fire time, the
kernel stops re-arming and increments the overrun counter. When a receive on the
delivery Field frees a slot, the kernel delivers the fire message with the
accumulated overrun count and re-arms.

### Operations

| Operation | Syscall            | Required Right | Description                                     |
| --------- | ------------------ | -------------- | ----------------------------------------------- |
| Clone     | `Clone` (code 8)   | `CLONE`        | Create a new capability with attenuated rights. |
| Destroy   | `Destroy` (code 7) | `DESTROY`      | Destroy the Pulsar (cancel the timer).          |
| Close     | `Close` (code 9)   | (none)         | Drop this capability reference.                 |

There are no operations to modify a Pulsar's parameters after creation (D62). To
change timing: destroy the Pulsar and create a new one. To cancel: destroy.
One-shot Pulsars in a loop provide the manual-control escape hatch for adaptive
timing.

### Destruction

**Syscall:** `Destroy` (code 7). Requires `DESTROY` right.

Pulsar destruction is O(1). The timer is deregistered from the per-core deadline
queue (D83). The backing Space is returned to the destroyer via reverse type
conversion (D98).

### Rights (D52)

| Right   | Bit | Description               |
| ------- | --- | ------------------------- |
| DESTROY | 1   | Destroy the Pulsar        |
| CLONE   | 4   | Create an attenuated copy |

Full mask: `PULSAR_ALL` = DESTROY | CLONE (2 bits).

### Key Invariants

- Armed on creation. No inert-to-armed transition (D62).
- Duration and period are immutable after creation.
- Re-arm is drift-compensated: `next = scheduled + period`, not
  `next = now + period` (D44).
- Overrun count accumulates when the delivery Field is full; reset to zero on
  successful re-arm.
- `ns_to_ticks` uses 128-bit intermediate arithmetic to avoid overflow for large
  durations.

---

## Capability Table

The capability table is a per-Observer kernel-managed flat array of capability
entries (D8). It is not an independent kernel object in the arena; rather, it is
structural backing within each Observer, allocated from the Observer's consumed
Space at creation time. It is documented here because it is the mechanism
through which all other objects are accessed.

### Structure

The table is a contiguous array of `Entry` structs with an intrusive freelist
through empty slots.

```rust
Entry {
    object:            Option<(ObjectType, ObjectId)>,  -- target, or None if empty
    rights:            Rights,                          -- per-cap rights bitmask
    badge:             Badge,                           -- minter-assigned, immutable
    slot_tag:          SlotTag,                         -- ABA prevention tag
    send_once:         bool,                            -- use-limited flag (D51)
    stored_generation: u64,                             -- D67 revocation check
}
```

Empty slots use `stored_generation` to store the next-free index in the
freelist, with `u64::MAX` as the end sentinel.

### Handle Encoding (D77)

Userspace presents capabilities as opaque `u64` handles:

- Lower 16 bits: slot index (supports up to 65,536 slots)
- Upper 48 bits: slot tag (approximately 281 trillion reuses before ABA
  aliasing)

The kernel decodes the handle, checks bounds, verifies the slot tag matches, and
proceeds with the operation. This encoding is an ABI contract.

### Resolution Sequence (D77)

Every syscall begins with capability resolution. The sequence, in order:

1. **Decode:** extract index (lower 16 bits) and slot tag (upper 48 bits) from
   the raw `u64` handle.
2. **Bounds check:** verify index is less than table capacity. Protected by a
   Spectre v1 speculation barrier in frame/ code.
3. **Occupancy check:** verify the entry has an object (not empty/freelist).
4. **Slot tag check:** verify the entry's tag matches the handle's tag (D11 ABA
   defense).
5. **Generation check:** compare the entry's stored generation against the
   object's live generation from the arena (D67 revocation).
6. **Rights check:** verify all required rights are present (D52).
7. **Type check:** verify the object type matches the expected type (for
   type-specific operations).

Failure at any step returns the corresponding `SyscallError`.

### Table Growth (D8, D40)

When an operation needs a free slot and none exists, the kernel:

1. Saves the syscall context in the Observer's `saved_syscall` field.
2. Delivers a `LABEL_CAP_TABLE_FULL` fault to the handler Field (slot 0).
3. Blocks the Observer.

The fault handler provides additional Space for table growth. On resume, the
kernel replays the saved operation transparently -- the Observer never observes
the fault.

### Slot Tag ABA Defense (D11)

Slot tags are bumped on every slot reuse (close or free). This prevents stale
handles from aliasing newly-installed capabilities in reused slots. Slot tag
mismatch is indistinguishable from an invalid capability from userspace's
perspective -- the recovery is the same (re-acquire through IPC).

### Operations

| Operation           | Syscall                       | Description                                                                                       |
| ------------------- | ----------------------------- | ------------------------------------------------------------------------------------------------- |
| Close               | `Close` (code 9)              | Free a slot, bump its tag, decrement target refcount.                                             |
| Install             | `ObserverInstallCap` (code 1) | Install a capability at the next free slot (via `INSTALL_CAP` right on the target Observer).      |
| Install at          | (kernel-internal)             | Install at a specific slot index (reserved slots, IPC transfer).                                  |
| Extract             | (kernel-internal)             | Move a cap out of the table for IPC transfer (D96). Captures as `TransferredCap`, frees the slot. |
| Install transferred | (kernel-internal)             | Install a `TransferredCap` from IPC at the next free slot, returning the encoded handle.          |

### Preemptible Cascade (D33, D98)

On Observer destruction, the kernel iterates the capability table and closes
every entry. This cascade is preemptible: each step processes at most 16 entries
(`CASCADE_STEP_SIZE`), then yields to the scheduler. The cascade may nest up to
4 levels deep (`MAX_CASCADE_DEPTH`) when closing a capability triggers
destruction of the target object (which itself may be an Observer with
capabilities).

The cascade state is stored in `CascadeContinuation` within `CoreState`:

```rust
CascadeContinuation {
    levels: [Option<CascadeLevel>; 4],  -- stack of active cascade levels
    depth: usize,                        -- number of active levels
    destroyer_ptr: ...,                  -- blocked Observer that issued Destroy
    backing_va: usize,                   -- for return Space cap
    backing_size: usize,                 -- for return Space cap
    target_id: ObjectId,                 -- Observer being destroyed
}
```

### Key Invariants

- Reserved slots 0, 1, 2 are not part of the user freelist.
- `count` tracks the number of occupied entries.
- `free_head` is `None` when all user slots are occupied (triggers table-full
  fault).
- Slot tags are monotonically bumped (wrapping) on reuse; they are never
  decremented.
- The `has_cap_to_object` scan is O(capacity), used only on the cold path for
  mapping bridge decisions (D24, D97).

---

## Object Type Summary

| Type             | Arena             | Rights Bits | Clonable | Linear    | Backing                     | Creation Syscall      | Struct Size |
| ---------------- | ----------------- | ----------- | -------- | --------- | --------------------------- | --------------------- | ----------- |
| Observer         | `Arena<Observer>` | 9           | Yes      | No        | Space (type conversion)     | `CreateObserver` (18) | 128 bytes   |
| Space            | `Arena<Space>`    | 4           | Yes      | No        | Root pool / split           | (none -- split only)  | 40 bytes    |
| Time             | `Arena<Time>`     | 2           | No       | Yes (D38) | Root pool / split           | (none -- split only)  | 16 bytes    |
| Field            | `Arena<Field>`    | 6           | Yes      | No        | Space (type conversion)     | `CreateField` (13)    | 200 bytes   |
| Pulsar           | `Arena<Pulsar>`   | 2           | Yes      | No        | Space (type conversion)     | `CreatePulsar` (16)   | (varies)    |
| Capability Table | (per-Observer)    | --          | --       | --        | Observer structural backing | (implicit)            | --          |

Five per-type arenas (D53). Lock ordering: `Arena<Field>` < `Arena<Observer>` <
`Arena<Pulsar>`. `Arena<Space>` and `Arena<Time>` are unordered.
