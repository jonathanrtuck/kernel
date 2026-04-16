# The Capability-Designated Memory Resource

**Question:** In a capability-based kernel, what is the kernel object that
represents physical memory as a capability? How is that object structured,
distributed, typed, and reclaimed across real systems?

---

## 1. Framing the Question

A capability-based kernel controls access to every resource via capabilities.
Physical memory is a resource like any other. The question is: what does a
capability _to physical memory_ look like before it has been put to any
particular use?

This is distinct from the mapping question (how do you establish
virtual-to-physical correspondence?) and from the kernel-object question (how do
you create a TCB or endpoint?). The question is specifically about the precursor
capability — the handle that represents raw physical storage and from which
typed resources are derived.

This question has several dimensions:

- **Granularity:** Is the capability to a single page, an arbitrary contiguous
  range, or some other unit?
- **Typing model:** Does allocating memory and typing it (into a frame, a TCB,
  etc.) happen in one step or two?
- **Zeroing:** When is memory scrubbed?
- **Device vs. RAM distinction:** How is MMIO handled vs. normal RAM?
- **Initial distribution:** How does the boot-time physical memory reach the
  first userspace process?
- **Revocation and reclamation:** How does memory return to the pool?
- **Accounting:** Who pays for the physical backing of kernel objects?

---

## 2. Survey of Systems

### 2.1 seL4 — Untyped Memory

**The capability:** `Untyped` — a capability to a contiguous, power-of-2-aligned
region of physical memory. Size is specified as a bit-width: an n-bit Untyped
covers 2^n bytes.

**Typing model:** Two-phase. Untyped memory exists before any particular
interpretation is applied. Invoking `seL4_Untyped_Retype` converts a sub-region
of the Untyped into a typed kernel object (CNode, TCB, Endpoint, Frame, smaller
Untyped, etc.). Until retyped, the bytes have no kernel semantics.

**Allocation strategy:** Bump allocator (watermark). Each Untyped tracks a
`FreeIndex` (watermark); allocation advances it forward. Memory before the
watermark is unavailable until all derived capabilities are revoked and the
Untyped is reset. Creating objects largest-first minimizes alignment waste;
allocating a small object then a large object can leave unreachable padding
between them.

**Zeroing:** Regular Untyped memory is zeroed by the kernel before being handed
to the root task and again before retype. Device Untyped memory (flagged at
boot) cannot be written by the kernel — it may map hardware registers or ROM,
not RAM.

**Device vs. RAM:** The `device` boolean flag distinguishes the two. Device
Untyped can only be retyped into Frame objects (mapped pages), never into
CNodes, TCBs, or other kernel objects. The kernel rejects attempts to create
non-memory objects from device Untyped.

**Initial distribution:** The root task's initial CSpace contains one Untyped
capability for every contiguous physical memory region the bootloader reported.
The root task begins with all physical memory explicitly enumerated as Untyped
capabilities. No physical memory is hidden in a kernel pool.

**Revocation:** The Capability Derivation Tree (CDT) records parent-child
relationships. `seL4_CNode_Revoke` deletes all capabilities derived from a CNode
slot (all children in the CDT subtree). To reset and reclaim an Untyped, all
derived capabilities must be revoked first.

**Accounting:** Every kernel object's physical backing comes from
user-controlled Untyped. The kernel has no private heap for kernel objects. A
TCB consumes exactly one TCB-sized chunk of an Untyped the caller provided.
Kernel memory is thus subject to userspace accounting.

**Source:** seL4 Reference Manual 14.0.0 (NICTA/CSIRO); seL4 Untyped tutorial at
docs.sel4.systems.

---

### 2.2 Barrelfish — RAM Capabilities

**The capability hierarchy:**

```text
PhysAddr (raw physical address range)
    └─ retype to ──> RAM (zeroed, reclaim-safe)
                     DevFrame (device/MMIO, not zeroed)
    RAM
    └─ retype to ──> Frame (mappable page), CNode, VNode, Dispatcher
```

`PhysAddr` is the root. It names a physical range without claiming the memory is
safe to use as RAM. `RAM` is the derived capability after zeroing is confirmed.
`Frame` is a `RAM` that has been committed for mapping.

**Typing model:** Multi-phase retype chain. `PhysAddr` → `RAM` → `Frame` (or
other typed object). Each retype produces a new capability of a new type; the
original is consumed. Capabilities can also be split into two halves (splitting
creates two capabilities of the same type, each covering half the original
region).

**Device vs. RAM:** `DevFrame` is a separate capability type produced by
retyping `PhysAddr` without zeroing. DMA-visible device memory goes through
`DevFrame`.

**Initial distribution:** The boot info structure passed to the init domain
contains a `PhysAddr` capability for all physical memory. The init domain
(acting as a user-level memory manager) retyped this into `RAM` capabilities and
distributed them to other components.

**Revocation:** Barrelfish moved capability management entirely to userspace.
Per-core monitors maintain local copies of the capability tree; consistency
between cores is maintained via user-level protocols (not kernel-enforced).
Revoking a capability requires informing all monitors holding copies — a
distributed coordination problem.

**Accounting:** Explicit. RAM capabilities are owned and tracked by the
user-level memory manager. The kernel has no memory pool; all kernel objects
(dispatchers, CNodes, VNodes) are backed by RAM capabilities provided by
userspace.

**Source:** Barrelfish OS Architecture Overview (ETH Zurich/MSR, TN-000);
Bodunhu blog "A Little Review on Barrelfish Memory Management"; Nevill,
"Capabilities in Barrelfish" (master's thesis, ETH Zurich).

---

### 2.3 EROS and KeyKOS — Page Keys

**The capability:** A _page key_ (KeyKOS) or _page capability_ (EROS) — a
capability to exactly one hardware page of physical memory. Pages are the atomic
unit; there is no multi-page "region" capability at the kernel level.

**Typing model:** Pages do not carry a type distinguishing intended use. A page
is always a page. The distinction between data pages and page-table pages is
made structurally: a page used as a node (the EROS/KeyKOS term for a
capability-holding page) is addressed via a node key. The kernel enforces that a
given page is used either as data or as a node, not both, through separate key
types.

In EROS and KeyKOS, the two fundamental kernel-managed storage units are:

- **Page:** one hardware page, holds data.
- **Node:** holds 16 capabilities (fixed size; no data).

**Space Bank:** Physical page management is NOT done by the kernel directly. The
Prime Space Bank (a privileged userspace service) holds _range keys_ —
capabilities to contiguous runs of pages. Applications hold a capability to a
space bank and call it to buy (allocate) or sell (reclaim) pages and nodes.

**Initial distribution:** The Prime Space Bank is the single userspace entity
holding all physical memory. It delegates sub-banks by creating derived space
banks with page/node quotas. Allocation is hierarchical: each buy decrements the
limit in the bank and all superior banks; if any superior limit hits zero, the
allocation fails.

**Accounting:** Per-page explicit accounting through the space bank tree. Banks
impose limits on how many pages subordinate banks may net-allocate. This
provides hierarchical delegation without kernel involvement.

**Source:** "EROS: A Fast Capability System" (Shapiro et al., SOSP 1999); "The
KeyKOS Nanokernel Architecture" (Bomberger, Hardy, et al., 1992); cap-lore.com
"KeyKOS Space Banks" and "Differences Between Coyotos and EROS."

---

### 2.4 Coyotos — Pages and CapPages

**The capabilities:**

- **Page:** The atomic mappable unit. One hardware page. Holds data.
- **CapPage:** One hardware page. Holds capabilities (not data). Capabilities
  are 16-byte aligned, opaque 16-byte values.
- **GPT (Generalized Page Table):** A fixed-length vector of 16 capabilities,
  each paired with a guard. GPTs compose hierarchical address spaces. The child
  of a GPT slot can be a Page, CapPage, or another GPT, enabling trie-like
  address space construction.

**Device pages:** A Page capability with cache-control attributes (CD — cache
disable, WT — write through) and permission restrictions (RO, NX). Device pages
represent MMIO regions.

**Typing model:** Single-phase. A Page is always a Page; a CapPage is always a
CapPage. No retype step; the distinction is established at allocation time. This
avoids the seL4 retype operation but means the kernel must track which pages are
used as capability storage vs. data storage.

**Initial distribution:** Coyotos documentation does not specify a standard
boot-time distribution mechanism in detail; the design preserves EROS's space
bank model.

**Source:** "Coyotos Microkernel Specification" (Shapiro); cap-lore.com
EROS/Coyotos comparison.

---

### 2.5 Genode — RAM Dataspaces

**The capability:** A `Ram_dataspace_capability` — a capability returned by the
`Ram_allocator` interface (implemented by the PD session). A dataspace is an
abstract handle for a contiguous physical memory region of arbitrary size (at
page granularity). Dataspaces do not carry physical addresses visible to the
holder.

**Typing model:** Dataspaces are not typed by intended use. A dataspace is
always a "region of RAM" regardless of how it will be used. Mapping (attaching
to a region map) is a separate step.

**Quota model:** Each component receives a RAM quota from its parent. The PD
session's `Ram_allocator` allocates dataspaces against this quota. When the
quota is exhausted, `Ram_allocator::alloc()` throws `Out_of_ram`. The
`Constrained_ram_allocator` wrapper enforces per-client quotas at the
application level. Quota is transferred between components to trade RAM
authority.

**Core as sole physical address holder:** Only Genode's `core` component (the
first user-level process, started by the kernel) holds physical addresses. All
other components interact exclusively via dataspaces. Core translates a
dataspace allocation request into a physical page allocation and returns an
opaque capability.

**Initial distribution:** Core receives all physical memory at boot (from kernel
boot info). The first component created by core (`init`) is given a RAM quota
that covers the system's total free RAM. Init further subdivides this among
child components.

**MMIO:** MMIO dataspaces are also issued by core via a separate `io_mem`
service. They are structurally identical to RAM dataspaces but map device
regions and carry cache-policy information.

**Revocation:** Dropping the dataspace capability and communicating to core that
the quota should be returned; core unmaps the pages from any region maps that
had attached them.

**Source:** Genode Foundations (Feske); Genode documentation on Physical Memory
Allocation and Resource Trading; genode.org/documentation.

---

### 2.6 Zircon / Fuchsia — VMO (Virtual Memory Object)

**The capability:** A `Handle` to a `Vmo` kernel object. A VMO represents a
container of zero to `size` bytes of memory managed by the OS. VMOs exist
independently of any virtual mapping.

**Creation variants:**

- `zx_vmo_create(size)` — anonymous, zero-filled, kernel-backed.
- `zx_vmo_create_physical(resource, paddr, size)` — maps a specific physical
  range; requires a `Resource` handle scoped to that physical range (a separate
  privilege capability).
- `zx_vmo_create_contiguous(bti, size, align_log2)` — requests physically
  contiguous backing for DMA.

**Typing model:** VMOs do not carry type information about intended use. A VMO
that will be used as stack memory is indistinguishable at the kernel level from
one used for IPC buffers. Type is a userspace concern.

**Kernel pool model:** Zircon does NOT expose raw physical memory to userspace.
The kernel manages a physical page allocator internally and services VMO
allocation requests from that pool. There is no "untyped" capability to physical
memory that userspace holds. This means physical memory accounting is implicit
(kernel-maintained stats) rather than explicit (user-controlled capabilities).

**Initial distribution:** No physical memory capability is given to the first
process. The kernel holds all physical memory in its internal allocator.
Userspace requests allocations via `zx_vmo_create`.

**Revocation:** Handle reference counting. When the last handle to a VMO is
closed (and no VMARs have active mappings), the VMO is destroyed and pages
returned to the kernel pool.

**MMIO:** Device drivers receive a `Resource` capability scoped to specific
physical ranges (established at boot). They invoke `zx_vmo_create_physical`
using this resource to get a VMO over that range.

**Source:** Fuchsia kernel concepts documentation (fuchsia.dev); `zx_vmo_create`
reference syscall documentation; Zircon Kernel Concepts.

---

### 2.7 Mach / XNU — vm_object

**The capability:** Not a single explicit physical memory capability. Mach's
`vm_object` is an internal kernel structure backing virtual regions; it is not
directly exposed as a user-visible capability. User-visible handles are ports
that mediate access to memory managers (including the kernel's default pager).

Users request memory via `vm_allocate`; the kernel creates a `vm_object` and
maps it into the task's address space. Physical backing is demand-paged.

The `memory_object` interface allows external pagers: a userspace server can
supply pages for a memory region by responding to page-fault messages sent to
its receive port. This is a different model from capability-to-physical-memory —
it's capability-to-pager.

**Source:** Mach 3 Kernel Principles (CMU); XNU source (osfmk/vm/).

---

### 2.8 L4 family (Fiasco.OC / L4Re)

**The capability:** L4Re uses a `Dataspace` abstraction (similar to Genode's
dataspace; Genode is built on top of L4Re/Fiasco.OC for one of its kernel
backends). The underlying Fiasco.OC kernel provides `Task` and `Vcpu` objects;
physical memory management at the kernel level uses a buddy allocator
internally.

L4Ka::Pistachio: the `sigma0` root pager holds identity-mapped capabilities to
all physical RAM and delegates mappings (fpages) to sigma1 and then to tasks.
The kernel does not expose an explicit "physical memory capability" type —
access to physical pages is represented as virtual memory mappings from sigma0.

**Source:** L4 eXperimental Kernel Reference Manual X.2; L4Re Architecture
Concepts documentation.

---

## 3. Design Dimensions Summary

### 3.1 Granularity of the physical memory capability

| System      | Granularity of physical memory capability        |
| ----------- | ------------------------------------------------ |
| seL4        | Arbitrary contiguous range (power-of-2 aligned)  |
| Barrelfish  | Arbitrary range; can be split                    |
| EROS/KeyKOS | Fixed: one page per capability                   |
| Coyotos     | Fixed: one page (Page or CapPage)                |
| Genode      | Arbitrary range (at page granularity)            |
| Zircon      | Arbitrary (but no explicit physical cap to user) |
| L4 classic  | fpage (power-of-2 aligned virtual region)        |

Fixed-page granularity (EROS/Coyotos) gives every page equal first-class status
at the cost of a O(n) capability space to manage n pages. Arbitrary-range
capabilities (seL4, Barrelfish, Genode) amortize this with O(log n) or fewer
capabilities for large contiguous regions, at the cost of fragmentation
complexity.

### 3.2 Typing model

| System     | When does memory get typed?                                          |
| ---------- | -------------------------------------------------------------------- |
| seL4       | At retype (explicit Untyped_Retype invocation); two-phase            |
| Barrelfish | Multi-step retype chain: PhysAddr → RAM → Frame/CNode/VNode          |
| EROS       | At buy/allocation from space bank; node vs. page is the only split   |
| Coyotos    | At allocation: Page vs. CapPage distinguished                        |
| Genode     | Dataspace is always "RAM region"; type assigned by user-level use    |
| Zircon     | VMO is the only user-visible type; no kernel-visible type within VMO |

### 3.3 Zeroing semantics

| System     | Zeroing guarantee                                                 |
| ---------- | ----------------------------------------------------------------- |
| seL4       | Scrubbed before first mapping; scrub before retype into object    |
| Barrelfish | RAM cap guarantees zeroed; PhysAddr and DevFrame do not           |
| EROS       | Kernel zeros pages on allocation (to prevent information leakage) |
| Zircon     | VMOs zero-initialized at creation                                 |
| Genode     | Core zeros RAM dataspace before issuing capability                |

### 3.4 Device vs. RAM distinction

| System     | How device/MMIO is distinguished                             |
| ---------- | ------------------------------------------------------------ |
| seL4       | `device` boolean on Untyped cap; can only retype into Frame  |
| Barrelfish | `DevFrame` capability type (separate from `RAM`)             |
| Coyotos    | `Page` with cache-control attribute bits (CD, WT)            |
| Genode     | Separate `io_mem` service; dataspace with cache-policy info  |
| Zircon     | `Resource` capability scoped to physical range; separate VMO |

### 3.5 Accounting model

| System      | Memory accounting mechanism                                         |
| ----------- | ------------------------------------------------------------------- |
| seL4        | All kernel objects backed by user-owned Untyped; no kernel heap     |
| Barrelfish  | Explicit: RAM caps owned by userspace; kernel has no pool           |
| EROS/KeyKOS | Hierarchical space bank tree; per-bank page/node quota              |
| Genode      | Explicit RAM quota; transferred between sessions; Out_of_ram on OOM |
| Zircon      | Implicit: kernel pool; userspace sees usage stats, not caps         |

---

## 4. Tradeoffs

**Explicit physical caps (seL4, Barrelfish, EROS) vs. implicit kernel pool
(Zircon, Mach)**

Explicit: The kernel cannot consume memory without the user's knowledge. All
kernel object memory is accounted to user-space capabilities. This enables
resource guarantees — a bounded process cannot cause kernel OOM. Cost: the root
task must carefully manage Untyped allocations at boot; the programming model is
more complex; fragmentation from alignment waste is visible and must be managed.

Implicit: Familiar programming model (malloc-like). Kernel hides memory
management complexity. Cost: kernel memory accounting is statistical; OOM
conditions are handled by kernel policy (OOM killer, allocation failures) rather
than by userspace policy; memory usage of kernel objects on behalf of a process
is not charged to that process.

**Two-phase retype (seL4) vs. single-phase allocation (Genode, Zircon)**

Two-phase separates "I have authority over this memory" from "I'm using this
memory as an X." This means a batch of Untyped memory can be typed into
heterogeneous objects (TCBs, CNodes, Frames) without multiple allocation steps.
Cost: the retype interface is a separate syscall surface; the two-phase model
requires the caller to understand both the allocation side and the retype side.

Single-phase is simpler: `create_object(type, size)` allocates and types in one
step. Cost: no separation between "who owns the backing memory" and "what is it
used for" — these become the same question.

**Fixed-page granularity (EROS/Coyotos) vs. range capabilities (seL4,
Barrelfish)**

Fixed-page gives every physical page equal first-class status. Authority can be
delegated at single-page precision without splitting. Cost: managing thousands
of pages requires thousands of capabilities; capability space overhead scales
with physical memory size.

Range capabilities allow the root to start with O(log n) capabilities for n
physical pages. Delegation at sub-page granularity requires splitting. The split
operation (or retype into smaller Untypeds) is the mechanism for delegating
smaller chunks. Cost: alignment and granularity constraints can cause
fragmentation waste (most visible in seL4 bump allocator alignment padding).

**Capability-as-page (EROS nodes) vs. separate object types (seL4 CNode/Frame)**

EROS and Coyotos use pages as capability storage (nodes/CapPages). The page is
the unit; what it stores (data vs. capabilities) is a secondary property. This
makes the kernel's type system minimal: page, node (EROS), or page/cappage
(Coyotos).

seL4 separates Frame (mappable data page) from CNode (capability-holding
structure) as distinct kernel object types. These are created from Untyped but
are structurally different. Cost: more kernel object types; benefit: the kernel
can enforce the distinction (a CNode slot cannot be misused as data and vice
versa).

**Userspace memory management (EROS SpaceBank, Barrelfish monitors) vs. kernel
memory management (Zircon, Mach)**

Userspace memory management pushes policy into applications: a SpaceBank can
implement quota policies, priorities, and sharing patterns the kernel doesn't
hardcode. Cost: correctness of the memory manager becomes critical system
security property; misbehaving memory managers can starve the system.

Kernel memory management simplifies the trusted computing base: one memory
allocator, one policy. Cost: inflexible — all processes subject to the same
policy; kernel OOM handling is a special case rather than a normal resource
constraint.

**CHERI note:** CHERI ISAs (ARMv8-CHERI, CHERI-RISC-V) provide hardware-enforced
capability pointers with bounds, permissions, and provenance. This allows
_intra-address- space_ access control at the hardware level. CHERI capabilities
are complementary to kernel capability tables: kernel caps control what objects
a process can name; CHERI caps control what memory within a named object a
program can access. A system combining both would use kernel capabilities for
inter-process naming and CHERI for intra-process memory safety, potentially
eliminating the need for mmap-permission-faults for bounds checking. seL4 has a
CHERI research port (CheriFreeOS); Zircon has not publicly pursued it.

---

## 5. Measured Data

**seL4 Untyped retype cost:** `seL4_Untyped_Retype` on ARM is a cold-path
operation (involves CDT manipulation); measured at ~400–800 cycles in published
seL4 benchmarks for AArch64. This is expected to be rare (system setup, not
steady-state).

**seL4 alignment waste:** A worked example from seL4 documentation: allocating a
4 KiB object then a 16 KiB object in an Untyped wastes 12 KiB to alignment. The
advice (largest-first) minimizes this but cannot eliminate it if allocations are
mixed-size and interleaved.

**Barrelfish RAM retype:** No published steady-state cycle count found in
surveyed sources. The cost involves capability type check + zeroing (for RAM
from PhysAddr). The multi-monitor coherence protocol for cross-core revocation
is described as the primary overhead in the Barrelfish architecture overview.

**Genode quota exhaustion:** `Out_of_ram` throws on exhaustion; no retry, no
kernel OOM killer. Applications are expected to handle it. No latency
measurements found in surveyed sources.

---

## 6. References

- seL4 Reference Manual 14.0.0. NICTA/CSIRO/seL4 Foundation.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 Untyped Tutorial. seL4 Foundation.
  https://docs.sel4.systems/Tutorials/untyped.html
- Shapiro, J. et al. "EROS: A Fast Capability System." SOSP 1999.
  https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf
- Shapiro, J. "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- Bomberger, A., Hardy, N., et al. "The KeyKOS Nanokernel Architecture." 1992.
  https://css.csail.mit.edu/6.5660/2017/readings/keykos.pdf
- cap-lore.com. "KeyKOS Space Banks."
  http://www.cap-lore.com/CapTheory/KK/KKBank.html
- cap-lore.com. "Differences Between Coyotos and EROS."
  http://www.cap-lore.com/CapTheory/KK/Shap/eros-comparison.html
- Baumann, A. et al. "The Multikernel: A New OS Architecture for Scalable
  Multicore Systems." SOSP 2009.
  https://people.inf.ethz.ch/troscoe/pubs/sosp09-barrelfish.pdf
- Nevill, D. "Capabilities in Barrelfish." Master's thesis, ETH Zurich.
  https://barrelfish.org/publications/nevill-master-capabilities.pdf
- Barrelfish Architecture Overview (TN-000). ETH Zurich/MSR.
  https://barrelfish.org/publications/TN-000-Overview.pdf
- Feske, N. Genode Foundations. Genode Labs.
  https://genode.org/documentation/genode-foundations/
- Genode Physical Memory Allocation.
  https://genode.org/documentation/genode-foundations/22.05/functional_specification/Physical_memory_allocation.html
- Fuchsia Kernel Concepts. Google.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
- zx_vmo_create reference. Fuchsia.dev.
  https://fuchsia.dev/reference/syscalls/vmo_create
- L4 eXperimental Kernel Reference Manual X.2 r7.
