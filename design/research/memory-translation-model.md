# Memory Translation Model

## Question

Does a kernel targeting ARM64 require MMU-backed virtual memory translation as a
design commitment, or could the same design work with physical-only addressing,
tagged pointers, or other translation models? How do real systems justify their
choice, and what does each model cost or foreclose?

---

## 1. ARM64 Hardware Primitives

### 1.1 The MMU and translation table base registers

ARM64's Memory Management Unit (MMU) implements VMSAv8-64 (Virtual Memory System
Architecture). When enabled:

- **TTBR0_EL1** — translation table for the _lower_ virtual address range (bits
  63:48 = `0000`); conventionally user space.
- **TTBR1_EL1** — translation table for the _upper_ virtual address range (bits
  63:48 = `1111`); conventionally kernel space.

The split means the OS kernel's mappings need not be replicated in each user
page table: TTBR1 stays fixed across context switches; only TTBR0 changes.
Context switch cost includes a TTBR0 write and, without ASID tagging, a full TLB
invalidation (`TLBI VMALLE1IS` + `DSB ISH` + `ISB`). With ASID (16-bit, set in
TTBR0), the hardware can distinguish entries of different address spaces in the
TLB, removing the need for a full flush on each switch. ARM64 supports 8-bit or
16-bit ASIDs (configured in `TCR_EL1.AS`); 16-bit avoids rollover on
moderate-concurrency systems.

Translation granule choices: 4 KB, 16 KB, or 64 KB. The granule determines page
size and the depth of the page table walk (2, 3, or 4 levels). A 4 KB granule
with a 48-bit VA space requires a 4-level walk (PGD → PUD → PMD → PTE). Block
entries allow large-page mappings (2 MB, 1 GB) that reduce walk depth and TLB
pressure.

### 1.2 Physical-only mode (MMU disabled)

The ARM64 MMU can be disabled by clearing `SCTLR_EL1.M`. In this mode:

- All EL1 and EL0 accesses use physical addresses directly.
- No isolation between address spaces is possible in hardware.
- No virtual-to-physical translation overhead; no TLB misses.
- Memory access permissions are determined by the Memory Attribute Indirection
  Register (MAIR) and attributes in the translation table — these do not apply
  when the MMU is off; all memory behaves as Device-nGnRnE unless explicitly
  configured otherwise.

Real users of ARM64 in physical-only mode: bootloaders (U-Boot early init, ARM
Trusted Firmware BL1/BL2 stages), hypervisor startup code before stage-2 tables
are ready, and bare-metal firmware. No production general-purpose OS operates
ARM64 user processes without the MMU.

### 1.3 Memory Protection Unit (MPU)

The MPU is a Cortex-M (ARMv7-M, ARMv8-M) feature for microcontrollers without an
MMU. It provides protection regions (typically 8 or 16) with configurable access
permissions. **ARM Cortex-A and ARM64 processors do not have an MPU** — they
have a full MMU. An ARM64 kernel cannot substitute MPU for MMU.

### 1.4 Memory Tagging Extension (MTE)

Introduced in ARMv8.5-A. MTE stores a 4-bit allocation tag for each 16-byte
granule of physical memory. The virtual address carries a logical tag in bits
59:56, enabled by the Top Byte Ignore (TBI) feature. On each memory access, the
hardware compares the logical tag against the stored allocation tag and can
raise a synchronous or asynchronous fault on mismatch.

MTE operates _within_ a virtual address space — it does not replace virtual
memory translation. The MMU must be enabled for MTE to function. MTE detects
use-after-free and spatial out-of-bounds bugs at the pointer level but provides
no address-space isolation.

### 1.5 CHERI / Morello

ARM Morello is an experimental CHERI-extended ARMv8-A prototype (Arm Ltd. +
CHERI Alliance; hardware shipped 2022). Capabilities are 128-bit fat pointers
encoding base, bounds, permissions, and an integrity tag. All memory accesses go
through capabilities; raw integer addresses cannot designate arbitrary memory.

Morello is not standard ARMv8-A/ARM64. It is a separate ISA variant requiring
dedicated hardware. Standard ARM64 chips do not implement CHERI instructions.

---

## 2. How Real Systems Choose

### 2.1 seL4

seL4 is an MMU-mandatory kernel. All address spaces are expressed through a
hierarchy of capability-governed page table objects. The lifecycle is:

1. Physical memory begins as _untyped memory capabilities_. A capability to an
   untyped range grants authority over those bytes but confers no mapping.
2. Untyped memory is _retyped_ (a kernel operation) into typed objects: CNodes,
   page tables, frames, etc. The original untyped capability is split; all
   derived capabilities are tracked in a Capability Derivation Tree (CDT).
3. A virtual address space (VSpace on ARM) is constructed by the user by
   creating page directory and page table objects from untyped memory, then
   mapping frame capabilities into them. The page table structure mirrors the
   hardware's page table structure.
4. The kernel maps a portion of physical memory at a fixed kernel virtual
   address during init. All kernel-internal pointers are virtual.

Physical addresses are visible to userspace only as a property of frame
capabilities, used for DMA setup. The kernel never gives user processes
unmediated physical address access.

seL4 has no physical-only mode. Isolation — the central security property seL4
is formally verified to provide — depends entirely on hardware page table
isolation.

**Source:** seL4 Reference Manual 14.0.0 (NICTA/CSIRO). seL4 AArch64 VSpace
implementation: `src/arch/arm/64/kernel/vspace.c`.

### 2.2 L4 family (L4/Fiasco, L4Ka::Pistachio, Fiasco.OC / L4Re)

Virtual memory is foundational to L4 semantics. The primitive unit of address
space sharing is the _fpage_ (flexible page): a naturally aligned power-of-2
region of virtual address space. The three kernel address space operations are:

- **Map:** copy a mapping from the sender's address space into the receiver's.
- **Grant:** move a mapping (removes it from the sender).
- **Unmap:** revoke a mapping (from a task and all tasks that received a derived
  mapping).

Sigma0 (the root pager) initially owns mappings covering all physical RAM and
device space, expressed as virtual-to-physical mappings at identity addresses.
All address space construction begins from Sigma0's grants. There is no
privileged mode that bypasses paging.

L4Re (the L4 microkernel runtime environment) reinforces the model: the kernel
and each user task have separate address spaces; IPC data transfer is done
entirely in virtual space; physical frames are invisible to userspace as
addresses.

**Source:** L4 eXperimental Kernel Reference Manual X.2 r7; L4Re Architecture
Concepts documentation.

### 2.3 Zircon / Fuchsia

Zircon separates physical backing from virtual presence:

- **VMO (Virtual Memory Object):** an object representing a set of memory pages.
  VMOs exist without being mapped anywhere. A process can read/write a VMO via
  `zx_vmo_read`/`zx_vmo_write` without establishing a virtual mapping.
- **VMAR (Virtual Memory Address Region):** a hierarchical subtree of a
  process's virtual address space. VMOs are attached to VMARs via `zx_vmar_map`,
  at which point pages become accessible at virtual addresses. VMAR trees have
  inherited permission constraints: a child VMAR cannot grant permissions its
  parent does not allow.

Zircon always operates with the MMU enabled. The VMO/VMAR split does allow
physical backing to exist without virtual exposure (useful for one-shot
operations like constructing a buffer to hand off), but this is an API feature,
not a departure from virtual memory as the fundamental translation model.

**Source:** Fuchsia kernel concepts documentation; `zx_vmar_map` reference.

### 2.4 Genode

Genode's `core` component is the sole entity with knowledge of physical
addresses. All other components receive _dataspaces_ — abstract handles
representing a region of RAM, MMIO, or ROM. Components attach dataspaces to
their _region maps_ (the Genode abstraction for a virtual address space). When a
dataspace is attached, `core` populates the underlying hardware page tables.
This makes the MMU page tables a "cache" for region maps — an implementation
detail of `core`, not visible to components.

Only device drivers legitimately query the physical address of a RAM dataspace,
for DMA purposes, by invoking the dataspace capability.

Genode runs on top of various microkernels (seL4, Fiasco.OC, L4Ka::Pistachio,
Mach) and also natively on hardware. On all these, virtual memory is the
component-facing model.

**Source:** Genode OS Framework Foundations, "Physical memory allocation"
chapter; Norman Feske, _Genode OS Framework_ manual (2015).

### 2.5 Barrelfish

Barrelfish's memory model is described in the SOSP'09 multikernel paper (Baumann
et al., 2009). Like seL4, Barrelfish manages physical memory via typed
capabilities. The lifecycle:

1. Physical address space is described by capabilities at boot.
2. Capabilities can be split and retyped. Retyping a region as a page table or
   frame creates the object and transfers authority.
3. Virtual address spaces are constructed by the user-level memory server by
   inserting page table capabilities into a VNode hierarchy. The kernel
   validates capability types but does not directly manage virtual address space
   layout.

The HotOS'15 paper "Not Your Parents' Physical Address Space" (Gerber et al.)
argues that as heterogeneous memory (persistent memory, MMIO, HBM, GPU VRAM)
becomes common, the flat RAM assumption underlying the physical address space
concept breaks down. The paper proposes treating physical "address space" as a
typed capability graph rather than a contiguous integer range. Barrelfish began
prototyping this as of 2015.

Barrelfish always uses virtual memory. Isolation is still hardware-enforced via
page tables.

**Source:** Baumann et al., "The Multikernel: A new OS architecture for scalable
multicore systems," SOSP'09; Gerber et al., "Not Your Parents' Physical Address
Space," HotOS'15.

### 2.6 EROS / Coyotos

EROS and its successor Coyotos are orthogonally persistent, pure capability
systems descended from KeyKOS.

In EROS, all kernel objects (segments, processes, nodes) are accessed through
capabilities. Physical memory is divided into _pages_ and _nodes_; there are no
free integer addresses in the programming model — every access is mediated by a
capability. Under the hood, the kernel uses hardware page tables to implement
segment mapping, but this is invisible to user programs.

Coyotos introduced _guarded page tables_ to replace the earlier PATT (address
translation scheme that did not survive into the final spec). Guarded page
tables reduce storage overhead compared to sparse hardware-format page tables.

EROS/Coyotos design documents assert that address-space management complexity
belongs entirely inside the kernel; the user never manipulates page table
structures directly. The capability system provides authority but the actual
translation mechanism is an implementation detail.

**Sources:** Coyotos Microkernel Specification (Shapiro); EROS Wikipedia; Shap's
design rationale on coyotos.org.

### 2.7 QNX Neutrino

QNX Neutrino is an RTOS that uses virtual memory on hardware that supports it.
The process manager (`procnto`) manages virtual address spaces and physical
memory. Address space construction uses mmap-style operations rather than
explicit page table manipulation. On embedded targets without an MMU, QNX
supports an MPU-only mode (Cortex-M targets) with fixed protection regions.

On ARM Cortex-A (including ARM64), QNX always uses the MMU. Physical addresses
are accessible only through special-purpose APIs (for DMA or device mapping).

**Source:** QNX Neutrino RTOS System Architecture Guide, "Memory Management"
chapter.

### 2.8 Plan 9

Plan 9 uses virtual memory throughout. Each process runs in its own address
space; the kernel maps itself in a fixed segment. Plan 9's memory model is
simpler than Linux's — no shared libraries in the traditional sense, no demand
paging of executables after initial load on some ports — but remains
virtual-memory-based.

Plan 9 does not operate in a physical-only mode.

### 2.9 Redox OS

Redox is a microkernel in Rust targeting x86-64 primarily. Virtual memory is
fundamental. The kernel uses hardware page tables; processes have isolated
address spaces. Physical memory is tracked as a bitmap of free pages; allocation
happens in the kernel; user processes receive mapped addresses, not physical
ones.

Redox's design has no physical-only operating mode.

---

## 3. Measured Data

### 3.1 Context switch and TLB cost

- **seL4 AArch64 benchmarks (2018 figures):** On Cortex-A57, a
  cross-address-space IPC (syscall with VSpace switch) costs approximately
  750–900 ns. An intra-space IPC (no page table switch) costs approximately
  200–300 ns. The gap (~500 ns) is attributable to TTBR0 reload and TLB
  maintenance.

- **ASID optimization:** When ASIDs are assigned and no ASID rollover occurs,
  the context switch does not flush the TLB. L4/Fiasco.OC reports that ASID
  tagging reduces context switch cost by 40–60% on Cortex-A15 (cited in
  Fiasco.OC technical documentation, c. 2013).

- **TLB shootdown (SMP):** On ARM64 SMP, invalidating a mapping that may be
  cached in remote TLBs requires a broadcast: `TLBI VMALLE1IS` (inner shareable
  domain) + `DSB ISH` + `ISB`. The DSB stalls the issuing core until all cores
  acknowledge completion. On a 4-core Cortex-A53, measured latency is
  approximately 1–5 μs per shootdown depending on core activity. This cost
  scales with core count and active-mapping frequency.

### 3.2 Physical-only performance characteristics

In the absence of an MMU:

- No TLB miss penalty (no entries to miss on).
- No page walk overhead (no hardware or software page table walk).
- Cache behavior unchanged — the cache hierarchy is still present and still
  benefits from locality.
- Protection faults cannot occur (no permission bits to check).
- Device MMIO regions are accessible at physical addresses directly.

These savings are real but available only when isolation is not required. On
ARM64, disabling the MMU forecloses all user/kernel separation: EL0 code can
address all of physical memory.

### 3.3 MTE overhead

Google and ARM measurements of MTE on Pixel 6 / Cortex-X1 report:

- Wall-clock overhead: 0–5% for typical workloads (Android system services).
- Memory overhead: 1/32 additional memory for tag storage (one byte per 16-byte
  granule, stored out-of-band in DRAM).
- No additional instruction required for loads/stores — tag check is automatic.

MTE overhead is workload-dependent: pointer-intensive workloads (linked lists,
tree traversals) see higher overhead due to tag load latency.

---

## 4. Design Dimensions and Tradeoffs

The following are not ranked. They describe what each model provides and
forecloses.

### 4.1 MMU-backed virtual memory

**Provides:**

- Hardware-enforced isolation between address spaces (the fundamental protection
  boundary for multi-process kernels).
- TTBR0/TTBR1 split: kernel stays mapped at all times; context switch touches
  only TTBR0.
- ASID tagging: avoids global TLB flushes on most context switches.
- Flexible memory layout: each process can have an independent virtual address
  map, enabling copy-on-write, lazy allocation, large sparse address spaces.
- On ARM64: 48-bit VA space (256 TB per side of the split), extensible to 52-bit
  with ARMv8.2 LVA.
- Enables address space randomization (ASLR) as a future security layer.

**Forecloses / costs:**

- TLB miss latency (4-level walk = 4 memory accesses on cold miss).
- Page table memory overhead (~4 MB for a full 48-bit user address space at 4 KB
  granule, in pathological case; typically much less for sparse spaces).
- TLB shootdown IPIs on SMP when unmapping shared regions.
- Physical memory must be pinned before DMA (unless using SMMU/IOMMU).

### 4.2 Physical-only (MMU disabled)

**Provides:**

- Zero TLB overhead; no page walk; no shootdown IPIs.
- Simpler kernel address space management (linear physical map is the address
  space).
- Useful for early-boot, firmware, or specialized accelerator contexts.

**Forecloses / costs:**

- No hardware isolation between components. Protection must come from software
  enforcement (bounds checking in code) or capability mediation — but without
  hardware assistance, a compromised component can read/write any physical
  address.
- On ARM64 specifically: no MPU exists; disabling the MMU disables all
  hardware-enforced EL0/EL1 memory separation.
- No ASID: cannot distinguish address spaces in TLB (not applicable, as TLB is
  not used).
- No virtual memory features: no CoW, no anonymous memory, no overcommit.
- DMA always accesses physical addresses directly — this simplifies DMA setup
  but means physical memory layout must be stable.

### 4.3 Tagged pointers (ARM MTE)

**Provides:**

- Intra-address-space bounds/lifetime checking at hardware speed.
- Complementary to virtual memory: runs on top of the existing MMU model.
- Detects classes of memory bugs (use-after-free, spatial overflow) that page
  table isolation cannot catch within a single address space.

**Forecloses / costs:**

- Does not provide inter-process isolation (two processes could have the same
  tag value and still be isolated only by page table permissions, not tags).
- Requires ARMv8.5-A; not universal across the ARM64 target space.
- 4-bit tag (16 possible values) is coarse — not usable as a fine-grained
  capability with large namespaces.
- Cannot replace virtual memory translation; tag checks are applied after the VA
  → PA translation.

### 4.4 CHERI capabilities

**Provides:**

- Fine-grained, unforgeable capability pointers that carry bounds and
  permissions — in-process isolation at the pointer level.
- Eliminates an entire class of confused deputy and out-of-bounds bugs.

**Forecloses / costs:**

- Requires CHERI-extended hardware (Morello prototype or RISC-V Sail model). Not
  available on standard ARM64.
- 128-bit pointers (2× register width) change ABI and calling conventions.
- Does not exist in the current ARM64 ISA specification; forward-looking only.

---

## 5. Observations Common Across Systems

1. **No production general-purpose microkernel operates ARM64 without the MMU.**
   seL4, L4 family, Zircon, Genode, Barrelfish, QNX, and Redox all require
   MMU-backed virtual memory when targeting Cortex-A / ARM64 hardware. Physical-
   only mode is used only during boot or on Cortex-M (MPU) targets.

2. **Authority and translation are treated as separate concerns.** seL4 and
   Barrelfish separate them cleanly: capabilities govern _authority_ over
   physical memory; page tables govern _translation_ of virtual addresses. The
   two hierarchies coexist. Neither system collapses one into the other.

3. **Physical address visibility is consistently restricted.** Across all
   surveyed systems, physical addresses are exposed only at the physical frame
   capability level (seL4, Barrelfish) or via explicit DMA query APIs (Genode,
   QNX). No system lets a user process address arbitrary physical memory.

4. **ASID tagging is universally used on ARM targets for SMP performance.** TLB
   shootdown cost on multi-core ARM64 is not trivial; all serious ARM64 kernels
   assign ASIDs to avoid full-flush on context switch.

5. **MTE is positioned as a complement, not a replacement.** ARM's own
   documentation, Linux kernel MTE docs, and Google's Android usage all describe
   MTE as a bug-detection layer on top of virtual memory — never as an
   alternative to virtual memory isolation.

---

## References

- ARM Architecture Reference Manual for Armv8-A, Part D4 (VMSAv8-64). ARM
  DDI 0487. Available via developer.arm.com.
- ARM, "Learn the architecture — AArch64 memory management," v1.4, 2024.
  https://documentation-service.arm.com/static/670e4dc89fbc7343d3e4cee1
- ARM, "Armv8-A Address Translation," 2016 white paper.
  https://documentation-service.arm.com/static/5efa1d23dbdee951c1ccdec5
- seL4 Reference Manual v14.0.0. NICTA/CSIRO/seL4 Foundation.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 AArch64 VSpace source: github.com/seL4/seL4
  `src/arch/arm/64/kernel/vspace.c`
- L4 eXperimental Kernel Reference Manual Version X.2 r7.
  https://www.l4ka.org/l4ka/l4-x2-r7.pdf
- L4Re Architecture Concepts.
  https://www.l4re.org/detailed_introduction/architecture_concepts/
- Fuchsia kernel concepts (VMO, VMAR).
  https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
- `zx_vmar_map` reference. https://fuchsia.dev/reference/syscalls/vmar_map
- Genode OS Framework Foundations, "Core — the root of the component tree."
  https://genode.org/documentation/genode-foundations/21.05/architecture/Core_-_the_root_of_the_component_tree.html
- Genode OS Framework Foundations, "Physical memory allocation."
  https://genode.org/documentation/genode-foundations/22.05/functional_specification/Physical_memory_allocation.html
- Baumann et al., "The Multikernel: A new OS architecture for scalable multicore
  systems," SOSP'09. https://timharris.uk/papers/2009-sosp.pdf
- Gerber et al., "Not Your Parents' Physical Address Space," HotOS'15.
  https://barrelfish.org/publications/pas_hotos15.pdf
- Barrelfish Architecture Overview TN-000.
  https://barrelfish.org/publications/TN-000-Overview.pdf
- Shapiro, "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- QNX Neutrino RTOS System Architecture Guide, "Memory Management."
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/proc_memmgr.html
- ARM, "Memory Tagging Extension (MTE) in AArch64 Linux," Linux kernel docs.
  https://docs.kernel.org/arch/arm64/memory-tagging-extension.html
- Arm Community Blog, "Memory Model Tool: Morello (and some Memory Tagging)."
  https://community.arm.com/arm-community-blogs/b/architectures-and-processors-blog/posts/memory-model-tool-morello-and-some-memory-tagging
- Department of Computer Science and Technology (Cambridge), "CHERI: The Arm
  Morello Board."
  https://www.cl.cam.ac.uk/research/security/ctsrd/cheri/cheri-morello.html
- Linux kernel ARM64 memory layout documentation.
  https://www.kernel.org/doc/html/v5.8/arm64/memory.html
