# Ownership-Transfer IPC: Mechanisms, Message Format, and Page Size Coupling

## Question

Can Rust's ownership semantics be extended across process boundaries to enforce
zero-copy message passing? If so, what does this commit constrain —
specifically:

1. What kernel mechanisms does ownership-transfer IPC require?
2. How must the IPC message format be structured to support it?
3. How does the hardware page size couple into the mechanism?

This document surveys how deployed and research systems have answered these
questions. It is descriptive: it records what exists and what the tradeoffs are.

---

## Survey

### Mechanism taxonomy

Ownership-transfer IPC in practice takes four distinct forms:

| Mechanism                      | How ownership transfers                                                          | Kernel involvement                      |
| ------------------------------ | -------------------------------------------------------------------------------- | --------------------------------------- |
| **Page remapping**             | Sender's virtual mapping removed; physical pages re-mapped into receiver's space | Kernel manipulates page tables          |
| **Handle transfer**            | A kernel-object handle is moved from sender to receiver atomically               | Kernel updates handle table             |
| **Language-enforced move**     | Compiler's move semantics prevent sender from accessing value after send         | None (compiler + shared memory region)  |
| **Shared-memory + revocation** | Sender writes to shared heap, then explicitly revokes its own access             | Kernel or runtime manages access rights |

Real systems combine multiple mechanisms. The survey below identifies which each
system uses and what constraints follow.

---

### Mach / XNU — Out-of-Line (OOL) Memory

**Mechanism:** Page remapping (virtual copy) or physical copy, controlled per
message.

Mach messages can contain **OOL descriptors** (`mach_msg_ool_descriptor_t`),
which are separate from the inline message body. Each descriptor carries:

```text
address   : vm_address_t   — base address of the region in sender space
size      : mach_msg_size_t — byte count
deallocate: boolean         — if true, remove from sender's address space after send
copy      : enum            — MACH_MSG_VIRTUAL_COPY | MACH_MSG_PHYSICAL_COPY
type      : MACH_MSG_OOL_DESCRIPTOR
```

**Ownership transfer:** Setting `deallocate = true` removes the region from the
sender's address space after the message is sent. Combined with
`MACH_MSG_VIRTUAL_COPY`, the kernel re-maps the underlying physical pages into
the receiver's address space without duplicating them. The sender is
type-unsafely obligated not to access the region after send — there is no
compiler or kernel enforcement that the sender has no other live pointers into
that region.

**Message format constraint:** The message structure must separate a `body`
(containing descriptors) from the `inline` (word-sized) data in the message
trailer. A `descriptor_count` field in the message header signals how many typed
descriptors follow. Inline and OOL data occupy different parts of the message
layout and follow different kernel copy paths.

**Page size coupling:** OOL virtual-copy operates at page granularity. The
"copy" field in the descriptor reflects the sender's intent, but the kernel
reserves the right to perform a physical copy if a virtual-copy is impossible
(e.g., non-page-aligned region, pages already shared). Testing reported by dmcyk
shows that even with `MACH_MSG_VIRTUAL_COPY` and `deallocate = true`, the kernel
sometimes creates separate physical copies rather than sharing pages CoW — the
resulting share mode is `SM_PRIVATE` rather than `SM_COW`.

**Sources:**

- [XNU IPC — OOL Data (dmcyk)](https://dmcyk.xyz/post/xnu_ipc_iii_ool_data/)
- [XNU IPC — OOL VM (dmcyk)](https://dmcyk.xyz/post/xnu_ipc_iv_ool_vm/)
- [Apple Kernel Programming Guide — Mach](https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html)

---

### Zircon (Fuchsia) — VMO Handle Transfer

**Mechanism:** Handle transfer. Virtual Memory Objects (VMOs) are first-class
kernel objects. A VMO handle can be included in a channel message.

**Ownership transfer:** When a handle is written into a channel, it is
**atomically removed** from the sending process. When a message is read from a
channel, the handles are **atomically added** to the receiving process. The
kernel enforces this: there is no state where both sender and receiver hold the
same handle. Exclusive ownership is a kernel invariant, not a convention.

**Large data pattern:**

1. Sender allocates a VMO (with `zx_vmo_create`)
2. Sender maps it, writes data, unmaps it (or keeps it mapped — the map lifetime
   is independent of the handle)
3. Sender includes the VMO handle in a channel message
4. Receiver reads the message; handle is now in receiver's table
5. Receiver maps the VMO to access the data

No copy of the VMO contents occurs. The "transfer" is a kernel table update.

**Message format constraint:** A Zircon channel message has two distinct payload
segments: a byte array (max `ZX_CHANNEL_MAX_MSG_BYTES` = 65 536 bytes) and a
handle array (max `ZX_CHANNEL_MAX_MSG_HANDLES` = 64). These are structurally
parallel, not interleaved. The caller provides both arrays; handles are
validated, moved into the channel, then delivered. This clean separation is
intrinsic to the API:
`zx_channel_write(handle, options, bytes, num_bytes, handles, num_handles)`.

**Page size coupling:** VMO operations and mappings operate at page granularity.
A VMO's committed size is always a multiple of the page size. However, the page
size does not appear in the IPC message format itself — it is a constraint on
how the receiver uses the VMO after receipt. The Zircon kernel runs with a fixed
4KB page size on x86-64 and AArch64; the page size is not variable at runtime.

**Sources:**

- [Zircon fundamentals](https://fuchsia.dev/fuchsia-src/get-started/learn/intro/zircon)
- [Zircon Kernel Concepts](https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts)
- [Zircon Handles](https://fuchsia.dev/fuchsia-src/concepts/kernel/handles)
- [zx_channel_write_etc syscall](https://fuchsia.dev/reference/syscalls/channel_write_etc)

---

### seL4 — Capability Transfer, No Native OOL

**Mechanism:** No built-in page-remapping IPC. Ownership transfer is achieved by
transferring a **Frame capability** through the IPC message, not by transferring
page contents.

**IPC message format:** seL4 IPC has two payload regions:

| Region                   | Capacity                                      | Notes                                                                  |
| ------------------------ | --------------------------------------------- | ---------------------------------------------------------------------- |
| Inline message registers | Up to `seL4_MsgMaxLength` = 120 machine words | First N MRs in hardware registers (fast path); remainder in IPC buffer |
| Extra capabilities       | Up to `seL4_MsgMaxExtraCaps` = 3 cap slots    | Transferred only if sender holds `Grant` right on the endpoint         |

The `seL4_MessageInfo_t` header word encodes `length`, `extraCaps`,
`capsUnwrapped`, and `label`. There is no descriptor type, no OOL address field,
no handle array — the format is uniform: words + cap slots.

**Large data pattern:** The sender maps a memory frame capability into the
message's extra-cap slots. The receiver receives the capability and maps the
frame into its own address space. Zero-copy is achieved at the capability level
(no data copy), but accessing the data still requires mapping the frame. The
page size of the Frame object determines the minimum transfer granularity: seL4
Frame types are hardware-page-sized (4KB, 2MB, 1GB on AArch64).

**Ownership semantics:** seL4 does not enforce exclusive ownership of a Frame
capability. Multiple processes can hold caps to the same Frame simultaneously.
If the sender wants to transfer exclusive ownership, it must derive a cap,
transfer it, and then explicitly delete (revoke) its own copy — this is
convention, not enforcement.

**Grant right:** The `Grant` right on an endpoint capability must be present for
the sender to include capabilities in the message. A cap without `Grant` can
send inline data only. This means the format divergence (inline vs. cap-bearing)
maps to a rights distinction at the endpoint level.

**Sources:**

- [seL4 Reference Manual 14.0.0 §4 (IPC)](https://sel4.systems/Info/Docs/seL4-manual-latest.pdf)
- [seL4 IPC tutorial](https://docs.sel4.systems/Tutorials/ipc.html)

---

### EROS / Coyotos — Key Invocation, No Explicit OOL

Both systems pass data in fixed-size typed slots through key invocation. EROS
keys carry up to 4 data words and 4 capability slots per invocation. There is no
large-buffer mechanism at the kernel interface. Large data transfer requires
setting up a shared memory window (granting a memory key) as a separate step and
then communicating offsets through the invocation payload.

**Implication:** Ownership of large buffers is managed entirely in userspace by
capability discipline, not enforced by the kernel at the IPC boundary.

**Source:**

- [Coyotos Microkernel Specification](https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html)

---

### Academic: Extending Rust with Zero-Copy IPC (PLOS 2023)

**Authors:** Lafrance, Detweiler, Li, Chen, Narayanan, Burtsev (University of
Utah)

**Core abstraction: RRefs (Remote References)**

An `RRef<T>` is a handle to a heap allocation in a **shared exchange heap** — a
dedicated memory region accessible to both sender and receiver during a
transfer. Each `RRef<T>` has a single owner at any point in time. When an
`RRef<T>` is passed across a domain boundary:

1. The compiler enforces the move — the sender's binding is invalidated
2. The exchange heap entry's owner field is updated
3. No copy of the `T` data occurs — the receiver accesses the same physical
   memory

The Rust move semantic (ownership transfer at compile time) is used to enforce
the zero-copy invariant. This is language-level ownership transfer, not page-
level.

**Message format constraint:** IPC messages may contain only:

- Plain data that can be stack-copied (small inline payloads)
- `RRef<T>` values (handles to exchange heap objects)

Raw pointers cannot cross domain boundaries. The type system enforces this via
trait bounds similar to Rust's `Send` trait. The exchange heap objects are not
ordinary heap allocations — they must be allocated in the shared region.

**Page size implications:** Exchange heap allocations are page-aligned (the
shared region is a set of physical pages accessible to both address spaces).
Transfers of types smaller than a page still occupy a full page if they are
individually allocated as `RRef<T>`. The paper acknowledges this as a
limitation: "Small messages may still consume full pages." The authors note that
granular packing (multiple small `RRef`s per page) is possible but requires a
custom allocator within the exchange heap.

**Hardware vs. language isolation distinction:** The PLOS 2023 system targets
hardware-isolated processes. The exchange heap uses kernel-managed memory
mappings to ensure both sides can access the region simultaneously during
transfer (and only during transfer). Revocation of sender access after the
receiver acknowledges requires a kernel operation to remove the sender's page
table entry.

In contrast, a language-isolated system (e.g., intra-kernel Rust modules) can
enforce ownership purely via the move semantic — no page table change is needed
because all modules share an address space. The zero-copy guarantee holds
without kernel involvement.

**Source:**

- [PLOS 2023 paper PDF](https://mars-research.github.io/doc/2023-plos-rust-zerocopy.pdf)
- [ACM DL entry](https://dl.acm.org/doi/10.1145/3623759.3624552)

---

### Academic: IPC Overheads — Hardware vs. Language Isolation (PLOS 2021)

**Authors:** Burtsev et al. (University of Utah)

Examines the overhead of hardware-isolated IPC vs. language-level IPC in a
Rust-based system (RedLeaf). Key finding:

- **Hardware IPC** (cross address-space): requires page table manipulation per
  call for ownership-transfer scenarios. Dominant overhead is TLB shootdowns and
  page table updates, not data copy.
- **Language IPC** (intra address-space, Rust move semantics): near-zero
  overhead — a pointer assignment. No kernel involvement.
- For small messages (< 100 bytes), hardware IPC overhead dominates and
  ownership transfer provides no benefit over copying.
- For large messages (> 4KB), hardware ownership transfer amortizes the
  page-table overhead across the eliminated copy cost.

**Source:**

- [PLOS 2021 paper PDF](https://users.cs.utah.edu/~aburtsev/doc/rust-ipc-plos21.pdf)

---

### Theseus OS (OSDI 2020)

**Mechanism:** Rust's built-in reference types (`&T`, `Arc<T>`) used for intra-
kernel resource sharing. Ownership invariants are upheld by the Rust type
system. IPC between tasks uses channel types where `T: Send`. No exchange heap;
all tasks share a single address space, so "zero-copy" is trivially achieved —
all transfers are pointer-sized.

**Implication for hardware-isolated kernels:** Theseus's approach works because
it is a single-address-space OS. When separate address spaces exist, pointer-
sharing across boundaries requires a distinct mechanism. Theseus does not
provide a template for cross-address-space ownership transfer.

**Source:**

- [Theseus OSDI 2020](https://www.usenix.org/system/files/osdi20-boos.pdf)

---

### Page Size and Zero-Copy Coupling

**The fundamental constraint:** VM-based zero-copy (page remapping) operates at
page granularity. The minimum unit of a zero-copy transfer is one page. On
AArch64:

| Granule | TTBCR.TG0/TG1 | Notes                                            |
| ------- | ------------- | ------------------------------------------------ |
| 4 KB    | `0b00`        | Standard; used by most ARM64 Linux, seL4, Zircon |
| 16 KB   | `0b10`        | Apple Silicon (M-series, A-series chips)         |
| 64 KB   | `0b01`        | Some server configurations                       |

The page granule is set at boot time (or per-translation-regime) and cannot
change at runtime. A kernel targeting AArch64 in general must be prepared for
any of these values (though in practice 4KB and 16KB cover > 99% of deployed
silicon).

**Cross-system practice:**

| System         | Zero-copy threshold                  | Page size exposed to userspace?           |
| -------------- | ------------------------------------ | ----------------------------------------- |
| Mach / XNU     | Any page-aligned OOL region          | Yes — `vm_page_size` is a public symbol   |
| Zircon         | VMO transfers (handle-based)         | Page size implicit in VMO operations      |
| seL4           | Frame capability granule (4KB / 2MB) | Exposed through untyped derivation sizes  |
| PLOS 2023 RRef | Exchange heap page alignment         | Implicit — allocator detail               |
| Linux pipe     | Splice/zero-copy for > 4KB           | `getpagesize()` / `sysconf(_SC_PAGESIZE)` |

**Key observation:** Systems that support VM-based zero-copy must expose the
page size to whoever constructs messages, so they can determine whether a given
buffer qualifies for the zero-copy path. If the page size is hidden, the kernel
must silently fall back to copying for non-aligned or sub-page regions, which
breaks predictability guarantees.

**Message format coupling:** If a message format has both inline and OOL/handle
slots, the page size determines:

1. The minimum OOL region size that avoids a physical copy fallback
2. The alignment padding required for OOL regions
3. The overhead of small-object RRef allocations on an exchange heap

A message format that does **not** support OOL/handle descriptors (e.g., seL4's
flat word array) routes large data through a second mechanism (capability to a
memory frame), decoupling the IPC message format from page size entirely — at
the cost of requiring two kernel operations for large transfers.

---

## Measured Data

### IPC roundtrip latency, hardware-isolated, AArch64

| System | Mechanism                       | AArch64 latency                                              |
| ------ | ------------------------------- | ------------------------------------------------------------ |
| seL4   | Inline fastpath (< 6 MRs)       | ~200–700 cycles                                              |
| seL4   | Frame cap transfer (large data) | Not separately benchmarked; involves 2 IPC calls + map/unmap |
| Zircon | Channel call (small inline)     | ~1 000+ cycles on x86-64; AArch64 similar order              |
| Mach   | OOL virtual copy (1 page)       | ~5 000–10 000 cycles (includes VM overhead)                  |
| Mach   | OOL physical copy (1 page)      | Higher than virtual copy; benchmark-dependent                |

### Copy vs. remap crossover (Linux-class systems, approximate)

Data from Linux `sendfile` / `splice` literature and Mach benchmarks:

- Below ~4 KB: physical copy is faster (cache-resident, no TLB manipulation)
- Above ~4 KB: page remapping becomes competitive; crossover is
  workload-dependent (sequential vs. random, cache pressure)
- Above ~64 KB–1 MB: remapping consistently faster for sequential access
  patterns

**Source:**

- [seL4 whitepaper](https://sel4.systems/About/seL4-whitepaper.pdf)
- [SJTU XPC TOCS 2022](https://ipads.se.sjtu.edu.cn/_media/publications/2022_-_a_-_tocs_-_xpc.pdf)

---

## Tradeoffs

### Language-level ownership enforcement vs. hardware enforcement

**Language-level (Rust move semantics + shared exchange heap):**

- Compiler enforces single-ownership invariant — no runtime check per send
- Requires either a shared address space or exchange heap visible to both
  parties
- For hardware-isolated processes: exchange heap requires kernel setup (mmap
  shared region) and kernel revocation (remove sender's mapping after transfer)
- In-language enforcement is only as strong as the language's unsafety barrier

**Hardware enforcement (page table manipulation):**

- Kernel-level invariant: once pages are remapped, the hardware enforces access
- Does not require language-level cooperation; works for any language
- Overhead: page table update + TLB shootdown per transfer; amortized over large
  buffers but non-negligible for small messages
- Does not prevent the sender from having aliased pointers to the region that
  survive the transfer (language safety is still needed)

### Two-field message format (inline + OOL/handles) vs. flat format

**Two-field (Mach OOL, Zircon handle array):**

- Single IPC call can carry both small metadata (inline) and large data
  (OOL/handle)
- Kernel must handle two distinct copy paths in one operation
- Message format is more complex; descriptor type system can grow
- Page size constraint is visible at the IPC boundary

**Flat format (seL4 word array + cap slots):**

- Simple, uniform message structure; bounded by `seL4_MsgMaxLength` words
- Large data handled by a separately transferred capability (Frame cap)
- Requires two separate interactions for large data (capability setup + IPC
  send)
- Page size is invisible at the IPC boundary; it appears only in memory
  management operations (untyped derivation)

### Exchange heap (PLOS 2023 RRef) vs. capability-to-frame (seL4)

**Exchange heap:**

- Single mechanism for both small and large ownership transfer
- Transfers are O(1) regardless of size (pointer handoff on shared heap)
- Exchange heap must be pre-established (one-time kernel cost per pair of
  domains)
- All transferable types must be allocated on the exchange heap; ordinary heap
  pointers are not transferable
- Sub-page granularity possible via custom allocator within the heap

**Capability-to-frame:**

- Standard mechanism in capability kernels; no special heap
- Sender must pre-allocate and map a frame; receiver must map after receipt
- Two mapping operations per large transfer (sender map → send cap → receiver
  map)
- Frame size is page-aligned by definition; no sub-page transfers

### Explicit vs. implicit page size exposure

**Explicit (Mach `vm_page_size`):**

- Userspace knows the threshold for virtual-copy eligibility
- Can pack data optimally for zero-copy paths
- Creates ABI dependency on page size (binaries may embed page size assumptions)

**Implicit (Zircon: page size in VMO operations):**

- Userspace interacts with VMOs without needing to know the page size directly
- VMO commit and resize operations internally round to page granularity
- Reduces ABI surface but makes the threshold opaque to message senders

**Hidden (seL4: flat IPC, no OOL):**

- IPC format has no dependence on page size
- Large data path is entirely separate from IPC
- Forces clean separation of concerns at the cost of two-step large transfers

---

## What Is Not Resolved in the Literature

1. **Revocation cost at scale:** The PLOS 2023 approach requires the kernel to
   revoke sender access after the receiver acknowledges. How this interacts with
   TLB shootdowns in multi-core settings is not benchmarked in the paper.

2. **Sub-page transfer granularity:** All VM-based mechanisms have a page-size
   floor. Whether a packing strategy within the exchange heap (multiple small
   objects per page) is practical without introducing fragmentation problems is
   not settled.

3. **Interaction with capability revocation:** In a capability kernel, if the
   Frame capability used for large data is transferred and later revoked, the
   question of who manages the physical pages is system-specific. Revocation
   semantics for owned-memory capabilities are not standardized.

4. **Hybrid inline/OOL with bounded guarantees:** Whether a verifiable formal
   model of a hybrid format (some words inline, some by reference) is tractable
   in a proof-carrying system like seL4 is not documented — seL4's flat format
   may be partially motivated by verification tractability.

---

## References

1. Lafrance, Detweiler, Li, Chen, Narayanan, Burtsev. "Extending Rust with
   Support for Zero Copy Communication." PLOS 2023.
   https://mars-research.github.io/doc/2023-plos-rust-zerocopy.pdf
   https://dl.acm.org/doi/10.1145/3623759.3624552

2. Burtsev et al. "Understanding the Overheads of Hardware and Language-Based
   IPC Mechanisms." PLOS 2021.
   https://users.cs.utah.edu/~aburtsev/doc/rust-ipc-plos21.pdf

3. Boos et al. "Theseus: an Experiment in Operating System Structure and State
   Management." OSDI 2020. https://www.usenix.org/system/files/osdi20-boos.pdf

4. seL4 Reference Manual Version 14.0.0 §4 (IPC).
   https://sel4.systems/Info/Docs/seL4-manual-latest.pdf

5. seL4 IPC tutorial. https://docs.sel4.systems/Tutorials/ipc.html

6. Apple Kernel Programming Guide — Mach.
   https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html

7. dmcyk. "XNU IPC — Introduction to OOL Data."
   https://dmcyk.xyz/post/xnu_ipc_iii_ool_data/

8. dmcyk. "XNU IPC — OOL Data and Virtual Memory."
   https://dmcyk.xyz/post/xnu_ipc_iv_ool_vm/

9. Zircon Kernel Concepts.
   https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts

10. Zircon Handles. https://fuchsia.dev/fuchsia-src/concepts/kernel/handles

11. zx_channel_write_etc syscall reference.
    https://fuchsia.dev/reference/syscalls/channel_write_etc

12. Coyotos Microkernel Specification.
    https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html

13. Du et al. "Boosting Inter-Process Communication with Architectural Support."
    TOCS 2022.
    https://ipads.se.sjtu.edu.cn/_media/publications/2022_-_a_-_tocs_-_xpc.pdf

14. seL4 whitepaper. https://sel4.systems/About/seL4-whitepaper.pdf
