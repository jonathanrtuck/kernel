# SMP: Prior Art and Research

Research document for the open question: **how does the kernel operate on
multiple cores?** Covers synchronization models, inter-core communication
mechanisms, and performance data from existing systems.

---

## Table of Contents

1. [Synchronization Models](#1-synchronization-models)
2. [Barrelfish and the Multikernel](#2-barrelfish-and-the-multikernel)
3. [Inter-Core Communication on ARM64](#3-inter-core-communication-on-arm64)
4. [Performance Data](#4-performance-data)
5. [Multicore Scheduling](#5-multicore-scheduling)
6. [Cross-Core Operations](#6-cross-core-operations)
7. [References](#7-references)

---

## 1. Synchronization Models

### Shared-everything + locking

One set of kernel data structures, multiple cores grab locks to modify them.
Fine-grained locking (per-object or per-field) enables maximum concurrency.

**Systems:** Linux (evolved from BKL to fine-grained over 20 years), FreeBSD,
Windows NT kernel.

**Tradeoff:** Scales with effort, massive complexity. Lock ordering bugs cause
deadlocks. On ARM64, each lock acquisition requires explicit memory barriers
(DMB/DSB), which are expensive (§4.2).

### Shared-everything + lock-free

Atomic compare-and-swap on shared data structures. No blocking. Requires careful
reasoning about memory ordering (acquire/release semantics on ARM64's weak
model).

**Systems:** No kernel does this comprehensively. Used for specific data
structures — Linux's RCU for read-mostly state, lockless queues for IPI
delivery.

**Tradeoff:** Correctness is extremely difficult. Hard to verify. Useful as a
building block, not a whole-system strategy.

### Partitioned (multikernel)

Each core runs its own kernel instance with its own state. No shared mutable
state between instances. Coordination via explicit message passing. See §2 for
Barrelfish.

**Systems:** Barrelfish.

**Tradeoff:** Zero contention on the hot path. Context migration and cross-core
IPC become distributed protocols.

### Big Kernel Lock

Only one core executes kernel code at a time. A single spinlock guards all
kernel entry.

**Systems:** seL4 (SMP version). Linux 2.0–2.6.39 (removed).

**Tradeoff:** Simple, correct, easy to reason about. Limits kernel throughput
under high syscall load. seL4 data: 23% overhead on ARM from the lock primitive
alone, but fine-grained locking costs >70% (§4.2).

### Hybrid (per-core hot path, shared cold path)

Per-core state for the fast path (scheduling, exception handling). Shared state
with synchronization for slow operations (migration, address space operations).

**Systems:** Linux (per-CPU run queues + load balancing), Zircon, QNX.

**Tradeoff:** Fast path is local with zero synchronization. Cross- core
operations pay IPI + coordination cost but are infrequent.

---

## 2. Barrelfish and the Multikernel

Reference: Baumann et al., "The Multikernel: A New OS Architecture for Scalable
Multicore Systems" (SOSP 2009).

### Thesis

Modern hardware is a distributed system (NUMA, cache hierarchies,
interconnects). Run N independent kernel instances ("CPU drivers"), one per
core, communicating via explicit asynchronous messages.

### Architecture

Each **CPU driver** is small and self-contained:

- Its own scheduler
- Its own capability space
- Its own interrupt handling
- No shared mutable state with other CPU drivers

Cross-core coordination is handled by **user-mode monitors** — one per core —
that communicate via **UMP** (User-level Message Passing): shared
cache-line-sized ring buffers, sender writes, receiver polls.

### Key components

**UMP (User-level Message Passing).** Inter-core transport. Two cores share a
cache-line-sized ring buffer. No kernel involvement in the steady state.

**Monitors.** Userspace processes handling cross-core state replication:
capability transfer, address space coordination, service discovery. The kernel
is not involved in cross-core coordination.

**Dispatchers.** Barrelfish schedules dispatchers (user-level entities that own
an address space and manage their own M:N thread pool). The CPU driver picks a
dispatcher and hands it the core.

**Per-core capability spaces.** Capabilities are replicated across cores by
monitors. A CPU driver never touches another core's capability table.

### Results

- Scaled linearly on NUMA machines where Linux stalled. TLB shootdown benchmarks
  were the strongest result.
- Zero kernel-level cross-core synchronization.
- Naturally models hardware heterogeneity.

### Limitations

- State replication between monitors is a distributed consensus problem.
  Capability revocation across cores is distributed garbage collection.
- Applications spanning cores must think in messages, not shared memory.
- On small, cache-coherent systems (4–8 cores), the coherence hardware works
  well enough that the distributed protocol may be over-engineering.

---

## 3. Inter-Core Communication on ARM64

### SGI (Software Generated Interrupt)

The IPI mechanism on ARM64. GICv3: write to `ICC_SGI1R_EL1` with target core
affinity. The target core takes an IRQ exception.

### Shared memory

Cache-coherent within a coherency domain. Requires explicit barriers:

- `DMB` (Data Memory Barrier) — ordering guarantee
- `DSB` (Data Synchronization Barrier) — completion guarantee
- `LDAR`/`STLR` — acquire/release on atomics

### WFE/SEV (Wait-for-Event / Send-Event)

Low-power spin mechanism. A core executes `WFE` and sleeps until woken by
another core's `SEV` or a cache-line write (via exclusive monitor). Useful for
short waits.

### Spinlock primitives

ARM64 uses `LDAXR`/`STXR` loops for spinlocks. ARMv8.1 adds LSE atomics (`CAS`,
`SWP`, `LDADD`) which perform better under contention.

### Common patterns

**Shared memory + IPI.** Core 0 writes a request to a per-core mailbox, sends
SGI. Core 1's reactor fires, processes the request, writes result, optionally
IPIs back. Looks asynchronous but fires immediately — closer to RPC with
interrupt delivery.

**Spinlock on shared state.** Both cores touch the same memory under a lock.
Viable for very short critical sections only.

**Lock-free atomics.** `LDXR`/`STXR` or `CAS`. Building block for specific
patterns (counters, flag sets, SPSC queues).

---

## 4. Performance Data

### 4.1 IPI latency on ARM64

| Platform         | One-way    | Round-trip                 |
| ---------------- | ---------- | -------------------------- |
| GICv2 (MMIO)     | ~150ns     | ~1us                       |
| GICv3 (sysreg)   | ~600–960ns | ~1–2us                     |
| Linux real-world | —          | 2–5us avg, 50–200us spikes |

Sources: ARM community measurements, GIC specification analysis.

### 4.2 Lock overhead in microkernels

Reference: Peters et al., "For a Microkernel, a Big Lock Is Fine" (APSys 2015).
Quad-core Cortex-A9, 1GHz.

| Strategy           | ARM overhead | x86 overhead |
| ------------------ | ------------ | ------------ |
| Big Kernel Lock    | 23%          | 3%           |
| Fine-grained locks | >70%         | lower        |

Findings:

- The 23% ARM overhead is from the **lock primitive itself** (memory barriers),
  not contention. Contention is rare because microkernel syscalls are short.
- Fine-grained locking is worse on ARM — each additional acquisition pays the
  barrier cost.
- IPI sends and page table updates should be moved **outside** the critical
  section.
- ARM's weaker memory model makes synchronization primitives fundamentally more
  expensive per-operation than x86.

### 4.3 The cost of wasted cores

Reference: Lozi et al., "The Linux Scheduler: a Decade of Wasted Cores" (EuroSys
2016).

- Scheduling bugs caused **13–23% throughput loss** in database workloads
- Up to **138x degradation** from a missing-scheduling-domains bug
- Up to **27x speedup** after fixing scheduling group construction
- Cores sat idle for **seconds** while other cores had full run queues

The overhead was not from IPIs or synchronization. It was from the scheduler
**failing to use IPIs** when it should have. The cost of an idle core (ms–s)
dwarfs the cost of an IPI (~1–2us).

### 4.4 Heterogeneous scheduling (big.LITTLE)

Reference: Linux Energy Aware Scheduling (EAS), mainline since 5.0.

- Up to **48% power reduction** with heterogeneity-aware scheduling
- **15% performance improvement** from ML-based task placement (academic)
- EAS models per-frequency energy costs for big vs. LITTLE cores, places tasks
  to minimize energy while meeting utilization targets

---

## 5. Multicore Scheduling

### Per-core run queues

Universal in production systems (Linux, FreeBSD, Zircon, Windows). A global run
queue creates contention bottlenecks.

### Load balancing

The hard problem. Work stealing (idle core pulls from busy core) and periodic
rebalancing (push tasks to equalize load). The "Wasted Cores" paper documents
how subtle bugs in Linux's load balancer went undetected for years.

### seL4 MCS multicore

Big Kernel Lock. Time budgets per scheduling context. Migration via explicit
kernel operations. BKL chosen because microkernel critical sections are short
and contention is rare.

### Barrelfish

Per-core dispatchers. No cross-core scheduling in the kernel. User- mode
monitors coordinate if needed.

### Heterogeneous cores (big.LITTLE / DynamIQ)

Core asymmetry adds a dimension: which core type should a task run on? Linux EAS
uses an energy model. macOS uses QoS classes for P-core vs. E-core placement.
Android ADPF adds app-level hints.

No surveyed system uses per-core scheduler algorithms as a first- class feature.
Linux has scheduling classes but they are global. Barrelfish has per-core
schedulers but doesn't emphasize heterogeneous policies.

---

## 6. Cross-Core Operations

Operations that require coordination between cores, ordered by typical
frequency.

### Context migration

Moving a schedulable entity from one core's run queue to another's. Triggered by
load balancing, core-type affinity, IPC locality. Requires ensuring the entity
is on exactly one run queue at any time.

### Cross-core IPC delivery

Delivering a message to an entity on a different core. Either touch remote state
directly (lock) or post a request and IPI the remote core.

### TLB invalidation

Unmapping a page from a shared address space requires invalidating stale TLB
entries on all cores that may have cached them.

**ARM64 vs. x86: a critical difference.** On x86, TLB shootdown historically
required software IPIs — no hardware broadcast mechanism existed. Core 0 had to
IPI every other core, each ran a handler to execute `INVLPG`, and core 0 waited
for all acks. This is what made Barrelfish's TLB benchmarks so dramatic.

ARM64 has **hardware TLB broadcast** via inner-shareable (IS) TLBI variants:

- `TLBI VAE1IS` — invalidate by VA, all cores in inner-shareable domain
- `TLBI ASIDE1IS` — invalidate by ASID, all cores
- `TLBI VMALLE1IS` — invalidate all EL1 entries, all cores
- `DSB ISH` — barrier: stall until all cores have completed the invalidation

The hardware handles the broadcast. No IPI needed. No remote reactor wakes up.
The initiating core executes `TLBI IS` + `DSB ISH` and stalls until hardware
confirms completion. Other cores process the invalidation in their
microarchitecture, transparent to software — even if those cores are in WFI
(low-power idle).

The cost is still nonzero: `DSB ISH` stalls the initiating core proportional to
how quickly remote TLB hardware processes the invalidation. But this is
nanoseconds-to-low-microseconds of stall, not a full IPI round trip per core.

This substantially changes the SMP cost model relative to x86 and relative to
the Barrelfish paper's context.

Reference: Amit et al., "Optimizing TLB Shootdowns" (USENIX ATC 2017) — x86
focused, but batching and page-access tracking strategies are still relevant for
reducing invalidation frequency.

### Capability operations

Cross-core capability transfer or revocation. If capabilities are in a shared
table, requires locking (no broadcast). If per-core (Barrelfish), requires a
replication/consensus protocol (broadcast).

### Broadcast analysis

Operations requiring all-cores broadcast on ARM64:

| Operation                        | Broadcast?         | Mechanism             |
| -------------------------------- | ------------------ | --------------------- |
| TLB invalidation                 | Hardware broadcast | `TLBI IS` + `DSB ISH` |
| Context migration                | Point-to-point     | IPI to one core       |
| Cross-core IPC                   | Point-to-point     | IPI to one core       |
| Capability revocation (shared)   | No broadcast       | Lock + modify         |
| Capability revocation (per-core) | Software broadcast | IPI all cores         |
| Core parking/unparking           | Point-to-point     | IPI to one core       |
| ASID exhaustion flush            | Hardware broadcast | `TLBI VMALLE1IS`      |
| System halt/panic                | Software broadcast | IPI all cores         |

On ARM64 with a shared capability table, almost no common operation requires
software broadcast IPIs. The operations that do (ASID flush, halt) are extremely
rare — ASID space is 8 or 16 bits (256 or 65536 IDs).

---

## 7. References

### Architecture and SMP models

- Baumann et al., "The Multikernel" (SOSP 2009) — Barrelfish
- Peters et al., "For a Microkernel, a Big Lock Is Fine" (APSys 2015) — BKL vs.
  fine-grained on ARM/x86
- Lozi et al., "The Linux Scheduler: a Decade of Wasted Cores" (EuroSys 2016)
- Amit et al., "Optimizing TLB Shootdowns" (USENIX ATC 2017)

### ARM64 hardware

- ARM GICv3 Architecture Specification — SGI, affinity routing
- ARM Architecture Reference Manual — barriers, atomics, WFE/SEV
- ARM PSCI Specification — CPU_ON, multicore bringup

### Scheduling

- Linux EAS documentation — big.LITTLE task placement, energy model
- seL4 MCS — time budgets, multicore with BKL
