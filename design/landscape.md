# Kernel Design Landscape

A reference document surveying how real microkernels and academic systems have
resolved the design decisions this kernel will face. Organized by decision
point, not by system. For each decision: what are the known approaches, who
chose what and why, what are the tradeoffs, and where might novelty lie.

**Systems referenced throughout:** seL4, L4 family (L4Ka::Pistachio, Fiasco.OC,
NOVA), EROS/Coyotos/KeyKOS, Genode, QNX, Plan 9/Inferno, Barrelfish, Redox,
Minix 3, Zircon/Fuchsia, Mach/Hurd, Spring OS, Singularity/Midori, Composite OS,
Hydra, Nemesis, CHERI/Morello, Capsicum.

**How to read this document:** Each section is self-contained. Start with
whichever area is relevant to the current design question. The
[Novelty Opportunities](#novelty-opportunities) section at the end synthesizes
cross-cutting ideas specific to this kernel's design.

**Relationship to claims.toml:** This document is _input_ to design decisions,
not _output_. When a decision is made, it goes in `claims.toml` with rationale.
This document provides the landscape that informed the decision.

---

## Table of Contents

1. [Capability Model](#1-capability-model)
2. [Memory Management](#2-memory-management)
3. [IPC: Inter-Process Communication](#3-ipc-inter-process-communication)
4. [Scheduling](#4-scheduling)
5. [Interrupt & Fault Handling](#5-interrupt--fault-handling)
6. [Naming, Namespaces & Process Model](#6-naming-namespaces--process-model)
7. [Boot Protocol & Early Initialization](#7-boot-protocol--early-initialization)
8. [Novelty Opportunities](#8-novelty-opportunities)
9. [References](#9-references)

---

## 1. Capability Model

### 1.1 Capability Representation (Handles, Keys, Tokens)

Systems diverge on where capabilities live and how they are named. **seL4** uses
a segregated capability model: capabilities reside in kernel-managed CNode
objects (arrays of typed slots), and a thread's capability space (CSpace) is a
directed graph of CNodes rooted at a CNode capability stored in the thread's
TCB. Capability addresses are integers indexing into this graph. This makes the
naming scheme explicit and verifiable but forces userspace to manage CNode
allocation and tree structure. **Zircon** takes the simpler approach:
capabilities are 32-bit integer handles local to a process, backed by a
kernel-side handle table that maps each handle to a kernel object pointer plus a
rights mask. No user-visible CNode structure exists -- the kernel table is
opaque.

**KeyKOS** and **EROS** use "keys" -- capabilities stored in 16-slot nodes.
Nodes are the only structuring primitive: a segment is a tree of nodes with
pages at the leaves, and a domain (process) is three nodes holding keys to its
address space, capabilities, and scheduling meter. Keys are typed (page key,
node key, segment key, start key, resume key, meter key, etc.), and the type
determines what operations the key permits. **Barrelfish** adapts seL4's CNode
model for its multikernel architecture, adding cross-core capability replication
via user-mode monitors. **Capsicum** (FreeBSD) repurposes UNIX file descriptors
as capabilities: after entering capability mode, a process can only operate on
file descriptors it already holds, each decorated with a rights mask. **CHERI**
takes a radically different path: capabilities are 128-bit tagged values in
hardware registers and memory, carrying bounds, permissions, and an integrity
tag bit. The tag cannot be forged by software -- any non-capability store to a
capability location clears the tag. This is the only system where capability
representation is a hardware primitive.

### 1.2 Capability vs. ACL-Based Access Control

The fundamental argument for capabilities over ACLs is the confused deputy
problem, identified by Norm Hardy at KeyKOS (1988). A deputy holding ambient
authority from an ACL cannot distinguish "acting on behalf of client A" from
"acting on behalf of client B." Capabilities solve this structurally: the client
passes a capability designating the specific resource, so designation and
authority are bundled. No ambient authority exists to be confused about.

Mark Miller's work formalizes this: capabilities eliminate ambient authority by
requiring explicit passing of authority for every resource access, making least
privilege the default. Tyler Close's "ACLs Don't" paper and Miller, Yee, and
Shapiro's "Capability Myths Demolished" (2003) argue that the critical property
is whether the system enforces "if you didn't receive a reference to it, you
can't name it." Systems with global namespaces (file paths, PIDs) leak ambient
authority; capability systems replace global names with unforgeable local
references.

### 1.3 Capability Transfer and Delegation

**seL4** transfers capabilities via IPC: the kernel copies capabilities from
sender's CSpace into receiver's CSpace. The sender can "mint" a copy with a
badge (an integer label) and optionally reduced rights. **Zircon** transfers
handles through channels: `zx_channel_write()` _moves_ handles from the calling
process's handle table into the channel, and `zx_channel_read()` moves them out.
Handles are moved, not copied. **EROS/KeyKOS** transfer keys as part of IPC
invocation. **Genode** mediates transfer through the component tree --
capability routing rather than direct transfer, where every intermediate node is
an explicit policy point. **Plan 9** achieves similar effects through
per-process namespaces: a parent constructs a child's namespace by mounting
specific file servers.

### 1.4 Capability Revocation

The hardest problem in capability design. **seL4** maintains a capability
derivation tree (CDT) tracking every copy and mint. `seL4_CNode_Revoke()`
deletes all derived capabilities recursively. Powerful but expensive: the CDT is
a global kernel data structure. **Zircon** takes the simplest approach: close
the handle. No mechanism to revoke transferred handles -- revocation is indirect
via destroying the underlying object. **EROS/KeyKOS** use the
constructor/factory pattern: a factory retains a "yield" key that can destroy
the entire subsystem it created. Revocation is architectural: destroy the
container.

The deeper issue: fine-grained selective revocation is inherently expensive
because it requires tracking the complete graph of who holds what. Systems
either pay this cost (seL4), avoid the problem (Zircon), or structure so
coarse-grained revocation suffices (EROS). Generation numbers offer a middle
ground -- incrementing an object's generation invalidates all old capabilities
-- but require validation on every use.

### 1.5 Capability Granularity

**seL4** attaches a rights mask (Read, Write, Grant, GrantReply, etc.) to each
capability. **Zircon** similarly attaches rights (ZX_RIGHT_READ, ZX_RIGHT_WRITE,
ZX_RIGHT_DUPLICATE, ZX_RIGHT_TRANSFER, etc.) to handles. **KeyKOS** achieves
granularity through key typing: a segment key vs. a node key provides different
operations. **Capsicum** applies over 60 specific rights at the file descriptor
level. **CHERI** provides per-pointer, per-access granularity enforced by
hardware.

### 1.6 Capability Composition and Attenuation

Attenuation -- deriving weaker from stronger -- is universal: seL4 minting,
Zircon `zx_handle_duplicate()` with reduced rights, CHERI hardware-enforced
monotonic narrowing. Composition diverges more: **Genode**'s session routing
lets parents interpose on any child's service access. **EROS/KeyKOS** achieve
composition through the constructor pattern. Miller's object-capability patterns
(membranes, caretakers, sealers/unsealers) provide a library of compositional
security abstractions. **Composite OS** uses thread migration, making capability
invocation look like a function call with automatic accounting.

### 1.7 Bootstrapping the Capability Space

The "first capability" problem. **seL4** gives the root task capabilities to all
resources at boot via BootInfo. **Zircon** embeds userboot in the kernel; it
receives handles via a bootstrap channel. **EROS/KeyKOS** sidestep bootstrapping
through orthogonal persistence -- the entire capability graph is checkpointed
and restored. **Genode** bootstraps recursively: core holds all resources,
starts init, which reads XML configuration and delegates.

### 1.8 Hardware Capabilities (CHERI)

**CHERI** extends conventional ISAs with hardware-enforced 128-bit capabilities
carrying address, bounds, permissions, and integrity tag. The **Arm Morello**
board is a prototype SoC implementing CHERI on Armv8-A. CheriBSD demonstrates
two compartmentalization models: library compartmentalization (c18n,
per-shared-library protection domains) and co-processes (multiple logical
processes sharing one address space, separated by CHERI instead of MMU,
achieving 1-2 orders of magnitude faster switching).

Current Arm silicon does not include CHERI, but the CHERI Alliance (Arm, Google,
Microsoft) signals intent toward productionization. A kernel designed today
should ensure its handle model does not conflict with CHERI: handles (software
capabilities) and CHERI capabilities (hardware capabilities) should be
complementary.

---

## 2. Memory Management

### 2.1 Memory Object Model

The surveyed systems fall into four families:

**Mach/Zircon (VM objects).** Mach introduced memory objects: kernel-managed
containers backed by pagers. Zircon modernizes this with VMOs: created with
`zx_vmo_create`, mapped via `zx_vmar_map`, cloned for COW. VMO sizes round up to
page size.

**seL4/Barrelfish (typed capabilities from untypeds).** All physical memory
starts as untyped capabilities. `seL4_Untyped_Retype` carves regions into typed
kernel objects. A watermark tracks the allocation frontier. The kernel records a
capability derivation tree for revocation. Memory accounting is fully explicit
and delegated to userspace. Barrelfish follows a similar model.

**L4/Genode (flexpages/dataspaces).** L4 uses flexpages: power-of-two-sized,
naturally-aligned virtual regions distributed via IPC (map, grant, flush).
Sigma0 owns all physical memory. Genode exposes dataspaces: contiguous physical
regions allocated from quota-bounded RAM sessions, attached to address spaces
via capabilities.

**EROS/KeyKOS (persistent store).** Two primitive types: pages (4096 bytes) and
nodes (16 capability slots). Address spaces are trees of nodes with page leaves.
Memory allocated from hierarchical space banks. Objects are persistent by
default.

Also notable: **Composite** unifies capability tables and page tables. **QNX**
uses POSIX `shm_open` + `mmap`. **Plan 9** uses named segments. **Redox** uses
scheme-based memory. **Minix 3** runs a userspace VM server.

### 2.2 Physical Memory Allocation Authority

Most systems use **kernel-managed allocation** (Mach, QNX, Zircon, Minix 3,
Redox, Plan 9). **seL4** is the extreme opposite: **userspace-managed**. The
kernel has no allocator after boot; all memory is untyped capabilities handed to
the root task. Motivated by formal verification (no kernel memory leaks by
construction) and userspace control. **Barrelfish** and **Composite** follow
similar user-managed models. **Genode** uses a hybrid: core converts physical
memory to dataspaces, but allocation policy flows through quota-bounded RAM
sessions. **Nemesis** has a kernel frames allocator but gives each application
guaranteed frames plus revocable optimistic frames.

### 2.3 Create vs. Map Separation

The **two-step model** is dominant. **Zircon**: `zx_vmo_create` then
`zx_vmar_map`. **Genode**: allocate dataspace then attach. **Mach**: create
memory object then `vm_map`. **seL4**: retype frame then map to page table. The
advantage: objects exist independently of mappings, enabling sharing, COW, and
clean lifecycle.

**L4 flexpages** are a partial exception (the flexpage is both memory and
mapping authority). **Plan 9** `segattach` combines both steps. **EROS** objects
exist in the persistent store; mapping means constructing an address space tree
referencing them.

### 2.4 Demand Paging and Fault Handling

Three patterns: **External pagers** (Mach, L4, seL4) delegate fault handling to
userspace. In L4, page faults become IPC messages to the thread's pager. In
seL4, faults go to a designated fault handler endpoint. **Kernel-internal
paging** (QNX, Redox) handles faults within the kernel. **Self-paging**
(Nemesis) makes each application responsible for its own faults using its own
physical frame allocation, eliminating cross-application interference.

See also [Section 5.3: Fault Delivery Mechanism](#53-fault-delivery-mechanism)
for the interrupt/fault dispatch side.

### 2.5 Copy-on-Write Semantics

**Mach** pioneered aggressive COW with shadow objects (causing shadow chain
problems). **Zircon** modernized it with VMO cloning (`ZX_VMO_CHILD_SNAPSHOT`).
**seL4** deliberately excludes COW from the kernel -- userspace pagers could
implement it. The key question: kernel mechanism or userspace policy?
Mach/Zircon build it in because shadow/clone infrastructure requires page-table
manipulation on every fault.

### 2.6 Overcommit and OOM Policy

**seL4**: no overcommit by design (formally verified). **QNX**: committed at
allocation time. **EROS/KeyKOS**: space bank quotas. **Zircon**: overcommits
with memory pressure signals and OOM reboot. **Nemesis**: guaranteed frames
(immune from revocation) plus optimistic frames (revocable). **Genode**:
quota-bounded RAM sessions.

Three viable positions for this kernel: (1) no-overcommit with reservation
accounting (QNX model), (2) overcommit with pressure signals (Zircon model), (3)
per-object/per-session quotas (Genode/Nemesis model). See claim
`overcommit-policy-open`.

### 2.7 Page Size Exposure

**Nearly every surveyed system exposes page size to userspace.** Zircon provides
`zx_system_get_page_size()`. seL4 exposes frame sizes directly. L4 flexpages are
inherently granularity-exposing. Mach, QNX, Plan 9, Minix 3, Redox all expose
`PAGE_SIZE`.

**This kernel's decision to hide page size is genuinely novel.** No surveyed
system fully hides it. The closest is Genode (which abstracts alignment behind
dataspaces). ARM64 supports 4K, 16K, and 64K base pages, with contiguous PTE
hints for larger mappings. Hiding page size requires the kernel to absorb
alignment, tail-waste, and large-page promotion -- complexity every other system
pushes to userspace. The payoff: an interface that survives page-size changes
without ABI breaks.

### 2.8 Cache Coloring and NUMA

Most microkernels ignore physical topology. **Barrelfish** is the notable
exception (designed for it). For ARM64 personal devices: cache line size
differences between big/LITTLE cores (128-byte on big, 64-byte on LITTLE in some
SoCs), cache coloring for deterministic performance, and memory controller
interleaving are relevant. Apple Silicon's unified memory makes NUMA irrelevant
but cache pressure still meaningful.

---

## 3. IPC: Inter-Process Communication

### 3.1 Synchronous vs. Asynchronous IPC

**Synchronous (rendezvous)** blocks the sender until the receiver accepts. No
kernel queues. seL4, L4 family, QNX, EROS, Spring all use synchronous IPC as the
primary primitive. Rationale: no buffering policy, no unbounded resource
consumption, enables direct process switch (sender-to-receiver without
scheduler).

**Asynchronous** decouples sender and receiver. Mach uses port-based queues.
Zircon channels are async. Singularity channels are async with
compile-time-verified contracts. Barrelfish uses async as its fundamental
inter-core primitive.

**Performance:** L4 achieved 5us vs. Mach's 114us on identical hardware
(486DX-50) -- 20x difference. seL4 on ARM64 Cortex-A57: 416 cycles one-way via
fastpath.

**Converged wisdom:** synchronous for same-core control flow, async notification
for event signaling and cross-core coordination. Every mature system ends up
with both (seL4 has endpoints + notifications, Zircon has channels + signals,
QNX has messages + pulses).

### 3.2 Message Passing vs. Shared Memory vs. Hybrid

Every production microkernel converges on **hybrid**. Genode documents this most
explicitly: (1) synchronous RPC for control + capability delegation, (2) async
notifications for signaling, (3) shared-memory dataspaces for bulk data. The
canonical pattern: use IPC to set up shared memory, exchange a capability to it,
use notifications to signal data availability. IPC is the control plane; shared
memory is the data plane.

Heiser's dictum: "IPC is a user-controlled context switch with benefits" -- it
should never carry bulk data. seL4 messages are explicitly small (fits in
registers). This aligns with claim `ipc-mechanism-deferred`'s three composable
primitives.

### 3.3 IPC Object Model

**Mach ports:** unidirectional async queues with send/receive rights. **seL4
endpoints:** rendezvous points (not queues). Any number of threads can
send/receive. Badge identifies sender. Separate notification objects and
one-shot reply capabilities. **Zircon channels:** bidirectional, queued,
two-endpoint. Ports aggregate signals from multiple objects. **Spring doors:**
cross-domain procedure call entry points (100-instruction round-trip on SPARC).
**EROS:** capability invocation IS IPC. **Singularity:** channels with
compile-time-verified state machine contracts. **Plan 9 / 9P:** everything is a
file; IPC happens through file descriptors.

### 3.4 Fast Path / Register-Only IPC

**Liedtke's L4 (1993):** no single trick -- synergistic optimization at every
level. Messages in registers, direct process switch, straight-line code. **seL4
fastpath:** formally verified, 188 cycles on ARM11, 416 cycles on ARM64
Cortex-A57. Requirements: message fits in registers, no capability transfer, no
higher-priority threads runnable. **LRPC (Bershad, 1990):** shared argument
stack, thread migration into server domain, 3x improvement over conventional
RPC.

**Cache-working-set principle** (L4 "20 years" retrospective): the IPC path
should occupy ~2-3% of L1 cache. If it pollutes the cache, caller and callee pay
on every subsequent access.

### 3.5 Capability-Mediated IPC

Spectrum from pure (seL4, EROS -- no communication without explicit capability)
to hybrid (Zircon handles with rights attenuation). In seL4, badge on endpoint
capability identifies sender to receiver. Reply capabilities prevent
impersonation. In EROS, invoking a capability IS IPC. In Genode, capabilities
are typed and sessions are capabilities. Key property: no ambient authority.

### 3.6 Notification / Signal Mechanisms

**seL4 notifications:** word-sized bitmap. Signaling OR's the badge in.
Critically, notifications can be bound to a TCB -- signals delivered even when
blocking on endpoint receive, letting one thread handle both IPC and async
events. **Zircon signals:** 32-bit bitmask per object, aggregated via ports
(epoll analog). **QNX pulses:** fixed-size non-blocking messages (8-bit code +
64-bit data). **Genode signals:** zero-payload, fire-and-forget.

Tradeoff: bitmap notifications (cheapest, coalesce) vs. queued notifications
(preserve per-event data, require buffers).

---

## 4. Scheduling

### 4.1 Kernel-Owned vs. Userspace Policy

**Kernel-owned** is dominant in production: QNX, Linux, Zircon, FreeBSD,
Windows, macOS. **seL4 MCS** introduced scheduling-context capabilities --
first-class objects with budget/period that transfer via IPC -- but the kernel
still enforces sporadic-server budgets and runs the dispatcher. **Composite OS**
goes furthest: no kernel scheduler at all, temporal capabilities enable
hierarchical user-level schedulers. **Scheduler Activations** (Anderson, 1991)
attempted kernel-to-userspace upcalls; NetBSD implemented then abandoned them.

**Why convergence on kernel scheduling:** scheduling decisions are triggered by
privileged events the kernel already handles. Pushing policy out requires
upcalling on every event (costly) or batching (loses responsiveness). See claim
`kernel-owns-scheduling-policy`.

### 4.2 Scheduling Algorithm

**Fixed-priority preemptive:** QNX (256 levels, POSIX SCHED_FIFO/RR), seL4
(non-MCS), L4 family. **Fair-share:** Linux CFS (2007-2023, red-black tree by
virtual runtime), Linux EEVDF (kernel 6.6+, virtual deadlines -- removed CFS
heuristics, ~30% UI latency improvement), Zircon WFQ (balanced tree by virtual
finish time). **Lottery/stride** (Waldspurger, 1994): proportional-share via
randomized tickets. Influenced all subsequent fair-share schedulers but no
production use. **Deadline-based:** Nemesis Atropos (EDF), seL4 MCS (sporadic
servers), Zircon deadline profiles, Linux SCHED_DEADLINE (CBS on EDF).

**Hybrid is the norm:** Zircon runs deadline > fair. QNX layers adaptive
partitioning on fixed-priority. Linux runs SCHED_DEADLINE > SCHED_FIFO/RR >
EEVDF > idle.

### 4.3 Multicore Scheduling

**Per-core run queues** are universal (Linux, FreeBSD, Zircon, Windows). A
global queue creates contention bottlenecks. **Load balancing** is the hard
problem: Lozi et al. ("Decade of Wasted Cores," EuroSys 2016) found four bugs in
CFS that caused idle cores alongside overloaded ones. Work stealing is secondary
to affinity-preserving balancing.

**ARM big.LITTLE/DynamIQ** adds core asymmetry. **Linux EAS** (Energy Aware
Scheduling, mainline since 5.0) uses an Energy Model mapping frequency/capacity
to power. **macOS** uses QoS classes for P-core vs. E-core placement. **Android
ADPF** adds app-level performance hints. For a personal device kernel,
heterogeneous scheduling is not optional.

### 4.4 Thread vs. Process as Schedulable Unit

All systems schedule threads. The difference is binding: **seL4** fully
separates TCBs from VSpaces and scheduling contexts. **QNX/Linux** bundle
threads in processes. **Barrelfish** schedules dispatchers (user-level entities
managing internal threads).

This kernel's claim (`thread-is-schedulable-unit`) aligns with seL4/L4
separation. The implementation question: scheduling parameters on the thread
object vs. separate capability (seL4 MCS scheduling contexts).

### 4.5 Priority Inversion Handling

**Priority inheritance** (QNX, Linux RT mutexes): holder inherits waiter's
priority. Standard solution. **Priority ceiling** (PCP): pre-assign ceiling
priority to each mutex. Bounds blocking but requires static analysis. **Random
boosting** (Windows): probabilistic, works for interactive responsiveness but no
formal guarantee. **seL4 MCS**: scheduling-context donation during IPC naturally
prevents inversion on client-server paths.

Mars Pathfinder (1997) is the canonical case study: priority inheritance was
available but disabled on the offending mutex.

### 4.6 Real-Time Guarantees

**For personal interactive devices, the relevant requirements are soft-RT:**

- Audio: 5-10ms round-trip (256 samples at 48kHz is ~5.3ms)
- Touch input: under 50ms (Apple Pencil targets 8-16ms)
- Display: 16.67ms at 60Hz, 8.33ms at 120Hz

A deadline-based scheduler with capacity/period budgets is well-suited: audio
threads get guaranteed budgets, compositor gets frame-aligned deadlines,
everything else runs fair-share. Hard-RT verification is not needed.

### 4.7 Energy-Aware Scheduling

**Linux EAS** evaluates energy cost of each placement decision using an Energy
Model. **macOS** maps QoS classes to P-cores/E-cores. **Android ADPF** layers
app-level performance hints.

For a microkernel: threads carry a QoS/energy hint (latency-sensitive /
throughput / efficiency), the kernel maps to hardware reality. The energy model
is a kernel-internal leaf node behind a stable interface -- critical given ARM
topologies change every generation.

### 4.8 Interactive Responsiveness

Three requirements: (1) input events preempt immediately, (2) compositor never
misses frame deadlines, (3) background work doesn't starve interactive threads.
**macOS QoS tiers** (userInteractive > userInitiated > utility > background).
**EEVDF** inherently improves latency via virtual deadlines. **Zircon's**
two-tier model (deadline + fair) handles all three directly.

---

## 5. Interrupt & Fault Handling

### 5.1 Kernel-Handled vs. Userspace Interrupts

**Minimal kernel + notification (L4/seL4/Minix 3/Redox):** mask, signal
notification, wait for ack before unmask. **ISR-in-userspace (QNX):**
`InterruptAttach()` runs user-mode ISR at interrupt priority in the driver's
address space -- monolithic latency with microkernel modularity. **Signal-based
(Genode/NOVA):** interrupts as signals/semaphores. **Per-core message
(Barrelfish):** treats interrupts as a distributed systems problem.

**What stays in-kernel universally:** masking/unmasking, EOI, preemption timer.
No microkernel delegates the preemption timer.

### 5.2 Interrupt Object Model

**seL4:** IRQHandler + Notification binding with badges. Multiple interrupts
multiplex onto one Notification via OR'd badges. Elegant: reuses general-purpose
notification primitive. **Zircon:** first-class interrupt objects bound to
ports. More specialized. **L4Re:** IRQ objects bound to threads. **QNX/Minix
3:** integer IRQ IDs with kernel calls. **Redox:** file descriptors (`irq:N`).
**NOVA:** Interrupt Semaphore Descriptors.

### 5.3 Fault Delivery Mechanism

**seL4 fault endpoints:** kernel acts as IPC sender, delivering fault type +
context. Handler replies to resume faulting thread. Purest capability-based
model. **Zircon exception channels:** hierarchical (thread > process > job),
debug channels separate. **Plan 9 notes:** string-based async messages. **POSIX
signals:** SIGSEGV/SIGBUS/SIGFPE. Well-understood but reentrancy and
async-signal-safety constraints. **Minix 3:** kernel sends notification to VM
server.

### 5.4 Exception Vector and Dispatch (ARM64)

ARM64's VBAR_EL1: 4x4 matrix (4 source contexts x 4 exception types). Each entry
is 128 bytes (32 instructions). **Minimal save + ESR dispatch** (seL4, most
microkernels): save a few scratch registers, read ESR_EL1 to classify, branch to
handler. **Full save** (Linux): entire register set up front, simplifies handler
code. **Lazy save** (optimization): only caller-saved registers in fast path.

Key registers: ESR_EL1 (exception class + syndrome), FAR_EL1 (faulting address),
ELR_EL1 (return address), SPSR_EL1 (saved PSTATE), DAIF bits (mask Debug,
SError, IRQ, FIQ).

### 5.5 Interrupt Priority and Preemption

GICv3 provides 8-bit hardware priority. **Linux** largely ignores it, using
PSTATE.I for coarse masking. **Practical microkernel approach:** PSTATE.I/F for
coarse masking (context switch), ICC_PMR_EL1 for fine-grained priority. GIC
Group 0 (FIQ) / Group 1 (IRQ) gives a free hardware two-level scheme.

Most microkernels keep the in-kernel path so short that nesting provides
negligible benefit. Blackham et al. (EuroSys 2012) showed even a non-preemptible
seL4 achieves 10k-100k cycle worst-case latency.

### 5.6 Deferred Processing

Linux has elaborate softirq/tasklet/workqueue/threaded-IRQ layering.
**Microkernels dissolve the problem:** the kernel's path is
mask-signal-EOI-return; the scheduler IS the deferred processing mechanism. The
entire "bottom half" is the userspace driver thread.

### 5.7 ARM64 GICv3/v4 Specifics

**GICv3 components:** Distributor (GICD, SPIs), Redistributors (GICR,
PPIs/SGIs/LPIs per PE), CPU Interface (ICC\_\* system registers). System
register interface reduces latency vs. GICv2 MMIO.

**Affinity routing:** MPIDR-based hierarchical routing. `GICD_IROUTER` per SPI.
Simpler on fixed-topology personal devices.

**LPIs and ITS:** Message-signaled interrupts. Peripheral writes to ITS
doorbell, ITS translates device ID + event ID to target. Configuration in memory
tables (scales to thousands).

**GICv4 direct virtual interrupt injection:** ITS maps physical events to
virtual interrupts, injects into running vPE without hypervisor intervention.
GICv4.1 adds virtual SGIs. Doorbell interrupt when vPE not scheduled. Designed
for virtualization but has microkernel implications.

### 5.8 Timer and Preemption

Modern consensus: **one-shot / tickless**. Program ARM generic timer
(CNTP_TVAL_EL0) for next event. Between events, no interrupts. Linux
(NO_HZ_FULL), Zircon, and seL4 MCS all use one-shot. **seL4 MCS** ties timers to
scheduling-context capabilities with sporadic server enforcement.

---

## 6. Naming, Namespaces & Process Model

### 6.1 Process Model

Spectrum from composed to bundled:

**No kernel process concept.** seL4: "process" is a userspace convention (TCB +
VSpace + CSpace wired together). Barrelfish: dispatchers bound to address spaces
independently. Composite OS: all components independent, no kernel bundling.
Hydra (1974): every resource an object addressed through capabilities.

**Lightweight container.** Zircon: process = address space (root VMAR) + handle
table. Jobs group processes hierarchically. Mach: task = address space + port
rights.

**POSIX-compatible.** QNX, Minix 3: full POSIX processes. Redox: POSIX-style but
with URL-scheme namespaces.

**Software-isolated.** Singularity/Midori: SIPs provide process isolation
through language safety, not hardware. Extremely lightweight (no page table
switch).

**Component tree.** Genode: parent-child tree, parent controls all child
resources. EROS: constructors stamp out instances with embedded capabilities.

### 6.2 Naming Architecture

**Capability-only (no string names in kernel).** seL4, EROS, KeyKOS, Barrelfish,
Hydra, Composite. A capability IS the name. No registries, no path lookups.

**Hierarchical filesystem.** Plan 9 (everything is a file, per-process namespace
via bind/mount), Inferno (Styx/9P2000), Hurd (translators on filesystem nodes),
QNX (pathname space with resource managers).

**Flat handles + userspace namespace.** Zircon: kernel knows only handles;
userspace component framework provides string-based `svc/` directory. Deliberate
split.

**URL-scheme naming.** Redox: `scheme:path` where schemes are per-process.

### 6.3 Per-Process vs. Global Namespace

**Per-process namespaces:** Plan 9/Inferno (bind/mount affect only calling
process), Redox (per-process schemes). **Capability spaces are inherently
per-process:** seL4 CSpace, Zircon handle table, Barrelfish per-core capability
space. **Parent-mediated sandboxing:** Genode (component sees only what parent
grants), Singularity (manifest-declared dependencies).

### 6.4 Service Discovery and Binding

**Parent-mediated introduction:** Genode routes session requests through parent
chain. seL4: root server hands capabilities to children. **Namespace-based:**
Zircon `svc/` directory, QNX `name_attach()`, Hurd filesystem paths.
**Constructor-embedded:** EROS bakes initial capabilities into constructors.
**Manifest-declared:** Singularity verifies channel dependencies at install
time.

### 6.5 Thread-Address Space Binding

**Fixed binding (most systems).** seL4 binds TCB to VSpace at configuration
time. Zircon, QNX, EROS, Barrelfish: threads belong to an address space.

**Migrating threads.** Spring doors: calling thread migrates into server domain.
LRPC (Bershad, 1990): 3x improvement via thread migration. Composite OS: thread
migration as primary IPC, one schedulable entity across domains. L4's "direct
process switch" is a limited form.

For a kernel where threads are already independent of address spaces, thread
migration is a natural extension rather than a special case. See
[Novelty: Thread Migration](#thread-migration-as-the-general-case).

### 6.6 Address Space Management

**Userspace-managed + kernel primitives:** seL4 (allocate page table objects,
map pages explicitly), Barrelfish. **Kernel-managed + userspace policy:** Zircon
(VMARs as tree), QNX (POSIX mmap). **Component-mediated:** Genode
(parent-delegated), Composite (user-level). **Software-defined:** Singularity
(single hardware address space, language safety).

### 6.7 Task Lifecycle

**Create-then-configure (explicit assembly):** seL4 (allocate TCB, VSpace,
CSpace, bind, set registers, resume). Most flexible but most labor-intensive.
**Spawn:** Zircon (`zx_process_create` + `zx_process_start`), Redox (`clone`).
**Fork+exec:** QNX, Minix 3. **Constructor-based:** EROS (frozen image stamps
out instances). **Manifest-based:** Singularity (verified + compiled at install
time).

The central challenge across all systems: how does a new task get its initial
capabilities?

### 6.8 Resource Accounting

**Capability-mediated budgets:** seL4 (can only spend what you were given in
untypeds -- formally verified), Barrelfish. **Job-based limits:** Zircon
(hierarchical job tree with per-level limits). **Component-tree quotas:** Genode
(RAM + caps per child). **Process limits:** QNX/Minix 3 (POSIX rlimits).
**Language-enforced:** Singularity (per-SIP heap, no shared mutable state).

---

## 7. Boot Protocol & Early Initialization

### 7.1 Firmware-to-Kernel Handoff

**Linux arm64:** 64-byte header, devicetree pointer in x0, MMU off, interrupts
off, enter at EL2 or EL1. **seL4:** elfloader intermediary (follows Linux
convention, unpacks kernel + root task, enables MMU, boots secondary cores,
jumps to kernel). **Zircon:** receives ZBI (typed container format) from
bootloader; passes it as VMO to userboot. **QNX:** IPL -> startup -> system page
-> procnto. **L4Re:** positional boot modules via multiboot. **UEFI:**
GetMemoryMap() then ExitBootServices(); rich discovery but leaves MMU enabled.

For ARM64 personal devices, devicetree is the expected hardware description
path.

### 7.2 Initial Process / Root Task

**seL4 rootserver:** receives capabilities to ALL physical resources via
BootInfo. Must subdivide and delegate everything. Purest capability delegation
-- kernel hands off everything.

**Zircon userboot:** embedded in kernel binary. Started using normal process
protocol (no special case). Receives handles via bootstrap message. Loads
component_manager, exits. Not privileged -- just has handles.

**QNX procnto:** kernel + process manager monolith. Avoids chicken-and-egg by
merging them.

**Genode core/init:** two-level (core provides resources, init provides policy
via XML).

**L4Re sigma0/Moe:** sigma0 is root pager, Moe is root task.

**EROS/KeyKOS:** orthogonal persistence eliminates traditional boot entirely.

### 7.3 Multicore Bringup

**PSCI** (Power State Coordination Interface):
`CPU_ON(target_cpu, entry_point, context_id)` via HVC/SMC. Modern ARM64
standard. **Spin tables:** secondary cores poll memory location, primary writes
entry point. Simpler but wastes power. **ACPI parking protocol:** per-CPU
mailbox pages (server platforms).

Under Apple Hypervisor.framework, the VMM controls vCPU creation. PSCI-via-HVC
emulation is the clean abstraction. Simplest path: start with single core, defer
multicore.

### 7.4 Memory Discovery and Initial Page Tables

Memory discovery: devicetree `/memory` nodes, UEFI GetMemoryMap(), or
boot-provided structure. **Initial page tables:** identity map (VA = PA) for
MMU-enable trampoline in TTBR0, kernel mapping in TTBR1. After MMU on and kernel
at high VA, identity map dropped.

Under Hypervisor.framework: VMM controls guest physical space. Can write a
minimal BootInfo struct to a known address, bypassing devicetree parsing
entirely.

### 7.5 Driver Initialization

The bootstrap dependency: microkernel drivers are userspace, but you need
drivers to load userspace. Universal solution: **embed enough in the boot image
to reach persistent storage.** Nearly every microkernel includes a minimal UART
compiled into the kernel for boot diagnostics (pragmatic violation of
"everything in userspace"). First process is always in the boot image (no
disk/network needed).

### 7.6 Boot Image Format

**Single ELF:** seL4 (elfloader with CPIO archive), Genode. **Kernel + data
blob:** Zircon (kernel + ZBI container). **Kernel + initramfs + devicetree:**
Linux. **Image Filesystem:** QNX (IFS -- browsable). **Checkpoint:**
EROS/KeyKOS.

ZBI-style (kernel + typed container) offers the best tradeoff for a new kernel:
self-describing, kernel parses minimally, hands off to userspace.

---

## 8. Novelty Opportunities

Cross-cutting ideas specific to this kernel's design -- memory objects with
hidden page size, create-then-map, demand paging, capability-mediated access,
threads independent of address spaces.

### Hidden Page Size (genuinely novel)

No surveyed system fully hides page size from userspace. Every existing
microkernel exposes it as a constant, frame-size parameter, or alignment
requirement. This kernel's hidden-page-size decision means the interface
survives page-size changes (4K -> 16K -> 64K on ARM64) without ABI breaks --
something Linux is currently struggling with. The kernel absorbs alignment,
tail-waste, and large-page promotion as internal optimizations. Combined with
demand paging, the kernel can choose commit granularity transparently (4K for
cold objects, 2M for hot) without userspace knowledge.

### Byte-Granularity Objects with Kernel-Managed Packing

seL4 has fixed-size typed objects. Zircon VMOs are page-granular. EROS pages are
4096 bytes. This kernel's slab-packing model (small objects share a physical
page within one address space, promote to own pages when shared across) is
novel. The closest analogy is Linux's slab allocator, but that's an in-kernel
mechanism, not a userspace-visible object model.

### Capability-Memory Object Unification

Most systems keep capabilities and memory objects as separate abstractions with
a mapping between them. This kernel could unify them: a memory object IS a
capability target, the handle IS the capability, rights on the handle govern
both syscall access and mapping permissions. This avoids duplicating permission
checks and eliminates confused-deputy bugs where mapping permissions disagree
with handle rights.

### IPC Through the Memory Object Model

If memory objects are the kernel's primary abstraction, IPC channels could _be_
memory objects: a ring buffer mapped into two address spaces with
kernel-provided notification. The "channel" is a pattern over existing objects +
notifications, not a separate kernel type. Benefits: unified resource accounting
(buffer size in memory budget), demand-paged message buffers, consistent
capability model (authority over memory = authority to communicate). Similar to
Barrelfish's UMP but as a kernel-blessed pattern.

### Thread Migration as the General Case

Most systems treat thread migration as an optimization hack. In this kernel,
where threads are already independent of address spaces, migration is the
natural case: a thread executing IPC simply changes which address space it runs
in. Avoids server thread pool sizing, makes resource accounting trivial (one
thread, one budget, across domains). Spring and Composite explored this, but
neither started from thread-address-space independence as a first principle.

### Capability-Mediated Energy Scheduling

No existing system fully integrates energy-aware scheduling with capabilities. A
scheduling interface carrying both temporal parameters (budget/period) and
energy hints (latency-sensitive / throughput / efficiency) would let userspace
express intent without dictating core placement. The kernel maps intent to
hardware using an internal energy model -- a leaf node behind a stable
interface.

### GICv4 Direct Injection for Userspace Drivers

GICv4's virtual interrupt injection (designed for hypervisors) could be
repurposed: each userspace driver as a "vPE," device interrupts injected
directly without kernel involvement when the driver is running. Doorbell
interrupt for when it's not. Near-zero-overhead delivery for the common case.
Constraint: LPI-only (MSI/MSI-X devices), need split model for legacy SPIs.

### Revocation via Memory Object Lifecycle

Since capabilities tie to memory objects, revocation follows object lifecycle:
revoking = unmapping. When last handle closes, object destroyed, all mappings
invalidated via MMU. Simpler than seL4's CDT traversal, more principled than
Zircon's "just close it." EROS factory pattern informs "capability groups" where
destroying a group revokes everything it contains.

### Contract-Verified Channels via Rust's Type System

Singularity's compile-time channel contracts required C#. Rust's ownership +
type system could enforce similar contracts without a managed runtime: channel
types parameterized by state machines, borrow checker ensures linear use. The
kernel doesn't verify contracts (userspace concern), but the channel primitive
can be contract-friendly.

### QoS-as-First-Class-Object

Userspace declares QoS contracts ("this thread needs 2ms every 16.67ms for frame
rendering"). The kernel admits or rejects based on capacity. Both sides have a
formal agreement. Prevents oversubscription; guarantees admitted contracts.
Closer to Nemesis's QoS model but for personal devices where "applications" are
GUI services.

---

## 9. References

### Foundational Papers

- Dennis & Van Horn, "Programming Semantics for Multiprogrammed Computations"
  (CACM 1966) -- introduced capabilities
- Liedtke, "Improving IPC by Kernel Design" (SOSP 1993) -- 20x over Mach,
  founded modern microkernel IPC
- Liedtke, "On Micro-Kernel Construction" (SOSP 1995, ACM SIGOPS Hall of
  Fame 2015) -- construction discipline
- Bershad et al., "Lightweight Remote Procedure Call" (ACM TOCS 1990) -- thread
  migration, LRPC
- Hardy, "The Confused Deputies" (1988) -- capability argument against ACLs

### Formal Verification

- Klein et al., "seL4: Formal Verification of an OS Kernel" (SOSP 2009) -- first
  complete kernel proof
- Shapiro & Weber, "Verifying the EROS Confinement Mechanism" (IEEE S&P 2000) --
  capability confinement proof

### Scheduling

- Anderson et al., "Scheduler Activations" (SOSP 1991) -- kernel upcalls for
  user-level scheduling
- Waldspurger, "Lottery Scheduling" (OSDI 1994) -- proportional-share via
  tickets
- Stoica & Abdel-Wahab, "Earliest Eligible Virtual Deadline First" (1995) -- the
  algorithm Linux adopted
- Lyons et al., "Scheduling-Context Capabilities" (EuroSys 2018) -- seL4 MCS,
  CPU time as capability
- Lozi et al., "The Linux Scheduler: A Decade of Wasted Cores" (EuroSys 2016) --
  CFS load balancing bugs
- Kolivas, "Brain Fuck Scheduler" (2009) -- desktop-focused EEVDF variant

### Memory

- Elphinstone & Heiser, "From L3 to seL4: 20 Years of L4 Microkernels"
  (SOSP 2013) -- retrospective
- Shapiro & Smith, "EROS: A Fast Capability System" (SOSP 1999) --
  capability-based persistent memory
- Hand, "Self-Paging in the Nemesis OS" (OSDI 1999) -- per-app memory QoS

### Architecture & IPC

- Baumann et al., "The Multikernel" (SOSP 2009) -- Barrelfish, cores as
  distributed system
- Hunt & Larus, "Singularity: Rethinking the Software Stack" (2007) -- SIPs,
  typed channels
- Steinberg & Kauer, "NOVA" (EuroSys 2010) -- microhypervisor, semaphore-based
  interrupts
- Blackham et al., "Improving Interrupt Response Time" (EuroSys 2012) --
  non-preemptible kernel latency

### Capability Systems

- Levy, _Capability-Based Computer Systems_ (1984) -- definitive first-gen
  survey
- Miller, "Robust Composition" (PhD thesis 2006) -- object-capability patterns
- Miller, Yee & Shapiro, "Capability Myths Demolished" (2003) -- refuted common
  objections
- Watson et al., "CHERI: A Hybrid Capability-System Architecture" (IEEE
  S&P 2015)
- Watson et al., "CheriABI: Enforcing Valid Pointer Provenance" (ASPLOS 2019)

### Naming & Process Model

- Pike et al., "Plan 9 from Bell Labs" and "The Use of Name Spaces in Plan 9"
  (1995)
- Nelson et al., "A Uniform Name Service for Spring's UNIX Environment"
  (USENIX 1994)
- Feske, "Genode OS Framework Foundations" (continuously updated)
- Parmer, "The Case for Thread Migration" (OSPERT 2010)

### ARM64 Hardware

- [ARM GICv3/v4 Architecture Specification](https://developer.arm.com/documentation/ihi0069/latest)
- [ARM Generic Timer](https://developer.arm.com/documentation/102379/latest)
- [ARM PSCI Specification](https://developer.arm.com/documentation/den0022/latest)
- [ARM Exception Model](https://developer.arm.com/documentation/100933/latest)
