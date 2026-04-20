# Page Size Exposure in the Memory Object Interface

## Question

Should the kernel's memory object interface be page-addressed — requiring
userspace to specify sizes and alignments at hardware page granularity — or
byte-addressed, with the kernel absorbing page alignment internally and hiding
the hardware granule from the interface?

This question is about the external interface to memory objects (creation,
sizing, and mapping), not about internal kernel implementation. Page-granular
accounting can coexist with byte-addressed creation. The question is what the
userspace-visible interface commits to.

---

## 1. Context: Why This Question Matters

ARM64 supports three hardware granule sizes — 4 KB, 16 KB, and 64 KB — each
selectable at boot via `TCR_EL1.TG0`/`TG1`. They are not interchangeable across
compile-time kernels without breaking userspace that assumes a fixed
`PAGE_SIZE`. The contiguous-PTE hint extends this further: hardware can
efficiently map 4K×16 = 64 KB "large pages" using the 4 KB granule, and 64K×16 =
1 MB using the 64 KB granule — an orthogonal page-size tier atop the base
granule.

Any interface that exposes page size becomes an ABI commitment to a specific
granule. Any interface that hides page size transfers alignment, padding, and
tail-waste accounting to the kernel.

---

## 2. Survey of Existing Systems

### 2.1 seL4 — Fully Page-Addressed

seL4 exposes page size through two mechanisms.

**Frame capability sizes.** `seL4_Untyped_Retype` creates Frame objects sized in
bit-width (`size_bits`): a Frame of `size_bits = 12` is a 4 KB page. On AArch64,
Frame types are explicitly enumerated in the kernel ABI:
`seL4_ARM_SmallPageObject` (4 KB), `seL4_ARM_LargePageObject` (2 MB),
`seL4_ARM_HugePageObject` (1 GB). Each maps to a specific hardware block/page
level in the page table hierarchy.

**Page table retype.** Intermediate page table objects (PageTable,
PageDirectory, PageUpperDirectory) are created at page granularity and
explicitly placed by userspace. The user constructs the page table hierarchy by
retyping Untypeds into page table nodes and then mapping Frame capabilities into
them. This is inherently page-addressed.

There is no `seL4_hide_page_size()` operation. Page size is a first-class
parameter. The seL4 ABI changes between kernels built with different `PAGE_SIZE`
constants — frame types and intermediate table levels change.

**Source:** seL4 Reference Manual 14.0.0; seL4 AArch64 VSpace RFC
(sel4.github.io/rfcs/implemented/0100-refactor-aarch64-vspace).

---

### 2.2 Zircon / Fuchsia — Byte-Addressed Creation, Page-Addressed Mapping

Zircon separates the concepts of _allocated capacity_ (page-granular) and
_stream size_ (byte-granular), a distinction formalized in RFC-0238 (2024).

**Pre-RFC-0238 behavior.** `zx_vmo_create(size)` always rounded up the size to
the next system page boundary (`zx_system_get_page_size()`). The rounded-up size
was the _only_ size the kernel tracked; the byte-level intent was lost.

**Post-RFC-0238.** Two independent sizes coexist on a VMO:

| Property              | Granularity   | Syscall                                             |
| --------------------- | ------------- | --------------------------------------------------- |
| VMO size (capacity)   | Page-rounded  | `zx_vmo_set_size` / `zx_vmo_get_size`               |
| Stream size (content) | Byte-granular | `zx_vmo_set_stream_size` / `zx_vmo_get_stream_size` |

The stream size is initialized to the unrounded argument passed to
`zx_vmo_create`. `zx_vmo_get_size` returns the rounded-up size. The kernel
manages the gap between stream_size and VMO_size as inaccessible ("fault beyond
stream size") rather than zero-filled padding visible to users.

**VMAR mapping remains page-addressed.** `zx_vmar_map` requires the `vmo_offset`
and `len` arguments to be page-aligned. A VMO with a byte-granular stream size
must still be mapped at page boundaries. Page size is exposed explicitly via
`zx_system_get_page_size()` — a documented, stable syscall.

**Summary.** Zircon implements a two-layer model: byte-addressed at the content
layer, page-addressed at the mapping layer. Page size is not hidden — it is
explicitly queryable and required for mapping operations.

**Source:** RFC-0238: VMO Size (fuchsia.dev); `zx_vmo_create` reference
(fuchsia.dev); `zx_system_get_page_size` reference (fuchsia.dev).

---

### 2.3 Genode — Byte-Specified Allocation, Page-Observable Backing

Genode's `Ram_allocator::alloc(size_t size)` accepts a byte-count argument.
Internally, the dataspace is backed by a whole number of pages; the backing size
is rounded up to page granularity. The dataspace capability returned to the
caller is opaque — it names a region without exposing physical addresses.

However, page granularity is observable:

- The Genode documentation states: "its base address and size are subjected to
  the granularity of physical pages as dictated by the MMU (typically 4 KiB)."
- Attaching a dataspace to a region map places it at a page-aligned virtual
  address.
- The `Dataspace` interface exposes `size()`, which returns the page-rounded
  size.

For MMIO dataspaces (via the `io_mem` service), the physical base address can be
specified at sub-page granularity; the kernel maps a page-aligned region
covering the requested physical address, and the precise offset within the page
is left to the driver to compute. This reveals the page structure to drivers.

**Summary.** Genode is the closest surveyed system to a byte-addressed
interface: `alloc(size)` accepts byte counts. But page size is visible through
the size() return value, the alignment of region-map attachments, and the io_mem
sub-page offset convention.

**Source:** Genode Foundations (Feske); Genode Physical Memory Allocation
documentation (genode.org); `Ram_allocator` C++ interface (genode source).

---

### 2.4 EROS / KeyKOS / Coyotos — Single-Page Atomic Unit

These systems make the hardware page the explicit atomic memory capability.

- **KeyKOS/EROS:** A page key designates exactly one hardware page. There is no
  capability type for a byte-ranged sub-region of a page or a multi-page range
  (beyond range keys held by the Space Bank). Page size is the fundamental unit
  of all memory management.
- **Coyotos:** The `Page` capability designates one hardware page. `CapPage`
  designates one hardware page used as capability storage. The GPT (Generalized
  Page Table) is a vector of 16 capability slots, also page-aligned. Nothing is
  byte-addressed.

Page size is not merely visible — it is the defining granularity of the entire
object model. An allocation smaller than one page requires consuming one full
page capability from the Space Bank. Tail waste is explicit and permanent.

**Source:** "EROS: A Fast Capability System" (Shapiro et al., SOSP 1999);
Coyotos Microkernel Specification (Shapiro).

---

### 2.5 L4 Family — fpages Are Inherently Page-Granular

L4's primitive sharing unit is the **fpage** (flexible page): a naturally
aligned, power-of-2-sized range of virtual address space. The minimum fpage size
is one hardware page. All mapping, granting, and unmapping operations take
fpages as parameters.

There is no byte-addressed memory object in L4. The fpage encoding includes a
size field (log2 of bytes, minimum 12 for 4 KB) and a base address
(page-aligned). The page size is encoded into every memory operation.

**Source:** L4 eXperimental Kernel Reference Manual X.2 r7.

---

### 2.6 Barrelfish — Page Table Frames Exposed as VNode Capabilities

Barrelfish gives userspace explicit capabilities to page table frames at each
hardware level (PGD, PUD, PMD, PTE). Building an address space requires the
user-level memory manager to retype RAM capabilities into VNode capabilities at
each level, then map Frame capabilities into the leaf VNodes. This is fully
page-granular; the user explicitly assembles the hardware page table tree.

Page size is implicit in the VNode type hierarchy. A PTE-level VNode (leaf page
table frame) maps Frame capabilities of hardware-page size. There is no
abstraction hiding the page level.

**Source:** Barrelfish Architecture Overview (TN-000); Gerber, "Virtual Memory
in a Multikernel — The Barrelfish OS," ETH Zurich master's thesis.

---

### 2.7 QNX Neutrino — POSIX-Compliant, Page Size Exposed

QNX Neutrino is POSIX-compliant. Page size is exposed via `getpagesize()` and
`sysconf(_SC_PAGE_SIZE)`. The `mmap()` system call requires page-aligned
`offset` and `len` arguments when `MAP_FIXED` is used. The `typed_mem` POSIX
extension for deterministic physical memory allocation also operates at page
granularity.

Documentation notes: "The MMU divides physical memory into fixed-size pieces
called pages that are usually (but not always) 4 KB." QNX exposes the actual
page size rather than fixing it at 4 KB, making it a portable POSIX approach.

**Source:** QNX Neutrino RTOS System Architecture Guide — Memory Management;
`getpagesize()` reference (qnx.com); `mmap()` reference (qnx.com).

---

### 2.8 Windows NT — Two-Tier Granularity Without Hiding Either

Windows NT exposes a two-tier memory granularity model, documented and distinct:

| Level                  | Size  | Mechanism                                  |
| ---------------------- | ----- | ------------------------------------------ |
| Page size              | 4 KB  | commit granularity for `VirtualAlloc`      |
| Allocation granularity | 64 KB | reservation granularity for `VirtualAlloc` |

`VirtualAlloc` reserves address space in 64 KB multiples and commits physical
pages in 4 KB multiples. Both tiers are exposed to userspace:
`SYSTEM_INFO.dwPageSize` returns 4096 and `SYSTEM_INFO.dwAllocationGranularity`
returns 65536. A caller reserving 4 KB of address space occupies 64 KB of
virtual address range.

The 64 KB allocation granularity was motivated by RISC processor instruction
encoding: RISC instruction sets (Alpha AXP, MIPS, PowerPC) load 32-bit
immediates as two 16-bit halves. If DLL relocation could shift an image by
non-64KB amounts, both 16-bit halves would need separate fixup patches, doubling
relocation overhead. Aligning DLL bases at 64 KB boundaries allows the upper 16
bits to be patched once.

Neither granularity is hidden. Both are documented API constants. This creates
wasted address space between reservation and commit granularity — a known
programming complexity.

**Source:** Raymond Chen, "Why is address space allocation granularity 64KB?"
(devblogs.microsoft.com, 2003); `VirtualAlloc` reference (Microsoft Learn).

---

### 2.9 Linux ARM64 — PAGE_SIZE as ABI Commitment

Linux defines `PAGE_SIZE` as a compile-time constant. On ARM64, `PAGE_SIZE` is
set by kernel config: `CONFIG_ARM64_4K_PAGES`, `CONFIG_ARM64_16K_PAGES`, or
`CONFIG_ARM64_64K_PAGES`. Different `PAGE_SIZE` values produce incompatible
kernel ABIs:

- Userspace programs using `mmap()` with 4 KB-aligned addresses fail on 16 KB
  kernels (alignment requirement not met).
- 64-bit binaries that embed 4 KB page size assumptions (e.g., JVM startup code,
  allocators, ELF segment alignment) break on 64 KB kernels.
- Apple Silicon (M-series chips) uses 16 KB pages; running existing ARM64 Linux
  binaries required special compat handling.

A 57-patch series ("boot-time page size selection for ARM64," LWN 2024) attempts
to decouple `PAGE_SIZE` from compile time. The series introduces
`PAGE_SIZE_MIN`/ `PAGE_SIZE_MAX` bounds and replaces compile-time constants with
`__ro_after_init` variables throughout the kernel. The userspace ABI
compatibility problem remains unresolved by this patch set — it addresses the
kernel-internal constant, not the exposed ABI.

This is the clearest documented evidence of the cost of page-size exposure: a
major engineering effort is required to make the kernel's internal page size
variable even when userspace ABI is not yet addressed.

**Source:** "Boot-time page size selection for arm64" (LWN,
lwn.net/Articles/993990, 2024); Red Hat Enterprise Linux 10 documentation on 64K
page kernels; Ampere Computing tuning guide for ARM64 page sizes.

---

### 2.10 Plan 9 — Page Size Exposed, Address Space Simpler

Plan 9 uses `segattach`, `segfree`, `segflush` for memory-mapped segments rather
than POSIX `mmap`. Segments are page-aligned and page-sized; there is no
sub-page addressing. The system does not expose an explicit `getpagesize`
syscall (Plan 9 is not POSIX), but the segment interface assumes page alignment
throughout.

Plan 9's approach is simpler than POSIX because it does not try to unify file
mapping, device mapping, and anonymous memory under a single interface. But it
remains page-addressed, not byte-addressed.

**Source:** Pike et al., "Plan 9 from Bell Labs" (USENIX UKUUG 1990); Plan 9
`segment(2)` manual page.

---

## 3. Sub-Page Packing: The Implementation Tension

When multiple memory objects are smaller than one hardware page, packing them
into a shared physical page reduces fragmentation. All surveyed systems that
have byte-addressed allocation layers (Genode, the Zircon stream-size concept,
userspace allocators like jemalloc) perform sub-page packing _above_ the
page-addressed interface — never inside the kernel's mapping layer.

The reason is structural: if two distinct objects share one physical page and
the kernel manages mappings at page granularity, unmapping object A (which the
kernel must do when object A's last capability is revoked) may need to keep that
page mapped for object B. This requires the kernel to track inter-object sharing
within pages, adding:

1. A per-page reference count beyond capability reference counting.
2. Logic to defer unmap until all objects sharing the page are revoked.
3. Handling the case where objects A and B belong to different processes but
   share a page — a security boundary management problem.

seL4 avoids this entirely: Frame = hardware page; no sharing possible. Zircon
avoids it by rounding VMO size to a full page; tail bytes within the page are
inaccessible but owned by the VMO. Genode similarly rounds up; the tail is
internal to core.

No surveyed kernel system performs sub-page packing in the kernel's mapping
layer. Sub-page packing is uniformly left to userspace allocators operating
above page-addressed interfaces.

---

## 4. Measured Data

**seL4 frame retype cost.** Creating a Frame from an Untyped
(`seL4_Untyped_Retype`) on AArch64 takes approximately 400–800 cycles (published
seL4 benchmarks, Cortex-A series). This is a cold-path cost; steady-state
operation does not invoke retype.

**Linux ARM64 boot-time page size patch series size.** The 57-patch series
required to decouple PAGE_SIZE from compile-time across the Linux kernel
(without yet fixing userspace ABI) affects filesystems, memory management,
drivers, and architecture-specific code. Microbenchmark overhead of the
resulting runtime-variable `PAGE_SIZE`: up to 12% on pointer-intensive paths,
attributed to instruction alignment rather than algorithmic cost. Real-workload
overhead: ~1%.

**Zircon VMAR map cost.** No published cycle count found for `zx_vmar_map`
alone. The operation is O(log n) in VMAR tree size. The rounding of VMO size to
page boundary is a free operation (bitwise alignment mask).

**Windows NT granularity waste.** Reserving a single 4 KB page in a process
consumes 64 KB of virtual address space (16× waste in virtual range). On a
32-bit system with a 2 GB user address space, this limits independent
reservations to ~32,000 before address space exhaustion, regardless of physical
memory availability.

**ARM64 granule choices.** 4 KB granule: 4-level walk (PGD/PUD/PMD/PTE), 4096 KB
range per PTE. 16 KB granule: 2-level walk, covers 32 KB per PTE entry at
level 3. 64 KB granule: 3-level walk. Contiguous-bit hint: hardware can treat
16×4KB PTEs as a single 64KB entry in the TLB, reducing TLB pressure for large
objects without changing the granule.

---

## 5. Tradeoffs

The following are presented without ranking.

**Interface commits to hardware granule.**

Page-addressed interfaces (seL4, L4, EROS/Coyotos, L4, QNX, Plan 9) make page
size an explicit ABI element. Userspace code must know the page size to call the
interface correctly. Changing the hardware granule (or supporting multiple
granules) requires either breaking the ABI or providing per-granule object
variants. The Linux ARM64 migration problem demonstrates the engineering cost of
this commitment.

Byte-addressed creation with page-addressed mapping (Zircon post-RFC-0238,
Genode) partially decouples creation from granule but leaves mapping exposed.
The kernel must internally round-up byte sizes; the rounding is observable
through size-query syscalls.

**Tail-waste visibility and accountability.**

Page-addressed interfaces make tail waste explicit: a 5 KB allocation requires a
4 KB + 4 KB = 8 KB allocation; the 3 KB tail is clearly unused. This makes
memory accounting transparent to the allocator.

Byte-addressed interfaces hide tail waste inside the kernel. An allocator
requesting 5 KB may receive 8 KB of backing but only observe 5 KB charged to its
budget (if the kernel charges byte-granular) or 8 KB charged (if the kernel
charges page-granular). Which policy the kernel applies determines whether the
accounting matches the physical reality.

**Sub-page packing and revocation complexity.**

A kernel that accepts byte-addressed allocations and packs multiple objects into
shared pages must solve the shared-page revocation problem: revoking one object
cannot unmap the shared page until all objects sharing it are revoked. This
requires per-page reference counts plus per-object offset tracking — not present
in any surveyed kernel.

If the kernel avoids sub-page packing (allocates full pages per object, tail
waste is internal), the byte-addressed creation interface is a thin wrapper over
page-addressed allocation, and the primary benefit is syntactic (callers need
not know the page size) rather than structural.

**Interface forward-compatibility (page size changes).**

A byte-addressed interface that never exposes page size makes it possible, in
principle, to change the hardware granule without breaking the userspace ABI. No
surveyed kernel has achieved this in practice — all systems that have attempted
hardware granule changes have found that page size leaks through the mapping
layer even when the creation interface is byte-addressed.

The specific leak points are: (a) virtual address alignment requirements for
mapping operations, (b) contiguous mapping requires power-of-2 aligned ranges,
(c) DMA alignment requirements exposed through the device memory API.

**Capability proliferation.**

Page-addressed capability systems (seL4, EROS, Barrelfish) face a proliferation
problem: a 1 MB allocation at 4 KB granule requires 256 Frame capabilities. Each
capability occupies space in the capability table (seL4: one CSlot per
capability; Barrelfish: one capability per page frame). Variable-size object
models (one capability per arbitrary allocation) avoid this regardless of
whether the capability interface is byte-addressed or page-addressed.

**Kernel complexity from hiding page size.**

Sub-page packing and page-size abstraction within the kernel increase the
kernel's internal complexity: the kernel must maintain a sub-page allocator,
track object-to-page-offset relationships, handle partial-page revocation, and
ensure that different objects in the same page do not observe each other's
contents after revocation. These are correctness-critical operations that expand
the kernel's trusted computing base. Systems that push this complexity to
userspace allocators (above a page-addressed kernel interface) localize the
risk.

---

## 6. References

- seL4 Reference Manual 14.0.0. NICTA/CSIRO/seL4 Foundation.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 AArch64 VSpace RFC. seL4 Foundation.
  https://sel4.github.io/rfcs/implemented/0100-refactor-aarch64-vspace.html
- RFC-0238: VMO Size. Fuchsia/Google.
  https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0238_vmo_size
- `zx_vmo_create` reference. Fuchsia.dev.
  https://fuchsia.dev/reference/syscalls/vmo_create
- `zx_system_get_page_size` reference. Fuchsia.dev.
  https://fuchsia.dev/reference/syscalls/system_get_page_size
- Feske, N. Genode OS Framework Foundations. Genode Labs.
  https://genode.org/documentation/genode-foundations/
- Genode Physical Memory Allocation.
  https://genode.org/documentation/genode-foundations/22.05/functional_specification/Physical_memory_allocation.html
- Shapiro, J. et al. "EROS: A Fast Capability System." SOSP 1999.
  https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf
- Shapiro, J. "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- L4 eXperimental Kernel Reference Manual X.2 r7.
  https://www.l4ka.org/l4ka/l4-x2-r7.pdf
- Baumann, A. et al. "The Multikernel." SOSP 2009.
  https://timharris.uk/papers/2009-sosp.pdf
- Gerber, R. "Virtual Memory in a Multikernel — The Barrelfish OS." ETH Zurich.
  https://barrelfish.org/publications/gerber-master-vm.pdf
- QNX Neutrino RTOS System Architecture Guide — Memory Management.
  https://www.qnx.com/developers/docs/7.1/com.qnx.doc.neutrino.sys_arch/topic/proc_memmgr.html
- QNX getpagesize() reference.
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.lib_ref/topic/g/getpagesize.html
- Chen, R. "Why is address space allocation granularity 64KB?" Microsoft
  DevBlogs, 2003.
  https://devblogs.microsoft.com/oldnewthing/20031008-00/?p=42223
- "Boot-time page size selection for arm64." LWN.net, 2024.
  https://lwn.net/Articles/993990/
- Red Hat Enterprise Linux 10 — The 64k page size kernel.
  https://docs.redhat.com/en/documentation/red_hat_enterprise_linux/10/html/managing_monitoring_and_updating_the_kernel/what-is-kernel-64k
- Ampere Computing. "Understanding Memory Page Sizes on Arm64."
  https://amperecomputing.com/tuning-guides/understanding-memory-page-sizes-on-arm64
- ARM Architecture Reference Manual Armv8-A, Part D4 (VMSAv8-64).
  https://developer.arm.com/documentation/ddi0487
- Pike, R. et al. "Plan 9 from Bell Labs." USENIX UKUUG Summer Conference, 1990.
