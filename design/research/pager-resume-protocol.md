# Pager Resume Protocol: Install-then-Resume vs. Reply-Carries-Install

## Question

When a pager receives a fault message and wants to resume the faulting thread,
must it first perform a memory installation operation (mapping a physical frame
into the faulting address space), or is calling `resume()` alone sufficient? And
what does the kernel validate — if anything — at resume time?

This is distinct from the routing question (covered in `page-fault-routing.md`).
The question here is: what is the **pager's syscall pattern on the resolution
path** — how many kernel calls, in what order, and with what atomicity
guarantees?

---

## 1. Three Structural Patterns

Real systems cluster into three models for the relationship between "install
backing" and "resume the faulting thread."

| Pattern                | Systems                        | Install and Resume                                              |
| ---------------------- | ------------------------------ | --------------------------------------------------------------- |
| Install-then-resume    | seL4, EROS/Coyotos, Barrelfish | Two separate kernel operations                                  |
| Reply-carries-install  | Original L4, Mach              | Install embedded in the resume message                          |
| Supply-triggers-resume | Zircon                         | Supplying pages implicitly unblocks waiters; no explicit resume |

---

## 2. Install-then-Resume

### seL4

seL4 separates the two operations completely. On receiving the VM fault message:

1. The pager holds a fault endpoint with the reply cap deposited by the kernel.
2. The pager calls one or more page-table syscalls — e.g.,
   `seL4_ARM_Page_Map(frame_cap, vspace_cap, vaddr, rights, attr)` — to install
   the frame into the faulting VSpace.
3. The pager calls `seL4_Reply(seL4_MessageInfo_new(0, 0, 0, 0))` with `label=0`
   to resume the faulting thread. Alternatively, the pager may call
   `seL4_TCB_Resume(tcb_cap)` if it holds a TCB capability for the faulting
   thread.

**The kernel performs no validation at resume time.** When the reply is received
with `label=0`, the kernel transitions the faulting thread from
`ThreadState_BlockedOnFault` to `ThreadState_Running` and returns to the
faulting instruction. The kernel does not check whether the fault address is now
mapped. If the pager called resume before completing the mapping, the thread
immediately re-faults and the fault message is delivered again.

This re-fault behaviour is by design. It provides a retry loop without requiring
the kernel to maintain state about whether a particular fault has been
"resolved." The cost is that a pager that calls resume prematurely causes a
fault storm until the mapping is eventually installed.

**`seL4_Reply` vs. `seL4_TCB_Resume`:** `seL4_Reply` consumes the one-shot fault
reply cap and requires no persistent authority over the faulting thread.
`seL4_TCB_Resume` requires a TCB capability — a more powerful and persistent
right — and does not consume the fault reply cap. Using `seL4_TCB_Resume` leaves
the reply cap valid; the pager must also clear that cap to avoid later misuse.
The standard pattern for fault handlers uses `seL4_Reply`.

**Source:** seL4 Reference Manual 14.0.0 §5 (Faults and Exceptions); seL4 Fault
Handlers Tutorial (docs.sel4.systems/Tutorials/fault-handlers.html); seL4 API
Reference — `seL4_Reply`, `seL4_TCB_Resume`.

---

### EROS / Coyotos

In EROS and its descendants, the fault handler is the **keeper** — a capability
stored in the process that the kernel invokes when the address space translation
fails.

**EROS protocol:**

1. The kernel invokes the keeper capability, passing a **restart key** (a
   capability that, when invoked, resumes the faulted domain) as an implicit
   argument in a designated key register.
2. The keeper executes as a separate domain. To resolve the fault, it must
   **modify the segment tree** of the faulting domain — installing a Page node
   at the appropriate position in the capability tree so that subsequent address
   translation will find the missing page.
3. After installing the page node, the keeper invokes the restart key (`RETURN`
   operation on the key). The kernel resumes the faulted domain at the faulting
   instruction.

**EROS does not validate the segment tree modification** when the restart key is
invoked. It resumes the domain and re-executes the faulting instruction. If the
keeper did not install the correct page, the fault recurs.

**Coyotos GPT (Generalized Page Table) variant:** The keeper receives a
`ProcessKey` of type `restart` rather than a separate restart key. The keeper
modifies the GPT tree to install the missing backing page or GPT node, then
invokes the process key (which resumes via a `CALL` → `RETURN` idiom). The
structural invariant is the same: install first, then restart.

**Source:** Shapiro et al., "EROS: A Fast Capability System," SOSP 1999; Coyotos
Microkernel Specification (Shapiro, 2007); CapROS Address Spaces reference
(capros.org/devel/ObRef/concepts/AddressSpaces); cap-lore.com EROS/Coyotos
comparison.

---

### Barrelfish (self-paging reference)

Barrelfish uses self-paging: the faulting dispatcher handles its own fault as an
upcall. There is no separate pager process.

1. The kernel delivers the fault as an upcall to the faulting dispatcher's
   control block. The dispatcher's fault handler code runs in the same address
   space.
2. The handler traverses the VNode tree and calls `vnode_map()` to install the
   missing frame into the appropriate VNode slot.
3. The handler calls `disp_resume()` (user-level, not a kernel syscall) to
   re-schedule the faulted thread.

Because handler and faulter share an address space, there is no IPC. The install
and resume are still logically ordered (install the VNode, then resume), but the
"resume" is a user-level scheduler call, not a kernel syscall. This is only
possible because the fault handler is in the same VSpace as the faulter.

**Source:** Barrelfish Architecture Overview (TN-000); Gerber, "Virtual Memory
in a Multikernel — The Barrelfish OS," ETH Zurich master's thesis, 2012.

---

## 3. Reply-Carries-Install

### Original L4 (Liedtke 1993)

L4 made the mapping grant atomic with the resume. When the pager replies to a
fault IPC, it embeds one or more **map items** or **grant items** in the reply
message. A map item is a typed IPC message item that specifies an fpage
(flexible page — a power-of-2-aligned region) and its permissions. When the
kernel processes the reply:

1. It iterates over the map/grant items in the reply.
2. For each map item, it installs the corresponding mapping into the faulting
   thread's address space (or grants the page, transferring ownership).
3. It resumes the faulting thread.

This is **one kernel operation from the pager's perspective**: a single
`l4_ipc_reply` call both installs the mappings and resumes the thread. The pager
cannot resume the thread without simultaneously specifying what to map.

One consequence: the pager cannot call resume without having decided on the
backing. There is no "install then separately resume" option. If the pager wants
to reply without installing any mapping (e.g., to send the fault to a chained
handler), it sends a reply with no map items; the faulting thread resumes
immediately and, if the mapping is still missing, re-faults.

L4 later descendants (L4Ka::Pistachio, Fiasco.OC, L4Re) retained typed IPC items
including map/grant items for some time, though newer L4 variants moved away
from kernel-mediated copy (seL4 removed string items entirely to avoid nested
page faults during kernel copy).

**Source:** Liedtke, "Improving IPC by Kernel Design," SOSP 1993; L4
eXperimental Kernel Reference Manual X.2 r7 §3 (IPC) and §5 (Fpages);
https://www.l4ka.org/l4ka/l4-x2-r7.pdf.

---

### Mach

Mach's external pager protocol delivers a page back to the kernel via a message,
after which the kernel installs it and resumes the faulting thread:

1. Kernel sends `memory_object_data_request` to the pager's port, specifying the
   requested offset and length.
2. Pager retrieves or creates the page content, then sends
   `memory_object_data_provided` back to the kernel, including the page data as
   a typed memory descriptor.
3. Kernel receives the message, installs the page into the task's `vm_map`, and
   resumes the faulting thread.

From the pager's perspective: one message supplies the page and causes the
thread to resume. There is no separate resume call. As with L4, the pager cannot
resume the thread without simultaneously providing page content.

**Source:** Mach 3 Kernel Principles (CMU Technical Report CMU-CS-90-125);
Draves, "Extending the Mach External Pager Interface," UW-CSE-90-09-05.

---

## 4. Supply-Triggers-Resume

### Zircon

Zircon's pager model decouples the pager's operation from any "resume" concept.
The pager owns a `Pager` kernel object and a pager-backed `VMO`:

1. A thread accesses an unmapped page in a pager-backed VMO. The kernel blocks
   the thread and delivers a `ZX_PKT_TYPE_PAGE_REQUEST` packet to the pager's
   associated `Port`.
2. The pager reads the packet, obtains or creates the backing pages (populating
   a separate "supply VMO"), and calls
   `zx_pager_supply_pages(pager, vmo, offset, length, supply_vmo, supply_offset)`.
3. The kernel receives `zx_pager_supply_pages`, takes ownership of the pages
   from the supply VMO, installs them as the backing for the requested range of
   the pager VMO, and **internally unblocks all threads waiting for any page in
   that range**.

The pager never calls a "resume" syscall. Supplying the pages is the mechanism
that unblocks waiting threads. The kernel maintains the list of threads blocked
on each VMO page slot and wakes them when the slot is populated.

This model is possible because Zircon holds the canonical backing for VMO pages
in the kernel. The pager's role is to populate slots in a kernel-managed object,
not to directly manipulate a thread's page tables.

**Source:** Zircon Pager kernel object reference
(fuchsia.dev/fuchsia-src/reference/kernel_objects/pager); RFC-0226: Zircon Pager
Writeback (fuchsia.dev); `zx_pager_supply_pages` syscall reference
(fuchsia.dev).

---

## 5. The Kernel-Validation Question

Across all three patterns, no system validates the completeness of the mapping
at resume time. The invariants each design relies on:

| System       | What kernel validates at resume                   | Failure mode if mapping absent               |
| ------------ | ------------------------------------------------- | -------------------------------------------- |
| seL4         | Nothing — label=0 → resume                        | Thread re-faults; fault message re-delivered |
| EROS/Coyotos | Nothing — restart key invocation → resume         | Domain re-faults; keeper re-invoked          |
| L4 original  | Nothing — map items processed, thread resumed     | If no map items: thread re-faults            |
| Zircon       | Pages are present in VMO before unblocking        | Kernel enforces: cannot supply partial       |
| Mach         | Page is present in kernel vm_object before resume | Enforced by receive-then-resume in kernel    |

Zircon and Mach enforce supply-before-resume structurally: the kernel owns the
backing and only unblocks threads after the pages are in its possession. seL4,
EROS, and L4 rely on the pager following the correct ordering. The re-fault
mechanism serves as the error signal.

---

## 6. Atomicity of the Mapping-Resume Sequence

**seL4 (two separate syscalls):** There is a window between `Page_Map` and
`Reply` during which the mapping is installed but the thread is still blocked,
and another (in the re-fault failure case) where the thread is running but the
mapping is not yet installed. This window is not exposed to other threads — the
faulting thread remains blocked until the reply arrives — but it is a real
ordering requirement the pager must respect.

**L4 original (reply carries mapping):** The mapping installation and thread
resumption occur in the same kernel entry triggered by the pager's reply. There
is no window where the thread is running without the mapping. The kernel
processes map items before transitioning the faulting thread.

**Zircon (supply triggers wake):** The kernel transitions the thread from
blocked to runnable only after the VMO page slot is populated. The thread cannot
run until the page is available. This is the strongest atomicity guarantee.

**EROS restart key:** The segment tree modification (installing the page node)
is committed before the restart key is invoked. The kernel processes the tree on
the next address translation. The window between "tree modification" and
"restart key invocation" exists but does not expose the faulted thread to an
intermediate state.

---

## 7. Number of Kernel Entries on the Resolution Path

| System      | Kernel entries for pager to resolve one fault                               |
| ----------- | --------------------------------------------------------------------------- |
| seL4        | 2+ (one `Page_Map` per page table level needed + one `Reply`)               |
| L4 original | 1 (single IPC reply with fpage embedded)                                    |
| Zircon      | 1 (`zx_pager_supply_pages`) + prior dequeue of port packet                  |
| Mach        | 1 message send (`memory_object_data_provided`)                              |
| EROS        | 2+ (capability invocations to modify segment tree + restart key invocation) |
| Barrelfish  | 0 kernel entries (all user-level in same VSpace)                            |

For seL4: if the page table structure is already in place (only a leaf Frame
mapping is missing), one `ARM_Page_Map` plus one `Reply` = 2 syscalls minimum.
If intermediate page table levels are absent, additional `ARM_PageTable_Map`
calls are needed first. seL4 documentation recommends the pager pre-populate
page tables at address space setup time to keep the hot path at 2 syscalls.

---

## 8. TCB_Resume vs. Reply as the Resume Primitive

seL4 documents two resume mechanisms:

**`seL4_Reply(label=0)` on the fault endpoint:**

- Consumes the one-shot fault reply cap deposited by the kernel.
- Does not require the pager to hold a TCB capability for the faulting thread.
- After consumption, the cap slot is empty; the next fault will deposit a fresh
  reply cap.
- This is the standard pattern for fault handlers.

**`seL4_TCB_Resume(tcb_cap)`:**

- Requires the pager to hold a TCB capability for the faulting thread.
- Does NOT consume the fault reply cap; the cap remains valid in the endpoint's
  reply slot.
- The pager must explicitly clear or consume the reply cap to avoid a second
  spurious resume on the next call.
- More powerful than `seL4_Reply` — holding a TCB cap authorizes many other
  operations on the thread.
- Used when the pager needs to take other actions on the faulting thread (e.g.,
  adjust registers, change priority) before releasing it.

No other surveyed system exposes two resume mechanisms at this level of
distinction. Mach, Zircon, and L4 all have a single resumption path built into
the fault-resolution message/syscall.

---

## 9. Tradeoffs

**Install-then-resume (two calls) vs. reply-carries-install (one call):**

Two-call: Pager has flexibility — it can install multiple pages, perform other
operations, or delegate to a chained handler before calling resume. The reply
cap (or TCB cap) that resumes the thread is decoupled from the install
operation. Cost: more kernel entries per fault; ordering is the pager's
responsibility; the pager could mistakenly resume before installing.

One-call: The kernel atomically applies the mapping and resumes the thread.
Fewer kernel entries per fault; no ordering requirement. Cost: the pager must
decide on the backing before calling resume; it cannot install incrementally
across multiple calls and then resume. If the mapping decision requires
interaction with another server, the pager must complete that interaction before
the single reply call.

**Kernel-owned backing (Zircon) vs. pager-installs (seL4/L4):**

Kernel-owned (Zircon VMO): The kernel maintains the canonical mapping state.
Pager supplies content to the kernel, which installs and unblocks. This is
structurally safe — threads cannot see a partially-supplied page. Cost: the
kernel must implement the VMO backing abstraction; pager cannot use arbitrary
physical memory layouts without going through the kernel's VMO interface.

Pager-installs (seL4/L4): The pager directly manipulates the VSpace page tables
via capability invocations. More flexible — the pager decides exactly which
physical frame gets mapped where. Cost: the pager must correctly order install
and resume; the kernel cannot enforce the order; re-fault is the only error
recovery path.

**Re-fault as error signal vs. kernel enforcement:**

Re-fault: Simple kernel mechanism. If the pager misorders install and resume,
the fault simply recurs. The pager loop is a correct retry mechanism (the same
fault handler will receive the message again and can complete the install).
Cost: a buggy pager that never installs the mapping produces an infinite fault
loop; no progress guarantee.

Kernel enforcement (Zircon): The kernel refuses to unblock threads until the
pages are supplied. This provides a hard progress guarantee: a thread is
unblocked if and only if the pages are available. Cost: the kernel must maintain
per-page waiter lists and track which threads are blocked on which VMO pages.

**Pager privilege model:**

`seL4_Reply` requires only the fault reply cap (deposited by the kernel into the
fault endpoint, consumed on resume). The pager does not need a TCB capability.
This is the minimum authority needed to resume a thread from a fault.

`seL4_TCB_Resume` requires a TCB cap — a more powerful, persistent authority.
Holding a TCB cap authorizes suspend/resume, priority change, register
read/write, fault endpoint configuration, and other operations. Granting this to
a pager increases the pager's authority beyond what fault resolution strictly
requires.

---

## 10. References

- Shapiro, J. et al. "EROS: A Fast Capability System." _SOSP 1999_.
  https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf
- Shapiro, J. "Coyotos Microkernel Specification." 2007.
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- seL4 Reference Manual v14.0.0. UNSW/NICTA/seL4 Foundation.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 Fault Handlers Tutorial. seL4 Foundation.
  https://docs.sel4.systems/Tutorials/fault-handlers.html
- seL4 API Reference — `seL4_Reply`, `seL4_TCB_Resume`. seL4 Foundation.
  https://docs.sel4.systems/projects/sel4/api-doc.html
- Liedtke, J. "Improving IPC by Kernel Design." _SOSP 1993_.
- L4 eXperimental Kernel Reference Manual X.2 r7. l4ka.org.
  https://www.l4ka.org/l4ka/l4-x2-r7.pdf
- Draves, R. "Extending the Mach External Pager Interface."
  UW-CSE-90-09-05, 1990.
  https://dada.cs.washington.edu/research/tr/1990/09/UW-CSE-90-09-05.pdf
- Mach 3 Kernel Principles. CMU Technical Report CMU-CS-90-125.
- Zircon Pager kernel object reference. Fuchsia.dev.
  https://fuchsia.dev/fuchsia-src/reference/kernel_objects/pager
- RFC-0226: Zircon Pager Writeback. Fuchsia.dev.
  https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0226_zircon_pager_writeback
- Gerber, S. "Virtual Memory in a Multikernel — The Barrelfish OS." Master's
  thesis, ETH Zurich, 2012.
- CapROS Address Spaces reference.
  http://www.capros.org/devel/ObRef/concepts/AddressSpaces.html
