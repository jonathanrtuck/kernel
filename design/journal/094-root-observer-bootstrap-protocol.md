# D94 — Root Observer bootstrap protocol

**Question:** How does the kernel discover the initial binary, create the root
Observer, and set up its address space and register state so it can begin
executing?

**Rests on:** D31 (boot architecture — kernel retains pool, root gets minimal),
D26 (kernel-assigned VA — Observer never chooses addresses), D32 (type
conversion — Space consumed becomes object backing), D35 (composable Observer
setup — create, install cap, write registers, resume), D46 (BSP creates root
Observer — all cores active first), D68 (chain terminus — kernel destroys
faulting Observer when no higher handler exists), D88 (TTBR split — TTBR0 user,
TTBR1 kernel), A3 (generic kernel — workload-independent binary), A4 (purely
reactive — no kernel thread), A5 (kernel absorbs complexity).

**Status:** settled.

---

## Settles

### Initial binary source

The initial binary is discovered from DTB, not embedded in the kernel image. The
bootloader (or hypervisor, in the development environment) loads the binary into
guest physical RAM and describes it via a DTB module node — the standard ARM64
convention used by U-Boot, GRUB, and hypervisor firmware.

The kernel parses the module node to find:

- Physical address of the binary in RAM.
- Size in bytes.

This preserves A3: the kernel binary is workload-independent. Different
workloads are loaded by the bootloader, not by recompiling the kernel. The
kernel's TCB does not include binary-format-specific parsing logic.

The DTB module node path and property names follow the devicetree specification
(`/chosen` node or a `/memory`-adjacent module description). The exact schema is
a firmware-interface concern settled at hardware-port time.

### Root Observer initial resource quantities

The root Observer receives a modest initial allocation — not all remaining RAM.
The kernel retains the majority as the root pool (D31).

| Resource      | Initial quantity                                        | Rationale                                |
| ------------- | ------------------------------------------------------- | ---------------------------------------- |
| Space (code)  | Enough to back the initial binary's pages               | Binary must be mapped and executable     |
| Space (stack) | Enough for a reasonable initial stack (e.g., 1-4 pages) | Observer needs stack to execute any code |
| Time          | Enough compute units for initial setup                  | Observer needs scheduling capacity       |

The root Observer requests additional Space and Time through the pager chain
mechanism (D31): syscall to kernel (root pager) which allocates from the root
pool or denies. This is the normal resource acquisition path — boot is not
special.

Exact quantities are tuning parameters, not design decisions. The design
invariant is: root Observer starts with enough to run, not enough to do
everything. The kernel retains the pool for:

- Arena slab page growth (D70 — as more kernel objects are created).
- Type conversion metadata (D32 — page table structures for new Spaces).
- Future grants through the pager chain.

### Root Observer initial address space

D26: the kernel assigns VA bases at Space creation time. The root Observer's
initial Spaces get kernel-chosen VA bases. No Observer — including the root —
ever chooses or manages virtual addresses.

The boot path constructs the root Observer's page tables directly:

1. Create a code Space from the root pool (physical pages = initial binary's
   pages). D32 type conversion allocates content pages and L3 table(s).
   `SpaceManager::create_space()` orchestrates this.
2. Create a stack Space from the root pool. Same path.
3. Assign VA bases to both Spaces via `SpaceManager::assign_va()`. The bases are
   32 MiB aligned (D89) for L3-table-aligned mapping.
4. Populate L3 table entries (D90) mapping the code Space's pages.
5. Populate L3 table entries for the stack Space.
6. Construct the root Observer's L1 page table (D92: charged to Observer's
   consumed Space from D35). Install L2 entries connecting to the L3 tables.
7. Set TTBR0 for the root Observer's address space (D88: user-range page table).

This happens before `ObserverResume` — the root Observer never executes with an
incomplete address space. A5: the kernel absorbs the complexity of initial
address space construction, just as it absorbs VA management for all subsequent
Spaces.

### Root Observer initial register state

The kernel sets the root Observer's initial registers before first execution:

| Register | Value                                                                                                  | Rationale                                                                     |
| -------- | ------------------------------------------------------------------------------------------------------ | ----------------------------------------------------------------------------- |
| PC       | Entry point of the initial binary (VA base of code Space + 0 for flat binary, or DTB-specified offset) | Where execution begins                                                        |
| SP       | Top of the initial stack (VA base of stack Space + stack size)                                         | AArch64 stacks grow downward                                                  |
| x0       | Number of initial capabilities in the cap table                                                        | Userspace knows its starting inventory size                                   |
| x1-x7    | 0 (reserved for future use)                                                                            | Minimal convention — avoids encoding decisions that restrict future extension |

This is a minimal convention. The root Observer discovers kernel state through
syscalls, not boot registers. Passing a DTB pointer, memory map, or rich boot
info in x0-x7 was rejected — the root Observer has no use for DTB (the kernel
already parsed it), and memory state is queried through the pager chain.

The single x0 value (cap count) tells userspace how many capabilities it holds
at startup, so it can iterate its cap table without guessing. Everything else is
discoverable through syscalls.

### Test exit and debug protocol

Two mechanisms for kernel-mode testing:

**(a) Fault-based exit.** The root Observer deliberately triggers a fault (e.g.,
an undefined instruction or a specific SVC code). The kernel sees a fault on the
root Observer. D68 Case C applies: the chain terminus is the kernel (root
pager), and no higher-level supervisor exists. The kernel destroys the faulting
Observer.

When the last Observer is destroyed (or specifically the root Observer faults at
chain terminus), the kernel calls PSCI `SYSTEM_OFF`. The hypervisor interprets
this as clean VM termination. An exit code can be conveyed through a register
convention (e.g., x0 at the point of the deliberate fault) — the hypervisor
reads it from the VCPU state.

This reuses existing mechanism (D68). No test-specific kernel code is added,
preserving A3.

**(b) Serial debug output.** The boot path provides a device-memory Space
capability covering the UART MMIO region (PL011 at the DTB-discovered base
address). The root Observer can write directly to UART for debug output. This is
also the first step toward a userspace serial driver — the same Space cap that
enables boot-time debug output becomes the serial driver's device-memory
mapping.

The UART Space is device memory (not normal RAM). The kernel marks it
appropriately in the page table attributes (D90: device-nGnRnE or device-nGnRE,
depending on the UART's requirements). The root Observer receives a cap to this
Space with appropriate rights.

### Test binary format

Flat binary: the entry point is at offset 0, no ELF header, no format parsing in
the kernel. The kernel maps the binary's physical pages into the code Space's VA
range and sets PC to the VA base.

This is consistent with the microkernel pattern. No surveyed microkernel parses
ELF in-kernel:

- seL4: ELF parsing in the root task loader (elfloader), not the kernel.
- Zircon: userboot is a flat VDSO image; ELF loading is in userspace.
- L4 family (Pistachio, Fiasco.OC): sigma0/roottask handle ELF.
- EROS/Coyotos: checkpoint images, no ELF in kernel.
- QNX: procnto loads binaries; microkernel does not parse.

ELF parsing would add ~500-1000 LOC to the kernel's TCB, handling section
headers, program headers, relocations, and format validation. The flat binary
format moves this complexity to the build toolchain (objcopy) and future
userspace loaders.

### Multi-Observer test bootstrap

D35's composable Observer creation API provides a complete sequence for creating
child Observers from the root Observer:

1. `SpaceSplit(parent_space_cap, size)` — allocate Space for the new Observer's
   structural backing.
2. `CreateObserver(space_cap, handler_field_cap, badge)` — create the Observer
   in inert state. Space is consumed (D32 type conversion).
3. `ObserverInstallCap(observer_cap, code_space_cap, slot)` — install code Space
   cap so the Observer has executable memory.
4. `ObserverWriteRegisters(observer_cap, register_state)` — set PC, SP,
   arguments.
5. `ObserverResume(observer_cap)` — transition from inert to runnable.

This 5-step sequence composes from existing operations. No additional kernel
surface is needed. The same sequence works for all Observer creation — the root
Observer creating a child, or any supervisor creating a subordinate.

---

## Rejected alternatives

- **Embedded binary (`include_bytes!`).** Couples the kernel binary to a
  specific workload. Every workload change requires recompiling the kernel.
  Violates A3 (generic kernel). Also inflates the kernel image, wasting memory
  on systems where the initial binary is large.

- **ELF format.** No surveyed microkernel parses ELF in-kernel (see above). Adds
  ~500-1000 LOC of format-specific parsing to the TCB. The flat binary format is
  simpler and sufficient — the build toolchain (objcopy) handles the conversion.
  ELF loading belongs in userspace loaders.

- **All remaining RAM to root Observer.** The kernel needs the root pool for
  arena slab page growth (D70), type conversion metadata (D32), and future
  grants through the pager chain. Giving everything to the root Observer would
  require it to grant memory back to the kernel for internal use — inverting the
  pager chain model (D31). The kernel should retain the pool and grant on
  request.

- **Rich boot info in x0-x7 (DTB pointer, memory map, core count).** The root
  Observer has no use for DTB — the kernel already parsed it. Memory state is
  queried through the pager chain. Core count is invisible to Observers (D46).
  Encoding rich boot info creates a versioned ABI contract between kernel and
  first userspace binary that must be maintained across kernel evolution. The
  minimal x0 = cap count is sufficient and stable.

- **Test exit via special syscall.** Adds a test-specific operation to the
  kernel's syscall surface. Violates A3 (workload-specific kernel code). The
  fault-based exit reuses D68's existing chain-terminus mechanism — the kernel
  already knows what to do when the root Observer faults with no higher handler.

- **Test exit via serial only (no clean VM termination).** Serial output is
  useful for debug but provides no clean termination mechanism. The
  hypervisor/test harness needs a way to distinguish "test passed" from "test
  hung" — PSCI SYSTEM_OFF gives a clean exit code path.

---

## Does NOT settle

- DTB module node schema (exact path and property names — firmware interface
  concern, settled at hardware-port time).
- Exact initial Space sizes (tuning parameters — enough to run, not enough to do
  everything).
- Exact initial Time quantity (tuning parameter — enough for setup, acquired
  through pager chain afterward).
- UART Space attributes (device-nGnRnE vs. device-nGnRE — hardware-port concern,
  depends on UART peripheral requirements).
- Root Observer cap table layout beyond the fault handler slot (D21) and initial
  Space/Time caps (the cap count in x0 lets userspace iterate without a fixed
  layout contract).
- Multi-binary boot (loading multiple initial binaries for multi-server setups —
  future extension; single root Observer is sufficient for initial development).
