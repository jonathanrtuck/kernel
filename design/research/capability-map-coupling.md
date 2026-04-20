# Capability Acquisition and Address-Space Mapping: Coupled or Separate?

## Question

When a process acquires a capability to a memory object, does that acquisition
automatically establish a virtual-address mapping for the object in the
process's address space (auto-map), or is establishing a mapping a separate,
explicit operation that the process must invoke independently?

This question has two sides:

- **Acquisition → mapping:** Does receiving/deriving a capability cause the
  memory to become accessible at some virtual address?
- **Loss → unmapping:** Does deleting/revoking the capability automatically
  remove the mapping?

The D24 question arose because the "map is explicit" decision from D9 addressed
only the forward direction: can the Observer choose an address? The orthogonal
question is whether the kernel collapses the two steps (acquire + map) into one.

---

## 1. Survey of Existing Systems

### 1.1 seL4 — Always Two Steps; No Coupling in Either Direction

In seL4, Frame capabilities and virtual-address mappings are entirely
independent:

**Acquisition:** When a process receives a Frame capability via IPC cap
transfer, the capability lands in a CNode slot specified by the receiver's IPC
buffer (`receiveCNode`, `receiveIndex`, `receiveDepth`). Landing in a CSlot
confers authority over that Frame — it does not establish any mapping.

**Mapping:** To make the frame accessible at a virtual address, the process must
explicitly invoke
`seL4_ARM_Page_Map(frame_cap, vspace_cap, vaddr, rights, attrs)`. This is a
separate kernel invocation that inserts a hardware page-table entry. The virtual
address is chosen by the caller.

**Unmapping:** The mapping is removed by `seL4_ARM_Page_Unmap(frame_cap)`.
Deleting the Frame capability does **not** automatically call Unmap — the caller
must unmap first, then delete. In practice, the kernel enforces this: attempting
to delete a Frame capability that has an active mapping returns an error; the
capability must be unmapped before it can be deleted or recycled.

**Key invariant:** A Frame capability exists in CSpace; a mapping entry exists
in the hardware page table. These are separate state. A Frame cap can exist
without a mapping (e.g., held in reserve). A mapping cannot exist without a
valid Frame cap (the cap tracks the mapping's existence, preventing dangling
PTEs).

**Source:** seL4 Reference Manual 14.0.0, §3.5 (Virtual Address Space), §4.2
(IPC cap transfer); seL4 Mapping Tutorial (docs.sel4.systems/Tutorials/mapping).

---

### 1.2 Zircon / Fuchsia — VMO Handle and Mapping Always Separate

A
`zx_vmar_map(vmar_handle, options, vmar_offset, vmo_handle, vmo_offset, length, &mapped_addr)`
call is required to establish a mapping. Receiving a VMO handle via a Zircon
Channel message does not map it.

The operations require separate rights:

- The VMAR handle must have `ZX_VM_CAN_MAP_READ` (and write/exec as needed).
- The VMO handle must have `ZX_RIGHT_MAP`.

These are checked independently. Having both handles is necessary but not
sufficient — the explicit `zx_vmar_map()` call must still be made.

**Closing the VMO handle does not unmap.** From the Zircon documentation: "The
mapping retains a reference to the underlying virtual memory object, which means
closing the VMO handle does not remove the mapping." An explicit
`zx_vmar_unmap()` or `zx_vmar_destroy()` removes mappings. This means the
mapping can outlive the original capability handle — the mapping itself holds a
reference.

**Source:** `zx_vmar_map` reference (fuchsia.dev/reference/syscalls/vmar_map);
Zircon Kernel Concepts (fuchsia.dev/fuchsia-src/concepts/kernel/concepts);
RFC-0013 on cloning VMO mappings (fuchsia.dev).

---

### 1.3 L4 (Original, X.2) — IPC Atomically Transfers Rights And Maps

Original L4 is the primary example where capability transfer and mapping are a
single atomic operation. L4's mapping primitive is not a separate syscall — it
is encoded into IPC:

**Map/grant via fpage IPC:** The sender's message descriptor specifies one or
more _map items_ or _grant items_, each containing an fpage (a naturally
aligned, power-of-2-sized virtual range in the sender's address space) and
transfer flags. When the kernel delivers the message, it establishes
virtual-to-physical mappings in the receiver's address space for every fpage in
the map items.

- **Map:** The sender retains its rights; the receiver gains the same (or
  weaker) rights to the same physical pages.
- **Grant:** The sender's rights are transferred and revoked atomically; the
  receiver gains them.

**Receive window:** The receiver specifies a _receive window_ — an fpage in its
own address space. The kernel maps the sent fpages into this window, with
virtual-address offsets preserved relative to the window's base. The receiver
does not independently invoke a "map" syscall.

**Consequences:**

- There is no concept of holding a capability to a Frame that is not (yet)
  mapped. Receiving an fpage mapping IS receiving a mapping.
- Conversely, to hold a capability to a frame without mapping it, L4 has no
  direct mechanism — one would have to map it to a scratch region and not use
  that region.
- Unmapping is a separate `unmap` syscall (later L4 variants).

**Later L4 variants (Pistachio, Fiasco.OC, seL4)** moved away from this model
toward separate capability-in-CSpace plus explicit mapping.

**Source:** L4 eXperimental Kernel Reference Manual X.2 r7, §4.5 (Map/grant);
Weigand, "Generalized-Mapping IPC for L4" (TU Dresden diploma thesis).

---

### 1.4 EROS / CapROS — Structural Identity: Space Tree Placement IS Mapping

EROS and CapROS use the capability tree to define the address space. The
relationship between capability acquisition and mapping depends on _where_ the
capability is placed.

**Two kinds of placement:**

1. **Key registers (general-purpose slots):** A process has 16 key registers
   (`k0`–`k15`) plus reserved slots. Holding a Page key in a key register does
   not map the page into the virtual address space. The key register is
   analogous to seL4's CNode slots: a holder of authority, not a mapping.

2. **Space tree placement:** A process has a "space key" slot that names the
   root of its address space tree. The tree is composed of Node objects, each
   with 16 key slots, plus a (size, lss) header encoding the subspace size. When
   a Page key is placed into a Node slot that is reachable from the space key,
   the kernel traverses the tree on fault and establishes a hardware mapping for
   that virtual address range.

**Consequence:** "Mapping" in EROS is the act of inserting a Page key into the
right position in the space tree. Acquiring the key (putting it in a key
register) is not mapping; placing it in the space tree is. The two operations
may coincide if a process directly modifies its own space tree, but they are
conceptually distinct.

**Kernel behavior on fault:** When a hardware page fault occurs, the EROS kernel
traverses the address space tree, performs any necessary object faults (loading
from disk), and installs hardware PTEs. The hardware page table is treated as a
cache of the space tree state.

**Revocation:** Revoking a Page key via the Audit Trail mechanism removes the
key from all locations — both key registers and space tree nodes. This causes
the hardware PTE to be removed at the next fault or proactively via shootdown,
since the space tree entry is gone.

**Source:** Shapiro et al., "EROS: A Fast Capability System," SOSP 1999; CapROS
Address Spaces reference (capros.org/devel/ObRef/concepts/AddressSpaces); CapROS
kernel design documentation (capros-os/capros on GitHub).

---

### 1.5 Coyotos — GPT Slot Placement, Same Structural Model as EROS

Coyotos replaces EROS nodes with **Generalized Page Tables (GPTs)**: fixed
vectors of 16 guard-prefixed capability slots. The space key names the root GPT.
Page capabilities placed in GPT slots reachable from the space key establish
mappings; capabilities held elsewhere do not.

The structural model is identical to EROS: placement in the space tree =
mapping; holding elsewhere = no mapping. The GPT guard extension allows sparser
address space representation with fewer tree levels for a given virtual range.

**Source:** Shapiro, "Coyotos Microkernel Specification."

---

### 1.6 Mach / XNU — vm_map() Is Always Explicit; Memory Object Port ≠ Mapping

In Mach, acquiring a send right to a memory object port does not map anything.
To map a memory object, a process must call:

```text
vm_map(task_port, &addr, size, mask, flags, memory_object_port, offset,
       copy, cur_prot, max_prot, inherit)
```

This is an explicit RPC to the kernel, naming both the task and the memory
object. The memory object port is a send right, but receiving it does not
trigger any mapping. The port represents authority to request a mapping, not the
mapping itself.

The first time a task maps an object, the kernel sends `memory_object_init()` to
the pager, beginning the demand-paging relationship. Subsequent page faults
cause `memory_object_data_request()` messages.

**Source:** GNU Mach Reference Manual — Mapping Memory Objects
(gnu.org/software/hurd/gnumach-doc/Mapping-Memory-Objects); XNU vm_map man page
(web.mit.edu/darwin/src/modules/xnu/osfmk/man/vm_map.html).

---

### 1.7 Genode — Dataspace Capability ≠ Mapping; Attach Is Explicit

A Genode component that receives a **dataspace capability** gains authority to
map the dataspace's contents into its virtual address space. The capability
itself does not establish any mapping.

To make the dataspace content visible, the component must call:

```cpp
env.rm().attach(ds_cap, size, offset, use_fixed_addr, fixed_addr, executable);
```

This is an explicit call on the component's region map. The return value is the
virtual address at which the dataspace was attached. Multiple attach calls with
the same capability create multiple mappings.

On the detach path, `env.rm().detach(addr)` removes the mapping. Destroying the
dataspace capability without detaching does not unmap it — the region map holds
its own reference.

**Source:** Genode OS Framework Foundations (Feske); Session interfaces of the
base API
(genode.org/documentation/genode-foundations/20.05/functional_specification).

---

### 1.8 Barrelfish — VNode Invocation Required; Cap Transfer Does Not Map

In Barrelfish, Frame capabilities and VNode capabilities are received through
the standard capability transfer path (mint/copy). Neither auto-maps.

To map a Frame at a virtual address, user-level memory management code invokes
the VNode capability at the appropriate page table level:

```text
err = vnode_map(ptable_cap, frame_cap, slot_index, flags, offset, ...);
```

This is an invocation on the kernel VNode object. The slot index within the
VNode determines the virtual address range being mapped. Multiple Frame caps can
be mapped into different slots of the same VNode.

Page faults are dispatched back to the application as upcalls (self-paging). The
application's fault handler modifies the VNode tree to satisfy the fault.

**Source:** Barrelfish Architecture Overview (TN-000); Gerber, "Virtual Memory
in a Multikernel" (ETH Zurich, master's thesis).

---

### 1.9 Plan 9 — No Capability Model; Segment Attach Is Explicit

Plan 9 does not have a capability model for memory objects. Virtual memory is
managed through segments, attached explicitly:

```c
segattach(attr, class, va, len)
```

Holding a file descriptor for a memory-mapped file does not map it. `segattach`
establishes the mapping. `segfree` removes it. There is no "auto-map on
descriptor acquisition."

**Source:** Plan 9 `segment(2)` manual page; Pike et al., "Plan 9 from Bell
Labs."

---

### 1.10 QNX Neutrino — POSIX mmap(); Explicit; File Descriptor ≠ Mapping

QNX follows POSIX. Acquiring a file descriptor (including memory objects) does
not map anything. `mmap()` with the appropriate `fd` establishes the mapping.
`munmap()` removes it. The file descriptor can be closed after mapping
(POSIX-compliant: the mapping holds its own reference); this matches Zircon's
behavior.

**Source:** QNX Neutrino RTOS System Architecture Guide — Memory Management.

---

## 2. The Structural Fork

Three distinct models appear across surveyed systems:

### Model A: Always Separate (Two-Step)

| System     | Acquire step                 | Map step                           |
| ---------- | ---------------------------- | ---------------------------------- |
| seL4       | Cap transfer into CNode slot | `Page.Map(vspace, vaddr, rights)`  |
| Zircon     | VMO handle in handle table   | `zx_vmar_map(vmar, vmo, ...)`      |
| Mach/XNU   | Memory object send right     | `vm_map(task, memory_object, ...)` |
| Genode     | Dataspace cap received       | `rm().attach(ds_cap, ...)`         |
| Barrelfish | Frame cap received           | VNode slot invocation              |
| Plan 9     | Segment descriptor / fd      | `segattach()`                      |
| QNX        | File descriptor              | `mmap()`                           |

Authority to map and the act of mapping are cleanly separated. A process can
hold a capability without ever mapping it. Mappings can be established and
removed independently of the capability lifecycle.

### Model B: Structural Identity (Space Tree Placement = Mapping)

| System        | Not-mapped state         | Mapped state                                  |
| ------------- | ------------------------ | --------------------------------------------- |
| EROS / CapROS | Page key in key register | Page key in space-tree node slot              |
| Coyotos       | Page cap outside GPT     | Page cap in GPT slot reachable from space key |

There is no separate "map" invocation. Mapping IS placing the capability in the
right structural position. Acquiring the capability into a key register does not
map; the process that wants to map must explicitly move the key into the space
tree (or instruct another process to do so on its behalf).

### Model C: Atomic Transfer-and-Map

| System   | Operation                    | Effect                              |
| -------- | ---------------------------- | ----------------------------------- |
| L4 (X.2) | Send message with fpage item | Receiver gets rights + mapping both |

The receiver specifies a receive window; within that window, the kernel maps the
sent fpages. There is no separate cap-acquisition step distinct from the mapping
— receiving the fpage message IS acquiring the (mapped) memory. No subsequent
explicit map call is needed.

This model was abandoned in later L4 derivatives (Pistachio, Fiasco.OC, seL4) in
favor of Model A. The reasons cited in seL4 design documents include: (a) it
mixes access control and virtual-memory policy in a single mechanism; (b) the
receiver window model has complex aliasing semantics; (c) it makes capability
transfer and memory mapping tightly coupled, hampering separate reasoning.

---

## 3. The Detachment / Revocation Direction

The question of whether **losing** a capability auto-unmaps is distinct from
whether **acquiring** one auto-maps.

| System        | Deleting last cap → auto-unmap? | Notes                                      |
| ------------- | ------------------------------- | ------------------------------------------ |
| seL4          | No — must unmap first           | Kernel rejects deletion of mapped Frame    |
| Zircon        | No — mapping holds own ref      | Closing VMO handle does not remove mapping |
| Mach/XNU      | No — mapping holds own ref      | Port close ≠ unmap                         |
| Genode        | No — must detach first          | Region map owns its own reference          |
| EROS / CapROS | Yes — revocation traverses tree | Removing key from space tree removes PTE   |
| L4 (X.2)      | Unmap syscall required          | Mappings have their own lifecycle          |
| Barrelfish    | No — must invoke VNode unmap    | Cap revocation ≠ address space change      |

The structural model (EROS/CapROS) has the tightest coupling in the removal
direction because the space tree IS the mapping structure — removing the key
from the tree removes the mapping. Model A systems have loose coupling in both
directions.

---

## 4. Measured Data

**seL4 Frame.Map cost (ARM64):** Approximately 300–600 cycles on Cortex-A53, per
seL4 AArch64 benchmarks. This includes page-table walk to find the target slot,
inserting the PTE, and bookkeeping in the Frame capability's mapping tracking
fields. The Frame.Unmap cost is similar.

**seL4 cap transfer via IPC (ARM64):** ~200 cycles for endpoint IPC delivering
one extra cap, per seL4 benchmark suite. This is the cost of moving the
capability into a CNode slot — no mapping included.

**L4 fpage map via IPC:** Not separately benchmarked from base IPC cost in
published seL4 benchmarks (L4 classic fpage semantics were replaced by seL4 time
of benchmarking). In L4Ka::Pistachio benchmarks (circa 2004), IPC with mapping
items adds ~100–200 cycles over base IPC due to page-table modification during
message delivery.

**Zircon zx_vmar_map cost:** O(log n) in the VMAR tree; no published cycle count
found. Separate from VMO handle transfer cost (handle table insert, O(1)).

**EROS space tree traversal on fault:** Shapiro et al. (SOSP 1999) report full
context switch + fault handling at ~67 µs on a 200 MHz Pentium; hardware PTE
installation is on the critical path of this measurement. The space tree
traversal replaces what in classical systems is a hardware page table walk.

---

## 5. Tradeoffs

The following are presented without ranking.

**Coupling complexity vs. interface simplicity.**

Model C (atomic transfer-and-map, L4 classic) reduces the number of distinct
operations a process must perform to gain mapped access to memory. A single IPC
call delivers both the authority and the mapping. The cost is that the mechanism
intermixes two concerns: the authority question (who is permitted to access
these pages?) and the virtual-memory question (at what address?). When
access-control and address-space policies diverge, a combined mechanism requires
additional mechanisms to separate them.

Model A (always separate) keeps these questions independent. A process can
decide independently when to map, at what address, with what permissions, and in
which address space. The cost is two distinct operations to achieve "I want to
access this memory."

**Holding without mapping.**

Model A systems naturally support holding a capability without establishing a
mapping: keep the cap in the CSpace, invoke Map only when needed. This enables
patterns like: receive a batch of Frame caps, map only those needed for the
current operation, unmap when done, keeping the cap for later.

In Model C (L4 classic), every received fpage is immediately mapped. Holding an
"unmapped capability" to a frame has no direct representation; one would need to
map it to an inaccessible region (e.g., with no-access permissions, if the
architecture supports that).

In Model B (EROS/CapROS), holding without mapping is represented by keeping the
key in a key register rather than in the space tree. Two positions exist for the
same key; the choice of position determines mapping.

**Multiple simultaneous mappings.**

In Model A systems, a single Frame capability can be passed to Map multiple
times at different virtual addresses (seL4 allows this; Zircon's `zx_vmar_map`
retains a reference from the mapping to the underlying VMO and can be called
multiple times). The capability's existence does not constrain the number or
locations of mappings.

In Model B (EROS), a Page key placed in two different nodes of the space tree
establishes two virtual-to-physical mappings for the same page. EROS explicitly
supports aliased physical pages via the space tree.

In Model C (L4 classic), multiple map fpages in a single message can map the
same physical page to multiple virtual addresses in the receiver's window.

**Revocation and the mapping lifecycle.**

In Model A with loose coupling (Zircon, Mach): mapping outlives the capability.
The capability can be revoked, but existing mappings persist until explicitly
removed. This is flexible but means revocation does not guarantee the mapped
view is gone.

In Model A with strict coupling (seL4): a Frame capability tracks its existing
mappings; the kernel prevents deletion of a capability that still has a live
mapping. This prevents dangling mappings but requires the process to unmap
before revoking or recycling the cap.

In Model B (EROS/CapROS): revocation is transitive through the capability
derivation tree. Revoking a Page key's parent removes the key from all positions
— both key registers and space tree nodes — causing mappings to disappear.

**Address control.**

In all surveyed Model A systems, the mapping call takes an address argument
(explicit or kernel-chosen). The caller has full control over which virtual
address receives the mapping.

In Model C (L4 classic), the receive window constrains the address: the receiver
specifies a virtual range; the sender's fpage addresses are mapped within that
window. The receiver does not freely choose the exact virtual address for each
fpage; it is determined by the sender's virtual address modulo the window size.
This can cause aliasing surprises if the sender and receiver have different
virtual address layouts.

**Interface complexity at the syscall boundary.**

Model A requires at minimum two syscall-equivalent operations to establish a
mapped access to a Frame: one to acquire (or derive) the cap, one to map it.
Model C collapses them into one. For workloads that always map immediately after
cap acquisition, Model C saves one round trip.

For workloads where caps are acquired in bulk and mapped selectively (e.g., a
memory allocator that pre-allocates Frame caps and maps on demand), Model A's
separation is the more natural fit.

---

## 6. References

- seL4 Reference Manual 14.0.0. NICTA/CSIRO/seL4 Foundation.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 Mapping Tutorial. seL4 Foundation.
  https://docs.sel4.systems/Tutorials/mapping.html
- seL4 IPC Tutorial. seL4 Foundation.
  https://docs.sel4.systems/Tutorials/ipc.html
- L4 eXperimental Kernel Reference Manual X.2 r7.
  https://www.l4ka.org/l4ka/l4-x2-r7.pdf
- Weigand, A. "Generalized-Mapping IPC for L4." TU Dresden diploma thesis.
  https://os.inf.tu-dresden.de/papers_ps/weigand-diplom.pdf
- Shapiro, J.S., Smith, J.M., Farber, D.J. "EROS: A Fast Capability System."
  SOSP 1999.
  https://citeseerx.ist.psu.edu/document?repid=rep1&type=pdf&doi=198d9c3e33be1f49b3e743f3dd17a2c237cdb69f
- CapROS Address Spaces reference.
  http://www.capros.org/devel/ObRef/concepts/AddressSpaces.html
- Shapiro, J.S. "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- zx_vmar_map reference. Fuchsia.dev.
  https://fuchsia.dev/reference/syscalls/vmar_map
- Zircon Kernel Concepts. Fuchsia.dev.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
- RFC-0013: Cloning a VMO Mapping. Fuchsia.dev.
  https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0013_cloning_a_vmo_mapping
- GNU Mach Reference Manual — Mapping Memory Objects.
  https://www.gnu.org/software/hurd/gnumach-doc/Mapping-Memory-Objects.html
- XNU vm_map man page. MIT/Darwin.
  https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/vm_map.html
- Genode OS Framework Foundations (Feske). Genode Labs.
  https://genode.org/documentation/genode-foundations/20.05/functional_specification/Session_interfaces_of_the_base_API.html
- Gerber, R. "Virtual Memory in a Multikernel — The Barrelfish OS." ETH Zurich.
  https://barrelfish.org/publications/gerber-master-vm.pdf
- Barrelfish Architecture Overview (TN-000). ETH Zurich / MSR.
  https://barrelfish.org/publications/TN-000-Overview.pdf
- QNX Neutrino RTOS System Architecture Guide — Memory Management.
  https://www.qnx.com/developers/docs/7.1/com.qnx.doc.neutrino.sys_arch/topic/proc_memmgr.html
- Pike, R. et al. "Plan 9 from Bell Labs." USENIX UKUUG Summer Conference, 1990.
- ARM Architecture Reference Manual Armv8-A, Part D4 (VMSAv8-64).
  https://developer.arm.com/documentation/ddi0487
