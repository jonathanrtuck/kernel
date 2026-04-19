# Bleeding-Edge OS Design: 2022--2026 Research Landscape

Survey of recent (2022--2026) dissertations, theses, conference papers, and
active projects pushing the boundaries of operating system design. Covers novel
architectures, verification approaches, IPC mechanisms, capability innovations,
memory models, and hardware-software co-design.

Prepared 2026-04-19 from SOSP 2023/2025, OSDI 2023/2024, EuroSys 2023--2025,
ASPLOS 2023/2024, HotOS 2023, ISCA, ATC 2025, PLOS workshops, and active project
repositories.

---

## Table of Contents

1. [Verified Kernel Development](#1-verified-kernel-development)
2. [Rust as an Architectural Boundary](#2-rust-as-an-architectural-boundary)
3. [Capability System Innovations](#3-capability-system-innovations)
4. [IPC and Communication Architectures](#4-ipc-and-communication-architectures)
5. [Asynchronous-First Kernel Design](#5-asynchronous-first-kernel-design)
6. [Static / Build-Time-Determined Architectures](#6-static--build-time-determined-architectures)
7. [Persistent and Crash-Consistent OS Architectures](#7-persistent-and-crash-consistent-os-architectures)
8. [Compartmentalization Without MMU](#8-compartmentalization-without-mmu)
9. [Zero-Copy Design Patterns](#9-zero-copy-design-patterns)
10. [Hardware-Software Co-Design](#10-hardware-software-co-design)
11. [WebAssembly as Execution Substrate](#11-webassembly-as-execution-substrate)
12. [Novel Paradigms and Provocations](#12-novel-paradigms-and-provocations)
13. [Active Projects Summary Table](#13-active-projects-summary-table)
14. [References](#14-references)

---

## 1. Verified Kernel Development

The question: **what does it cost to formally verify a kernel, and is that cost
dropping fast enough to matter for new designs?**

### Atmosphere (University of Utah, SOSP 2025)

The first practically-verified full-featured microkernel written in Rust with
proofs in Verus. L4-style architecture with address spaces, page tables,
threads, and IPC. All functional correctness properties proved via SMT.

- **Proof-to-code ratio:** 7.5:1 (vs. seL4's ~20:1 in Isabelle/HOL).
- **Effort:** ~2 person-years, under 1 calendar year.
- **Mechanism:** Verus translates Rust + annotations into Z3 SMT queries. Rust's
  ownership model eliminates large classes of proof obligations that seL4 had to
  discharge manually (aliasing, lifetime, resource cleanup).
- **Limitation:** Currently x86-64 only. Concurrent/multicore verification not
  yet complete.

Source: Chen et al., "Atmosphere: Practical Verified Kernels with Rust and
Verus," SOSP 2025. https://dl.acm.org/doi/10.1145/3731569.3764821

### Verus (MPI-SWS / CMU / VMware Research, SOSP 2024)

The verification tool underpinning Atmosphere. Brings Dafny-style automated
verification to Rust. Verifies page tables, concurrent allocators, and
crash-safe storage. Two of three best papers at OSDI 2024 were built on Verus.

- **Performance:** 3--61x faster verification than prior tools.
- **Key innovation:** Linear ghost types — proof-only values that track logical
  state without runtime cost, integrated with Rust's ownership.
- **VerusSync extension:** Handles concurrent Rust verification via tokenized
  state machines.

Source: Lattuada et al., "Verus: A Practical Foundation for Systems
Verification," SOSP 2024 (Distinguished Artifact Award).
https://dl.acm.org/doi/10.1145/3694715.3695952

### TickTock (SOSP 2025)

Formally verified process isolation in the Tock embedded OS using Flux, an
SMT-based Rust type-refinement verifier. The verification covers Tock's
MPU-based isolation, grant mechanism, and syscall interface.

- **Found 7 previously unknown isolation bugs**, including paths to full OS
  compromise.
- **Performance impact:** Verified kernel within 0.3% of unverified.
- **Mechanism:** Flux adds refinement types to Rust (e.g.,
  `fn get(i: usize{i < self.len})`) and discharges them via Z3.

Source: "TickTock," SOSP 2025. https://dl.acm.org/doi/10.1145/3731569.3764856

### Pancake (UNSW / Data61, 2023--2025)

A new imperative language purpose-built for verified device drivers, with a
verified compiler built on CakeML. Unlike Rust-based approaches, Pancake gives
explicit memory control with end-to-end correctness proofs from source to
binary.

- **Landmark result (2025):** First verified, realistic, performant Ethernet NIC
  driver for a non-trivial device.
- **Tradeoff vs. Rust+Verus:** Pancake provides stronger guarantees (verified
  compiler) but requires a new language. Verus works with existing Rust code.

Source: Pohjola et al., CakeML project. https://cakeml.org/pancake and
https://arxiv.org/abs/2501.08249

### Beyond Isolation (ETH Zurich / UBC / VMware, HotOS 2023)

A vision paper arguing that verified isolation alone is insufficient — verified
applications need to reason _through_ the OS specification to hardware for
end-to-end correctness. Current verified OS work stops at the kernel boundary.

Source: Brun et al., HotOS 2023. https://dl.acm.org/doi/10.1145/3593856.3595899

### Tradeoffs

| Approach            | Proof-to-code    | Effort       | Concurrency         | Compiler trust             |
| ------------------- | ---------------- | ------------ | ------------------- | -------------------------- |
| Isabelle/HOL (seL4) | ~20:1            | Decades      | Verified (seL4-MCS) | Unverified C compiler      |
| Verus (Atmosphere)  | 7.5:1            | Person-years | Not yet             | Rust compiler (unverified) |
| Flux (TickTock)     | Refinement types | Months       | Limited             | Rust compiler (unverified) |
| Pancake/CakeML      | N/A (new lang)   | Years        | Limited             | Verified compiler          |

The gap: all Rust-based approaches trust `rustc` and LLVM. Pancake/CakeML is the
only path to a fully verified compilation chain, but requires abandoning Rust.

---

## 2. Rust as an Architectural Boundary

The question: **can Rust's type system replace or supplement hardware isolation
within a kernel?**

### Asterinas "Framekernel" (Intel / SUSTech, ATC 2025)

Introduces a new kernel architecture class. All `unsafe` Rust is confined to a
small core library called OSTD (~14% of codebase). The remaining 86% — all
kernel services — is written in safe Rust against OSTD's API. OSTD encapsulates
hardware interaction, raw memory access, and concurrency primitives behind safe
abstractions.

- **TCB comparison:** 14.0% (Asterinas) vs. 43.8% (Tock) vs. 62.4% (Theseus) vs.
  66.1% (RedLeaf).
- **Compatibility:** 210+ Linux syscalls. Runs unmodified Linux binaries.
- **Performance:** On par with Linux for most workloads.
- **Architecture:** x86-64; AArch64 in progress.
- **Verification:** OSTD is small enough to verify with Verus. CertiK completed
  verification of the page management module.

Source: Peng et al., USENIX ATC 2025. https://arxiv.org/html/2506.03876v1

### Theseus (Rice / Yale, OSDI 2020, ongoing)

The extreme position: single address space, single privilege level, all
isolation via Rust's affine type system. No hardware protection boundaries at
all. The OS is a collection of "cells" (crate-level modules) with compiler-
enforced ownership of all resources.

- **Live evolution:** Any cell can be swapped at runtime. The compiler knows
  what each cell owns, so safe handoff is possible.
- **Fault recovery:** No redundancy needed. The type system tracks resource
  ownership, enabling precise reclamation on failure.
- **Tradeoff:** Trusts the Rust compiler absolutely. A compiler bug or unsound
  `unsafe` block breaks all isolation. No defense-in-depth from hardware.

Source: Boos et al., OSDI 2020; ongoing work at
https://github.com/theseus-os/Theseus

### Tock Capsule Model (UVA / Stanford, production)

Dual isolation: "capsules" (in-kernel Rust modules) isolated by the type system
at compile time with zero runtime overhead, and "processes" isolated by hardware
MPU. Capsules cannot access unauthorized peripherals because the type system
prevents it.

- **The "grant" mechanism:** Kernel-allocated memory within a process's address
  space that the process itself cannot access. Allows the kernel to borrow
  process memory safely.
- **Deployed:** Securing 10+ million computers via Microsoft Pluton.
- **Bugs found by verification:** TickTock (see above) found 7 isolation bugs in
  this model, suggesting type-system isolation needs formal backup.

Source: https://tockos.org/ and SOSP 2025 retrospective.

### Rex: Rust Replacing eBPF (USENIX ATC 2025)

Replaces eBPF's in-kernel bytecode verifier with Rust's type system for kernel
extensions. Rex programs are safe Rust compiled to native code by LLVM — no BPF
bytecode, no JIT, no in-kernel verifier. Removes eBPF's program complexity
constraints.

Source: Jia et al., USENIX ATC 2025; Linux Plumbers Conference 2025.
https://github.com/rex-rs/rex

### Evolving Kernel-Driver Interfaces (Utah, HotOS 2023)

Analyzes attack vectors in current kernel isolation frameworks and proposes heap
isolation with single ownership (linear-types-inspired) as the correct
abstraction for kernel-driver boundaries. Current interfaces are designed around
C assumptions; Rust's ownership model enables a fundamentally different
interface contract.

Source: Burtsev et al., HotOS 2023.
https://dl.acm.org/doi/10.1145/3593856.3595914

### Tradeoffs

| System                  | Hardware isolation  | Type-system isolation     | TCB               | Defense-in-depth |
| ----------------------- | ------------------- | ------------------------- | ----------------- | ---------------- |
| Asterinas               | MMU (for userspace) | Safe Rust (in-kernel)     | 14%               | Yes              |
| Theseus                 | None                | Affine types (everything) | Compiler          | No               |
| Tock                    | MPU (processes)     | Types (capsules)          | Kernel + capsules | Partial          |
| Traditional microkernel | MMU (everything)    | None                      | Kernel only       | Yes              |

The emerging consensus: type-system isolation is valuable _alongside_ hardware
isolation, not as a replacement. Asterinas's layered approach (unsafe core +
safe services) appears to be the most practical architecture.

---

## 3. Capability System Innovations

The question: **what new approaches exist for capability representation,
revocation, and sealing?**

### Cornucopia Reloaded (Cambridge, ASPLOS 2024)

Solves capability revocation using per-page load barriers, inspired by garbage
collection techniques. When a capability is revoked, the page containing it is
marked. Subsequent loads from that page trigger a barrier that checks whether
the loaded capability has been revoked.

- **Performance:** Median 87% of original DRAM traffic overhead. No application
  pauses (vs. stop-the-world sweeps in prior approaches).
- **Hardware requirement:** Per-page capability load barrier, implemented in Arm
  Morello and CHERI-RISC-V.
- **Mechanism:** On revocation, quarantine the memory. Mark pages containing
  capabilities to the freed object. Load barrier intercepts capability loads and
  substitutes null for revoked capabilities.

Source: https://dl.acm.org/doi/10.1145/3620665.3640416

### CHERIoT Temporal Safety (Microsoft Research, SOSP 2025)

Achieves deterministic use-after-free protection on embedded devices with no
MMU. Replaces Cornucopia Reloaded's MMU-based load barrier with a hardware load
filter purpose-built for the embedded constraint set.

- **Hardware:** CHERI-extended RISC-V with ~7,000 lines of core RTOS code.
- **Compartments:** Interact via direct function calls passing capabilities as
  arguments — no marshaled IPC.
- **Auditing:** The linker generates reports of all compartment relationships
  and capability allocations, enabling security audit in CI/CD.
- **"Trust Nothing" variant (2025):** Even the scheduler and allocator are
  untrusted. Security holds by construction via hardware capabilities.

Source: https://dl.acm.org/doi/10.1145/3731569.3764844 and
https://arxiv.org/html/2603.08400v1

### CHERIoT Sealed Capabilities (2025)

`__sealed_capability` as a first-class type qualifier in the LLVM toolchain.
Sealed capabilities are opaque, tamper-proof tokens: they carry authority but
cannot be dereferenced until unsealed by the designated receiver. Enables
type-safe interfaces between mutually distrusting compartments.

- **Compiler enforcement:** Sealing/unsealing checked at the type level, not
  just at runtime.
- **Use case:** Cross-compartment handles, callback tokens, unforgeable return
  addresses.

Source:
https://cheriot.org/sealing/compiler/2025/01/30/introducing-sealed-types.html

### Caplification (SACMAT 2025)

Addresses the adoption problem: running capability-oblivious software alongside
capability-aware software without sacrificing capability guarantees. Proposes
mechanisms for seamless co-existence without a trusted central authority.

Source: https://dl.acm.org/doi/10.1145/3734436.3734449

### TreeSLS Capability Tree (SJTU, SOSP 2023)

Uses the capability tree as the single structure governing _all_ system state.
Not just access control — the tree _is_ the system. This makes whole-system
checkpointing trivial: persist the tree and you've persisted the system.

- **Checkpoint latency:** ~100 microseconds on NVM.
- **Overhead:** Even 1ms checkpoint intervals are tolerable.
- **Systems tested:** Memcached, Redis, RocksDB.

Source: https://dl.acm.org/doi/10.1145/3600006.3613160

### seL4 Time Protection / fence.t (UNSW / ETH Zurich, 2019--2024)

Treats time as a first-class capability via scheduling-context capabilities.
Plus a hardware `fence.t` instruction (implemented on RISC-V at ETH Zurich) for
flushing microarchitectural state between partition switches.

- **Goal:** Verify time protection as a functional-correctness property within
  seL4's existing proof framework.
- **Status:** Ongoing. The most ambitious attempt to close the temporal
  side-channel gap at the OS level.
- **Implication:** Timing channels are "the last fundamentally unsolved security
  problem in operating systems" (Heiser).

Source: https://trustworthy.systems/projects/timeprotection/

### S3K Time-as-Capability (KTH, ongoing)

A bare-metal separation kernel for RISC-V where time is a transferable
capability. Partitions can donate execution time to others via IPC. Monitor
capabilities allow trusted processes to supervise and recover others.

- **Hardware:** PULP CVA6 with temporal fence instruction.
- **Target domain:** Safety/security-critical (avionics).

Source: https://github.com/kth-step/s3k

### Tradeoffs: Revocation

| Approach                 | Latency       | Stop-the-world  | Hardware required | Completeness        |
| ------------------------ | ------------- | --------------- | ----------------- | ------------------- |
| seL4 CSpace delete       | O(tree depth) | Local (per-cap) | None              | Complete            |
| Cornucopia sweep         | Background    | Yes (pause)     | None              | Complete            |
| Cornucopia Reloaded      | Barrier cost  | No              | Page load barrier | Complete            |
| CHERIoT load filter      | Barrier cost  | No              | Custom hardware   | Complete            |
| Epoch-based (Barrelfish) | Deferred      | No              | None              | Eventually complete |

---

## 4. IPC and Communication Architectures

The question: **what are the fastest and most structurally sound IPC mechanisms
in recent OS research?**

### XPC: Hardware-Assisted IPC (SJTU, ISCA 2019 / TOCS 2022)

Hardware-level IPC reducing call latency from 664 to 21 cycles (14x--123x
improvement). Introduces x-entry (a hardware endpoint descriptor) and xcall/xret
instructions for direct user-level process switching without kernel trapping.

- **Zero-copy:** Messages passed across invocation chains without copying.
- **Prototyped:** RISC-V FPGA, with ports to seL4, Zircon, Android Binder.
- **The 2022 TOCS version** is the mature evaluation with full security
  analysis.
- **ARM64 relevance:** The concept (hardware endpoint + direct EL0 switching)
  could map to ARM64 with firmware-assisted fast paths, though no ARM
  implementation exists.

Source: https://dl.acm.org/doi/10.1145/3532861

### Managarm: Fully Asynchronous IPC (community, 2022--present)

A microkernel where system calls never block. All completion is reported
asynchronously. Uses "streams" as the IPC primitive — a single syscall can
submit multiple actions.

- **Capability-based** resource management.
- **Runs:** Sway, WebKitGTK, substantial Linux software.
- **Architectures:** x86-64, RISC-V.
- **Tradeoff:** Async-everywhere is powerful but complicates programming model.
  Every operation requires a completion handler or poll.

Source: https://managarm.org/ and https://github.com/managarm/managarm

### LionsOS IPC Model (UNSW, 2025)

Hybrid: asynchronous event-driven handlers via shared memory and seL4
notifications as the dominant path, with synchronous protected procedure calls
where needed.

- **Data path:** Lock-free SPSC queues in shared memory. Notifications signal
  new data.
- **Control path:** Synchronous seL4 IPC for setup and teardown.
- **Zero-copy data:** Data regions separated from metadata. Data unmapped from
  driver address spaces.

Source: https://arxiv.org/html/2501.06234v2

### Extending Rust with Zero-Copy IPC (PLOS 2023)

Academic work on extending Rust's ownership transfer semantics to support
zero-copy inter-process communication. If IPC passes capabilities via ownership
transfer, the type system enforces zero-copy as an invariant.

Source: https://dl.acm.org/doi/10.1145/3623759.3624552

### Iris + Session Types for Verified IPC (POPL 2024)

Demonstrates using session types within concurrent separation logic (mechanized
in Coq) to verify message-passing programs. Represents the formal-methods
frontier for verified channel implementations.

Source: POPL 2024 tutorial.
https://popl24.sigplan.org/details/POPL-2024-tutorialfest/4/

### IPC Latency Data Points

| System       | Mechanism         | One-way latency    | Notes                  |
| ------------ | ----------------- | ------------------ | ---------------------- |
| XPC          | Hardware xcall    | 21 cycles          | RISC-V FPGA prototype  |
| seL4         | Fastpath IPC      | ~200--400 cycles   | ARM64, verified        |
| Zircon       | Channel           | ~1000+ cycles      | x86-64, unverified     |
| Linux pipe   | Kernel buffer     | ~3000+ cycles      | x86-64                 |
| Managarm     | Async stream      | N/A (non-blocking) | Completion-based model |
| L4/Fiasco.OC | Synchronous IPC   | ~150--300 cycles   | x86-64                 |
| CHERIoT      | Direct call + cap | ~function call     | No kernel involvement  |

---

## 5. Asynchronous-First Kernel Design

The question: **what does a kernel look like when async is the default, not an
afterthought?**

### LionsOS (UNSW, 2025)

Static async-first microkernel on seL4. All components are event-driven. The
notification + shared-memory model avoids synchronous IPC for data transfer
entirely. Components react to notifications, process queued requests, and signal
completion — no blocking.

- **Static architecture:** All components and channels defined at configuration
  time.
- **Scheduling:** Components are scheduled by seL4's MCS scheduler, which
  provides temporal isolation via scheduling contexts (capabilities over CPU
  time).

Source: https://arxiv.org/html/2501.06234v2

### MnemOS (James Munns, early stage)

The kernel _is_ a cooperative async executor. All kernel services (drivers,
protocols) are async tasks communicating via message passing. The kernel is a
`no_std` Rust library crate.

- **Porting model:** Write a bare-metal application that instantiates the kernel
  library, provide hardware-specific drivers as async tasks.
- **Includes:** Browser-based simulator (Pomelo) via WASM.
- **Inspiration:** Erlang's actor model mapped to Rust async.

Source: https://github.com/tosc-rs/mnemos

### Managarm (community, 2022--present)

All syscalls are non-blocking. The "stream" primitive allows a single syscall to
submit multiple operations. No synchronous kernel entry point exists.

### io_uring as Model (Linux, 2019--2025)

Linux's io_uring demonstrates the async syscall pattern at scale: submission
queue + completion queue in shared memory, kernel processes submissions
asynchronously. SQPOLL mode enables completely syscall-free I/O.

- **Security warning:** Google disabled io_uring on Android, ChromeOS, and
  internal servers in 2023 after 60% of their 2022 bug bounty exploits targeted
  io_uring. The attack surface of a general-purpose async interface is large.

### Copier (SOSP 2025 Best Paper)

Memory copying as an asynchronous first-class OS service. Applications overlap
execution with copy; the OS coordinates hardware (DMA engines) for both
user-mode and kernel-mode copies.

- **Insight:** Some copies are unavoidable. Making them async and
  hardware-accelerated is more practical than eliminating them.

Source: https://dl.acm.org/doi/10.1145/3731569.3764800

### Tradeoffs

| Approach                    | Programming model        | Latency     | Complexity | Attack surface |
| --------------------------- | ------------------------ | ----------- | ---------- | -------------- |
| Sync-first (seL4, L4)       | Simple call/return       | Higher      | Low        | Small          |
| Async-first (Managarm)      | Completion handlers      | Lower tail  | High       | Medium         |
| Hybrid (LionsOS)            | Async data, sync control | Balanced    | Medium     | Medium         |
| Executor-as-kernel (MnemOS) | Rust async/await         | Cooperative | Medium     | Small (no_std) |
| io_uring pattern            | SQ/CQ shared memory      | Lowest      | Very high  | Large          |

---

## 6. Static / Build-Time-Determined Architectures

The question: **what can you gain by fixing all components and communication
channels at build or configuration time?**

### Hubris (Oxide Computer, production)

~2,000 LOC Rust microkernel. All tasks specified at build time and statically
linked into a single image. No heap in the kernel. No dynamic task creation. No
runtime resource allocation.

- **Isolation:** ARM MPU per-task.
- **IPC:** Strictly synchronous, typed.
- **Fault model:** Drivers run unprivileged. Individually restartable on failure
  without affecting other tasks.
- **Deployed:** Production rack servers (Oxide).
- **Attestation:** The build system composes kernel + tasks into a single
  attestable image.

Source: https://hubris.oxide.computer/ and
https://github.com/oxidecomputer/hubris

### LionsOS (UNSW, 2025)

All components and channels defined in a system configuration file. The build
system generates glue code. No runtime discovery, no dynamic channel creation.

- **Enables:** Static verification of information-flow properties, static
  allocation of all shared-memory regions, dead-code elimination of unused
  kernel paths.

### CHERIoT RTOS (2025)

All compartments, their interfaces, and their capability allocations are
determined at link time. The linker generates a human-readable audit report.

- **Enables:** CI/CD security auditing of the exact capability graph that will
  run on hardware.

### Tradeoffs

| Property            | Static                        | Dynamic                                |
| ------------------- | ----------------------------- | -------------------------------------- |
| Verification        | Tractable                     | Difficult                              |
| Zero-copy setup     | Free (pre-mapped)             | Requires runtime negotiation           |
| Resource exhaustion | Impossible (pre-allocated)    | Must handle at runtime                 |
| Flexibility         | None (rebuild to reconfigure) | Full                                   |
| Attack surface      | Minimal                       | Larger (allocation, naming, discovery) |

---

## 7. Persistent and Crash-Consistent OS Architectures

The question: **how does the volatile/persistent boundary change OS design?**

### TreeSLS (SJTU, SOSP 2023 Best Paper)

Whole-system persistent microkernel. Simplifies all system state to a capability
tree. Failure-resilient checkpoint manager on NVM persists the tree.

- **Checkpoint latency:** ~100 microseconds.
- **Overhead at 1ms intervals:** Reasonable for real workloads.
- **Key insight:** The capability tree is already the system's state
  representation. Persistence is a structural consequence, not an added feature.
- **External synchrony:** Transparent to applications. No application-level
  changes needed for persistence.

Source: https://dl.acm.org/doi/10.1145/3600006.3613160

### Twizzler (UC Santa Cruz, 2020--2025)

Removes the kernel from the I/O path for persistent memory. Programs use
memory-style access to persistent data. 64-bit object-relative pointers decouple
addressing from any process's address space.

- **Pointer overhead:** <0.5 ns per operation.
- **SQLite on Twizzler:** Up to 4.2x faster than PMDK.
- **Object lifetime:** Managed through "ties" (capability-like lifetime
  bindings).
- **Recently rebuilt** with a pure-Rust kernel (2024--2025).

Source: https://twizzler.io/ and https://dl.acm.org/doi/10.1145/3454129

### CXL Pivot (2025)

With Intel Optane discontinued (2023--2025 wind-down), persistent memory
research is pivoting to CXL-attached memory. CXL's multi-host shared memory
model introduces new consistency semantics: multiple machines sharing volatile
and persistent memory with different memory models.

Source: https://arxiv.org/html/2504.17554v1

---

## 8. Compartmentalization Without MMU

The question: **what isolation mechanisms exist beyond traditional page
tables?**

### CHERI Hardware Capabilities

CHERI extends every pointer with bounds and permissions in hardware. Fat
pointers (128-bit on 64-bit architectures) carry their own authority. No
separate capability table needed — the pointer _is_ the capability.

- **Morello (Arm):** CHERI on ARMv8. Evaluation boards available. CheriBSD
  provides a full FreeBSD experience.
- **CHERIoT (RISC-V):** CHERI scaled to embedded. No MMU, ~256 KiB SRAM.
  Multiple silicon implementations (Codasip, Wyvern).
- **CHERI Alliance (2024):** Google, Arm, Microsoft, others. Industry adoption
  accelerating.

### ARM MPU (Memory Protection Unit)

Available on Cortex-M and some Cortex-A profiles. Region-based (not page-based)
access control. Fewer regions than pages (typically 8--16), coarser granularity,
much simpler hardware.

- **Hubris:** Uses MPU for per-task isolation. ~2,000 LOC kernel.
- **Tock:** Uses MPU for process isolation, types for capsule isolation.
- **Limitation:** Region count limits number of isolated compartments. No
  demand-paging.

### Software Fault Isolation (SFI)

Compiler-inserted bounds checks on every memory access within a compartment. No
hardware support needed. WebAssembly is a modern SFI implementation.

- **k23:** Uses WASM as the SFI boundary for all userspace.
- **KFlex (SOSP 2024):** Combines SFI with eBPF's range analysis to elide checks
  when provably safe.
- **Overhead:** Typically 5--20% depending on check density.

### Rust Type System

Compile-time isolation with zero runtime cost. Requires all code to be compiled
with the same toolchain. Cannot isolate pre-compiled or adversarial code.

- **Theseus:** Type-only isolation (no hardware backup).
- **Asterinas:** Type isolation for services, hardware isolation for userspace.
- **Tock:** Type isolation for capsules, hardware isolation for processes.

### Tradeoffs

| Mechanism            | Granularity       | Runtime cost      | Adversarial code  | Hardware required |
| -------------------- | ----------------- | ----------------- | ----------------- | ----------------- |
| MMU (pages)          | 4 KiB             | TLB miss penalty  | Yes               | MMU               |
| CHERI (capabilities) | Byte-level        | In-pointer checks | Yes               | CHERI ISA         |
| MPU (regions)        | Configurable      | Region check      | Yes               | MPU               |
| SFI / WASM           | Instruction-level | Bounds checks     | Yes               | None              |
| Rust types           | Module-level      | Zero              | No (must compile) | None              |

---

## 9. Zero-Copy Design Patterns

The question: **how far can zero-copy be pushed as a systemic design
principle?**

### Ownership Transfer IPC (PLOS 2023)

Extend Rust's ownership semantics across process boundaries. When a message is
sent, ownership of the backing memory transfers to the receiver. The type system
enforces that the sender cannot access the memory after send.

- **Limitation:** Requires shared address space or shared physical memory with
  per-process mappings.

Source: https://dl.acm.org/doi/10.1145/3623759.3624552

### LionsOS Data/Metadata Separation

Data regions in shared memory are unmapped from driver address spaces. Drivers
see only metadata (descriptors, offsets). DMA goes directly to data regions
without driver involvement in the copy path.

### CHERIoT Direct-Call Compartments

Cross-compartment calls pass capability pointers as function arguments. No
marshaling. No copying. No kernel involvement for trusted paths.

### Cornflakes: NIC-Aware Serialization (SOSP 2023)

Co-designs serialization format with NIC scatter-gather DMA capabilities. The
NIC assembles the serialized message from scattered memory regions without
CPU-side copying.

- **Throughput:** 15.4% higher than prior approaches on Twitter cache traces.

Source: https://dl.acm.org/doi/10.1145/3600006.3613137

### Copier: Async Copy Service (SOSP 2025 Best Paper)

When copying is unavoidable, make it asynchronous and hardware-accelerated.
Applications overlap computation with OS-managed DMA copies.

---

## 10. Hardware-Software Co-Design

The question: **what happens when OS and hardware are designed together?**

### Xous + Custom Silicon (betrusted.io)

Designed an entire SoC (Baochip-1x, 22nm TSMC) specifically to run this
microkernel. Pure-Rust libstd (no C library). RISC-V with MMU — unusual for
embedded, enabled by open silicon.

- **Motivation:** Existing embedded hardware lacked MMU support for the desired
  isolation model.
- **Presented:** 39C3 (2025).

Source: https://github.com/betrusted-io/xous-core

### Software-Defined CPU Modes (Barkhausen Institut, HotOS 2023)

Proposes replacing hardware privilege modes (EL0/EL1/EL2 on ARM, ring 0--3 on
x86) with a single general hardware mechanism that software configures. The
argument: current privilege modes are overspecified, limiting OS design freedom.

- **Implication:** If privilege modes were software-defined, the kernel could
  create custom isolation domains without being constrained to 2--4 hardware
  levels.

Source: https://sigops.org/s/conferences/hotos/2023/papers/roitzsch.pdf

### Multi-Tag: ARM MTE + PAC Combined (ISCA 2023)

First hardware-software co-design combining ARM Memory Tagging Extension (MTE)
and Pointer Authentication Codes (PAC) for multi-granular memory safety. Goes
beyond using either feature alone.

Source: https://dl.acm.org/doi/fullHtml/10.1145/3579856.3590331

### XPC Hardware IPC (SJTU, ISCA/TOCS)

Custom hardware endpoints (x-entry) and instructions (xcall/xret) for
kernel-bypass IPC. 21-cycle one-way latency. See IPC section.

### CPU-Free Computing (HotOS 2023)

Proposes removing the CPU from the critical data path. Smart devices communicate
directly via fabric. The OS role reduces to configuration and policy.

Source: https://sigops.org/s/conferences/hotos/2023/papers/trivedi.pdf

### seL4 fence.t (UNSW / ETH Zurich)

Hardware instruction to flush microarchitectural state between security domain
switches. Implemented on RISC-V at ETH Zurich. See capability section.

---

## 11. WebAssembly as Execution Substrate

The question: **can WASM serve as a universal process model?**

### k23 (Jonas Kruckenberg, 2024--present)

Embeds a WASM JIT compiler in a microkernel (written in Zig). All userspace is
WASM. The kernel's knowledge of hardware informs compiler optimizations; the
compiler's knowledge of all programs informs kernel scheduling.

- **IPC:** Near-function-call cost through compiler-kernel co-optimization.
- **Capabilities:** Snapshot, trace, use-after-free detection — all transparent
  to WASM programs.
- **Targets:** RISC-V and x86-64.

Source: https://github.com/JonasKruckenberg/k23

### WALI: WASM + Linux Syscalls (CMU, EuroSys 2025)

Instead of giving WASM a high-level API (WASI), WALI virtualizes raw Linux
syscalls inside the WASM sandbox. Creates a new class of virtualization where
WASM becomes a universal binary format with near-native OS interaction.

Source: https://dl.acm.org/doi/10.1145/3689031.3717470

### Hyperlight + WASM (Microsoft, 2025)

Sub-millisecond micro-VMs with a Wasmtime runtime. Creates a hardware VM per
function call (~900 microsecond cold start). No OS boot, no device emulation.

Source: https://github.com/hyperlight-dev/hyperlight

### Tradeoffs

| Approach               | Isolation mechanism | IPC cost           | Hardware trust | Portability     |
| ---------------------- | ------------------- | ------------------ | -------------- | --------------- |
| Native processes + MMU | Hardware pages      | Syscall + switch   | CPU modes      | Binary per arch |
| WASM + SFI             | Compiler bounds     | Near-function-call | None needed    | Universal       |
| WASM + micro-VM        | Hardware VM         | VM exit            | Hypervisor     | Universal       |

---

## 12. Novel Paradigms and Provocations

### Declarative OS Interfaces (2024--2025)

Replacing imperative syscall APIs with declarative ones where programs state
goals, not procedures. Motivated by LLM agents as a first-class workload.

Source: https://arxiv.org/html/2510.04607v2

### NrOS: OS as Replicated Data Structure (Utah / VMware, OSDI 2021)

Treats the kernel as a replicated data structure — each NUMA node gets a
replica, mutations propagated via node replication (NR). Eliminates most
cross-node synchronization. Scales to 96 cores, dominating Linux by orders of
magnitude.

- **Key insight:** The multikernel idea (Barrelfish) realized more simply
  through data-structure replication rather than message passing.

Source: https://www.usenix.org/conference/osdi21/presentation/bhardwaj

### Fabric-Centric Computing (HotOS 2023)

The OS should manage memory fabric, not CPUs. Resource orchestration centered on
fabric rather than processors.

Source: https://sigops.org/s/conferences/hotos/2023/papers/liu.pdf

### LithOS: OS for GPU Workloads (CMU, SOSP 2025)

"Kernel atomization" — partitioning GPU kernels into schedulable atoms (subsets
of thread blocks), decoupling submission from execution.

Source: https://www.pdl.cmu.edu/PDL-FTP/BigLearning/lithos_sosp25.pdf

### DuVisor: User-Space Hypervisor (SJTU, OSDI 2023)

Moves all VM runtime handling to user space. A small hardware extension enables
user-mode handling of VM exits, stage-2 page tables, and virtual devices. Rust-
based, <5% overhead, outperforms KVM by up to 48%.

Source: https://www.usenix.org/conference/osdi23/presentation/chen

---

## 13. Active Projects Summary Table

| Project       | Language     | Architecture            | Novel contribution                      | State        |
| ------------- | ------------ | ----------------------- | --------------------------------------- | ------------ |
| Atmosphere    | Rust + Verus | x86-64                  | Verified microkernel, 7.5:1 proof ratio | SOSP 2025    |
| Asterinas     | Rust         | x86-64 (AArch64 WIP)    | Framekernel (14% unsafe TCB)            | ATC 2025     |
| TreeSLS       | C            | x86-64                  | Cap tree = system state = checkpoint    | SOSP 2023 BP |
| Hubris        | Rust         | ARM Cortex-M            | Build-time-determined, 2K LOC           | Production   |
| LionsOS       | C            | ARM + seL4              | Static async-first microkernel          | 2025         |
| CHERIoT       | C/C++        | CHERI-RISC-V            | Hardware caps at embedded scale         | SOSP 2025    |
| Theseus       | Rust         | x86-64                  | Type-only isolation, live evolution     | Research     |
| Tock          | Rust         | ARM Cortex-M, RISC-V    | Capsule + MPU dual isolation            | Production   |
| MnemOS        | Rust         | Multi (no_std)          | Kernel-as-async-executor                | Early        |
| k23           | Zig          | RISC-V, x86-64          | Compiler-in-kernel, WASM userspace      | Prototype    |
| Managarm      | C++          | x86-64, RISC-V          | Fully async IPC                         | Active       |
| Xous          | Rust         | RISC-V (custom SoC)     | Co-designed OS + silicon                | Production   |
| Twizzler      | Rust         | x86-64                  | Data-centric persistent objects         | Research     |
| R9            | Rust         | AArch64, x86-64, RISC-V | Plan 9 in Rust                          | Active       |
| Hermit        | Rust         | x86-64, AArch64         | Pure-Rust unikernel                     | Research     |
| Rex           | Rust         | Linux/x86-64            | Rust type system replaces eBPF verifier | ATC 2025     |
| S3K           | C            | RISC-V                  | Time-as-capability, temporal fence      | Research     |
| Genode/Sculpt | C++          | x86-64, ARM             | Self-hosted microkernel desktop         | Production   |

---

## 14. References

### Conference Proceedings

- Chen et al., "Atmosphere," SOSP 2025.
- Lattuada et al., "Verus," SOSP 2024.
- "TickTock," SOSP 2025.
- Wu et al., "TreeSLS," SOSP 2023 (Best Paper).
- Peng et al., "Asterinas," USENIX ATC 2025.
- Boos et al., "Theseus," OSDI 2020.
- Bhardwaj et al., "NrOS," OSDI 2021.
- Du et al., "XPC," ISCA 2019 / ACM TOCS 2022.
- Chen et al., "DuVisor," OSDI 2023.
- "Copier," SOSP 2025 (Best Paper).
- "Cornflakes," SOSP 2023.
- Raza et al., "UKL," EuroSys 2023.
- Ramesh et al., "WALI," EuroSys 2025.
- Dwivedi et al., "KFlex," SOSP 2024.
- Fried, "Next Generation OS for the Datacenter," MIT PhD 2025.
- Jia et al., "Rex," USENIX ATC 2025.

### Hardware and Capability Systems

- "Cornucopia Reloaded," ASPLOS 2024.
- CHERIoT RTOS, SOSP 2025.
- CHERIoT sealed capabilities, 2025. https://cheriot.org/
- CHERI Alliance, 2024. https://cheri-alliance.org/
- seL4 Time Protection. https://trustworthy.systems/projects/timeprotection/
- Multi-Tag, ISCA 2023.

### Vision and Workshop Papers

- Brun et al., "Beyond Isolation," HotOS 2023.
- Burtsev et al., "Evolving Kernel-Driver Interfaces," HotOS 2023.
- Roitzsch et al., "Software-Defined CPU Modes," HotOS 2023.
- Trivedi, "CPU-Free Computing," HotOS 2023.
- Liu, "Fabric-Centric Computing," HotOS 2023.
- "Extending Rust with Zero-Copy Communication," PLOS 2023.
- Iris + Session Types, POPL 2024 tutorial.

### Active Projects

- Hubris: https://github.com/oxidecomputer/hubris
- Asterinas: https://github.com/asterinas/asterinas
- Theseus: https://github.com/theseus-os/Theseus
- Tock: https://github.com/tock/tock
- MnemOS: https://github.com/tosc-rs/mnemos
- k23: https://github.com/JonasKruckenberg/k23
- Managarm: https://github.com/managarm/managarm
- Xous: https://github.com/betrusted-io/xous-core
- Twizzler: https://github.com/twizzler-operating-system/twizzler
- R9: https://github.com/r9os/r9
- Hermit: https://github.com/hermit-os/kernel
- S3K: https://github.com/kth-step/s3k
- Genode: https://github.com/genodelabs/genode
- LionsOS: https://arxiv.org/html/2501.06234v2
- Rex: https://github.com/rex-rs/rex
- Verus: https://github.com/verus-lang/verus
- Hyperlight: https://github.com/hyperlight-dev/hyperlight
- Pancake: https://cakeml.org/pancake
