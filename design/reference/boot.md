# Boot Protocol Reference

This document describes the kernel boot sequence from power-on through the root
Observer's first instruction. It is a reference for anyone writing the first
userspace program or building a boot-time test binary.

Source of truth: `src/frame/boot.rs` and `src/main.rs`. When this document and
the code disagree, the code wins.

## Overview

The boot sequence has three phases:

1. **Hardware initialization** (assembly and architecture code)
2. **Kernel state construction** (BSP Rust entry point)
3. **Root Observer creation and EL0 entry** (`enter_first_observer`)

The kernel is purely reactive (A4): there is no kernel thread. After creating
the root Observer, the BSP context-switches to EL0 and never returns to the boot
path. The kernel re-enters only through exception vectors.

---

## Phase 1: Hardware Initialization

Power-on begins in `boot.S` (assembly). The firmware or hypervisor starts the
bootstrap processor (BSP) at EL2 with the DTB physical address in x0.

| Step | Code                      | What happens                                                                          |
| ---- | ------------------------- | ------------------------------------------------------------------------------------- |
| 1    | `boot.S`                  | Drop from EL2 to EL1, clear BSS, set up initial stack, jump to `kernel_main`          |
| 2    | `exception::init()`       | Install exception vector table (VBAR_EL1)                                             |
| 3    | `platform::init(dtb_ptr)` | Parse DTB: extract RAM base/size, core count, module location                         |
| 4    | `mmu::init()`             | Build page tables, enable MMU with TTBR0 (identity map) and TTBR1 (kernel linear map) |
| 5    | `serial::enable_lock()`   | Enable UART output with spinlock protection                                           |
| 6    | `entropy::init()`         | Initialize hardware random number generator                                           |
| 7    | `interrupts::init()`      | Configure GICv3 distributor and redistributor                                         |
| 8    | `timer::init()`           | Configure generic timer                                                               |

After Phase 1, the BSP prints `alive` to confirm serial output works.

### DTB Discovery

The DTB blob is at the physical address passed in x0 by the hypervisor. The
kernel's DTB scanner (`frame/firmware/dtb.rs`) extracts three categories of
information:

- **RAM**: base address and size from the `/memory` node
  (`device_type = "memory"`, `reg` property with
  `#address-cells = 2, #size-cells = 2`).
- **Core count**: number of `cpu@N` child nodes under `/cpus`.
- **Module binary**: physical address and size from the `/chosen` node
  (`module-start` and `module-end` properties, both 64-bit big-endian).

The hypervisor loads the userspace flat binary into guest physical RAM and
describes it via the DTB `/chosen` node. This is the standard ARM64 convention
used by U-Boot, GRUB, and hypervisor firmware. The kernel does not embed or
parse ELF -- the binary format is flat with entry point at offset 0 (D94, D102).

---

## Phase 2: Kernel State Construction

`kernel_main` (in `src/main.rs`) constructs the global `KernelState` (D82) and
activates secondary cores.

### Physical Memory Partitioning (D93)

Usable memory starts at `__kernel_end` (the end of the kernel image in RAM) and
extends to the end of physical RAM. Everything before `__kernel_end` -- the
kernel image, DTB blob, boot stack, and any module binary -- is reserved.

```text
0             __kernel_end                    RAM end
|--- reserved ---|---------- usable pool --------|
  kernel image      root pool (SpaceManager)
  DTB blob
  boot stack
```

The usable region becomes the SpaceManager's root pool. All subsequent memory
allocations (arena slab pages, page tables, Observer register states, code/stack
pages) draw from this pool.

### KernelState Construction (D82)

The `KernelState` bundles six lock-wrapped arenas and ancillary state:

- Five per-type arenas: Field, Observer, Pulsar, Space, Time
- SpaceManager (physical memory allocation)
- IRQ routing table (1024-entry direct-indexed by GIC INTID)
- ASID allocator (sequential, 8-bit or 16-bit width from hardware)
- Per-core IPI mailboxes (lock-free circular queues)

Arenas start empty. The first allocation of each type triggers a slab page
request from the SpaceManager's root pool (D70, D93). This resolves the
chicken-and-egg between "arenas need a page allocator" and "the allocator is
constructed at boot" through sequencing, not special-casing.

After `frame::init_kernel_state()` places the bundle in the global static, the
BSP activates secondary cores.

### Secondary Core Activation (D46, D93)

Secondary cores are activated via PSCI `CPU_ON` (HVC calling convention) after
`KernelState` is complete. This ordering guarantee prevents races on the global
state: secondaries can safely call `frame::kernel_state()` because the BSP set
the initialization flag with Release ordering before issuing any `CPU_ON`.

For each secondary core (1 through core_count - 1):

1. BSP calls `psci::cpu_on(target_mpidr, __secondary_entry, stack_top)`.
2. The secondary enters `__secondary_entry` (assembly), sets up its stack, and
   calls `secondary_main`.
3. `secondary_main` initializes: exception vectors, MMU (secondary init), GIC
   redistributor, per-core data (TPIDR_EL1 -> PerCoreData -> CoreState).
4. The secondary enters the idle loop (`__enter_idle`): unmasks IRQs, executes
   WFI, and waits for an IPI.

Each secondary's `CoreState` is initialized with:

- `core_id`: the core's linear index (1, 2, ...)
- `current`: None (no Observer assigned yet)
- `scheduler`: `RoundRobin::new()` (const, no allocation)
- `deadlines`: empty array (capacity: 32 per core)

The BSP waits up to 500 milliseconds for all secondaries to report online, then
continues regardless. The core count is printed to serial (for example,
`4/4 cores online`).

Maximum supported cores: 8 (`config::MAX_CORES`). Kernel stack size per core:
256 KiB (`config::KERNEL_STACK_SIZE`).

---

## Phase 3: Root Observer Creation and EL0 Entry

If a module binary was discovered in the DTB (`module_start != 0`), the BSP
calls `enter_first_observer`. If no module is present, the kernel prints an idle
message, masks interrupts, and halts.

### Memory Layout

The page size is 16 KiB (Apple Silicon native granule). User virtual addresses
are assigned by the kernel (D26) -- Observers never choose addresses.

The root Observer's user address space uses L2 index 0 in the kernel's shared L2
root table (TTBR0 identity map):

| L3 index | Virtual address | Content                    | Permissions                     |
| -------- | --------------- | -------------------------- | ------------------------------- |
| 0        | `0x0000`        | Unmapped (null guard page) | --                              |
| 1        | `0x4000`        | Code page                  | Read-only, EL0-executable       |
| 2        | `0x8000`        | Stack page                 | Read-write, EL0, not executable |

The code page contains the flat binary copied from the DTB module. The binary
must fit in one 16 KiB page (asserted at boot). The stack page is zeroed.

### Initial Register State (D94)

| Register | Value                     | Description                                                   |
| -------- | ------------------------- | ------------------------------------------------------------- |
| PC       | `0x4000` (USER_CODE_VA)   | Entry point: first instruction of the flat binary             |
| SP       | `0xC000` (USER_STACK_TOP) | Top of the stack page (stacks grow downward on AArch64)       |
| PSTATE   | `0`                       | EL0t (AArch64 mode), all condition flags clear, IRQs unmasked |
| x0-x30   | `0`                       | All general-purpose registers zeroed                          |

The root Observer executes at EL0 with interrupts unmasked. The kernel
configures CNTKCTL_EL1 to enable direct EL0 counter access
(`clock_access = true`), so the root Observer can read CNTVCT_EL0 without
trapping.

### Per-Observer Page Table

Each Observer receives its own L1 page table (D89). The L1 table has one entry:
L1[0] points to the kernel's shared L2 root. User pages are installed at L2
indices (0 for the root Observer, 1 for the first child, and so on). The
per-Observer ASID (assigned sequentially from the kernel's ASID allocator, D101)
is encoded in TTBR0 bits[63:48]. The nG (not-global) bit on user page
descriptors plus distinct ASIDs prevent cross-Observer TLB aliasing.

---

## Root Observer Capability Table

The root Observer's capability table has 16 entries (slots 0-15). Three slots
are reserved by the kernel; the remainder are user slots.

### Reserved Slots (D21, D43, D57)

| Slot | Name          | Content                                                            |
| ---- | ------------- | ------------------------------------------------------------------ |
| 0    | Fault handler | Empty at boot (no higher handler -- the kernel is root pager, D68) |
| 1    | Reply field   | Empty at boot (populated by kernel during IPC Call)                |
| 2    | Self-cap      | Observer cap pointing to this Observer, with `OBSERVER_ALL` rights |

### User Slots

| Slot | Object type | Rights         | Badge | Description                                                               |
| ---- | ----------- | -------------- | ----- | ------------------------------------------------------------------------- |
| 3    | Space       | `SPACE_ALL`    | 0     | Root Space -- all remaining usable physical memory after boot allocations |
| 4    | Field       | `RECEIVE`      | 0     | IPC Field (receive end) -- for receiving messages from child Observers    |
| 5    | Field       | `RECEIVE`      | 0     | Handler Field (receive end) -- for receiving fault messages from children |
| 6    | Field       | `RECEIVE`      | 0     | Timer Field (receive end) -- for receiving Pulsar timer fire messages     |
| 7    | Observer    | `OBSERVER_ALL` | 0     | Child Observer cap -- for controlling the boot-time child Observer        |

Slots 8-15 are free (linked into the freelist). The freelist head starts at
slot 8.

Total installed capabilities: 6 (self-cap at slot 2, plus 5 user caps at slots
3-7). `cap_table_count = 6`.

### Slot Numbering Constants

These constants are defined in `src/capability.rs`:

```text
SLOT_FAULT_HANDLER = 0
SLOT_REPLY_FIELD   = 1
SLOT_SELF          = 2
SLOT_USER_START    = 3
```

User slots begin at index 3. The first user slot (slot 3) always contains the
root Space cap. Subsequent slots are allocated sequentially during boot.

### Handle Encoding (D77)

Userspace presents capability handles as u64 values in registers. The encoding
is:

- Lower 16 bits: slot index
- Upper 48 bits: slot tag (generational ABA defense, D11)

At boot, all slot tags are 0. The root Observer's self-cap handle encodes as
`0x0000_0000_0000_0002` (index 2, tag 0). The root Space handle encodes as
`0x0000_0000_0000_0003` (index 3, tag 0).

---

## Root Space

The root Space (slot 3) represents all physical memory remaining after boot
allocations. Its fields:

- `va_base`: physical address of the first free page (same as
  `SpaceManager.next_va_base` at the time of creation)
- `size`: total free bytes remaining in the root pool
- `l3_table_pa`: physical address of a populated L3 page table mapping these
  pages

This is the memory the root Observer can subdivide (via `SpaceSplit`) to create
child Observers, Fields, Pulsars, and additional Spaces. The kernel retains no
memory beyond what it already consumed for boot-time structures -- the root
Space IS all remaining memory.

The root Space's size depends on the total RAM and how much was consumed during
boot (kernel image, page tables, arena slab pages, per-core stacks, boot-time
Objects). On a system with 256 MiB of RAM and a small kernel image, the root
Space will be approximately 250+ MiB.

---

## Boot-Time Child Observer

The current boot sequence creates one child Observer for integration testing.
This is kernel-internal scaffolding, not part of the permanent boot protocol. It
demonstrates the multi-Observer bootstrap pattern that userspace will eventually
perform through syscalls.

The child Observer:

- Has its own address space (L2 index 1, VA base `0x200_0000`)
- Has a 4-slot capability table (handler, self-cap, IPC send cap, Space cap)
- Is either enqueued on the BSP scheduler (single-core) or migrated to core 1
  via IPI (multi-core)

The child's IPC send cap carries badge `0x99`. On multi-core systems, the child
is migrated to core 1 via `IpiRequest::ObserverMigration`, exercising the full
SMP path: PSCI boot, SGI delivery, mailbox drain, Observer migration, and
cross-core IPC.

---

## Boot-Time Pulsar

A one-shot Pulsar is created at boot for timer integration testing:

- Duration: 50 milliseconds
- Badge: `0xBEEF`
- Target: the timer Field at root Observer slot 6
- Period: 0 (one-shot, does not repeat)

The Pulsar's deadline is installed in the BSP's per-core deadline array. When
the timer fires, the kernel constructs a `timer_fire` message with badge
`0xBEEF` and enqueues it on the timer Field.

---

## Context Switch to EL0

The final steps before the root Observer begins executing:

1. Initialize BSP per-core data: write `PerCoreData` to a static, set TPIDR_EL1
   to point to it. This must happen before any exception handler runs.
2. Set the root Observer as the BSP's current Observer and enqueue it in the
   round-robin scheduler.
3. Install the Pulsar deadline in the BSP's deadline array.
4. Create and schedule the child Observer.
5. Call `__restore_observer(register_state_ptr, page_table_root, clock_access)`
   -- this assembly routine loads TTBR0, restores all registers from the
   RegisterState, and executes ERET to drop to EL0.

`__restore_observer` does not return. From this point, the kernel executes only
in response to exceptions (syscalls, interrupts, faults).

---

## What the Root Observer Should Do

The root Observer is the first and initially only userspace program. It is
responsible for bootstrapping the rest of the system. The standard pattern:

1. **Discover initial capabilities.** The root Observer knows its slot layout
   (slots 0-2 are reserved, user slots start at 3). It holds the root Space at
   slot 3.

2. **Create Fields for IPC.** Use `CreateField` (typed syscall, SVC #0) with a
   Space cap to create communication channels. Install send caps in child
   Observers, keep receive caps.

3. **Split Space for children.** Use `SpaceSplit` to carve off portions of the
   root Space for child Observer backing and code/data pages.

4. **Create child Observers.** The 5-step composable sequence (D35, D102):
   - `SpaceSplit` -- allocate backing Space
   - `CreateObserver(space_cap, handler_field_cap, badge)` -- creates inert
   - `ObserverInstallCap(observer_cap, code_space_cap, slot)` -- map code
   - `ObserverWriteRegisters(observer_cap, ...)` -- set PC, SP
   - `ObserverResume(observer_cap)` -- transition Inert to Runnable

5. **Delegate resources.** Send Space and Time caps to children via IPC.
   Children acquire additional resources through the pager chain (D31): resource
   request syscalls are routed to the fault handler, which is a Field receive
   cap held by the parent.

6. **Handle faults.** Receive on the handler Field to get fault messages from
   children (page faults, capability faults, resource requests). Resolve faults
   by installing caps, writing registers, and resuming the faulted Observer.

### Test Exit Protocol (D94, D68)

For test binaries, the root Observer signals completion via a deliberate fault.
The kernel convention: execute `BRK #imm16` where the immediate encodes the test
result. The kernel sees the fault on the root Observer, and because slot 0
(fault handler) is empty, the chain terminus rule (D68) applies -- the kernel
destroys the Observer and calls PSCI `SYSTEM_OFF` to cleanly shut down the
virtual machine.

The hypervisor reads the exit code from the VCPU state to determine pass/fail.

---

## Summary of Derivation References

| Derivation | Topic                                   | Relevance to boot                                                            |
| ---------- | --------------------------------------- | ---------------------------------------------------------------------------- |
| D31        | Resource acquisition, boot architecture | Kernel retains root pool; root Observer gets minimal initial resources       |
| D46        | Core lifecycle                          | All cores activated before root Observer creation; cores are kernel-internal |
| D82        | KernelState bundle                      | Global state constructed by BSP, placed in MaybeUninit static                |
| D83        | PerCoreData layout                      | TPIDR_EL1 -> PerCoreData -> CoreState; initialized per-core at boot          |
| D88        | TTBR split                              | TTBR0 = user pages (identity map), TTBR1 = kernel linear map                 |
| D89        | Per-Observer L1 page table              | Each Observer gets its own L1; L1[0] chains to kernel L2 root                |
| D93        | Boot memory and multi-core init         | Physical memory partitioning; arena page source; secondary core handoff      |
| D94        | Root Observer bootstrap protocol        | DTB module discovery; initial resources; register state; test exit           |
| D101       | ASID allocator                          | Sequential ASID assignment; ASID 0 reserved for global entries               |
| D102       | Test infrastructure                     | Flat binary format; hypervisor + DTB module discovery; bootstrap patterns    |
