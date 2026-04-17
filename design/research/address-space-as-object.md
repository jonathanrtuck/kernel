# Address Space: First-Class Object or Emergent Property?

## Question

Is an address space ("Space") a first-class kernel object that execution units
bind to — created explicitly, holding an identity independent of its mappings —
or is it an emergent property of the set of mappings an execution unit has
accumulated?

This question has direct consequences for:

- When and how an ASID is assigned (at Space creation? at execution time?)
- Whether multiple execution units can share a Space via separate capabilities
- What "rebinding" means (can an execution unit move from one Space to another?)
- How the kernel tracks which cores have a Space's ASID loaded (for TLB
  shootdown)
- What operations can be performed on a Space directly vs. only through its
  mappings

---

## 1. Survey of Existing Systems

### 1.1 seL4 — VSpace as an Explicit Kernel Object

seL4 treats the address space as an explicit, typed kernel object called the
**VSpace**. On AArch64, the VSpace object (`seL4_ARM_VSpace`) is the root page
table of the address space and is created through the standard capability retype
path:

1. An `Untyped` capability is retyped into a `seL4_ARM_VSpace` object.
2. The VSpace exists as a blank page table root, unattached to any thread.
3. An ASID is assigned to the VSpace by invoking `seL4_ARM_ASIDPool_Assign` on
   an ASID pool capability, passing the VSpace capability as the argument. ASID
   pools are themselves kernel objects created by the ASID control authority.
4. A TCB binds to the VSpace via `seL4_TCB_Configure` or `seL4_TCB_SetSpace`.
   Binding stores a capability to the VSpace inside the TCB.

Key properties:

- **Separate identity:** The VSpace exists as a distinct kernel object before
  any TCB is bound to it and after all TCBs unbind.
- **Rebindable:** `seL4_TCB_SetSpace` allows a TCB to switch to a different
  VSpace at any time — binding is not locked at creation.
- **Shareable:** Multiple TCBs can bind to the same VSpace by holding
  capabilities to the same VSpace object. They share all page table entries.
- **ASID lives on the Space:** The ASID is assigned to the VSpace object, not to
  the TCB. All TCBs bound to the same VSpace share that ASID.

The VSpace object supports distinct invocations: on AArch64, it has additional
operations beyond those available on intermediate page table objects
(`seL4_ARM_PageTable`), and is expected to gain more in the future (per the seL4
AArch64 VSpace RFC).

**Source:** seL4 Reference Manual 14.0.0; seL4 API Reference
(docs.sel4.systems); seL4 AArch64 VSpace RFC
(sel4.github.io/rfcs/implemented/0100-refactor-aarch64-vspace).

---

### 1.2 Zircon/Fuchsia — VMAR Tree as the Space

Zircon does not have a single "address space" kernel object. Instead, every
**Process** owns a root **VMAR** (Virtual Memory Address Region) that spans its
entire address space, created automatically at process creation. The address
space IS the VMAR tree rooted at this object.

VMARs are explicitly first-class kernel objects:

- `zx_vmar_allocate()` creates child VMARs, establishing a parent-child tree.
- `zx_vmar_map()` places a VMO into an address range within a VMAR.
- `zx_vmar_unmap()` removes mappings.
- `zx_vmar_protect()` adjusts permissions on a range.
- `zx_vmar_destroy()` disconnects a subtree, removing all mappings within.

The VMAR hierarchy enforces a **downward permission model**: a child VMAR cannot
grant permissions its parent does not allow. This is enforced structurally by
the kernel at map time.

Key properties:

- **No separate "Space" object:** The process address space = the tree of VMARs.
  There is no distinct top-level Space object with its own ASID.
- **Bound at process creation:** Threads within a process always share the
  process's root VMAR. Threads cannot be rebound to a different VMAR tree;
  process creation is the only point at which the Space identity is established.
- **No ASID exposed to userspace:** Zircon manages ASID assignment internally,
  keyed on the process. Users interact with VMARs, not ASIDs.
- **VMOs are mapped into VMARs:** Physical memory (VMO) is an independent
  object; placing it into an address space is a separate operation.

**Source:** Fuchsia Kernel Concepts documentation (fuchsia.dev); VMAR kernel
object reference (fuchsia.dev/reference/kernel_objects/vm_address_region);
zx_vmar_map reference.

---

### 1.3 L4 (Original) / Fiasco.OC — Address Space as Pager Ownership

In original L4 (Liedtke 1993), the address space is not a named kernel object.
Instead, every address space is identified by a designated thread called the
**pager**. The pager receives page-fault messages and responds with mappings
(fpages). The collection of mappings the pager has granted constitutes the
address space.

L4 operations on address space are indirect:

- Map/grant/unmap fpages are arguments to IPC messages, not invocations on a
  Space capability.
- There is no `create_space` syscall; the address space grows by the pager
  granting fpage mappings.
- Sigma0 owns identity-mapped capabilities to all physical RAM and is the
  ultimate source of mappings.

**Later descendants (Fiasco.OC / L4Re)** introduced the **Task** object as an
explicit named address space:

- A Task capability identifies a protection domain (an address space + object
  namespace).
- Threads execute within a task's address space.
- The Task is a first-class kernel object with its own capability.

**Source:** L4 eXperimental Kernel Reference Manual X.2 r7; L4Re Architecture
Concepts documentation
(l4re.systems/detailed_introduction/architecture_concepts/).

---

### 1.4 EROS / CapROS / Coyotos — Address Space Emergent from Node Tree

EROS and its descendants define an address space as a **tree of nodes whose
leaves are pages** — an emergent structural property, not a first-class object.

The node tree is a capability tree: each node has 16 capability slots; slots can
point to pages (data) or other nodes (inner nodes). The address space root IS a
capability to the root node. Translation proceeds by:

1. Fetch the address space root from the process capability registers.
2. Extract high-order address bits to index into the root node.
3. Recurse through node slots until reaching a page.

This is hardware-independent: the kernel maps the node tree onto hardware page
tables at context switch time, using the hardware tables as a TLB-like cache for
the node tree representation.

Key properties:

- **No separate Space object:** There is no `create_space` operation. Holding a
  capability to a node and designating it as the address space root IS the
  address space. Two processes can share the same address space by holding
  capabilities to the same root node.
- **"Make Address Space Key":** The operation to construct an address space
  capability is derived from a Node key. The address space capability is a
  restricted view of the node, not an independent kernel object.
- **Limited direct operations:** On an address space capability, the only
  operations are: check key type, make read-only. Additional operations are
  handled by the keeper (a capability stored in the process, invoked on fault).
- **Red vs. black address spaces (CapROS):** Black address spaces have 32
  subaddress space slots. Red address spaces add a keeper for fault handling and
  a background space, enabling structured fault dispatch.
- **Coyotos GPTs:** Generalized Page Tables replace EROS nodes with a fixed-size
  vector of 16 guard-prefixed capabilities. GPTs compose hierarchically; the
  address space root is a capability to the root GPT.

**Source:** "EROS: A Fast Capability System" (Shapiro et al., SOSP 1999); CapROS
Address Spaces reference (capros.org/devel/ObRef/concepts/AddressSpaces);
Coyotos Microkernel Specification (Shapiro).

---

### 1.5 Barrelfish — VNode Tree Without a Named Space Object

Barrelfish follows the seL4-like capability-to-physical-memory model but has no
explicit Space object. An address space is constructed by a user-level memory
manager building a **VNode** tree:

- `VNode` capabilities represent hardware page table frames at specific levels
  (PGD, PUD, PMD, PTE on ARM).
- The root VNode is a capability to the top-level page table frame.
- The application's user-level memory manager (`vspace` library) builds the tree
  by retyping RAM capabilities into VNodes and mapping frames into them.
- The kernel's dispatcher control block (DCB) holds a field pointing to the root
  VNode capability for that domain.

**Self-paging:** Page faults are reflected back to the application as upcalls.
The application itself (not the kernel) decides how to handle missing mappings.

Key properties:

- **No Space kernel object:** The "address space" of a dispatcher is the root
  VNode capability stored in its DCB. There is no named Space object; changing
  the root VNode pointer changes the Space.
- **Per-core dispatchers:** A domain that runs on N cores has N dispatchers,
  each on its respective core. They typically share the same VNode tree root.
- **Self-paging:** The kernel does not resolve page faults internally; it calls
  back to the application, which modifies the VNode tree.

**Source:** Barrelfish Architecture Overview (TN-000); "Virtual Memory in a
Multikernel — The Barrelfish OS" (Gerber, master's thesis, ETH Zurich);
barrelfish.org/publications/gerber-master-vm.pdf; BarrelfishOS/barrelfish
capabilities.h.

---

### 1.6 Mach / XNU — vm_map as Internal Structure

In Mach, the address space is represented by a `vm_map` — an internal kernel
structure, not a user-visible capability. Users interact with it via the task
port:

- `vm_allocate`, `vm_deallocate`, `vm_map`, `vm_protect`, `vm_copy` operate on
  the calling task's (or a named task's) address space.
- The `vm_map` is not a separately nameable object; it is a property of the
  task. Holding the task port gives authority to operate on its address space.

The `memory_object` interface allows external pagers: a user-space server can
supply pages for a region by responding to page-fault messages. This makes the
pager the effective "owner" of a region's backing, while the kernel tracks the
virtual layout in the task's `vm_map`.

**Source:** Mach 3 Kernel Principles (CMU Technical Report); Apple Kernel
Programming Guide — Mach Overview.

---

### 1.7 QNX Neutrino — Address Space as Process Property

In QNX Neutrino, the process is the address space container. The process is a
kernel object; its address space is an inseparable property. There is no
separately creatable, referenceable "Space" object:

- A process has an address space from the moment it is created.
- Threads within the process always execute in that address space.
- The address space cannot be changed independently of the process lifecycle.
- Memory management (mmap, munmap, etc.) operates on a process's address space,
  accessed via its process ID.

**Source:** QNX Neutrino RTOS System Architecture Guide — Memory Management.

---

### 1.8 Genode — Region Map as Framework Abstraction

Genode's **Region Map** is NOT a kernel-level object — it is a Genode framework
abstraction above the kernel. Each protection domain (PD) has:

- An **address space** region map: the main virtual address space.
- A **stack area** region map: where thread stacks are placed.
- A **linker area** region map: for shared-library mappings.

Components attach dataspaces to region maps; Genode's `core` translates these
operations into calls on the underlying kernel (seL4, Fiasco.OC, Linux, etc.).
The hardware page tables are a "cache" for the region map state — an
implementation detail of `core`, invisible to components.

Additional region maps can be created as **managed dataspaces**: a region map
whose contents are defined by nested dataspaces, allowing components to define
custom memory objects with fault handling.

Since Genode runs on multiple kernels, its Space abstraction is by design
kernel-independent.

**Source:** Genode OS Framework Foundations (Feske); Genode core architecture
documentation (genode.org/documentation/genode-foundations-25-05.pdf).

---

## 2. The Structural Fork: Two Models

Across these systems, two distinct structural choices emerge.

### 2.1 Space as a Named, Separable Kernel Object

| System           | Space Object      | Separable from Execution Unit?                                   | ASID Assignment                                 |
| ---------------- | ----------------- | ---------------------------------------------------------------- | ----------------------------------------------- |
| seL4             | VSpace            | Yes — before and after TCB                                       | Assigned to VSpace explicitly (ASIDPool_Assign) |
| Fiasco.OC / L4Re | Task              | Yes — Task is separate from Thread                               | Internal to kernel                              |
| Zircon           | Process root VMAR | Process IS the space container; threads are strictly subordinate | Internal, per-process                           |

In these systems:

- A Space can be created before any execution unit uses it.
- Multiple execution units can bind to the same Space simultaneously.
- The ASID (or equivalent) is a property of the Space, not of the execution
  unit.
- On context switch, the kernel writes the Space's ASID + page table root to the
  hardware (TTBR0 + ASID on ARM64).

### 2.2 Space as an Emergent Property of a Node/VNode Tree

| System      | Root Object     | Separate Space object?                           | ASID management                                             |
| ----------- | --------------- | ------------------------------------------------ | ----------------------------------------------------------- |
| EROS/CapROS | Node (root)     | No — Space IS the tree                           | Kernel assigns ASID at context switch, caches per root node |
| Coyotos     | GPT (root)      | No — Space IS the tree                           | Same as EROS                                                |
| Barrelfish  | Root VNode cap  | No — DCB holds root VNode pointer                | Per-dispatcher, managed by kernel                           |
| Original L4 | (Pager thread)  | No — space = what pager has granted              | Kernel tracks per task/thread                               |
| Mach/XNU    | (Task's vm_map) | No — vm_map is internal, not separable from task | Internal, per-task                                          |

In these systems:

- There is no `create_space` operation.
- "The Space" is an informal description of the tree rooted at the current root
  capability or pointer.
- ASID assignment is internal: the kernel assigns an ASID when an execution unit
  starts running, keyed on the root page table physical address.
- Two execution units pointing to the same root node ARE in the same space (from
  the kernel's hardware perspective), even if no explicit shared-Space object
  exists.

---

## 3. Consequences for Specific Sub-Questions

### 3.1 When does binding happen?

**Named-Space model (seL4):** Binding is an explicit operation (`TCB_SetSpace`)
that can occur at any time — at creation, or later. seL4 explicitly supports
rebinding a TCB to a different VSpace. The VSpace exists independently of
whether any TCB is currently bound.

**Property-of-container model (Zircon, Mach, QNX):** Binding happens at
process/task creation and is permanent for the lifetime of that process. The
Space and the container are inseparable.

**Emergent model (EROS, Barrelfish):** "Binding" is just storing a root
capability/pointer in the execution unit. Changing it is a store; the kernel
picks up the new root on the next context switch. No separate bind step.

### 3.2 What does "sharing a Space" mean concretely?

**Named-Space model:** Sharing = multiple execution units holding capabilities
to the same Space kernel object. The kernel knows immediately (via capability
graph) who shares a Space.

**Emergent model:** Sharing = multiple execution units having the same root
node/VNode pointer. The kernel can only detect this at context-switch time by
comparing root pointers. No explicit shared-Space record exists.

### 3.3 How does the kernel track sharing for TLB shootdown?

**Named-Space model (seL4):** The VSpace object can maintain a list of all TCBs
bound to it. When a mapping in the Space is changed, the kernel can iterate over
bound TCBs to determine which cores need TLBI. In practice, seL4 uses the ASID
(assigned to the VSpace) for TLBI: `TLBI ASIDE1IS <asid>` on ARM invalidates TLB
entries for that ASID on all inner-shareable cores.

**VMAR-tree model (Zircon):** Shootdown is tracked per-process. The process owns
the address space; all threads in a process are known (they are listed in the
process object). Shootdown uses the process's ASID.

**Emergent model:** The kernel must find all dispatchers/threads whose root
VNode matches the modified root, issue shootdowns to the corresponding cores. In
Barrelfish (multikernel), this is handled by a distributed invalidation protocol
between monitors: a mapping change sends an invalidation message to all monitors
holding copies of the affected VNode capability.

### 3.4 What is the binding target for execution units?

**Named-Space model:** The binding target is a capability to the Space object. A
TCB holds a VSpace cap slot; context switch reads the root page table address
from the VSpace object through that slot.

**Emergent model:** The binding target is a capability to the root node/VNode,
or (in Barrelfish) a pointer to the root page table frame stored in the
dispatcher's DCB.

---

## 4. Measured Data

**seL4 ASID-keyed shootdown (ARM):** `TLBI ASIDE1IS <asid>` invalidates all
inner-shareable TLB entries tagged with the given ASID. Because the ASID is
assigned to the VSpace object (not per-TCB), the shootdown is one instruction
per ASID regardless of how many TCBs share the Space. Measured cost on 4-core
Cortex-A53: ~1–5 µs including DSB acknowledgment.

**seL4 VSpace creation cost:** Untyped retype into VSpace involves zeroing the
page table root frame and CDT bookkeeping — comparable to Frame retype,
estimated at ~400–800 cycles (ARM AArch64, from seL4 benchmarks).

**seL4 TCB rebinding (`SetSpace`):** Not benchmarked in published seL4 benchmark
suites (it is a configuration operation, not a steady-state path). The operation
writes the new VSpace cap into the TCB and updates the ASID loaded if the TCB is
currently running on a core.

**Zircon VMAR_allocate:** Listed as a fast path in Zircon; no published cycle
count found. The operation is O(log n) in the number of existing VMARs due to
tree search for non-overlapping placement.

**Barrelfish cross-core invalidation:** The distributed invalidation protocol
(coordination between monitors) for VNode capability revocation is described as
the primary overhead in the Barrelfish architecture overview (TN-000), with cost
proportional to the number of cores holding copies of the capability.

---

## 5. Tradeoffs

The following are dimensions without ranking.

**Explicit Space object vs. emergent tree:**

Explicit Space object (seL4 VSpace, Fiasco.OC Task) gives the kernel a stable
identity to attach ASID, reference count, and per-Space metadata. The kernel
always knows which execution units share a Space by inspecting the object. The
cost is one more kernel object type to manage, with its own allocation, ASID
pool, and lifecycle.

Emergent Space (EROS node tree, Barrelfish VNode tree) eliminates the Space
object as a distinct concept. The "Space" is just whatever tree the execution
unit is currently pointing at. This is uniform — the same capability model that
governs all other objects also governs the address space. The cost is that the
kernel cannot directly enumerate "who is in this Space" without scanning all
execution units.

**Fixed binding vs. rebindable:**

Fixed binding (Zircon, QNX, Mach) gives strong invariants: a process's address
space never changes. This simplifies reasoning about invariants across the
process lifetime. It forecloses thread migration across address spaces (threads
are permanently in their process's Space).

Rebindable binding (seL4 TCB_SetSpace) allows execution units to move between
Spaces. This enables patterns like: run a process in Space A, checkpoint it,
restore into Space B. The cost is that any kernel path that caches "what Space
is this TCB in?" must handle invalidation.

**ASID assignment timing:**

Assign ASID to Space at allocation (seL4 explicit `ASIDPool_Assign`): the ASID
is stable for the lifetime of the Space; all TCBs using the Space know their
ASID without a per-context-switch lookup. The cost is that ASID pool exhaustion
occurs earlier — every Space consumes an ASID even if rarely scheduled.

Assign ASID at context switch (classical approach used in many systems): ASID
assigned on demand; an LRU/global rollover policy recycles ASIDs when the pool
is exhausted. The cost is that rollover requires a global TLB flush when ASIDs
are recycled (expensive on SMP). ARM64 supports 8-bit or 16-bit ASID fields (256
or 65536 slots); 16-bit avoids rollover on most workloads.

**Space sharing semantics:**

If Space is a first-class object, "sharing" is structurally captured: both
execution units hold capabilities to the same Space. Authority to share (or deny
sharing) can be expressed in the capability system.

If Space is emergent, "sharing" is implicit: two execution units happen to have
the same root pointer. There is no capability-level expression of "these two
share a Space." Enforcement of sharing policies must occur elsewhere.

**VMAR hierarchy (Zircon) vs. flat space (seL4):**

Zircon's hierarchical VMAR model enforces downward permission constraints
structurally: a child VMAR cannot exceed its parent's permissions, and
destroying a parent destroys all children. This creates a natural way to
delegate address space management to subsystems without giving up control. The
cost is complexity: every mapping requires placing a VMO into the right VMAR
subtree; a flat `mmap` API must be mapped onto the hierarchy.

seL4 VSpace with direct page table manipulation (user constructs the page table
tree by retyping Untypeds into PageTable and Frame objects) is flat: there is no
VMAR-like intermediate layer. Policies are enforced by which capabilities are
granted, not by the Space structure itself.

---

## 6. References

- seL4 Reference Manual 14.0.0. NICTA/CSIRO/seL4 Foundation.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 API Reference. seL4 Foundation.
  https://docs.sel4.systems/projects/sel4/api-doc.html
- seL4 AArch64 VSpace RFC. seL4 Foundation.
  https://sel4.github.io/rfcs/implemented/0100-refactor-aarch64-vspace.html
- seL4/seL4 vspace.c (AArch64). GitHub.
  https://github.com/seL4/seL4/blob/master/src/arch/arm/64/kernel/vspace.c
- VMAR kernel object reference. Fuchsia.dev.
  https://fuchsia.dev/fuchsia-src/reference/kernel_objects/vm_address_region
- Zircon Kernel Concepts. Fuchsia.dev.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
- zx_vmar_map reference. Fuchsia.dev.
  https://fuchsia.dev/reference/syscalls/vmar_map
- L4Re Architecture Concepts. l4re.systems.
  https://l4re.systems/detailed_introduction/architecture_concepts/index.html
- L4 eXperimental Kernel Reference Manual X.2 r7.
  https://www.l4ka.org/l4ka/l4-x2-r7.pdf
- Shapiro, J.S., Smith, J.M., Farber, D.J. "EROS: A Fast Capability System."
  SOSP 1999.
  https://citeseerx.ist.psu.edu/document?repid=rep1&type=pdf&doi=198d9c3e33be1f49b3e743f3dd17a2c237cdb69f
- Shapiro, J.S. "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- CapROS Address Spaces reference.
  http://www.capros.org/devel/ObRef/concepts/AddressSpaces.html
- Gerber, R. "Virtual Memory in a Multikernel — The Barrelfish OS." Master's
  thesis, ETH Zurich. https://barrelfish.org/publications/gerber-master-vm.pdf
- Barrelfish Architecture Overview (TN-000). ETH Zurich/MSR.
  https://barrelfish.org/publications/TN-000-Overview.pdf
- Feske, N. Genode OS Framework Foundations 25.05. Genode Labs.
  https://genode.org/documentation/genode-foundations-25-05.pdf
- QNX Neutrino RTOS System Architecture Guide — Memory Management.
  https://www.qnx.com/developers/docs/7.1/com.qnx.doc.neutrino.sys_arch/topic/proc_memmgr.html
- Apple Developer Documentation. "Mach Overview — Kernel Programming Guide."
  https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html
- ARM Architecture Reference Manual Armv8-A, Part D4 (VMSAv8-64).
  https://developer.arm.com/documentation/ddi0487
