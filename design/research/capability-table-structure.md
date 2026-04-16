# Capability Table Structure

**Question:** How do real kernels organize and resolve per-execution-unit
capability storage? Specifically: the structural choice between a kernel-owned
flat table (userspace holds opaque integer handles) versus a
userspace-composable tree of capability nodes (the table itself is a
capability-managed object).

---

## 1. The Design Axis

Two poles define the space:

**Kernel-owned flat namespace.** The kernel allocates and manages a single, flat
data structure per execution unit. Userspace references capabilities by opaque
integer handles. The kernel is the only entity that can change the shape or
layout of the namespace. Examples: Zircon (handle table), EROS/KeyKOS (c-list),
Mach (IPC table), NOVA (capability selector table), QNX.

**User-composable tree.** Capability tables are first-class kernel objects,
themselves allocated and arranged by userspace. The kernel resolves capability
addresses by walking a tree of these objects from a per-thread root. Examples:
seL4 (CNode/CSpace), Barrelfish (CNode tree per core), Genode (recursive session
hierarchy).

Fiasco.OC/L4Re sits between: capabilities are stored per-task in a flat
namespace, but the task can delegate capabilities to other tasks through IPC.

---

## 2. System Surveys

### 2.1 seL4 — CNode Tree (CSpace)

**Unit of table:** Per-thread. Each thread control block (TCB) holds a pointer
to a root CNode capability that defines the thread's CSpace. Multiple threads
can share the same root CNode (pointing to the same physical CNode), making
their CSpaces identical.

**CNode structure.** A CNode is a kernel object that is an array of 2^k
capability slots, where `k` is its _radix_. On 64-bit platforms each slot
occupies 32 bytes (seL4_SlotBits = 5 on AArch64). A CNode with radix 8 has 256
slots and occupies 8 KiB of kernel memory, allocated from untyped memory by
userspace.

**Capability address encoding.** A capability address in a CSpace is a machine
word (64 bits on AArch64). The kernel resolves it by consuming bits from the
most-significant end. At each CNode in the path:

1. Check _guard_: compare the top `guardSize` bits of the remaining address
   against the guard value stored in the CNode capability. Mismatch → fault.
2. Consume _radix_ bits as an array index into the current CNode's slot array.
3. If the located slot contains another CNode capability, recurse with the
   remaining bits.
4. If the located slot contains a leaf capability (non-CNode), resolve is
   complete.

For a 64-bit platform, guards + radices across all levels must sum to 64 bits. A
common configuration is a 2-level CSpace: root CNode radix 10 (1024 slots), leaf
CNode radix 10 (1024 slots), 44-bit guard — giving 1M cap slots per thread.

The guard is not a property of the CNode object itself but of the CNode
_capability_ (the reference). This allows the same physical CNode to be shared
between two threads with different guards, effectively placing it at different
addresses in each thread's CSpace.

**Syscall invocation.** The kernel resolves the capability address relative to
the calling thread's root CNode on every syscall. The encoded address is a
parameter in the message registers.

**Resolution cost (measured).** For a 2-level CSpace, resolution is 2 array
lookups (one per CNode level) after confirming the guard. The seL4 team measured
capability lookup as a minority contributor to total fast-path IPC cost; the
dominant cost is context switch and register save/restore. No published
standalone benchmark for capability lookup latency in isolation.

**Slot size.** 32 bytes on 64-bit. A slot holds one capability. The capability
encoding is architecture-specific; seL4 stores the object pointer, rights word,
and type information within the 32 bytes.

**Allocation.** Userspace calls `seL4_Untyped_Retype` to convert a range of
untyped memory into a CNode. The kernel does not maintain a heap — all CNode
memory is accounted to userspace untyped allocations. CNode memory cannot be
reclaimed while any capability derived from it exists.

**CSpace operations (CNode syscalls).** `seL4_CNode_Copy`, `seL4_CNode_Move`,
`seL4_CNode_Mint`, `seL4_CNode_Delete`, `seL4_CNode_Revoke`,
`seL4_CNode_Rotate`, `seL4_CNode_Mutate`, `seL4_CNode_SaveCaller`. All take
explicit (root, index, depth) triples rather than implicit thread CSpace.

---

### 2.2 Zircon — Per-Process Handle Table

**Unit of table:** Per-process (not per-thread). All threads within a process
share one handle table. Threads hold no handles independently.

**Handle representation.** In userspace, a handle is a 32-bit integer
(`zx_handle_t`). The value 0 is ZX_HANDLE_INVALID. Valid handles always have the
two least-significant bits set (tagging convention). The integer value is not a
direct pointer or array index — the kernel derives the internal Handle object
pointer via an arithmetic mapping that incorporates a per-table pseudorandom
salt, making handles unguessable between processes.

**Internal structure.** The handle table (`HandleTable`) maintains handles in a
doubly-linked list (`fbl::DoublyLinkedList`). The table also holds a
`uint32_t random_value_` for salt and a reader-writer lock (`BrwLockPi`) for
thread safety. The conversion from `zx_handle_t` to a kernel Handle pointer is
O(1) arithmetic (pointer reconstruction from the encoded value), not a list
walk; the linked list is used for iteration (e.g., closing all handles on
process death) and for tracking count.

**Per-Handle data.** Each kernel Handle object stores: a reference to the kernel
object (ref-counted Dispatcher subclass), the rights word (zx_rights_t), and
list linkage. Rights are per-handle, not per-object — a process can hold two
handles to the same object with different rights.

**No table manipulation syscalls.** Userspace cannot add, remove, or inspect
arbitrary handle table entries. Handles are created by object-creation or
transfer syscalls (e.g., `zx_channel_create`, `zx_handle_duplicate`, handles
received via channel messages). The only explicit management is
`zx_handle_close` (remove one entry) and `zx_handle_close_many`.

**Handle transfer.** Handles are transferred between processes via
`zx_channel_write` with an explicit handles array. On write, the handles are
atomically moved from the sender's table to the receiver's table; the sender
loses them.

---

### 2.3 EROS/KeyKOS — C-List

**Unit of table:** Per-process (called a "domain" in KeyKOS, "process" in EROS).

**Structure.** KeyKOS gave each domain exactly 16 key slots (capabilities) plus
a handful of special-purpose slots (meter, schedule, address space, keeper).
EROS extended this to a flat c-list with a fixed maximum per process. Slots are
indexed by small non-negative integers; lookup is direct array indexing — O(1)
with no guard or guard check.

**Allocation.** The c-list is part of the process/domain object itself,
allocated wholesale by the kernel when the process is created. Userspace cannot
resize it.

**Capability passing.** Capabilities are passed in IPC messages as explicit
slots in the message. The kernel copies the capability from sender's c-list slot
to receiver's slot.

**Revocation.** EROS uses a capability link chain: each capability that points
to an object is linked into that object's list of holders. Revocation traverses
this list and nulls out each holder. Cost is O(holders).

---

### 2.4 Mach / XNU — IPC Port Namespace

**Unit of table:** Per-task (task ≈ process). Threads share the task's port
namespace.

**Structure.** Mach maintains an IPC table per task mapping port names
(integers) to port entries. The port entry stores the port right type (send,
receive, send-once, port-set, dead name) and a pointer to the underlying
ipc_port_t kernel object. Lookup is O(1) by integer name (sparse array or hash
internally).

**Port names.** Mach port names are arbitrary integers allocated by the kernel;
userspace cannot choose them directly (except bootstrapping). A thread's IPC
table is the only place that maps names to ports — ports themselves have no name
field.

---

### 2.5 NOVA Microhypervisor — Capability Selectors

**Unit of table:** Per-protection-domain (PD). Each PD has its own flat object
capability table, referenced by _selectors_ (small integers). Threads (execution
contexts in NOVA are called ECs) belong to a PD and share that PD's capability
namespace.

**Structure.** The capability table is a flat array indexed by selector value.
Kernel owns the table; userspace references capabilities by selector integer. No
hierarchical composition. NOVA distinguishes portal capabilities (for IPC), PD
capabilities, SC (scheduling context) capabilities, SM (semaphore) capabilities,
etc. all in the same flat namespace.

**Delegation.** `create_pd`, `delegate`, and `revoke` operations manage
capability presence in a PD's table. The `delegate` operation copies a
capability from one PD's table to another.

---

### 2.6 Barrelfish — Per-Core CNode Trees

**Unit of table:** Per-core. Each CPU driver (kernel instance) manages a local
capability space for the entities on that core. Capabilities are not shared in
kernel memory across cores; cross-core copies are managed by user-mode Monitor
processes using a two-phase protocol.

**Structure.** Uses a CNode model similar to seL4's: a tree of CNode objects
with indexed slots. Resolution walks the tree from a root CNode, consuming bits
of the capability address at each level.

**Cross-core revocation cost.** Nevill (2012 master's thesis, ETH Zürich)
measured: local cap operations ~100 µs, cross-core revoke (2-core round-trip) ~1
ms, scaling with the number of cores holding copies. The paper concludes
fine-grained cross-core revocation is an architectural concern.

---

### 2.7 Genode — Session Capability Tree

**Unit of table:** Per-component (each Genode process). Capabilities are
references to RPC objects; they are passed as session establishment arguments
and responses.

**Structure.** Genode uses whatever the underlying kernel provides (NOVA
selectors on NOVA, seL4 CSpace on seL4, etc.). At the Genode framework level,
authority flows strictly from parent to child: a child can only hold
capabilities its parent granted. The session hierarchy mirrors the component
tree, providing a human-visible authority graph that the underlying kernel's
table structure does not itself enforce.

---

## 3. Structural Properties Compared

| System     | Table unit  | Shape      | Lookup cost           | Userspace controls shape |
| ---------- | ----------- | ---------- | --------------------- | ------------------------ |
| seL4       | per-thread  | tree       | O(depth), ~2 accesses | Yes — via CNode retype   |
| Zircon     | per-process | flat list  | O(1) arithmetic       | No                       |
| EROS       | per-process | flat array | O(1) direct index     | No (fixed size)          |
| Mach       | per-task    | flat table | O(1)                  | No                       |
| NOVA       | per-PD      | flat array | O(1)                  | No                       |
| Barrelfish | per-core    | tree       | O(depth)              | Yes (monitor manages)    |

---

## 4. Granularity: Thread vs. Process vs. Merged Unit

Most systems split execution unit (thread) from capability namespace
(process/task/PD). The split serves a specific purpose: multiple threads in one
process sharing a single capability namespace without kernel-mediated copying.

seL4 holds the capability namespace at the thread level (each TCB has a root
CNode), but allows threads to share namespaces by pointing their TCBs at the
same root CNode. The kernel enforces no sharing constraint; sharing is userspace
policy.

Zircon places the namespace firmly at the process level; threads cannot have
independent namespaces, cannot hold handles, and cannot be transferred between
processes. This design choice was deliberate in Zircon's model — threads are not
first-class authority holders.

In systems where execution unit = capability holder (as in a design where there
is no separate process/thread split), the mapping is one-to-one by construction:
each execution entity has exactly one capability table, and no sharing question
arises at the kernel level (sharing becomes a user-level concern).

---

## 5. Known Costs and Tradeoffs

### 5.1 Tree depth vs. flat lookup

A flat table with O(1) arithmetic lookup (Zircon's pointer reconstruction,
EROS's direct index) has lower per-syscall overhead than a tree walk. However,
the seL4 team's measurements show that for 2-level CSpaces the added cost of two
memory lookups is small relative to total syscall overhead (context switch
dominates).

For real-time workloads, a tree introduces variable lookup depth depending on
CSpace configuration — though a fixed-depth configuration (always 1-level or
always 2-level) bounds the cost at design time.

### 5.2 Who controls table shape

Kernel-owned flat tables are simpler and easier to formally reason about: the
kernel has complete knowledge of every capability in every table at all times.
seL4's formal verification relies on this: the kernel's capability derivation
tree (CDT) is a kernel-side data structure that tracks all extant capabilities
independent of CSpace layout.

Userspace-composable trees (seL4, Barrelfish) allow userspace to partition its
authority space hierarchically without kernel involvement — useful for
implementing language-level or component-level capability abstractions. The cost
is increased complexity for the kernel resolver and for userspace programmers.

### 5.3 Table size and overflow

Flat tables with a hard capacity (EROS: 16 KeyKOS slots, fixed EROS slots) bound
memory consumption but limit expressiveness. Zircon's linked-list table grows
dynamically (bounded by process memory). seL4's CNode approach requires
userspace to pre-allocate physical memory for each CNode from a finite untyped
pool; over-allocation wastes physical memory and under-allocation causes
failure.

### 5.4 Sharing and isolation

In a flat per-process table (Zircon), threads in the same process share the
table by construction — there is no kernel mechanism for intra-process
capability isolation between threads. seL4's per-thread root allows
intra-process thread isolation: two threads in the same process can have
disjoint CSpaces, allowing language runtimes (e.g., a WebAssembly sandbox inside
one process) to implement authority isolation without a new process boundary.

### 5.5 Revocation

Flat table models typically revoke at object granularity: destroy the object
(Zircon), and all handles to it become invalid. Per-handle revocation is not
native in Zircon (requires wrapper objects). EROS supports it via link chains.
seL4 supports it via CDT traversal, which is O(derived caps) and can be
unbounded in the non-MCS kernel.

---

## 6. Measured Data

| Data point                                                       | Source                           |
| ---------------------------------------------------------------- | -------------------------------- |
| seL4 CSlot size: 32 bytes on 64-bit (AArch64)                    | seL4 Reference Manual 14.0.0     |
| seL4 2-level CSpace lookup: ~2 memory accesses                   | seL4 docs; confirmed on tutorial |
| Cap lookup is minority of IPC cost in seL4                       | seL4 team measurements (see ref) |
| Barrelfish local cap op: ~100 µs                                 | Nevill 2012 thesis, ETH Zürich   |
| Barrelfish cross-core revoke (2-core): ~1 ms                     | Nevill 2012 thesis, ETH Zürich   |
| Zircon handle value: 32-bit, 2 LSBs always set                   | Fuchsia docs                     |
| Zircon handle table: doubly-linked list + O(1) arithmetic lookup | Fuchsia source (handle_table.h)  |

---

## References

- seL4 Reference Manual Version 14.0.0.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf

- seL4 Capabilities Tutorial.
  https://docs.sel4.systems/Tutorials/capabilities.html

- seL4 Mailing List: Capability resolution depth mismatches.
  https://devel.sel4.systems.narkive.com/x8EjFWIu/capability-resolution-depth-mismatches

- Fuchsia. "Zircon Handles."
  https://fuchsia.dev/fuchsia-src/concepts/kernel/handles

- Fuchsia. "Zircon Kernel Objects."
  https://fuchsia.dev/fuchsia-src/reference/kernel_objects/objects

- Zircon handle_table.h source.
  https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/kernel/object/include/object/handle_table.h

- Jonathan Shapiro, Jonathan Smith, David Farber. "EROS: A Fast Capability
  System." _SOSP 1999_. https://flint.cs.yale.edu/cs428/doc/eros.pdf

- Simon Nevill. "Capabilities in Barrelfish." Master's thesis, ETH Zürich, 2012.
  https://barrelfish.org/publications/nevill-master-capabilities.pdf

- Udo Steinberg, Bernhard Kauer. "NOVA: A Microhypervisor-Based Secure
  Virtualization Architecture." _EuroSys 2010_. https://hypervisor.org/

- Genode on NOVA.
  https://genode.org/documentation/genode-foundations/20.05/under_the_hood/Execution_on_the_NOVA_microhypervisor_(base-nova).html

- Andrew Baumann et al. "The Multikernel: A New OS Architecture for Scalable
  Multicore Systems." _SOSP 2009_.
