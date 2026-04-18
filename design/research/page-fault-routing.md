# Page Fault Routing: Kernel-Internal vs. Userspace Delegation

**Question:** Does the kernel resolve page faults internally, or does it route
them to a userspace pager?

**Why it matters:** The answer determines whether pager traffic becomes a
first-class IPC workload. If faults are delegated to userspace, the fault
round-trip cost (at minimum, two IPC traversals) shapes the performance envelope
of any workload with cold memory accesses — including IPC buffers themselves.

---

## 1. Three Patterns Across Real Systems

Real kernels cluster into three approaches:

| Pattern                                  | Systems                                                                    |
| ---------------------------------------- | -------------------------------------------------------------------------- |
| In-kernel resolution                     | QNX Neutrino, EROS (base), Redox (kernel default), most monolithic kernels |
| External pager via IPC                   | Original L4, seL4, Mach/XNU, Zircon, GNU Hurd                              |
| Self-paging (upcall to faulting process) | Barrelfish, Nemesis                                                        |

---

## 2. In-Kernel Resolution

### QNX Neutrino

QNX Neutrino handles page faults entirely within the microkernel (procnto). The
memory manager is the kernel itself; there is no external pager interface
exposed to user processes. Anonymous and file-backed pages are managed by
in-kernel policy. Userspace programs have no mechanism to intercept page faults
for a region.

**Source:** QNX Neutrino RTOS System Architecture Guide — Memory Management.

### EROS

EROS resolves page faults by traversing its capability/node tree (segments)
entirely in the kernel. When a page fault occurs:

1. The kernel traverses the segment tree to find the backing node.
2. If the node is not in memory, the kernel loads it from disk itself (via an
   in-kernel object cache).
3. The kernel installs the PTE and resumes the thread.

EROS has a "keeper" concept (a capability stored in a process, invoked on
exceptional conditions), but the keeper handles process-level exceptions — not
the routine page fault path. Routine demand paging is kernel-internal.

Coyotos extends EROS with per-region fault handlers in Guarded Page Tables
(GPTs): any 2^k-page region may have an associated fault handler capability.
When a fault occurs in that region, the kernel invokes the handler capability.
This is a hybrid: common cases stay in-kernel; special regions delegate.

**Source:** "EROS: A Fast Capability System" (Shapiro et al., SOSP 1999);
Coyotos Microkernel Specification (Shapiro, 2007).

---

## 3. External Pager via IPC

### Original L4 (Liedtke 1993)

L4 made the pager model central to its design. Every address space is identified
by a **pager thread**. The pager receives page-fault IPC messages and replies
with fpage (flexible page) grants.

**Protocol:**

1. Thread T accesses address A, no PTE present.
2. Kernel blocks T.
3. Kernel sends IPC to T's configured pager thread:
   `{fault_addr, fault_type, faulting_thread_id}`.
4. Pager thread wakes, establishes or locates the backing frame, replies with
   fpage grant.
5. Kernel installs the mapping, resumes T.

The pager IPC uses the same fastpath as regular IPC. On i486 at ~100MHz (1993):
L4 IPC round-trip ≈ 10 µs → minimum fault resolution latency ≈ 20 µs (one
round-trip: kernel→pager→kernel).

**Source:** Jochen Liedtke, "Improving IPC by Kernel Design" (SOSP 1993); L4
eXperimental Kernel Reference Manual Version X.2
(https://www.l4ka.org/l4ka/l4-x2-r7.pdf).

### seL4

seL4 inherits L4's pager model but integrates it with capability-based access
control. Each TCB holds a **fault handler endpoint** capability. On fault:

1. Kernel blocks the faulting TCB.
2. Kernel acts as IPC sender and calls the fault handler endpoint.
3. The fault message is delivered via message registers (MRs):

   | MR index | Field                        | Content                           |
   | -------- | ---------------------------- | --------------------------------- |
   | MR0      | `seL4_VMFault_IP`            | Instruction pointer at fault      |
   | MR1      | `seL4_VMFault_Addr`          | Faulting virtual address          |
   | MR2      | `seL4_VMFault_PrefetchFault` | 1 if instruction fetch, 0 if data |
   | MR3      | `seL4_VMFault_FSR`           | ARM Fault Status Register         |

4. The fault handler maps the missing page (using `seL4_ARM_Page_Map` or
   equivalent) and replies with `seL4_Reply(label=0)`.
5. Kernel resumes the faulting TCB.

The fault handler endpoint supports badged capabilities, so a single handler
thread can distinguish faults from multiple supervised threads by badge value.
If no fault handler endpoint is configured, the TCB is killed on fault.

**Measured IPC latency** (seL4, hot cache, same-priority fastpath):

| Platform                              | IPC call              | IPC reply             |
| ------------------------------------- | --------------------- | --------------------- |
| ARM Cortex-A9 @ 1.0 GHz (i.MX6)       | 340 cycles (340 ns)   | 359 cycles (359 ns)   |
| ARM Cortex-A57 @ 1.9 GHz (Jetson TX1) | ~200 cycles (~105 ns) | ~210 cycles (~110 ns) |
| x86_64 Haswell @ 3.4 GHz              | ~400 cycles (~118 ns) | ~400 cycles (~118 ns) |

Minimum fault resolution latency = (kernel→handler IPC call) + handler work +
(handler reply). The kernel-to-handler call is not on the seL4 fastpath (the
kernel is the sender, not a user thread), so it may be slightly higher than the
fastpath call cost. Handler work includes at minimum one `seL4_ARM_Page_Map`
syscall (~500–800 cycles additional). Total cold-fault resolution: roughly
1000–2000 cycles on A57 (≈0.5–1 µs, cache hot), not counting TLB miss effects.

**Sources:** seL4 Reference Manual 14.0.0
(https://sel4.systems/Info/Docs/seL4-manual-latest.pdf); seL4 benchmark suite
(https://sel4.systems/performance.html); seL4 fault handler tutorial
(https://docs.sel4.systems/Tutorials/fault-handlers.html).

### Mach / XNU

Mach introduced the **external pager** (memory object) interface. A userspace
server that implements the `memory_object` port interface can back virtual
regions. On a fault into an externally-paged region:

1. Kernel sends `memory_object_data_request` message to the pager's receive
   port.
2. Pager fetches/creates the page content and replies with
   `memory_object_data_provided`, giving the kernel a page to install.
3. Kernel installs the page and resumes the faulting thread.

**Performance critique:** Mach IPC round-trip cost was ≈230 µs on i486 (versus
L4's ≈10 µs, Liedtke 1993). A page fault through an external Mach pager
therefore cost ≈460 µs minimum — 23× slower than L4's pager model. This was a
primary target of the second-generation microkernel critique.

Preliminary measurements from UW research (1990) found that using a "premo
pager" (predictive remote memory object) added ≈10% to application runtime while
reducing page faults by 15% — but this benefit depended on workload access
patterns, not the pager overhead itself.

Apple's XNU retains the `memory_object` interface but its practical use has
narrowed; the default pager handles most anonymous memory in-kernel.

**Sources:** Mach 3 Kernel Principles (CMU Technical Report CMU-CS-90-125);
Liedtke, "Improving IPC by Kernel Design" (SOSP 1993); "Extending the Mach
External Pager Interface" (Draves, UW-CSE-90-09-05); GNU Hurd external pager
documentation
(https://www.gnu.org/software/hurd/microkernel/mach/external_pager_mechanism.html).

### Zircon (Fuchsia)

Zircon uses a **port-based asynchronous pager** model tied to Virtual Memory
Objects (VMOs). A pager VMO is created with `zx_pager_create()`; page requests
are delivered to an associated port rather than via direct IPC rendezvous.

**Protocol:**

1. Thread accesses a page in a pager-backed VMO; no mapping present.
2. Kernel blocks the accessing thread.
3. Kernel writes a `ZX_PKT_TYPE_PAGE_REQUEST` packet to the pager's port.
4. Userspace pager service dequeues the packet, reads backing storage, calls
   `zx_pager_supply_pages()` with a populated VMO.
5. Kernel installs the mapping and resumes the faulting thread.

Unlike L4/seL4's synchronous rendezvous, Zircon's port model allows the pager to
batch supply operations or pipeline requests. Prefetching is the pager's
responsibility — the kernel does not prefetch ahead of faults.

Syscalls that operate on pager-backed VMOs fail by default if they would need to
block for pager response (preventing deadlock in kernelspace). No public
benchmarks for Zircon's pager round-trip latency are available.

**Source:** Zircon Pager kernel object reference
(https://fuchsia.dev/fuchsia-src/reference/kernel_objects/pager); RFC-0226:
Zircon Pager Writeback
(https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0226_zircon_pager_writeback).

### Linux userfaultfd (non-microkernel reference point)

Linux 4.3 introduced `userfaultfd` — a mechanism for user threads to intercept
page faults on designated ranges. The kernel delivers a fault event to a file
descriptor; a handler thread reads it and resolves the fault via `ioctl`.

**Measured latency** (x86 desktop, idle system):

- Average per-fault with thread-based handler: **7.8 µs**
- Write-protection SIGBUS mode: **1.85 µs** (0.77 µs kernel→user, 0.27 µs
  resolution)
- Under high concurrency (64 threads): **up to 107 µs**

This provides a rough upper bound for "userspace fault handling on commodity
hardware with a non-microkernel kernel." Microkernel systems with faster IPC
(seL4) achieve lower latency in the common case, but the same structural costs
(context switch, message delivery, reply) apply.

**Sources:** "Measuring userfaultfd page-fault latency" (Noah Watkins, 2016,
https://makedist.com/posts/2016/10/10/measuring-userfaultfd-page-fault-latency/);
userfaultfd-wp latency measurements
(https://xzpeter.org/userfaultfd-wp-latency-measurements/).

---

## 4. Self-Paging (Upcall to Faulting Process)

### Barrelfish

Barrelfish does not resolve page faults in the kernel or route them to a
separate pager process. Instead, it uses **self-paging**: the kernel delivers
the fault as an upcall to the faulting dispatcher (a user-level scheduling
entity). The dispatcher's own user-level scheduler invokes the fault handler
within the same address space.

**Protocol:**

1. Thread T faults; kernel preempts it.
2. Kernel delivers an "pagefault upcall" to T's dispatcher control block (DCB),
   saving register state into the dispatcher's save area.
3. Dispatcher handler code runs in the same VSpace, traverses the VNode tree,
   installs the missing mapping, then resumes the faulting thread via
   `resume()`.

Key constraint: the dispatcher's handler code and the VNode tree structures it
manipulates must be in always-resident (wired) memory. There is no recursive
fallback — if the handler itself faults, the system panics.

**Source:** Barrelfish Architecture Overview (TN-000); "Virtual Memory in a
Multikernel — The Barrelfish OS" (Gerber, master's thesis, ETH Zurich, 2012).

### Nemesis

Nemesis (Cambridge, 1990s) used an **activation model** for all OS events,
including page faults. The kernel delivered faults as events to the faulting
domain's activation handler — a user-level function in the domain itself.

The NTSC (Nemesis kernel) handles: CPU scheduling, event delivery, context
switching. Even page faults run in userspace via the activation mechanism.
Activation delivery latency: **100–200 ns** (measured, Nemesis papers).

This was faster than any external pager model because no process switch was
required — the same address space handled the fault.

**Source:** "Nemesis: a service-oriented operating system for multimedia"
(Leslie et al., USENIX ATC 1996); syscall-landscape.md.

---

## 5. The Pager Deadlock Problem

All userspace pager models face a structural deadlock risk: if the pager thread
itself faults while handling a fault for another thread, a cycle results: the
kernel waits for the pager, which needs a page the kernel won't provide without
the pager.

**Known mitigations:**

1. **Wire the pager's memory.** The pager's code, stack, and all data structures
   it touches during fault handling must be physically pinned and never subject
   to eviction. seL4 and L4 systems require this implicitly. seL4 "initial
   thread" (the pager for the boot image) runs with all memory mapped from boot.

2. **Kernel reserve pool.** Maintain a physically-wired kernel memory region
   that the pager can use without triggering its own faults. This is the
   approach taken in some Mach deployments.

3. **Separate fault stacks.** The pager thread runs on a separate stack whose
   pages are always present, even if its main data pages are not.

4. **Self-paging with wired handler.** Barrelfish's approach: the handler cannot
   fault, because all handler pages are wired at startup.

5. **Timeout + kill.** Mach used timeouts: if the pager didn't respond within a
   deadline, the kernel could kill the faulting task. Critics (Hand et al.,
   HotOS 2005) called this "liability inversion" — the kernel cannot guarantee
   liveness independently of userspace pagers.

The seL4 mailing list and design documentation acknowledge this constraint:
fault handler endpoints must be carefully provisioned so the handler itself does
not depend on memory that could fault.

**Source:** "Are Virtual Machine Monitors Microkernels Done Right?" (Hand et
al., HotOS 2005,
https://www.cs.utexas.edu/~witchel/380L/papers/hand05hotos-vm-micro.pdf); GNU
Hurd external pager mechanism discussion; seL4 fault-handlers tutorial notes.

---

## 6. Nested Fault During IPC Copy

A separate hazard specific to external-pager systems: if the kernel performs an
IPC copy operation that reads from a user buffer, and that buffer page is not
present, the kernel triggers a fault during IPC. This is a "nested page fault."

seL4 eliminated long IPC (kernel-mediated copy of large messages) precisely to
avoid this: seL4 IPC is register-only (small, bounded size) or uses shared
memory the parties arrange separately. Large transfers go through shared memory
regions that the sender is responsible for mapping before calling IPC.

Liedtke's original L4 supported string items (kernel-mediated copy) in IPC, and
the seL4 team removed them:

> "Long IPC complicated the kernel (nested page faults during copy), and shared
> memory performs better for large transfers anyway."
>
> — seL4 design rationale, syscall-landscape.md

**Source:** seL4 design notes; syscall-landscape.md in this repository.

---

## 7. Tradeoffs

### In-kernel resolution

| Dimension             | Characteristic                           |
| --------------------- | ---------------------------------------- |
| Latency               | Minimum (no IPC, no context switch)      |
| Policy location       | In kernel (hard to customize)            |
| TCB size              | Larger if swap/disk I/O required         |
| Verifiability         | Harder (more mechanism in kernel)        |
| Custom backing stores | Not possible without kernel modification |
| Failure modes         | Simpler (kernel controls all paths)      |
| Deadlock risk         | None (kernel is not paged by itself)     |

### External pager (IPC-based)

| Dimension             | Characteristic                                                  |
| --------------------- | --------------------------------------------------------------- |
| Latency               | IPC round-trip minimum (seL4: ~700+ cycles per fault on A57)    |
| Policy location       | Userspace (fully customizable)                                  |
| TCB size              | Smaller kernel; pager complexity is userspace                   |
| Verifiability         | Kernel provably minimal; pager verified separately              |
| Custom backing stores | Arbitrary (filesystem, database, compressed, network)           |
| Failure modes         | Pager death → faulting threads block indefinitely or are killed |
| Deadlock risk         | Yes — pager memory must be wired; requires careful provisioning |
| IPC amplification     | Fault-heavy workloads generate proportional IPC traffic         |

### Self-paging (upcall)

| Dimension             | Characteristic                                                          |
| --------------------- | ----------------------------------------------------------------------- |
| Latency               | Minimum (no process switch, ~100–200 ns in Nemesis)                     |
| Policy location       | Faulting process itself                                                 |
| TCB size              | Very small kernel                                                       |
| Custom backing stores | Per-application only; cross-domain backing requires additional protocol |
| Wired constraint      | Handler code must always be resident — no demand-paging for handler     |
| Failure modes         | Handler fault → undefined (typically crash)                             |
| Cross-domain sharing  | Complex; handler only has access to its own VSpace                      |

---

## 8. Summary Table

| System               | Who resolves fault           | IPC involved            | Latency (approx)               |
| -------------------- | ---------------------------- | ----------------------- | ------------------------------ |
| QNX Neutrino         | Kernel                       | No                      | Kernel path only               |
| EROS                 | Kernel (tree traversal)      | No (unless keeper)      | Kernel path only               |
| Coyotos (GPT keeper) | Hybrid                       | Yes (keeper invocation) | Capability invocation overhead |
| Original L4          | Pager thread                 | Yes — synchronous IPC   | ~20 µs (i486, 1993)            |
| seL4                 | Fault handler thread         | Yes — endpoint IPC      | ~700–2000 cycles (A57)         |
| Mach                 | External pager (port)        | Yes — message port      | ~460 µs (i486, 1993)           |
| Zircon               | Pager service (port)         | Yes — async port        | Not published                  |
| Barrelfish           | Faulting dispatcher (upcall) | No (same process)       | ~domain switch overhead        |
| Nemesis              | Faulting domain (activation) | No (upcall in-domain)   | 100–200 ns                     |
| Linux userfaultfd    | Userspace handler thread     | Yes (fd, ioctl)         | 1.85–7.8 µs (x86)              |

---

## References

- Liedtke, J. "Improving IPC by Kernel Design." _SOSP 1993_.
- Shapiro, J. et al. "EROS: A Fast Capability System." _SOSP 1999_.
- Coyotos Microkernel Specification. Shapiro, 2007.
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- seL4 Reference Manual v14.0.0. UNSW/NICTA/seL4 Foundation.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 Performance Benchmarks. https://sel4.systems/performance.html
- seL4 Fault Handlers Tutorial.
  https://docs.sel4.systems/Tutorials/fault-handlers.html
- L4 eXperimental Kernel Reference Manual v X.2.
  https://www.l4ka.org/l4ka/l4-x2-r7.pdf
- Draves, R. "Extending the Mach External Pager Interface."
  UW-CSE-90-09-05, 1990.
  https://dada.cs.washington.edu/research/tr/1990/09/UW-CSE-90-09-05.pdf
- GNU Hurd external pager mechanism.
  https://www.gnu.org/software/hurd/microkernel/mach/external_pager_mechanism.html
- Zircon Pager kernel object reference.
  https://fuchsia.dev/fuchsia-src/reference/kernel_objects/pager
- RFC-0226: Zircon Pager Writeback.
  https://fuchsia.dev/fuchsia-src/contribute/governance/rfcs/0226_zircon_pager_writeback
- Leslie, I. et al. "Nemesis: a service-oriented operating system for
  multimedia." _USENIX ATC 1996_.
- Barrelfish Architecture Overview. TN-000. barrelfish.org.
- Gerber, S. "Virtual Memory in a Multikernel — The Barrelfish OS." Master's
  thesis, ETH Zurich, 2012.
- Hand, S. et al. "Are Virtual Machine Monitors Microkernels Done Right?" _HotOS
  2005_. https://www.cs.utexas.edu/~witchel/380L/papers/hand05hotos-vm-micro.pdf
- Watkins, N. "Measuring userfaultfd page-fault latency." 2016.
  https://makedist.com/posts/2016/10/10/measuring-userfaultfd-page-fault-latency/
- userfaultfd-wp latency measurements.
  https://xzpeter.org/userfaultfd-wp-latency-measurements/
