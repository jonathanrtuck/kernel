# Space Resize: Operations, Authority, and Constraints

## Question

Can a memory object (Space) be resized after creation, and if so, what are the
semantics — syscall shape, authority required, constraints on direction and
alignment, and effects on existing accesses?

The question arises because of a structural constraint specific to
capability-addressed memory systems: when each Space has a kernel-assigned VA
base fixed at creation, faults on `base_of(S) + offset` where
`offset >= size(S)` cannot be resolved by providing a new Space (a new Space has
its own VA base, so the retrying instruction still misses). Resize-in-place is
the only mechanism that resolves such faults transparently. The question is how
existing systems handle this and what design dimensions exist.

---

## 1. Zircon / Fuchsia — `zx_vmo_set_size()`

Zircon's Virtual Memory Object (VMO) is the closest system analogue: a memory
object with an explicit size attribute, independent of any mapping.

### Syscall shape

```c
zx_status_t zx_vmo_set_size(zx_handle_t handle, uint64_t size);
zx_status_t zx_vmo_get_size(zx_handle_t handle, uint64_t* size);
```

The VMO's size is set and queried independently of any VMAR mapping.

### Authority encoding

Two distinct mechanisms gate resize authority:

1. **Opt-in at creation.** A VMO must be created with `ZX_VMO_RESIZABLE` (or
   `ZX_VMO_CHILD_RESIZABLE` for clone VMOs). A non-resizable VMO returns
   `ZX_ERR_UNAVAILABLE` on any `zx_vmo_set_size()` call, regardless of rights
   held. This is a property of the object, established at creation, that cannot
   be added later.

2. **Capability rights.** The handle must carry both `ZX_RIGHT_WRITE` and
   `ZX_RIGHT_RESIZE`. Holding WRITE alone is insufficient to resize. RESIZE is a
   separate right bit that can be stripped when delegating a handle: a holder
   can be given WRITE (to read/write content) without RESIZE (to change the
   object's size). Stripping RESIZE is irreversible on that handle.

**Source:** `zx_vmo_set_size` reference, Fuchsia.dev; Zircon Kernel Concepts
documentation (fuchsia.dev/fuchsia-src/concepts/kernel/concepts).

### Size rounding

The kernel rounds the requested size up to the next page boundary. Subsequent
`zx_vmo_get_size()` returns the rounded value. RFC-0238 (January 2024)
introduced a separate "stream size" (byte-precise content size) tracked via
`zx_vmo_get_stream_size()` / `zx_vmo_set_stream_size()`, decoupling page-aligned
allocation size from byte-precise content size.

### Grow semantics

New pages added by a grow operation are demand-zero: physical pages are not
allocated immediately. They are faulted in on first access. No pager is invoked
for demand-zero pages.

### Shrink semantics

On shrink:

- Bytes in the last remaining partial page (up to the page boundary) are zeroed
  by the kernel.
- Physical pages beyond the new size are freed.
- `ZX_ERR_BAD_STATE` is returned if any pages that would be freed are pinned
  (DMA-locked). Shrink of a pinned VMO is rejected.

### Effect on existing VMAR mappings

Zircon separates two behaviors:

- **Default (without `ZX_VM_FAULT_BEYOND_STREAM_SIZE`):** The VMAR mapping
  remains at its original size. Accessing addresses in the mapping that now fall
  beyond the VMO's new size generates a page fault delivered to the thread. The
  kernel does not automatically trim the VMAR mapping. The application must
  unmap the excess range explicitly.

- **With `ZX_VM_FAULT_BEYOND_STREAM_SIZE` flag (RFC-0238):** Zircon
  automatically unmaps pages beyond the stream size when the stream size
  shrinks. This provides POSIX-like semantics (Linux, macOS, Windows for
  memory-mapped files) where reads past the end of a file generate SIGBUS rather
  than reading zeros. This behavior requires `ZX_VM_ALLOW_FAULTS` on the VMAR
  mapping and is opt-in per-mapping.

### Error conditions

| Code                   | Condition                                          |
| ---------------------- | -------------------------------------------------- |
| `ZX_ERR_ACCESS_DENIED` | Handle lacks `ZX_RIGHT_RESIZE` or `ZX_RIGHT_WRITE` |
| `ZX_ERR_UNAVAILABLE`   | VMO was not created with `ZX_VMO_RESIZABLE`        |
| `ZX_ERR_BAD_STATE`     | Shrink would free pinned pages                     |
| `ZX_ERR_OUT_OF_RANGE`  | Requested size exceeds system limits               |
| `ZX_ERR_NO_MEMORY`     | Kernel cannot allocate internal bookkeeping        |

**Source:** `zx_vmo_set_size` reference
(fuchsia.dev/reference/syscalls/vmo_set_size); RFC-0238: VMO size
(fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0238_vmo_size).

---

## 2. seL4 — VSpace has no size attribute

seL4's VSpace (the address space root object on AArch64, `seL4_ARM_VSpace`) is a
page table root frame covering the entire 2^48-byte user virtual address space.
There is no size attribute on a VSpace: it is not a sized container. The VSpace
object does not grow or shrink — it is always the full ARM64 user address range.

"Growing" an address space in seL4 means installing new page table structures
and frame mappings within the VSpace. The operations are:

- **`seL4_ARM_PageTable_Map`**: Installs an intermediate page table (PGD, PUD,
  PMD level) at a specific virtual address in the VSpace.
- **`seL4_ARM_Page_Map`**: Installs a Frame at a specific virtual address (maps
  physical memory into the page table).

Both operations require:

- A capability to the VSpace or the Page/PageTable.
- The Frame or PageTable capability to be mapped in.
- The caller to manage VA placement explicitly.

Authority for mapping comes from holding the VSpace capability and the
Frame/PageTable capabilities. There is no separate "resize authority" because
there is no resize operation. The virtual address range is fixed by the hardware
(ARM64 TTBR0 covers 0 through 2^48 - 1); the question is only what is mapped
within it.

"Resizing" in the seL4 model is a userspace concept: user-level memory managers
(libsel4utils vspace library) track reserved virtual address ranges and can be
asked to "move or resize a reservation in any direction." This is a userspace
bookkeeping operation — the kernel has no reservation concept.

**Source:** seL4 Reference Manual 14.0.0; seL4 API Reference
(docs.sel4.systems); seL4_libs/libsel4vspace (GitHub seL4/seL4_libs).

---

## 3. Mach / XNU — No memory object resize; region-level operations

Mach does not expose the backing `vm_object` to userspace directly. Users
interact with a task's `vm_map` (address map) through:

- **`vm_allocate(task, &addr, size, flags)`**: Creates a new anonymous
  zero-filled region of the given size in the task's address space. Returns the
  chosen VA.
- **`vm_deallocate(task, addr, size)`**: Removes a range from the address space.
- **`vm_map(task, &addr, size, mask, flags, obj, offset, copy, prot, max_prot, inherit)`**:
  Maps a memory object at a chosen address.
- **`vm_remap(target_task, &addr, size, mask, flags, src_task, src_addr, copy, prot, max_prot, inherit)`**:
  Copies or moves a mapping from one task's address space to another.

There is no "grow this region in place" syscall. If a region needs to grow:

1. If the adjacent virtual range is free: `vm_allocate` at `old_end` for the
   delta. The two regions are contiguous in virtual space but are separate
   vm_map entries (unless coalesced by the kernel heuristically).
2. If the adjacent range is occupied: there is no in-place resize. The
   application must allocate a new larger region, copy or remap data, and
   deallocate the old region.

Authority: all vm\_\* operations require a send right to the target task's task
port (the task port = full control over that task's address space). There is no
finer-grained "resize authority" distinct from "map authority."

**Source:** Mach Kernel Interface Reference Manual (MIT Darwin docs);
`vm_map.defs` (Apple open source XNU).

---

## 4. EROS / CapROS / Coyotos — Address space extension via tree

In EROS and descendants, an address space is a tree of nodes/GPTs whose leaves
are page capabilities. "Resizing" the address space means structurally extending
the tree.

### EROS / CapROS

A Node has 16 capability slots. Adding a page key to an empty slot extends the
region that the Node covers at that index. Extension is always by adding
capabilities into existing slots — there is no "grow the Node" operation because
Nodes are fixed-size (16 slots, each covering a fixed address range).

To cover a wider address range, the caller adds a new level of Node above the
current root (increasing address space height). This is a structural operation:
create a new root Node, install the old root in slot 0, install new Nodes or
pages in subsequent slots.

Authority required: a key to the Node with write permission (`WK` — writable
key). Page keys are obtained from the Space Bank (a userspace allocator).

### Coyotos GPTs

GPTs (Guarded Page Tables) are fixed-size (16 slots) but each slot has a guard
value that enables efficient sparse address space representation. Adding a GPT
at a new guard/offset extends coverage. Like EROS nodes, GPT slots are filled
with capabilities (Page, CapPage, or sub-GPT). The address space grows by
populating previously empty slots or adding a new root GPT level.

Authority: a writable key to the GPT. No separate "resize" operation — growth is
insertion into existing slot positions.

**Source:** "EROS: A Fast Capability System" (Shapiro et al., SOSP 1999); CapROS
Address Spaces reference (capros.org); Coyotos Microkernel Specification
(Shapiro).

---

## 5. Genode — Region Map is fixed-size at creation

In Genode, a Region Map is created with a fixed virtual address size. The size
is specified at creation (`create_managed_dataspace(size)` or implicit for the
process's main address space). There is no `resize` operation on a region map.

To achieve larger coverage:

- Detach the current dataspace from the region map, attach a larger one.
- Attach additional dataspaces at higher offsets within the region map (if space
  is available).
- Create a new region map of the desired size.

None of these are in-place resize operations at the kernel level.

**Source:** Genode OS Framework Foundations (Feske); Genode documentation on
Region Maps (genode.org/documentation).

---

## 6. Linux — `mremap()` for in-place and relocating resize

Linux's `mremap()` is the most complete in-place resize mechanism in any
surveyed system.

```c
void *mremap(void *old_addr, size_t old_size, size_t new_size,
             int flags, ... /* new_addr */);
```

Flags:

- **`MREMAP_MAYMOVE`**: If in-place growth is not possible (the adjacent VA is
  occupied), the kernel is allowed to move the entire mapping to a new address.
  Returns the new address. The old mapping is removed.
- **`MREMAP_FIXED`**: Move to a specific new address (implies MAYMOVE). The
  destination range, if already mapped, is unmapped first.
- **No flag**: In-place only. Fails with `ENOMEM` if the adjacent VA is
  occupied.

Shrink: always succeeds; pages beyond the new size are unmapped. No authority
distinction for shrink vs. grow.

Authority: `mremap()` acts on the calling process's own address space; no
explicit capability is required beyond being the process owner.

**Constraint — `MREMAP_MAYMOVE` invalidates existing pointers:** After a move,
all existing pointers into the old VA range are invalid. This is a fundamental
tradeoff: in-place growth preserves pointer validity but may fail; relocation
always succeeds but breaks pointers. Linux exposes both modes; the caller
chooses.

**Source:** `mremap(2)` man page; Linux kernel source (mm/mremap.c).

---

## 7. Summary: Design Dimensions

### 7.1 Resize as a property of the object vs. the mapping

| System       | What is resized            | Resize target                                                |
| ------------ | -------------------------- | ------------------------------------------------------------ |
| Zircon       | VMO (memory object)        | Object's size attribute; mappings see faults on excess range |
| seL4         | N/A (VSpace covers all VA) | N/A                                                          |
| Mach         | vm_map entry (region)      | New region at adjacent VA; no "object" resize                |
| EROS/Coyotos | Node tree                  | Structural: new slots populated or new root added            |
| Genode       | N/A (Region Map is fixed)  | New region map required                                      |
| Linux        | mmap'd region              | In-place or relocated                                        |

Zircon is unique in having the resize operate on the memory object independently
of its mappings. All other systems either resize the mapping directly (Linux
`mremap`) or have no resize concept on the object (seL4, Genode).

### 7.2 Authority encoding for resize

| System       | Resize authority mechanism                                                           |
| ------------ | ------------------------------------------------------------------------------------ |
| Zircon       | Separate `ZX_RIGHT_RESIZE` right on the VMO cap; `ZX_VMO_RESIZABLE` flag at creation |
| seL4         | No resize; authority to map/unmap pages comes from VSpace + Frame caps               |
| Mach         | Task port (coarse-grained; task port = all authority over address space)             |
| EROS/Coyotos | Writable key to Node/GPT; page allocation from Space Bank                            |
| Linux        | Process context (no fine-grained resize authority)                                   |

Zircon provides the finest-grained authority: RESIZE is a separable right that
can be withheld when delegating a VMO cap. A read-only holder (WRITE stripped)
cannot resize even if the VMO was created resizable. A WRITE holder without
RESIZE can read/write content but cannot change the size.

### 7.3 Opt-in vs. always-resizable

Zircon requires `ZX_VMO_RESIZABLE` at creation. A non-resizable VMO cannot be
resized by any holder. This allows the creator to publish an immutable-size
contract to all holders. Other systems have no opt-in mechanism because they
have no object-level resize concept.

### 7.4 Grow: demand-zero vs. pager-backed new pages

| Scenario         | Behavior                                                                         |
| ---------------- | -------------------------------------------------------------------------------- |
| Zircon grow      | New pages are demand-zero; pager not invoked                                     |
| seL4 new mapping | New Frame capability required; page faults handled by userspace pager            |
| Linux grow       | New pages are demand-zero (anonymous mapping) or file-backed (backed file grown) |
| EROS/Coyotos     | New page capability must be explicitly provided from Space Bank                  |

Systems diverge on who supplies new pages after a grow. Demand-zero (Zircon,
Linux anonymous) means new pages are always available as long as physical memory
exists. Pager-backed (seL4, EROS) means new pages require explicit capability
provision from a resource manager.

### 7.5 Shrink with existing mappings

| System                                  | Effect on existing mappings                                             |
| --------------------------------------- | ----------------------------------------------------------------------- |
| Zircon (default)                        | Mapping remains; access beyond new VMO size faults the accessing thread |
| Zircon (ZX_VM_FAULT_BEYOND_STREAM_SIZE) | Kernel unmaps excess pages automatically on stream size reduction       |
| Linux mremap shrink                     | Excess VA immediately unmapped                                          |
| Mach vm_deallocate                      | Removes the entire region; no partial shrink on a single entry          |

Zircon's default behavior (mapping preserved; access faults) is a "lazy shrink":
the VMO's logical size shrinks but the VMAR mapping remains, and the thread
discovers the shrink via fault. This can be desirable (detect access beyond
content end) or undesirable (silent data loss if zeros are returned instead of
faults). RFC-0238's `ZX_VM_FAULT_BEYOND_STREAM_SIZE` flag makes the fault
behavior explicit and opt-in.

### 7.6 Pointer validity after resize

| Operation                              | Pointer validity                                                |
| -------------------------------------- | --------------------------------------------------------------- |
| In-place grow                          | All existing pointers remain valid                              |
| In-place shrink                        | Pointers beyond new size dangle (access faults or is undefined) |
| Relocating grow (Linux MREMAP_MAYMOVE) | All old pointers invalidated                                    |
| Zircon grow                            | All existing pointers remain valid (same VA base)               |

In-place grows always preserve pointer validity. This is the fundamental
advantage of in-place resize over allocate-copy-free for objects where pointers
have been shared.

### 7.7 Pinned pages and resize

Zircon explicitly rejects shrinking a VMO if it would free pinned (DMA-locked)
pages (`ZX_ERR_BAD_STATE`). This is a safety constraint: DMA-in-progress pages
cannot be freed.

Other systems handle this differently: seL4 prevents mapping removal if the
kernel has active state on a page; EROS/Coyotos prevent page capability
revocation while the page is in use.

---

## 8. Tradeoffs

**Object-level resize (Zircon) vs. region-level resize (Linux mremap) vs. no
resize (seL4, Genode)**

Object-level: The memory object has a size attribute independent of any mapping.
Resize changes the object; mappings observe the change (via fault on excess
access or automatic unmap). The authority question ("who can resize?") can be
answered at the capability level. Cost: the object model is more complex — the
object has a size lifecycle in addition to a content lifecycle.

Region-level (Linux mremap): Resize is an operation on a mapping (region), not
on a backing object. The backing object (anonymous pages, file) may be
implicitly grown. Simple mental model for applications but provides no
object-identity invariants across resize — the underlying backing object is
opaque.

No resize (seL4, Genode): The object is fixed-size; growing coverage requires
new objects. Maximum simplicity at the kernel level; growth is explicit (caller
provides a new object). Cost: in-place growth of a region accessed by pointer
requires allocate-copy-free at the application level.

**Separate RESIZE right (Zircon) vs. coarse task-port authority (Mach)**

Separate RESIZE right allows constructing delegation patterns like: "holder can
read and write content but cannot change the size." This enables immutable-size
contracts even when WRITE is delegated. Cost: more rights to manage; callers
must explicitly propagate RESIZE when delegation should include resize
authority.

Coarse authority (Mach task port, Linux process context): simpler for most
cases; any code that can map can also grow the region. Cost: no way to express
"delegated writer without resize authority."

**Opt-in resizability (Zircon ZX_VMO_RESIZABLE) vs. always-resizable**

Opt-in: The creator establishes a non-resizable contract. All holders receive
the same guarantee: the size will not change. Useful for shared read-only data,
page-aligned protocol buffers, and any use case where stable size is a protocol
invariant. Cost: the creator must predict at creation time whether resize will
ever be needed.

Always-resizable: Any holder with RESIZE authority can change size. Simpler
programming model. Cost: shared holders cannot rely on stable size without
out-of-band coordination.

**Demand-zero grow vs. pager-provisioned grow**

Demand-zero: New pages are always available (bounded only by physical memory);
no coordination needed with a pager. Cost: new pages cannot carry pre-existing
content; the pager has no opportunity to prefault or pin pages for the new
region.

Pager-provisioned: New pages are provided by the designated pager on first
access, same as any other page fault. Uniform fault path; the pager can
distinguish "first access to a grown region" from "retry of an existing fault."
Cost: requires a fault round-trip for every newly grown page; pager must handle
the case where it receives faults on addresses beyond the original Space size.

**Lazy shrink (Zircon default) vs. eager unmap (Linux mremap)**

Lazy: The VMAR mapping remains after VMO shrink; access to the excess region
faults. Good for use cases where the application needs to detect access beyond
the data stream. Avoids TLB shootdown cost at shrink time (shootdown deferred to
first access). Cost: accessing excess pages does not give an immediate
address-space-level signal; the fault path must distinguish "beyond VMO end"
from "normal page fault."

Eager: The mapping is unmapped immediately on shrink. Access to the old range
immediately raises a protection fault (SIGSEGV in Linux terms). No deferred
cost; no ambiguity. Cost: TLB shootdown must happen synchronously at shrink
time; more expensive for concurrent readers.

---

## 9. Measured Data

**Zircon `zx_vmo_set_size()` cost:** No published cycle count found in surveyed
sources. The operation modifies the VMO's size field (one atomic write) plus
potential page reclamation for shrink (involves freeing physical pages to the
kernel allocator). For grow with no backing pages required immediately, the cost
is close to an atomic size update.

**Linux `mremap()` in-place grow cost:** Benchmarks from the Linux community
show in-place grow involves adding a vm_area_struct entry or extending an
existing one — O(log n) in the number of VMAs. TLB shootdown is deferred to
first access (demand-zero). Cost is roughly comparable to `mmap()` for a new
anonymous region.

**Linux `mremap()` relocating grow cost (MREMAP_MAYMOVE):** Involves copying
page table entries from old to new VA range (O(n) in the number of pages), plus
TLB shootdown of the old range. For large regions, this can take milliseconds.

**Pointer invalidation on relocation:** No measured data specific to
capability-addressed systems. In standard userspace, pointer invalidation is a
correctness issue, not a performance issue — any access to a stale pointer is a
bug.

---

## 10. References

- `zx_vmo_set_size` reference. Fuchsia.dev.
  https://fuchsia.dev/reference/syscalls/vmo_set_size
- RFC-0238: VMO size. Fuchsia.dev (January 2024).
  https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0238_vmo_size
- `zx_vmar_map` reference. Fuchsia.dev.
  https://fuchsia.dev/reference/syscalls/vmar_map
- Zircon Kernel Concepts. Fuchsia.dev.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
- seL4 Reference Manual 14.0.0. seL4 Foundation.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 API Reference. seL4 Foundation.
  https://docs.sel4.systems/projects/sel4/api-doc.html
- seL4_libs/libsel4vspace/include/vspace/vspace.h. seL4 Foundation.
  https://github.com/seL4/seL4_libs/blob/master/libsel4vspace/include/vspace/vspace.h
- Mach Kernel Interface Reference Manual. MIT Darwin.
  https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/
- `vm_map` reference. MIT Darwin.
  https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/vm_map.html
- Shapiro, J.S., Smith, J.M., Farber, D.J. "EROS: A Fast Capability System."
  SOSP 1999.
  https://citeseerx.ist.psu.edu/document?repid=rep1&type=pdf&doi=198d9c3e33be1f49b3e743f3dd17a2c237cdb69f
- Shapiro, J.S. "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- CapROS Address Spaces reference.
  http://www.capros.org/devel/ObRef/concepts/AddressSpaces.html
- Feske, N. Genode OS Framework Foundations 25.05. Genode Labs.
  https://genode.org/documentation/genode-foundations-25-05.pdf
- `mremap(2)` Linux man page.
  https://man7.org/linux/man-pages/man2/mremap.2.html
