# Bootstrap Authority: Initial Capability Graph and Object Creation

## The Question

When the kernel completes hardware initialization — MMU active, interrupts
routed, secondary cores parked — it must start one userspace entity and hand it
something. Three coupled sub-questions arise:

1. **Creation authority model.** What kernel mechanism allows userspace to
   create kernel objects (address spaces, execution units, communication
   endpoints, time/scheduling objects)? Who holds that authority and how is it
   initially distributed?

2. **Initial capability graph.** What objects exist at the moment userspace
   first executes, and exactly what authority does the first userspace entity
   hold?

3. **Root fault handling.** The first userspace entity has no peer to route its
   faults to. What do real systems do when the root entity faults?

These questions are structurally coupled: the answer to (1) determines what can
appear in (2), and the answer to (2) constrains the options for (3).

---

## 1. Object Creation Authority

### 1.1 seL4 — Untyped Retype

seL4's object creation mechanism is `seL4_Untyped_Retype`. Every kernel object
(TCB, CNode, Endpoint, PageTable, Frame, SchedContext) is created by invoking an
Untyped capability — a capability that represents a contiguous region of
physical memory not yet assigned to any kernel object type.

`seL4_Untyped_Retype(untyped_cap, type, size_bits, root, node_index, node_depth, node_offset, num_objects)`

The caller supplies:

- The Untyped capability to draw memory from.
- The target object type (seL4_TCBObject, seL4_EndpointObject,
  seL4_ARM_PageTableObject, etc.).
- The size in bits (for variable-size objects like CNodes).
- A CNode address indicating where to place the resulting capability.

**Authority chain:** Only the holder of an Untyped capability can create objects
from that memory. Untyped memory cannot be fabricated — it can only be split
(via Retype into a smaller Untyped) or converted into a typed object. The
authority to create objects thus reduces to: "who holds untyped capabilities?"

**No ambient authority.** There is no kernel call to create a TCB without
supplying an Untyped that backs it. The kernel keeps no "root authority" handle
— even the initial thread must build its capability tree from explicitly-held
Untypeds.

**Typed enforcement.** Once memory is retyped, it cannot be retyped again until
all derived capabilities are revoked and the Untyped is made available again.
This is enforced via the Capability Derivation Tree (CDT): `seL4_Untyped_Retype`
checks that the target range has no live children before proceeding.

**Source:** seL4 Reference Manual 14.0.0, §2.4 (Untyped Memory); seL4 Untyped
tutorial (https://docs.sel4.systems/Tutorials/untyped.html).

### 1.2 Zircon — Object-Specific Creation Syscalls with Job Authority

Zircon creates each kernel object type via its own syscall: `zx_channel_create`,
`zx_vmo_create`, `zx_process_create`, `zx_thread_create`, `zx_port_create`,
`zx_timer_create`, etc.

**Job-based process/thread creation.** Creating a process or thread requires
holding a Job handle with ZX_RIGHT_MANAGE_JOB and ZX_RIGHT_WRITE. Without a
parent Job handle, a component cannot create new processes. The root job is
created by the kernel and passed to userboot (the first userspace entity). All
subsequent jobs and processes must be descendants of the root job.

**No Untyped equivalent.** Zircon does not model memory as a capability that
gates object creation. Memory (VMO) and process/thread authority are separate: a
process can create a VMO without any special parent authority (bounded by its
address space), but cannot create a child process without a job handle.

**Resource capabilities.** Some object types require a resource capability:
`zx_vmo_create_contiguous` requires ZX_RSRC_KIND_ROOT, and physical memory
mapping requires appropriate resource rights. The root resource handle is passed
to userboot at boot and not derivable from other handles.

**Source:** Fuchsia Kernel Concepts
(https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts); Zircon userboot
documentation (https://fuchsia.dev/fuchsia-src/concepts/process/userboot).

### 1.3 L4Re (Fiasco.OC + L4Re) — Resource Manager Delegation

L4Re separates object creation into two layers:

- **Kernel layer (Fiasco.OC).** The kernel creates thread, address space, and
  IPC object types when requested. Authority to create is implicit in holding a
  factory capability. Threads can be created within a task; capabilities are
  delegated into CSpaces by the holder.

- **Framework layer (L4Re).** `Moe` (the root task/resource manager) holds
  physical memory and acts as the initial resource manager. `Sigma0` initially
  holds all physical memory frames as identity-mapped capabilities. Sigma0
  grants pages to Moe on demand; Moe delegates dataspaces and other resources to
  child components. Child components cannot create objects without a capability
  obtained from their parent.

In L4Re, the root task (Moe) holds the following at boot: a task capability for
itself, a scheduler capability for a system-wide priority domain, a factory
capability (for creating kernel objects), Sigma0's send capability, and all
bootloader module ROM capabilities.

**Source:** L4Re Architecture Concepts
(https://l4re.org/detailed_introduction/architecture_concepts/index.html); L4Re
Servers documentation (https://l4re.org/doc/l4re_servers.html).

### 1.4 Genode Core — Parent-Child Resource Delegation

Genode's `Core` component is the root of the component tree and holds all
physical resources: physical memory pages, CPU time, device I/O regions, and
hardware-backed interrupt capabilities.

Core offers services — RAM, CPU, PD (protection domain), ROM, IRQ — that it
implements directly. Creating a new component means Core allocates:

1. A Protection Domain (address space object)
2. A CPU session (capability to run threads within a scheduling context)
3. A RAM quota (backing memory for the component's dataspaces)

No component other than Core can create hardware-backed objects (physical memory
mappings, actual CPU threads). Non-Core components create components by
requesting Core's services through their parent. Each child starts with a single
capability: a reference to its parent.

**Source:** Genode OS Framework Foundations 25.05, "Core — the root of the
component tree" section
(https://genode.org/documentation/genode-foundations/19.05/architecture/Core_-_the_root_of_the_component_tree.html).

### 1.5 EROS/KeyKOS — Typed Factory Capabilities

In KeyKOS and EROS, object creation uses explicit "factory" or "constructor"
capabilities — kernel-provided objects that, when invoked, produce new kernel
objects. The "SpaceBank" capability in KeyKOS/EROS is the mechanism for
allocating nodes and pages: holders of a SpaceBank capability can buy/sell nodes
and pages up to the space bank's budget.

No ambient object creation. A process cannot allocate kernel objects without
holding a SpaceBank capability with sufficient budget. The root SpaceBank is
created at kernel init with the entire physical address space as its budget. The
initial process holds the root SpaceBank.

**Source:** Shapiro et al., "EROS: A Fast Capability System," SOSP 1999
(https://courses.cs.washington.edu/courses/cse551/19wi/readings/eros-sosp99.pdf);
KeyKOS documentation, cap-lore.com.

---

## 2. Initial Capability Graphs at Boot

### 2.1 seL4 Rootserver

The kernel creates exactly one userspace entity: the rootserver (root task). Its
capability space (CSpace) is constructed by the kernel from a region of physical
memory designated as rootserver memory. The rootserver does not perform Retype
to create itself — the kernel does this.

**Fixed initial slots** (from `seL4_RootCNodeCapSlots` in bootinfo_types.h):

| Slot | Capability                  | Notes                                           |
| ---- | --------------------------- | ----------------------------------------------- |
| 0    | seL4_CapNull                | Null cap                                        |
| 1    | seL4_CapInitThreadTCB       | Rootserver's own TCB                            |
| 2    | seL4_CapInitThreadCNode     | Rootserver's root CNode                         |
| 3    | seL4_CapInitThreadVSpace    | ARM: PageGlobalDirectory                        |
| 4    | seL4_CapIRQControl          | Global IRQ controller (one per system)          |
| 5    | seL4_CapASIDControl         | Global ASID pool allocator                      |
| 6    | seL4_CapInitThreadASIDPool  | Rootserver's ASID pool                          |
| 7    | seL4_CapIOPortControl       | I/O port control (x86; not present on ARM)      |
| 8    | seL4_CapIOSpace             | IOMMU space (platform-specific)                 |
| 9    | seL4_CapBootInfoFrame       | Mapped frame containing seL4_BootInfo           |
| 10   | seL4_CapInitThreadIPCBuffer | IPC buffer frame                                |
| 11   | seL4_CapDomain              | Domain controller (for domain-based scheduling) |
| 14   | seL4_CapInitThreadSC        | Scheduling context (MCS kernel only)            |
| 15   | seL4_CapSMC                 | ARM SMC capability (platform-specific)          |

After these fixed slots, the rootserver receives:

- Capabilities to all rootserver image frames (user image pages).
- Capabilities to all rootserver image paging structures (page tables).
- Capabilities to all extra bootinfo pages.
- Capabilities to all Untyped memory regions — one Untyped cap per contiguous
  free physical memory region, enumerated in the `untypedList[]` array of
  seL4_BootInfo.

The `seL4_BootInfo` frame (slot 9) is mapped into the rootserver's address
space. It describes all the above ranges: `empty.start` gives the first free
slot for the rootserver to use; `untyped.start`/`untyped.end` enumerate all
untyped caps.

**Notable singletons:** seL4*CapIRQControl, seL4_CapASIDControl, and
seL4_CapDomain are \_unique* across the system — only one exists at any time.
Delegating them to another component removes them from the rootserver's CSpace
(they must be moved, not copied).

**Source:** seL4 Reference Manual 14.0.0, §8 (Kernel Boot Interface);
seL4/libsel4/include/sel4/bootinfo_types.h
(https://github.com/seL4/seL4/blob/master/libsel4/include/sel4/bootinfo_types.h).

### 2.2 Zircon Userboot

Zircon's first userspace process is `userboot`, a small program built into the
kernel image. The kernel creates userboot directly and passes it handles via the
process bootstrap message (processargs protocol).

The documented initial handles include:

- Process-self handle (ZX_RIGHT_WRITE | ZX_RIGHT_DESTROY | ...)
- Thread-self handle
- Root VMAR handle (spans the full 64-bit address space)
- Root job handle (the kernel-created root job; all jobs descend from this)
- VMO containing the ZBI (Zircon Boot Image, including bootfs)
- A VMO handle to the kernel's vDSO
- Resource handle for physical memory access (ZX_RSRC_KIND_ROOT or similar)
- A clock handle

Userboot itself is a shim — it loads the real first process
(bootsvc/component_manager) from bootfs, passes the important handles to it, and
exits. After userboot exits, the root job's exception handler must be set or
process faults go unhandled.

The root job is the authority root for process/thread creation. Any component
that loses or never holds a job handle in the root job's subtree cannot create
processes. Root resource handles are not derivable; once distributed they are
irrevocably delegated (no system-wide root resource holder exists after userboot
distributes them).

**Source:** Zircon userboot documentation
(https://fuchsia.dev/fuchsia-src/concepts/process/userboot); Fuchsia Jobs
documentation (https://fuchsia.dev/fuchsia-src/concepts/process/jobs).

### 2.3 L4Re — Three-Component Bootstrap (Kernel + Sigma0 + Moe)

L4Re bootstraps with three initial entities launched from the boot image:

1. **Fiasco.OC kernel** — the kernel itself.
2. **Sigma0** — the root pager. At boot, Sigma0 holds identity-mapped
   capabilities to all physical RAM. Sigma0 responds to memory requests from Moe
   by granting page-frame capabilities. Sigma0's own address space is fully
   wired; it has no pager.
3. **Moe** — the root task. Receives physical memory from Sigma0 and acts as the
   resource manager for all child components. Moe holds capabilities to all boot
   module ROM images, a factory capability for creating new kernel objects, and
   the Sigma0 capability for requesting more physical memory.

This division separates the "who owns physical memory at t=0" problem (Sigma0)
from the "who manages user-visible resources" problem (Moe). Sigma0 is minimal
and exists solely to hand out physical frames; Moe is the policy layer.

**Source:** L4Re Servers (https://l4re.org/doc/l4re_servers.html); L4Re boot
process (http://www.geocities.ws/munkee_chuff/l4/boot_process.html).

### 2.4 Genode — Core as the Implicit Root

Genode's Core starts with all physical resources and no parent. Core's initial
state is constructed by the kernel (on seL4: Core is the rootserver; on NOVA:
Core uses kernel capability slots directly). Core's capability space includes:

- Physical RAM capabilities (for distributing to child components as
  dataspaces).
- CPU time capabilities (scheduling contexts for all cores).
- I/O region capabilities (device registers, DMA ranges).
- All bootloader modules as ROM capabilities.

Core's first child (typically `init`) receives a parent capability pointing to
Core's services. Init is configured via a static policy (XML) embedded in the
boot image.

**Source:** Genode Foundations 25.05, "Core" section; Genode Component Creation
documentation
(https://genode.org/documentation/genode-foundations/21.05/architecture/Component_creation.html).

### 2.5 EROS — Root Domain with SpaceBank + Sched + Keeper Keys

EROS boots a "primordial" domain (process) with:

- A root SpaceBank capability (covering all physical pages/nodes).
- A root schedule capability (the CPU's full time budget).
- Capabilities to all persistent-store segments.
- A keeper slot set to null (see §3.3 below).

The primordial domain uses the SpaceBank to allocate nodes and pages, then
constructs all other domains from these resources. The root SpaceBank can
subdivide itself — creating child space banks with a fraction of its budget —
enabling hierarchical resource accounting.

**Source:** Shapiro, "EROS: A Fast Capability System," SOSP 1999; EROS design
documentation at cap-lore.com.

---

## 3. Root Fault Handling

### 3.1 seL4 — Halted Thread with No Recovery

Each seL4 TCB has a fault handler endpoint capability slot. On any exception (VM
fault, undefined instruction, hardware exception, IPC error) the kernel attempts
to deliver a fault message to this endpoint. If the slot is empty (null
capability), the kernel cannot deliver the fault — the faulting thread is
permanently blocked. The kernel does not kill it; the thread simply never runs
again.

**Root task behavior.** The seL4 rootserver is initialized with no fault handler
endpoint configured. The seL4 documentation states: "The root task is expected
to ensure that it does not cause a fault." If the rootserver faults, it halts
and the system becomes permanently paused (no recovery possible without a
restart).

**Rationale documented by the seL4 team:** Configuring a self-fault handler
would require the rootserver to have an endpoint to itself and a second
execution context to service it — adding complexity to the initial state. The
design accepts the constraint that the rootserver must be correct enough not to
fault.

**MCS variant.** The MCS kernel adds a timeout fault handler endpoint, also
unconfigured by default on the rootserver. Budget exhaustion without a timeout
handler also halts the thread.

**Source:** seL4 Fault Handlers tutorial
(https://docs.sel4.systems/Tutorials/fault-handlers.html); seL4 Reference Manual
14.0.0, §9 (Faults). seL4 debugging guide
(https://cgi.cse.unsw.edu.au/~cs9242/17/project/debugging.shtml).

### 3.2 L4/Pistachio — No Pager → Task Killed

In original L4 and L4Ka::Pistachio, each thread has a "pager" thread ID. A page
fault causes the kernel to send a fault IPC to the pager. Sigma0, the root
pager, has no pager of its own — its pager is configured as the kernel (a
special thread ID 0 in some implementations).

If Sigma0's pager ID is 0 (the null thread), and Sigma0 faults, the fault IPC is
silently dropped and Sigma0 is blocked indefinitely. The system then deadlocks:
Moe (or the root task) depends on Sigma0 for page resolution; Sigma0 is stuck.
In practice, Sigma0's code and data are fully mapped at boot with no demand
paging — a Sigma0 fault means a kernel bug, not a recoverable event.

**Source:** L4Ka::Pistachio Reference Manual; L4Re boot documentation.

### 3.3 EROS — Keeper Null → Domain Dormant

Each EROS domain (process) has a "keeper" capability slot. The keeper is invoked
when the domain encounters an unhandleable condition (fault, exhausted
SpaceBank, etc.). When the keeper slot is null, the domain transitions to the
"dormant" state — it stops executing and makes no further progress. The domain
can be restarted by external management, but the EROS persistence model means
dormant domains survive across reboots.

The primordial (root) domain initially has a null keeper. If it faults, it
becomes dormant and the system stalls. Bootstrapping code is expected to install
a keeper (a watchdog domain) before exposing the system to potentially-faulting
conditions.

**Source:** EROS design documentation (cap-lore.com); Shapiro, SOSP 1999.

### 3.4 Zircon — Root Job Exception Handler Required

Zircon delivers exceptions to exception channels hierarchically: thread →
process → job → root job → kernel fallback. If no exception channel is
registered at any level, the process is killed. The root job has no parent, so
if an exception propagates to the root job with no handler, the kernel applies a
default action: the process is killed.

For the root process (userboot or the component manager), userboot sets up the
root job's exception channel before handing off to component_manager. If
component_manager itself crashes before registering exception handlers, the
system becomes inconsistent.

The Fuchsia team's approach: the root job's exception channel is connected very
early, and any unhandled exception terminates the affected process but does not
halt the whole system (other processes in other jobs continue running).

**Source:** Fuchsia Jobs documentation
(https://fuchsia.dev/fuchsia-src/concepts/process/jobs); Zircon exception
documentation.

### 3.5 Genode Core — Kernel Panic

Genode Core has no parent component and runs directly on the kernel (seL4 or
NOVA). If Core's own execution encounters an unhandled exception, the behavior
is kernel-level: the underlying kernel detects a fault in its rootserver (or the
Core-equivalent component) and halts. On seL4 this is a blocked-forever thread;
on NOVA it may trigger a kernel assertion failure. In either case there is no
recovery mechanism above the kernel level.

The Genode design accepts that Core faults are unrecoverable bugs; Core's code
must be correct.

**Source:** Genode Foundations 25.05, "Interaction of Core with the underlying
kernel"
(https://genode.org/documentation/genode-foundations/21.05/under_the_hood/Interaction_of_core_with_the_underlying_kernel.html).

---

## 4. Tradeoffs

### 4.1 Object Creation Authority Models

| Model                     | Systems          | Authority gate                     | Granularity                      |
| ------------------------- | ---------------- | ---------------------------------- | -------------------------------- |
| Untyped memory capability | seL4, Barrelfish | Holder of Untyped creates objects  | Per-object, from physical memory |
| Job/process hierarchy     | Zircon           | Holder of Job can create processes | Per-process-tree                 |
| Space bank                | EROS/KeyKOS      | Holder of SpaceBank allocates      | Budget-limited, divisible        |
| Parent-to-child session   | Genode           | Core holds all; delegates down     | Per-service, policy-driven       |
| Factory capability        | L4Re/Fiasco.OC   | Factory cap required to create     | Per-factory-type                 |

**Untyped model (seL4):**

- All memory must be explicitly allocated from Untyped, giving precise
  accounting.
- Object creation requires knowing the physical size of each kernel type
  (fragile if sizes change).
- Memory cannot be reclaimed until all derived capabilities are revoked.

**Job hierarchy (Zircon):**

- Process tree structure enforced by the kernel; no orphan processes.
- Object creation (VMOs, channels, ports) is orthogonal to job authority.
- Root resource handles are special-cased and not expressible in the normal
  handle model.

**Space bank (EROS):**

- Budget conservation: a SpaceBank cannot over-commit.
- Hierarchical subdivision enables nested resource accounting.
- Type system is separate from the space bank — you allocate pages/nodes, then
  stamp them with types.

### 4.2 Initial Capability Graph Shape

| Dimension                             | seL4                            | Zircon                          | L4Re                              | Genode                             |
| ------------------------------------- | ------------------------------- | ------------------------------- | --------------------------------- | ---------------------------------- |
| Who constructs initial CSpace?        | Kernel                          | Kernel                          | Kernel                            | Kernel (+ rootserver)              |
| Number of fixed initial caps          | 16 (fixed) + N untyped          | Varies; not formally enumerated | Not fixed; passed via processargs | Not fixed; Core's own resource set |
| Physical memory authority             | Full (all Untyped caps)         | Via root resource handle        | Via Sigma0 delegation             | Via Core RAM session               |
| Singleton authorities (system-unique) | IRQControl, ASIDControl, Domain | Root resource, root job         | Sigma0's grant authority          | Core itself                        |

**seL4's enumeration model:**

- Completeness: every physical resource is enumerated in the rootserver's
  initial CSpace.
- There is no physical resource the rootserver does not know about.
- The rootserver is therefore the system's initial authority over all hardware.

**Zircon's distributed model:**

- Userboot distributes handles to multiple recipients; the "root authority" is
  split at boot.
- Component_manager becomes the long-running authority holder; userboot exits.
- Less visible enumeration: no single data structure lists all initial
  authority.

### 4.3 Root Fault Handling

| System  | Root entity                      | If root entity faults                                   |
| ------- | -------------------------------- | ------------------------------------------------------- |
| seL4    | Rootserver (initial thread)      | Thread blocked forever; no recovery                     |
| L4/L4Re | Sigma0, Moe                      | Sigma0: deadlock; Moe: system-dependent halt            |
| Zircon  | userboot, then component_manager | Process killed; root job exception handler required     |
| Genode  | Core                             | Kernel-level halt                                       |
| EROS    | Primordial domain                | Domain becomes dormant; recoverable if keeper set later |

All systems converge on the same observation: there is no architectural escape
from "the root entity must not fault." The differences are in how graceful the
failure mode is:

- Zircon allows partial recovery (other processes survive).
- EROS permits eventual restart of the dormant domain.
- seL4 and Genode result in permanent stall (reboot required).

**Self-fault handling:** No surveyed system provides a kernel mechanism for the
root entity to handle its own faults with no external peer. The structural
reason: fault handling requires a second execution context (to service the fault
endpoint). That second context is itself subject to faults, creating an infinite
regress. The common resolution is to require the root entity's code to be
trusted correct, not to provide a kernel escape hatch.

---

## 5. Measured Data

| Fact                                      | Value                                      | Source                 |
| ----------------------------------------- | ------------------------------------------ | ---------------------- |
| seL4 initial CNode fixed slots            | 16 fixed (slots 0–15)                      | bootinfo_types.h       |
| seL4 Untyped cap count (typical embedded) | 10–50 (depends on memory map)              | seL4 tutorials         |
| seL4 CNode slot size                      | 32 bytes (AArch64)                         | seL4 Ref Manual 14.0.0 |
| seL4 initial rootserver memory            | ~512 KiB on ARM (image + structures)       | seL4 boot measurements |
| Zircon root job handle rights             | ZX*RIGHT*\* bitmask; published in handle.h | Fuchsia source         |
| EROS root SpaceBank budget                | All physical pages/nodes at t=0            | SOSP 1999              |

---

## References

- seL4 Reference Manual Version 14.0.0.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf

- seL4 Capabilities Tutorial.
  https://docs.sel4.systems/Tutorials/capabilities.html

- seL4 Untyped Tutorial. https://docs.sel4.systems/Tutorials/untyped.html

- seL4 Fault Handlers Tutorial.
  https://docs.sel4.systems/Tutorials/fault-handlers.html

- seL4/libsel4/include/sel4/bootinfo_types.h
  https://github.com/seL4/seL4/blob/master/libsel4/include/sel4/bootinfo_types.h

- Shapiro, J., Smith, J., Farber, D. "EROS: A Fast Capability System."
  _Proceedings of the 17th ACM SOSP_, 1999.
  https://courses.cs.washington.edu/courses/cse551/19wi/readings/eros-sosp99.pdf

- Fuchsia. "Zircon Kernel to Userspace Bootstrapping (userboot)."
  https://fuchsia.dev/fuchsia-src/concepts/process/userboot

- Fuchsia. "Jobs." https://fuchsia.dev/fuchsia-src/concepts/process/jobs

- Fuchsia. "Zircon Kernel Concepts."
  https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts

- L4Re Architecture Concepts.
  https://l4re.org/detailed_introduction/architecture_concepts/index.html

- L4Re Servers (Sigma0, Moe, Ned). https://l4re.org/doc/l4re_servers.html

- Norman Feske. _Genode OS Framework Foundations_ 25.05.
  https://genode.org/documentation/genode-foundations-25-05.pdf

- Genode: Core — the root of the component tree.
  https://genode.org/documentation/genode-foundations/19.05/architecture/Core_-_the_root_of_the_component_tree.html

- Genode: Interaction of Core with the underlying kernel.
  https://genode.org/documentation/genode-foundations/21.05/under_the_hood/Interaction_of_core_with_the_underlying_kernel.html

- seL4 Debugging Guide (root task fault behavior).
  https://cgi.cse.unsw.edu.au/~cs9242/17/project/debugging.shtml
