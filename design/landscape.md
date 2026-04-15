# Kernel Design Landscape

A reference document surveying how real microkernels and academic systems have
resolved the design decisions a new microkernel typically faces. Organized by
decision point, not by system. For each decision: what are the known approaches,
who chose what, what are the tradeoffs.

**Depth and role.** This is a **survey-depth jumping-off point**, not a
research-depth reference. Expect paragraph-per-system summaries across ~50
subsections. Use this document to orient across the problem space and to
identify which systems and papers to study further for a specific question. For
research-depth on a specific derivation question, a targeted document under
`design/research/` should be produced — either cold (before a decision is on the
table) or at the moment the question becomes live. `design/research/ CLAUDE.md`
defines the descriptive-only rule those documents must follow.

**Systems referenced throughout:** seL4, L4 family (L4Ka::Pistachio, Fiasco.OC,
NOVA), EROS/Coyotos/KeyKOS, Genode, QNX, Plan 9/Inferno, Barrelfish, Redox,
Minix 3, Zircon/Fuchsia, Mach/Hurd, Spring OS, Singularity/Midori, Composite OS,
Hydra, Nemesis, CHERI/Morello, Capsicum.

**How to read this document:** Each section is self-contained. Start with
whichever area is relevant to the current design question. The annotated
[References](#8-references) section points at specific papers to read for depth
on each major theme.

**Relationship to spec.md:** This document is _input_ to design decisions, not
_output_. Nothing here prescribes what this kernel should do — when a decision
is made, it goes in `design/spec.md` with rationale (and a journal entry
recording how it was derived). This document surveys the landscape that informed
the decision.

---

## Table of Contents

1. [Capability Model](#1-capability-model)
2. [Memory Management](#2-memory-management)
3. [IPC: Inter-Process Communication](#3-ipc-inter-process-communication)
4. [Scheduling](#4-scheduling)
5. [Interrupt & Fault Handling](#5-interrupt--fault-handling)
6. [Naming, Namespaces & Process Model](#6-naming-namespaces--process-model)
7. [Boot Protocol & Early Initialization](#7-boot-protocol--early-initialization)
8. [References](#8-references)

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

Three family positions appear across the surveyed systems: no-overcommit with
reservation accounting (QNX), overcommit with pressure signals (Zircon),
per-object/per-session quotas (Genode, Nemesis).

### 2.7 Page Size Exposure

**Nearly every surveyed system exposes page size to userspace.** Zircon provides
`zx_system_get_page_size()`. seL4 exposes frame sizes directly. L4 flexpages are
inherently granularity-exposing. Mach, QNX, Plan 9, Minix 3, Redox all expose
`PAGE_SIZE`.

**Page size hiding appears nowhere in the surveyed systems.** The closest is
Genode (which abstracts alignment behind dataspaces), but even there the
granularity is observable. On ARM64 — which supports 4K, 16K, and 64K base pages
with contiguous-PTE hints for larger mappings — full hiding would require the
kernel to absorb alignment, tail-waste, and large-page promotion itself rather
than pushing them to userspace. The interface-stability argument for hiding is
that page-size changes would not break ABI; Linux has struggled with its exposed
`PAGE_SIZE` across 4K/16K/64K migrations.

### 2.8 Cache Coloring and NUMA

Most microkernels ignore physical topology. **Barrelfish** is the notable
exception (designed for it). On ARM64: cache line size differences between
big/LITTLE cores (128-byte on big, 64-byte on LITTLE in some SoCs), cache
coloring for deterministic performance, and memory controller interleaving are
topology factors that may matter. Apple Silicon's unified memory makes NUMA
distance uniform, but cache pressure and core-cluster partitioning remain.

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
registers).

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
upcalling on every event (costly) or batching (loses responsiveness).

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
ADPF** adds app-level performance hints. Heterogeneous scheduling has become a
mandatory concern on any modern ARM64 target; ignoring it leaves cores stranded
or mispriced.

### 4.4 Thread vs. Process as Schedulable Unit

All systems schedule threads. The difference is binding: **seL4** fully
separates TCBs from VSpaces and scheduling contexts. **QNX/Linux** bundle
threads in processes. **Barrelfish** schedules dispatchers (user-level entities
managing internal threads).

The implementation split that cuts across systems: scheduling parameters on the
thread object (most systems) vs. as a separate first-class capability (seL4 MCS
scheduling contexts).

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

The real-time spectrum runs from hard-RT (missed deadline = system failure;
aerospace, safety-critical control) through firm-RT (missed deadline = dropped
result; industrial) to soft-RT (missed deadline = degraded experience; audio,
graphics, touch).

**Representative soft-RT deadlines** on interactive devices:

- Audio: 5-10ms round-trip (256 samples at 48kHz is ~5.3ms)
- Touch input: under 50ms (Apple Pencil targets 8-16ms)
- Display: 16.67ms at 60Hz, 8.33ms at 120Hz

Systems approach this differently. **seL4 MCS** provides hard-RT via sporadic
servers with formally bounded WCET. **QNX** targets firm-RT with 256
fixed-priority levels and bounded interrupt latency. **Zircon** uses deadline
profiles (capacity/period) layered above fair-share for soft-RT. **Linux
SCHED_DEADLINE** applies CBS on EDF. The choice of RT target shapes which
scheduling model fits.

### 4.7 Energy-Aware Scheduling

**Linux EAS** evaluates energy cost of each placement decision using an Energy
Model. **macOS** maps QoS classes to P-cores/E-cores. **Android ADPF** layers
app-level performance hints. **seL4** and most traditional microkernels do not
address energy at the kernel level — policy is pushed to userspace or absent.

The observable split: systems that integrate energy into the scheduler (Linux
EAS, macOS, Android) accept tight coupling between scheduler policy and a
device-specific energy model; systems that ignore it (seL4, QNX, most L4
derivatives) treat energy as out-of-scope or userspace-managed. ARM SoC
topologies change every generation, so any kernel that owns the energy model
accepts continuous churn in that component.

### 4.8 Interactive Responsiveness

Three requirements commonly cited: (1) input events preempt immediately, (2) the
compositor never misses frame deadlines, (3) background work doesn't starve
interactive threads. **macOS QoS tiers** (userInteractive > userInitiated >
utility > background) are the canonical user-facing model. **EEVDF** inherently
improves latency via virtual deadlines. **Zircon** combines a deadline class for
frame-bound work with a fair class for everything else.

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
Straightforward on fixed-topology systems; more involved with hotplug or dynamic
core availability.

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

On ARM64, devicetree is the dominant hardware description format across both
embedded and personal-device deployments. Server-class ARM64 platforms
increasingly use ACPI.

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

Under Apple Hypervisor.framework, the VMM controls vCPU creation and PSCI is
emulated via HVC.

### 7.4 Memory Discovery and Initial Page Tables

Memory discovery: devicetree `/memory` nodes, UEFI GetMemoryMap(), or
boot-provided structure. **Initial page tables:** identity map (VA = PA) for
MMU-enable trampoline in TTBR0, kernel mapping in TTBR1. After MMU on and kernel
at high VA, identity map dropped.

Under Hypervisor.framework, the VMM controls guest physical space and can pass a
custom boot structure to the guest rather than requiring the guest to parse
firmware tables.

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

Across these, the tradeoff axis is how much the kernel must understand the
container format. Single-ELF is simplest but least flexible. Typed containers
(ZBI) are self-describing at the cost of a minimal parser in the kernel.
Orthogonal-persistence (EROS/KeyKOS) sidesteps the boot-image question by
reconstructing state from a snapshot.

---

## 8. References

An annotated reading list. Each entry names the paper and, in italics, what
question it's the right source to consult. When survey-depth in the sections
above isn't enough, follow these pointers rather than taking summaries as
definitive.

### Foundational Papers

- **Dennis & Van Horn, "Programming Semantics for Multiprogrammed
  Computations"** (CACM 1966). _Read for: the original definition of
  capabilities and the "sphere of protection" (C-list). Where "designation and
  authority are the same thing" first appears in print._
- **Liedtke, "Improving IPC by Kernel Design"** (SOSP 1993). _Read for: the
  origin of the L4 microkernel performance tradition. 20x over Mach by ruthless
  attention to IPC path. No single optimization — a discipline._
- **Liedtke, "On Micro-Kernel Construction"** (SOSP 1995, SIGOPS Hall of Fame
  2015). _Read for: the minimality argument. "A concept is tolerated inside the
  microkernel only if moving it outside would prevent the implementation of the
  system's required functionality." The canonical statement of the microkernel
  principle._
- **Bershad et al., "Lightweight Remote Procedure Call"** (ACM TOCS 1990). _Read
  for: thread migration as an IPC optimization, and the argument that most
  cross-domain calls are cross-machine-unnecessary. Influenced Spring and
  Composite._
- **Hardy, "The Confused Deputies"** (1988). _Read for: the canonical
  two-paragraph argument for why capabilities beat ACLs. The compiler billing
  example drives home the confused-deputy problem._

### Formal Verification

- **Klein et al., "seL4: Formal Verification of an OS Kernel"** (SOSP 2009).
  _Read for: what it takes to prove functional correctness of a kernel. 7500
  LOC, 200k LOC of proof script, multiple person-years. Defines the cost basis
  of verification._
- **Shapiro & Weber, "Verifying the EROS Confinement Mechanism"** (IEEE S&P
  2000). _Read for: the only formal proof that a capability system confines.
  Constructor certification without code analysis._

### Scheduling

- **Anderson et al., "Scheduler Activations"** (SOSP 1991). _Read for: the
  canonical attempt at kernel-to-userspace scheduling delegation. NetBSD
  implemented and abandoned it — read for why upcall-based models lose
  responsiveness._
- **Waldspurger, "Lottery Scheduling"** (OSDI 1994). _Read for: the foundational
  proportional-share paper. Influenced all fair-share schedulers downstream; no
  production deployment itself._
- **Stoica & Abdel-Wahab, "Earliest Eligible Virtual Deadline First"** (1995).
  _Read for: the algorithm Linux adopted in 6.6 to replace CFS. Virtual
  deadlines for latency bounds without heuristics._
- **Lyons et al., "Scheduling-Context Capabilities"** (EuroSys 2018). _Read for:
  how to make CPU time a first-class capability. Solves the seL4 MCS budget
  enforcement problem. Relevant wherever you want CPU time to be transferable
  like any other authority._
- **Lozi et al., "The Linux Scheduler: A Decade of Wasted Cores"** (EuroSys
  2016). _Read for: the failure mode of a scheduler nobody understands
  holistically. Four bugs in CFS that caused idle cores alongside overloaded
  ones — cautionary._
- **Kolivas, "Brain Fuck Scheduler"** (2009). _Read for: an outsider's critique
  of Linux CFS and a desktop-focused alternative design. Useful framing even
  though BFS itself never mainlined._

### Memory

- **Elphinstone & Heiser, "From L3 to seL4: 20 Years of L4 Microkernels"** (SOSP
  2013). _Read for: the retrospective on what L4 got right and wrong. The "20
  years" paper is the single best orientation to microkernel evolution. Start
  here when you need to understand why modern L4-family kernels look the way
  they do._
- **Shapiro & Smith, "EROS: A Fast Capability System"** (SOSP 1999). _Read for:
  capability-based persistent memory. Why "everything is persistent by default"
  changes the capability model fundamentally._
- **Hand, "Self-Paging in the Nemesis OS"** (OSDI 1999). _Read for: the argument
  that per-application physical memory frames prevent cross-application QoS
  interference. The deepest answer to the Nemesis crosstalk problem on the
  memory side._

### Architecture & IPC

- **Baumann et al., "The Multikernel"** (SOSP 2009). _Read for: the
  treat-cores-as-nodes-in-a-distributed-system position. The strongest argument
  against shared-everything kernel state on modern hardware._
- **Hunt & Larus, "Singularity: Rethinking the Software Stack"** (2007). _Read
  for: compile-time-verified channel contracts, SIPs (software isolated
  processes), and what it costs to make isolation a language property rather
  than a hardware one._
- **Steinberg & Kauer, "NOVA"** (EuroSys 2010). _Read for: a microkernel
  designed for virtualization. Semaphore-based interrupts and the
  micro-hypervisor pattern._
- **Blackham et al., "Improving Interrupt Response Time"** (EuroSys 2012). _Read
  for: the measurement that a non-preemptible seL4 still achieves 10k-100k cycle
  worst-case interrupt latency. When deciding whether kernel preemption is worth
  the complexity._

### Capability Systems

- **Levy, _Capability-Based Computer Systems_** (1984). _Read for: the
  definitive first-generation survey. Plessey System 250, CAP, Hydra, IBM
  System/38. Everything before mainstream microkernels._
- **Miller, "Robust Composition"** (PhD thesis 2006). _Read for: the
  object-capability foundation. Membranes, caretakers, sealers/unsealers — the
  patterns that compose capabilities into security architectures. Long, but the
  definitive source._
- **Miller, Yee & Shapiro, "Capability Myths Demolished"** (2003). _Read for:
  the three-page refutation of "capabilities = ACLs," "capabilities can't
  confine," and "capabilities can't be revoked." Cite this when someone claims
  capabilities are insufficient._
- **Watson et al., "CHERI: A Hybrid Capability-System Architecture"** (IEEE S&P
  2015). _Read for: what capabilities look like when they're a hardware
  primitive. 128-bit tagged pointers with bounds and permissions._
- **Watson et al., "CheriABI: Enforcing Valid Pointer Provenance"** (ASPLOS
  2019). _Read for: how CHERI integrates with a C ABI at scale. Lessons for any
  capability system that wants to accommodate legacy code._

### Naming & Process Model

- **Pike et al., "Plan 9 from Bell Labs"** and **"The Use of Name Spaces in Plan
  9"** (1995). _Read for: per-process namespaces via bind/mount, and the radical
  choice to make everything a file at scale. The cleanest alternative to
  capability-based naming._
- **Nelson et al., "A Uniform Name Service for Spring's UNIX Environment"**
  (USENIX 1994). _Read for: how Spring handled naming when object-based systems
  met UNIX expectations._
- **Feske, "Genode OS Framework Foundations"** (continuously updated). _Read
  for: the parent-child recursive-delegation model. The practical answer to "how
  do you actually build a microkernel system with hierarchical trust?"_
- **Parmer, "The Case for Thread Migration"** (OSPERT 2010). _Read for:
  Composite OS's argument that thread migration should be the primary IPC
  primitive. Pairs with Bershad's LRPC paper._

### ARM64 Hardware

_Primary sources for architecture questions. When uncertain about an instruction
encoding, memory ordering semantic, or register layout, read the relevant
section of the ARM ARM directly rather than a summary._

- [ARM GICv3/v4 Architecture Specification](https://developer.arm.com/documentation/ihi0069/latest)
  — _read for: interrupt routing, LPIs, ITS, direct virtual injection._
- [ARM Generic Timer](https://developer.arm.com/documentation/102379/latest) —
  _read for: one-shot timer programming, EL1/EL0 physical and virtual timers,
  timer frequency discovery._
- [ARM PSCI Specification](https://developer.arm.com/documentation/den0022/latest)
  — _read for: multicore bringup via HVC/SMC calls._
- [ARM Exception Model](https://developer.arm.com/documentation/100933/latest) —
  _read for: VBAR_EL1 vector layout, ESR_EL1 exception classification, SPSR_EL1
  saved state semantics._
