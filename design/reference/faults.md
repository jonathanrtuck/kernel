# Fault Handling Reference

Faults in this kernel are IPC messages. When an Observer encounters a fault
condition, the kernel constructs a message and delivers it to the Observer's
designated handler Field. The fault handler -- a separate Observer receiving on
that Field -- inspects the fault, resolves it, and resumes the faulted Observer.
There is no separate fault mechanism; faults compose entirely from the existing
IPC and capability primitives.

Source of truth: `src/fault.rs` (fault types, message construction, delivery),
`src/core_manager.rs` (dispatch integration, chain terminus). When this document
and the code disagree, the code wins.

---

## Overview

The kernel is the sender. Fault messages flow from the kernel into a handler
Field (D7, D12). The handler Field is identified by the faulting Observer's
capability table slot 0 (`SLOT_FAULT_HANDLER`, D21). The badge on the message
comes from the handler cap entry, not from the faulting Observer -- the handler
uses the badge to identify which child Observer faulted (same pattern as normal
IPC badge injection, D17).

Each fault message carries:

- A **kernel-reserved label** identifying the fault type (in the reserved range
  `0xFFFF_FFFF_FFFF_0000` and above).
- **Four data words** with fault-type-specific content.
- An **Observer capability** with a 5-right subset, granting the handler
  authority to inspect and resolve the fault.
- **No reply capability.** Fault messages are kernel deposits, not RPC calls.
  The handler resolves the fault through typed operations on the Observer cap,
  not by replying.

The faulting Observer transitions to the `Faulted` state (D39) before the
message is delivered. It is descheduled and remains stopped until the handler
calls `ObserverResume`.

For IPC mechanics (message format, queuing, direct delivery, overflow) see
[ipc.md](ipc.md). This document covers only the fault-specific protocol.

---

## Fault Message Format

Fault messages use the standard D28 fixed message format. The kernel fills the
fields as follows:

### Register layout on receive (x0-x7)

| Register | Field          | Content                                                                          |
| -------- | -------------- | -------------------------------------------------------------------------------- |
| x0       | data[0]        | Fault-type-specific (see per-type tables below)                                  |
| x1       | data[1]        | Fault-type-specific                                                              |
| x2       | data[2]        | Fault-type-specific                                                              |
| x3       | data[3]        | Reserved (always 0)                                                              |
| x4       | label          | Kernel-reserved fault label (identifies fault type)                              |
| x5       | badge          | Handler cap's badge (identifies faulting Observer to the handler)                |
| x6       | user cap slot  | Handle to the fault Observer cap (5-right subset, installed in receiver's table) |
| x7       | reply cap slot | `u64::MAX` (no reply cap -- kernel-as-sender, not Call)                          |

### Fault labels

| Label                   | Constant                   | Fault type         |
| ----------------------- | -------------------------- | ------------------ |
| `0xFFFF_FFFF_FFFF_0003` | `LABEL_VM_FAULT`           | Virtual memory     |
| `0xFFFF_FFFF_FFFF_0004` | `LABEL_RESOURCE_REQUEST`   | Resource request   |
| `0xFFFF_FFFF_FFFF_0005` | `LABEL_CAP_TABLE_FULL`     | Cap table full     |
| `0xFFFF_FFFF_FFFF_0006` | `LABEL_HARDWARE_EXCEPTION` | Hardware exception |

Labels are defined in `src/field.rs`. They occupy the kernel-reserved range and
cannot collide with user-chosen labels.

---

## The Four Fault Types

### VmFault

A page fault: the Observer accessed memory outside the virtual address ranges
covered by its Space mappings.

**When it occurs:** the hardware raises a data abort or instruction abort at EL1
because no valid page table entry exists for the faulted address. The kernel
translates the raw fault into Space-relative coordinates (D26: virtual addresses
are kernel-internal, not exposed to userspace).

**Data words:**

| Word    | Register | Content                  | Type / encoding                                                              |
| ------- | -------- | ------------------------ | ---------------------------------------------------------------------------- |
| data[0] | x0       | Space slot index         | `u32` as `u64` -- cap table slot of the Space that should cover this address |
| data[1] | x1       | Byte offset within Space | `u64` -- offset from the Space's VA base                                     |
| data[2] | x2       | Access type              | `0` = Read, `1` = Write, `2` = Execute                                       |
| data[3] | x3       | Reserved                 | `0`                                                                          |

**Label:** `LABEL_VM_FAULT` (`0xFFFF_FFFF_FFFF_0003`).

**Resolution:** the handler must map the faulted page. The typical sequence:

1. Read data[0] and data[1] to identify which Space and offset need backing.
2. Split a page from a Space the handler owns (`SpaceSplit`).
3. Install the new Space cap into the faulted Observer's table
   (`ObserverInstallCap` using the fault Observer cap).
4. Resume the faulted Observer (`ObserverResume`).

The Observer re-executes the faulting instruction. If the page is now mapped,
execution continues. If the fault recurs, a new VmFault is delivered.

### ResourceRequest

An Observer explicitly requested a resource (Space or Time) through the
`ResourceRequest` typed operation (code 19, D31). For non-root Observers, the
kernel converts this into a fault message to the handler Field rather than
allocating directly.

**When it occurs:** a non-root Observer issues `SVC #0` with operation code 19.
The kernel checks for a valid handler at slot 0, constructs the fault message,
and delivers it. Root Observers (those with no handler at slot 0) are handled
differently -- the kernel allocates directly from the pool (D104).

**Data words:**

| Word    | Register | Content       | Type / encoding                                |
| ------- | -------- | ------------- | ---------------------------------------------- |
| data[0] | x0       | Resource type | `0` = Space, `1` = Time                        |
| data[1] | x1       | Quantity      | `u64` -- bytes (Space) or compute units (Time) |
| data[2] | x2       | Reserved      | `0`                                            |
| data[3] | x3       | Reserved      | `0`                                            |

**Label:** `LABEL_RESOURCE_REQUEST` (`0xFFFF_FFFF_FFFF_0004`).

**Resolution:** the handler provides the requested resource:

1. Read data[0] to determine whether Space or Time is requested.
2. Read data[1] for the quantity.
3. Split the appropriate resource from the handler's own pool (`SpaceSplit` or
   `TimeSplit`).
4. Install the new capability into the faulted Observer's table
   (`ObserverInstallCap`).
5. Resume the faulted Observer (`ObserverResume`).

The faulted Observer's original `ResourceRequest` syscall returns the handle of
the newly installed capability. From the Observer's perspective, the syscall
succeeded -- it never observes the fault.

### CapTableFull

The Observer's capability table has no free slots and a kernel operation needs
to install a new capability (D8, D40).

**When it occurs:** during any operation that would install a capability (IPC
receive with cap transfer, `Clone`, `SpaceSplit`, etc.), the kernel discovers
the target Observer's table has no free slots. The kernel saves the in-progress
syscall context and delivers a CapTableFull fault.

**Data words:**

| Word    | Register | Content  | Type / encoding |
| ------- | -------- | -------- | --------------- |
| data[0] | x0       | Reserved | `0`             |
| data[1] | x1       | Reserved | `0`             |
| data[2] | x2       | Reserved | `0`             |
| data[3] | x3       | Reserved | `0`             |

**Label:** `LABEL_CAP_TABLE_FULL` (`0xFFFF_FFFF_FFFF_0005`).

**Resolution:** the handler must provide Space for table growth:

1. Split a Space from the handler's pool (`SpaceSplit`).
2. Install it into the faulted Observer's table as the growth backing
   (`ObserverInstallCap`).
3. Resume the faulted Observer (`ObserverResume`).

On resume, the kernel replays the saved syscall transparently. The Observer
never observes that a table growth occurred.

### HardwareException

A hardware exception that the kernel does not handle internally (D61). This
covers illegal instructions, alignment faults, debug exceptions, and any other
synchronous exception from EL0 that is not a page fault or syscall.

**When it occurs:** the hardware takes a synchronous exception to EL1 with an
exception class that the kernel does not recognize as a handled fault type. The
kernel captures the ARM64 exception registers and delivers them as a fault
message.

**Data words:**

| Word    | Register | Content  | Type / encoding                                   |
| ------- | -------- | -------- | ------------------------------------------------- |
| data[0] | x0       | ESR_EL1  | `u64` -- Exception Syndrome Register (full value) |
| data[1] | x1       | ELR_EL1  | `u64` -- Exception Link Register (faulting PC)    |
| data[2] | x2       | FAR_EL1  | `u64` -- Fault Address Register (if applicable)   |
| data[3] | x3       | Reserved | `0`                                               |

**Label:** `LABEL_HARDWARE_EXCEPTION` (`0xFFFF_FFFF_FFFF_0006`).

ESR_EL1 encodes the exception class in bits [31:26] and the instruction-specific
syndrome in bits [24:0]. Common exception classes the handler may encounter:

| ESR_EL1[31:26] | Exception class                          |
| -------------- | ---------------------------------------- |
| `0b000000`     | Unknown reason                           |
| `0b001110`     | Illegal execution state (AArch32 in EL0) |
| `0b101100`     | BRK instruction (software breakpoint)    |
| `0b100101`     | Data abort from EL0                      |
| `0b100001`     | Instruction abort from EL0               |

**Resolution:** depends on the exception class:

- **BRK (debug/test exit):** the handler may destroy the Observer or log and
  resume. The BRK immediate is in ESR_EL1[15:0]. The test exit protocol uses
  this to signal pass/fail (D94).
- **Illegal instruction:** the handler may emulate, skip, or destroy the
  Observer. To skip: read registers (`ObserverReadRegisters`), advance PC by 4,
  write back (`ObserverWriteRegisters`), and resume.
- **Other:** inspect ESR_EL1 to determine the cause. The handler decides whether
  to resume, modify state, or destroy.

---

## Fault Observer Capability

Every fault message carries an Observer capability in x6 (the user cap slot).
This capability has a specific 5-right subset chosen for fault resolution (D61,
D100):

| Right           | Bit | Purpose in fault resolution                                                     |
| --------------- | --: | ------------------------------------------------------------------------------- |
| RESUME          |   3 | Transition the faulted Observer back to Runnable                                |
| DESTROY         |   1 | Terminate the Observer if the fault is unrecoverable                            |
| INSTALL_CAP     |   5 | Install capabilities into the Observer's table (map pages, provide resources)   |
| WRITE_REGISTERS |   6 | Modify the Observer's PC, SP, x0, PSTATE (skip faulting instruction, fix state) |
| READ_REGISTERS  |   7 | Inspect the Observer's register state for diagnostics                           |

These five rights are the constant `Rights::FAULT_OBSERVER` in
`src/capability.rs`.

### Why five rights, not all nine

The four excluded rights are not needed for fault resolution and would expand
the handler's authority beyond what the fault protocol requires:

| Excluded right    | Bit | Reason for exclusion                                                                                                   |
| ----------------- | --: | ---------------------------------------------------------------------------------------------------------------------- |
| SUSPEND           |   8 | The Observer is already stopped (Faulted). Suspend is redundant.                                                       |
| CHANGE_HANDLER    |   9 | Would allow the handler to redirect faults to a different Field -- an escalation of privilege beyond fault resolution. |
| MODIFY_SCHEDULING |  10 | Scheduling policy changes are not a fault resolution action.                                                           |
| CLONE             |   4 | The handler already holds the cap. Cloning it is not needed for resolution.                                            |

### Construction

The kernel constructs the fault Observer cap directly from its knowledge of the
Observer's arena identity and generation (D100). This is NOT an attenuation of
an existing capability -- the kernel is the sender and creates authority from
scratch, the same pattern as D16 reply cap construction. The cap carries:

- Object type: Observer
- Object ID: the faulting Observer's arena slot
- Rights: `FAULT_OBSERVER` (exactly the 5 rights above)
- Badge: 0 (the handler badge is on the message itself, in x5)
- Send-once: false (the cap is persistent -- the handler can use it for multiple
  operations before closing it)
- Stored generation: the Observer's current generation

---

## Resolution Protocol

### General pattern

All fault types follow the same general sequence:

1. **Receive the fault message** from the handler Field (normal `SVC #2` Receive
   or `SVC #4` ReplyRecv).
2. **Identify the fault** from x4 (label) and x5 (badge).
3. **Extract the fault Observer cap** from x6.
4. **Inspect the fault** from x0-x2 (data words) and optionally read the
   Observer's registers (`ObserverReadRegisters`).
5. **Resolve the fault** by performing typed operations on the fault Observer
   cap (install capabilities, write registers).
6. **Resume the Observer** (`ObserverResume` on the fault Observer cap).
7. **Close the fault Observer cap** (`Close`) to release the cap table slot.

### Per-type resolution summary

| Fault type        | Typical resolution                                                                          |
| ----------------- | ------------------------------------------------------------------------------------------- |
| VmFault           | Map the faulted page: `SpaceSplit` + `ObserverInstallCap` + `ObserverResume`                |
| ResourceRequest   | Provide the resource: `SpaceSplit` or `TimeSplit` + `ObserverInstallCap` + `ObserverResume` |
| CapTableFull      | Provide growth Space: `SpaceSplit` + `ObserverInstallCap` + `ObserverResume`                |
| HardwareException | Depends on exception class. May resume, skip, or destroy.                                   |

### Unrecoverable faults

If the handler cannot resolve the fault (out of resources, unknown exception),
it should destroy the faulted Observer using the DESTROY right on the fault
Observer cap. This initiates the preemptible cascade (D33, D98) and returns the
Observer's backing Space to the destroyer.

---

## State Transitions

### Observer lifecycle during a fault

```text
Runnable ──── fault() ────> Faulted ──── resume() ────> Runnable
                              │
                              │ (handler calls Destroy)
                              ▼
                           Destroyed
```

1. The Observer is `Runnable` (executing or in the run queue).
2. A fault condition occurs (page fault, resource request, cap table full,
   hardware exception).
3. The kernel calls `observer.fault()`, transitioning the state to `Faulted`
   (D39). The Observer is removed from the scheduler queue.
4. The kernel constructs and delivers the fault message to the handler Field.
5. The handler receives the message, resolves the fault, and calls
   `ObserverResume` (code 0) on the fault Observer cap.
6. `resume()` transitions the Observer from `Faulted` to `Runnable`. It is
   enqueued in the scheduler and eventually re-executes the faulting instruction
   (or continues after it, if the handler advanced PC).

The `fault()` transition is valid only from `Runnable`. The `resume()`
transition is valid from both `Inert` (first start) and `Faulted` (fault
resolution). All other transitions return `InvalidTransition` (D39).

### Suspension overlay

The suspension flag (D39) is orthogonal to the primary state. A `Faulted`
Observer can also be suspended. When the handler calls `ObserverResume`, both
the fault and the suspension are cleared -- the Observer transitions to
`Runnable`.

---

## Chain Terminus Rule (D68)

If an Observer's fault handler slot (slot 0) is empty, has the wrong type, has
insufficient rights, or points to a destroyed Field (stale generation), the
kernel cannot deliver the fault. This is the **chain terminus** -- the
supervision chain has no further handler to escalate to.

### What the kernel does

The kernel acts as the root fault handler (D68, D100):

1. Logs diagnostic information to serial:
   - Fault label (hex)
   - All four data words (hex)
   - The faulting PC (hex)
2. Returns `DispatchResult::FatalFault`.
3. The frame layer calls PSCI `SYSTEM_OFF`, cleanly shutting down the system.

This is the intended behavior for the root Observer. The root Observer's slot 0
is empty at boot (see [boot.md](boot.md)) -- there is no higher handler. Any
unrecoverable fault in the root Observer terminates the system.

### Test exit protocol

Bare-metal test binaries exploit the chain terminus rule (D94). A test signals
completion by executing `BRK #imm16`:

- The hardware takes a synchronous exception (ESR class `0b101100`).
- The kernel attempts to deliver a HardwareException fault.
- Slot 0 is empty (root Observer), so the chain terminus applies.
- The kernel logs and calls PSCI `SYSTEM_OFF`.
- The hypervisor reads the exit code from the VCPU state.

This is a deliberate design: the test does not need a fault handler; the
kernel's chain terminus behavior provides clean shutdown.

### Non-root Observers

For non-root Observers, the handler at slot 0 is typically set during creation
via `CreateObserver` (which takes a handler Field cap as an argument). If that
handler Field is later destroyed (making the cap stale), faults on the child
Observer also trigger the chain terminus. The kernel does not attempt to
escalate to a grandparent handler -- supervision chains are flat, not recursive.

---

## Handler Field Overflow (D18)

When the kernel delivers a fault message and the handler Field's queue is full,
three outcomes are possible depending on the Field state:

| Handler Field state     | Outcome                                                           |
| ----------------------- | ----------------------------------------------------------------- |
| Receiver waiting        | Direct delivery (D13) -- message bypasses the queue entirely      |
| Queue has space         | Message enqueued normally                                         |
| Queue full, no receiver | `Deferred` -- Observer linked into the Field's pending list (D18) |

### Deferred delivery

When delivery is deferred, the faulting Observer remains in `Faulted` state. The
fault message is stored as a pending entry on the handler Field. On the next
`Receive` (or ReplyRecv) that dequeues a message and frees a queue slot, the
kernel drains the pending list: the deferred fault message is delivered to the
receiver, and the faulting Observer becomes eligible for handler attention.

The handler does not need to do anything special -- deferred faults arrive as
normal messages once queue space becomes available. The only observable effect
is increased latency between the fault and the handler receiving it.

### Zero-capacity Fields

A handler Field with zero queue capacity and no waiting receiver always defers.
This is a valid but degenerate configuration -- the handler must already be
blocked on Receive when the fault occurs for delivery to succeed.

---

## Examples

### Assembly: VmFault handler loop

A minimal page-fault handler that maps pages from a pool Space:

```asm
    // x19 = handler Field handle (receive cap)
    // x20 = pool Space handle (for splitting pages)
fault_loop:
    mov     x5, x19
    svc     #2                  // Receive on handler Field
    b.cs    .recv_error         // should not happen

    // x4 = label, x5 = badge (identifies faulting Observer)
    // x6 = fault Observer cap handle
    // x0 = data[0] (Space slot index for VmFault)
    // x1 = data[1] (byte offset)
    // x2 = data[2] (access type)

    // Check label is VM fault
    movz    x8, #0x0003
    movk    x8, #0xFFFF, lsl #16
    movk    x8, #0xFFFF, lsl #32
    movk    x8, #0xFFFF, lsl #48
    cmp     x4, x8
    b.ne    .not_vm_fault

    // Save fault Observer cap handle
    mov     x21, x6

    // Split a page from the pool Space
    mov     x4, #11             // op_code: SpaceSplit
    mov     x5, x20             // target: pool Space
    mov     x0, #16384          // 16 KiB (one page)
    svc     #0
    tbnz    x0, #63, .split_error
    mov     x22, x0             // x22 = new Space cap handle

    // Install the new Space into the faulted Observer's table
    mov     x4, #1              // op_code: ObserverInstallCap
    mov     x5, x21             // target: fault Observer cap
    mov     x0, x22             // source: the new Space cap
    svc     #0
    tbnz    x0, #63, .install_error

    // Resume the faulted Observer
    mov     x4, #0              // op_code: ObserverResume
    mov     x5, x21             // target: fault Observer cap
    svc     #0

    // Close the fault Observer cap (free the slot)
    mov     x4, #9              // op_code: Close
    mov     x5, x21
    svc     #0

    b       fault_loop
```

### Rust: dispatching on fault type

```rust
let msg = receive(handler_field_handle);

match msg.label {
    LABEL_VM_FAULT => {
        let space_slot = msg.data[0] as u32;
        let byte_offset = msg.data[1];
        let access_type = msg.data[2]; // 0=Read, 1=Write, 2=Execute

        // Split a page and install it
        let page_cap = space_split(pool_handle, PAGE_SIZE);
        observer_install_cap(msg.user_cap, page_cap);
        observer_resume(msg.user_cap);
        close(msg.user_cap);
    }
    LABEL_RESOURCE_REQUEST => {
        let resource_type = msg.data[0]; // 0=Space, 1=Time
        let quantity = msg.data[1];

        match resource_type {
            0 => {
                let space_cap = space_split(pool_handle, quantity);
                observer_install_cap(msg.user_cap, space_cap);
            }
            1 => {
                let time_cap = time_split(time_handle, quantity);
                observer_install_cap(msg.user_cap, time_cap);
            }
            _ => { /* unknown resource type -- destroy the Observer */ }
        }
        observer_resume(msg.user_cap);
        close(msg.user_cap);
    }
    LABEL_CAP_TABLE_FULL => {
        let growth_space = space_split(pool_handle, TABLE_GROWTH_SIZE);
        observer_install_cap(msg.user_cap, growth_space);
        observer_resume(msg.user_cap);
        close(msg.user_cap);
    }
    LABEL_HARDWARE_EXCEPTION => {
        let esr = msg.data[0];
        let elr = msg.data[1];
        let far = msg.data[2];
        let exception_class = (esr >> 26) & 0x3F;

        if exception_class == 0b101100 {
            // BRK -- software breakpoint or test exit
            let brk_immediate = esr & 0xFFFF;
            // Handle or destroy
            observer_destroy(msg.user_cap);
        } else {
            // Unrecoverable -- destroy the faulting Observer
            observer_destroy(msg.user_cap);
        }
    }
    _ => { /* not a fault message */ }
}
```

---

## Derivation References

| Topic                                 | Derivations |
| ------------------------------------- | ----------- |
| Fault delegation model                | D7, D12     |
| Handler Field at slot 0               | D21         |
| D28 message format                    | D28         |
| Resource acquisition pager chain      | D31         |
| Observer lifecycle states             | D39         |
| Cap table growth protocol             | D8, D40     |
| Four fault types and data word layout | D61         |
| Chain terminus rule                   | D68         |
| Error and fault delivery protocol     | D80         |
| Fault Observer cap construction       | D100        |
| ResourceRequest dual-path dispatch    | D104        |
| Overflow and pending list             | D18         |
| Badge injection                       | D17         |
| Test exit protocol                    | D94         |
