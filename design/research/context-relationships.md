# Context Relationships: Prior Art and Research

Research document for the open question: **what is the relationship structure
between Contexts?** This determines naming, fault routing, and resource
accounting — the three remaining open questions at Level 1 all reduce to this
one.

Prepared for study before making the design decision.

---

## Table of Contents

1. [The Decision Space](#1-the-decision-space)
2. [Relationship Models](#2-relationship-models)
   - 2.1 Flat (capability-only)
   - 2.2 Hierarchical (tree)
   - 2.3 Pragmatic middle ground
3. [Naming and Addressing](#3-naming-and-addressing)
   - 3.1 Capability-only naming
   - 3.2 Namespace-based naming
   - 3.3 Endpoint/channel naming
4. [Fault Routing](#4-fault-routing)
   - 4.1 Single-handler delegation
   - 4.2 Hierarchical escalation
   - 4.3 Centralized monitor
   - 4.4 Erlang supervision (software analogy)
5. [Resource Accounting](#5-resource-accounting)
   - 5.1 Capability-mediated budgets
   - 5.2 Parent-mediated quotas
   - 5.3 Job/group-based limits
   - 5.4 Per-application guarantees
6. [Academic Foundations](#6-academic-foundations)
   - 6.1 Actor model
   - 6.2 CSP and pi-calculus
   - 6.3 Dennis and Van Horn (1966)
   - 6.4 Object-capability model
7. [Cross-Cutting Analysis](#7-cross-cutting-analysis)
8. [References](#8-references)

---

## 1. The Decision Space

The relationship structure between Contexts is the root question. The answer
constrains three dependent design decisions:

- **Naming:** how does Context A refer to Context B in a syscall?
- **Fault routing:** when Context A faults, who gets the message?
- **Resource accounting:** who is "responsible" for Context A's resource
  consumption?

These three can be decided independently in theory, but in practice every real
system couples them because the same structure that answers "who can talk to
whom" also answers "who is responsible for whom."

The spectrum runs from fully flat (no kernel-imposed structure, only
capabilities) to deeply hierarchical (tree of parent-child relationships
enforced by the kernel). Every real system has landed somewhere on this
spectrum, and every position has produced both successes and instructive
failures.

---

## 2. Relationship Models

### 2.1 Flat: capability-only (seL4, EROS/KeyKOS, Barrelfish)

**The model.** The kernel has no concept of process hierarchy. It provides
primitives (threads, address spaces, capability spaces, endpoints) that
userspace composes into "processes" by convention. A Context's relationships are
defined entirely by which capabilities it holds. There is no parent, no owner,
no group — only capabilities.

**seL4** is the purest modern example. "Process" is a userspace convention: a
TCB + VSpace + CSpace wired together. The kernel provides mechanisms; all
process-management policy lives in userspace.

**EROS/KeyKOS** uses the "constructor" pattern: a capability that, when invoked,
creates a new Context with a precisely defined initial capability set.
Constructors can certify that a newly created Context is _confined_ — unable to
communicate outside its initial capabilities — by inspecting only the initial
capability set, without analyzing code. Shapiro and Weber formally verified this
property.

**What went right:**

- Maximum policy freedom. Any process model (flat, tree, graph) can be built in
  userspace without kernel changes.
- Formal verification tractable because the kernel is radically minimal. seL4 is
  the only production kernel with a complete functional correctness proof.
- EROS constructor confinement is the strongest isolation guarantee any system
  has achieved — provably no unauthorized communication channels. Sufficient for
  mandatory access control.
- Capability distribution is the naming system. Dennis and Van Horn's insight
  (1966): a capability simultaneously names and authorizes. No confused deputy
  attacks by construction.

**What went wrong:**

- **Ecosystem fragmentation (seL4).** Because the kernel has no process concept,
  every deployer builds their own userspace framework. CAmkES was the original
  answer, but "proved to be too complex, static and maintenance intensive."
  Multiple companies (DornerWorks, Cog Systems) built incompatible frameworks.
  The seL4 Foundation had to create Microkit as a course correction. The VMM
  layer is "quite fragmented and not uniform."

- **CAmkES performance overhead.** CAmkES mapped one CAmkES thread to one seL4
  thread. You couldn't block without unscheduling the entire component. Moving
  to Microkit showed "70% improvement in networking performance, 150% over old
  CAmkES" — meaning the static framework was leaving enormous performance on the
  table.

- **Static systems only.** Microkit explicitly targets "statically- architected
  systems." Dynamic features like OTA updates and runtime component management
  "add significant complexity" and "must include some privileged components that
  retain authority to resources used by other components."

- **No natural grouping.** Without hierarchy, there's no built-in "kill all 5
  processes in this application" or "account for their collective resource
  usage." Grouping must be built from capability patterns.

- **EROS abandoned.** Work halted due to "very challenging security issues
  intrinsic to any system architecture based on synchronous IPC primitives."
  KeyKOS ran in production at Tymshare but the lineage died. Ideas influenced
  seL4 and others, but the constructor pattern has not been validated at modern
  scale.

- **Gernot Heiser's explicit framing.** "seL4 is a minimal wrapper around
  hardware... you shouldn't expect it to be easier to use than bare metal." The
  "what went wrong" is the entire userspace complexity budget that every
  deployer must pay.

**Key lesson:** flat capability-only models are maximally flexible and formally
tractable, but they export all structural decisions to userspace. This works for
constrained, static embedded systems. It has not yet succeeded for dynamic,
general-purpose systems.

---

### 2.2 Hierarchical: tree (Genode, Zircon Jobs)

**The model.** Contexts exist in a parent-child tree. Parents create children
from their own resources, mediate their access to services, and are responsible
for their lifecycle. The tree provides natural answers to naming (parent
introduces child to services), fault routing (escalate to parent), and resource
accounting (parent's budget bounds child's consumption).

#### Genode: component tree

The entire system is a recursive tree of components. Each component runs in its
own sandbox. Parents create children, define their execution environment, and
mediate service access. Core is the root, delegates all resources to Init, which
distributes further. The parent-child interface is uniform at every level.

**What went right:**

- Recursive resource accounting: parents assign RAM/CPU quotas from their own
  budgets. A child cannot exceed its parent's resources. Quota returns when a
  child's PD session closes.
- Natural fault containment: a runtime environment "can destroy and possibly
  restart child components at any time."
- Policy at every level: parents choose which services to announce to children,
  creating trust domains.
- Attack surface reduction: components get only the access needed for their
  purpose, mediated by their parent.

**What went wrong:**

- **Deep hierarchies add latency.** Service requests that traverse multiple tree
  levels incur overhead. If two peers under different subtrees need to
  communicate, the request must route through common ancestors or the parent
  must broker the connection.

- **Reparenting is unnatural.** Moving a component to a different parent (e.g.,
  for load balancing) requires destroying and recreating it. The tree is static
  by design.

- **Quota fragmentation.** Parents must estimate children's needs upfront.
  Over-provision wastes resources; under-provision starves children. Bug
  evidence: `acpi: does not transfer memory quota to pci driver correctly`
  (GitHub #1550).
  `wm gives error when starting and killing subsystems from cli_monitor several times`
  (GitHub #2366). Quota management is correct in theory, fiddly in practice.

- **Parent as trust anchor.** A malicious or buggy parent can deny service to
  all children. Trust flows downward — you trust your parent and all ancestors.
  Problematic if any ancestor is compromised.

- **Limited adoption.** Norman Feske has been iterating since 2006 with steady
  progress, but adoption remains narrow outside Genode Labs.

#### Zircon: job tree

All processes live in a tree of Jobs. Each Job can contain child Jobs and
processes. The root Job is created at boot and passed to userboot. Processes can
only be created inside a Job. Jobs enforce policies and manage resource limits.

**What went right:**

- Grouping related processes for collective lifecycle management ("applications"
  spanning multiple processes).
- Hierarchical policy enforcement — parent Job policies cascade.
- Exception propagation: faults walk up the Job tree until handled (thread →
  process → job → parent job → root).
- Resource tracking at the Job level.

**What went wrong:**

- **Syscall surface bloat.** Zircon has "over 170 syscalls, vastly more than a
  typical microkernel." The Job/Process/Thread/Handle abstractions all require
  kernel support, expanding the attack surface.

- **Inconsistent isolation model.** The design "disallows signals because of
  isolation problems and then allows creating threads in other processes."
  Devhosts "combine several components within one process," weakening driver
  segmentation.

- **Component Manager as single point of failure.** If compromised, "they
  essentially have control over the entire system, even though it's not running
  in the kernel." The hierarchy concentrates authority at higher levels.

- **Security gaps in practice.** KASLR produced identical kernel addresses
  across reboots. CVE-2022-0882 showed a capability check bypass with explicit
  TODO comments acknowledging the weakness. A C++ vtable vulnerability enabled
  control-flow hijacking. Alexander Popov achieved arbitrary kernel code
  execution from an unprivileged component.

- **Still evolving.** The Job specification is "currently being iterated on and
  is subject to change." Resource-limiting aspects appear aspirational in parts.

**Key lesson:** hierarchical models provide clean answers to grouping, fault
escalation, and resource accounting, but they impose rigidity. Quota management
is harder in practice than in theory. Deep hierarchies add latency and
concentrate trust at the top.

---

### 2.3 Pragmatic middle ground (QNX, Minix 3)

**QNX** uses a traditional process model where the Process Manager runs
alongside the microkernel in `procnto`. Processes are isolated via MMU. Process
groups exist but are simpler than Linux's tree.

- _Right:_ Proven in safety-critical systems (automotive, medical, industrial).
  Fault containment works: "faults are confined to the program that caused
  them." Self-healing: failed drivers restart without rebooting.
- _Wrong:_ "QNX provides no way to set restrictions on a process or a group of
  processes." Less sophisticated than Linux cgroups or Zircon Jobs. The
  `procnto` monolith (kernel + process manager + memory manager) creates a
  larger TCB than a pure microkernel.

**Minix 3** uses a centralized Reincarnation Server that monitors all drivers
and servers via periodic heartbeat messages (1–5 second intervals).

- _Right:_ Automatic self-healing. "Since many bugs are transient, triggered by
  unusual timing, in most cases, restarting the faulty component solves the
  problem." No reboot required.
- _Wrong:_ Single point of failure (the Reincarnation Server itself).
  Polling-based detection adds seconds of latency. Stateful services lose state
  on restart. Centralized rather than hierarchical — all services report to one
  monitor.

**Key lesson:** pragmatic systems get real work done but accumulate
inconsistencies. QNX's 30+ years of safety-critical deployment prove that simple
process isolation is sufficient for many domains. But neither QNX nor Minix 3
offers a principled answer to "who is responsible for whom" beyond simple
process isolation.

---

## 3. Naming and Addressing

How does a Context refer to another Context (or a communication endpoint) in a
syscall?

### 3.1 Capability-only naming (seL4, EROS, Barrelfish)

A capability IS the name. There are no global IDs, string paths, or registries
in the kernel. You can only refer to things you hold capabilities for.

**seL4 endpoint badges.** A server creates an endpoint and distributes badged
copies (via `seL4_CNode_Mint`) to clients. The badge is an opaque integer
delivered to the receiver with every message. Clients cannot see or forge the
badge — only the receiver knows which badged capability was used.

- _Right:_ No ambient authority. Server can identify clients via badges.
  Fine-grained access control without global namespace.
- _Wrong:_ Badge space is limited (28 bits on 32-bit, 64 bits on 64-bit). "There
  is no way for a protection domain to look up the badge value of a badged
  endpoint capability." Badge allocation is entirely a userspace concern. No
  built-in service discovery — bootstrapping requires userspace framework.

**Dennis and Van Horn's insight (1966):** The process–capability relationship IS
the naming relationship. A process's identity is defined by its C-list
(capability list). Two processes with identical C-lists are interchangeable.
Process relationships are entirely determined by capability distribution, not
position in a tree.

### 3.2 Namespace-based naming (Plan 9, QNX, Hurd)

**Plan 9 per-process namespaces.** Every process has its own filesystem
namespace. `mount()` and `bind()` construct the view. Everything — devices,
networks, processes — is accessed through file operations.

- _Right:_ Heterogeneity handled elegantly (arch-specific binaries bind to
  `/bin` transparently). Security through visibility — namespaces are isolated.
  Remote access is transparent.
- _Wrong:_ "Not everything maps to files." Process creation is "too intricate,"
  network name resolution "doesn't map uniformly to hierarchies," and shared
  memory "raises architectural questions." Commercial failure — "only a system
  designed from the start with such coherence can achieve something so simple
  yet so powerful." The web and hardware heterogeneity outpaced the model.

**QNX pathname space.** Resource managers register paths. Processes find
services by pathname (`/dev/ser1`, `/net/...`).

- _Right:_ Familiar, discoverable.
- _Wrong:_ String-based names are ambient authority — any process that knows a
  path can attempt access. Requires separate ACL layer.

### 3.3 Endpoint/channel naming (Zircon, Mach, Singularity)

**Zircon.** Kernel knows only handles (integer indices into a per- process
handle table). Userspace component framework provides string- based `svc/`
directory for service discovery. Deliberate split: kernel naming is
capability-based, userspace naming is string-based.

- _Right:_ Clean separation. Kernel stays simple. Userspace can evolve naming
  conventions without kernel changes.
- _Wrong:_ The split means two naming systems to understand. The kernel's handle
  table plus the userspace directory creates a layered complexity.

**Singularity/Midori.** Channels with compile-time-verified state machine
contracts. Processes declare dependencies in manifests, verified at install
time.

- _Right:_ Statically verified communication contracts. No runtime surprises.
- _Wrong:_ Developers found explicit capability management awkward. "Big bags of
  capabilities" accumulated. Eliminating ambient authority means you can't read
  the clock without an explicit Clock capability. Never deployed externally.

**Key lesson across naming approaches:** there is a fundamental tension between
"no ambient authority" (capability-only, most secure) and "discoverability"
(namespaces/directories, most ergonomic). Every production system provides both:
a kernel-level capability mechanism and a userspace-level naming convention. The
question is where the boundary sits and how much the kernel enforces.

---

## 4. Fault Routing

When a Context faults unrecoverably, who gets the information?

### 4.1 Single-handler delegation (seL4)

Each thread has one fault handler endpoint. Fault → kernel blocks thread →
kernel sends IPC to handler endpoint. Handler receives fault type and context,
can fix the problem, and resumes the thread.

- _Right:_ Complete delegation to userspace. Multiple threads can share a
  handler (distinguished by badges). Handler can be any Context with the
  capability.
- _Wrong:_ Only one handler per thread — no kernel-provided escalation chain. If
  the handler itself fails, there's no fallback. "Resuming a thread without
  rectifying the underlying anomaly simply retriggers the fault repeatedly."
  Building supervision requires userspace orchestration; the kernel provides
  single-level delegation, not a tree.

### 4.2 Hierarchical escalation (Zircon)

Exception channels at three levels. Propagation: (1) process debugger, (2)
thread channel, (3) process channel, (4) debugger second-chance, (5) job
hierarchy (parent → grandparent → root). If nothing catches it, kernel kills the
process with `ZX_TASK_RETCODE_EXCEPTION_KILL`.

- _Right:_ Built-in escalation chain — no userspace framework needed. Debugger
  gets first and second chance. Job-level handlers enable application-wide fault
  policies.
- _Wrong:_ "Threads cannot handle their own exceptions." "Exception handles are
  non-copyable; only one active handler exists per exception." Fixed hierarchy —
  can't customize the escalation order. `zx_task_kill()` races with handler
  recovery.

### 4.3 Centralized monitor (Minix 3)

Reincarnation Server monitors everything via heartbeat polling.

- _Right:_ Simple. Proven to work for transient bugs.
- _Wrong:_ Single point of failure. Seconds of detection latency. Centralized,
  not composable.

### 4.4 Erlang supervision (software analogy)

Hierarchical supervisors and workers. Strategies: one-for-one (restart only the
failed child), one-for-all (restart all siblings), rest-for-one (restart
failed + all started after it).

- _Right:_ Hierarchical fault escalation — "if a particular level is incapable
  of correcting a given error, it eventually gives up and passes responsibility
  higher." Fast recovery (0.1–0.5 seconds for the failed process only). "Let it
  crash" — fail fast, recover through structure rather than defensive coding.
- _Wrong for OS design:_ Assumes processes are cheap and stateless enough to
  restart. OS processes have expensive state (mapped memory, open handles,
  in-flight I/O). Restart strategies assume the supervisor understands causal
  dependencies — in an OS, service dependencies are complex and sometimes
  cyclic.

**Key lesson:** the Erlang model works because its cost model allows it (cheap
processes, immutable state, pure message passing). The architectural insight
transfers — fault escalation needs a defined path, and "who supervises whom"
must be explicit — but the mechanism doesn't transplant directly.

**Cross-model observation:** seL4 provides single-level delegation (maximally
flexible, no escalation). Zircon provides fixed multi-level escalation (less
flexible, works out of the box). The choice mirrors the flat vs. hierarchical
relationship structure: flat models delegate escalation to userspace,
hierarchical models bake it in.

---

## 5. Resource Accounting

Who is "responsible" for a Context's resource consumption?

### 5.1 Capability-mediated budgets (seL4 MCS)

Scheduling Contexts (SCs) are kernel objects providing capability-based CPU time
access. Each SC has budget and period; the kernel enforces sporadic server
scheduling. A TCB binds to an SC to become schedulable.

**Passive server pattern:** servers run on _client_ scheduling contexts. When a
server blocks, its SC unbinds; during RPC it uses the caller's SC. This
naturally prevents priority inversion — the server inherits the client's
priority.

- _Right:_ CPU time as a capability. Per-component budgets — bugs can't exceed
  allocation. Passive servers avoid priority inversion.
- _Wrong:_ "If the replenishment data structure fills, replenishments are merged
  and the upper bound on execution is reduced." Higher `extra_refills` counts
  increase scheduling overhead through budget fragmentation. The sporadic server
  algorithm is complex. Verification still underway for MCS extensions. The
  many-to-many relationship between threads and scheduling contexts over time
  adds conceptual complexity.

### 5.2 Parent-mediated quotas (Genode)

Each component's PD session has a quota for RAM and capabilities. Parents
transfer quota from their own PD session. Total across all components cannot
exceed physical resources.

- _Right:_ Every byte accounted for. No over-allocation possible. Clean
  lifecycle — quota returns when child closes.
- _Wrong:_ Over-provisioning pressure — parents must estimate upfront. Quota
  can't be easily shared between peers. Real bugs in quota transfer logic
  (GitHub #1550, #2366). Dynamic workloads must pre-allocate worst-case, wasting
  resources on average.

### 5.3 Job/group-based limits (Zircon)

`zx_job_set_policy()` on an empty Job applies to all child processes and Jobs.
Policies combine top-down.

- _Right:_ Centralized policy for process groups.
- _Wrong:_ Resource limiting still described as aspirational. The specification
  is "currently being iterated on."

### 5.4 Per-application guarantees (Nemesis)

Nemesis provides "fine-grained guaranteed levels of all system resources
including CPU, memory, network bandwidth and disk bandwidth" per application.
Key design choice: execute most OS functionality in the application's own
process, not in shared servers.

**Self-paging:** each application handles its own page faults using its own
guaranteed physical frames. Eliminates kernel involvement and prevents QoS
crosstalk between applications.

- _Right:_ Solves the resource accounting crosstalk problem: "in
  microkernel-based systems, an application is typically implemented by a number
  of processes, most of which are servers performing work on behalf of more than
  one client, leading to enormous difficulty in accounting for resource usage."
  Self-paging isolates memory performance completely.
- _Wrong:_ Vertical structuring means duplicated code across applications.
  "Resources are multiplexed in space but not in time." Never achieved
  commercial adoption.

### 5.5 Unified resource tables (Composite OS)

Composite unifies capability tables and page tables as "resource tables."
Temporal Capabilities (TCaps) extend capability-based access control to CPU
time. The kernel has no scheduler — schedulers are user-level components.

- _Right:_ Thread migration-based IPC links execution of cross-domain requests
  to a single schedulable entity — clean accounting. TCaps enable different
  subsystems to use different scheduling policies while controlling
  interference.
- _Wrong:_ "Manual capability ID management." Flat primitives push complexity to
  user-level. Limited adoption and maturity.

**Key lesson:** the Nemesis crosstalk observation is profound and
under-appreciated. In any system with shared servers, a client's resource usage
gets charged to the server, not the client. seL4 MCS's passive server pattern
addresses this for CPU time (server borrows client's scheduling context). No
system has solved it cleanly for all resources simultaneously.

---

## 6. Academic Foundations

### 6.1 Actor model (Hewitt, 1973)

The fundamental unit is an actor: a concurrent entity with a stable address that
processes messages asynchronously. Actors can create new actors, send messages,
and designate behavior for the next message. Actors create "arbitrarily variable
topological relationships."

Key properties:

- **Flat addressing.** Actors have addresses (mailboxes), not hierarchical
  names.
- **Asynchronous by default.** No synchronous coupling.
- **Dynamic topology.** Addresses can be shared in messages, creating new
  communication paths at runtime.
- **No built-in supervision.** Erlang added this pragmatically.

Connection to capabilities: "If you can maintain the integrity of addresses, you
get capabilities for free" — Hewitt. Actor addresses ARE capabilities in
Miller's sense.

### 6.2 CSP and pi-calculus

**CSP (Hoare, 1978).** Communicating Sequential Processes. Synchronous
communication over channels. Critical evolution: the original 1978 paper had
processes name each other directly. Modern CSP (as in Go) uses anonymous
processes with named channels.

This mirrors the OS design question: do processes name each other (actor-style),
or do they name communication endpoints (channel- style)? Capability systems
answer: endpoints are the names, and holding one is the authority.

**Pi-calculus (Milner, Parrow, Walker, 1992).** Extends CSP by allowing channel
names to be communicated along channels — enabling dynamic reconfiguration of
communication topology. "The pi-calculus allows channel names to be communicated
along the channels themselves."

This is the formal model closest to how capabilities work. A capability is a
name that can be transmitted, and receiving one changes what you can communicate
with. The pi-calculus proves this is sufficient for universal computation —
validating capability-only naming as a complete mechanism.

### 6.3 Dennis and Van Horn (1966)

"Programming Semantics for Multiprogrammed Computations" introduced the term
"capability" and the "sphere of protection" (C-list). The key insight: a
capability simultaneously names and authorizes. This means process relationships
are entirely determined by capability distribution, not by position in any tree.

Two processes with identical C-lists are interchangeable from the system's
perspective. Identity IS the capability set.

### 6.4 Object-capability model (Miller, 2006)

Mark Miller's PhD thesis "Robust Composition" unified access control and
concurrency through object capabilities. Core principle: **"No Designation
Without Authority"** — a capability reference bundles the name with the
permission. You cannot name what you cannot access; you cannot access what you
cannot name.

"Capability Myths Demolished" (Miller, Yee, Shapiro, 2003) refuted three myths:

- **Equivalence Myth:** capabilities = ACLs. False — capabilities prevent
  confused deputy; ACLs don't.
- **Confinement Myth:** capabilities can't confine. False — EROS constructors
  prove otherwise.
- **Irrevocability Myth:** capabilities can't be revoked. False — caretaker and
  membrane patterns enable revocation.

**Midori lessons.** Microsoft's research OS took object capabilities to their
conclusion: type-safe, memory-safe, all authority through explicit parameters.
"Banned mutable statics entirely." Required explicit capability requests for
everything — even reading the clock needed a Clock object passed explicitly.

- _Right:_ Theoretically clean. No ambient authority backdoors.
- _Wrong:_ Developers found it awkward. "Big bags of capabilities" accumulated.
  Never deployed externally. Project cancelled, though ideas influenced .NET and
  Windows.

---

## 7. Cross-Cutting Analysis

### The spectrum and its failure modes

| Position         | Examples    | Strength                             | Failure mode                                                   |
| ---------------- | ----------- | ------------------------------------ | -------------------------------------------------------------- |
| Pure flat        | seL4, EROS  | Max flexibility, formal tractability | Ecosystem fragmentation, bootstrapping complexity, no grouping |
| Deep hierarchy   | Genode      | Clean accounting, fault containment  | Rigidity, quota fragmentation, latency, trust concentration    |
| Pragmatic middle | Zircon, QNX | Gets real systems running            | Inconsistencies, bloated surface, aspirational features        |

### The naming–relationship identity

Dennis and Van Horn (1966) and Miller (2006) showed that naming and relationship
structure are the same question. A capability is simultaneously a name and a
relationship edge. The capability graph IS the relationship graph.

This means: if you choose capability-only naming, the relationship structure is
implicit in the capability distribution. If you choose a tree, the tree
constrains capability distribution. The choice of naming mechanism IS the choice
of relationship model.

### The Nemesis crosstalk problem

In any microkernel with shared servers, resource accounting is misleading:
client A's work gets charged to server S, not to A. This is the deepest unsolved
problem in microkernel resource accounting.

Known mitigations:

- seL4 MCS passive servers (for CPU time only)
- Nemesis self-paging (for memory only)
- Composite thread migration (for CPU time)
- No system solves it for all resources

### Fault routing follows relationship structure

- Flat → single-level delegation (seL4). Userspace builds escalation.
- Tree → hierarchical escalation (Zircon, Genode). Kernel provides the chain.
- The choice is: escalation as mechanism (baked in) or escalation as policy
  (built in userspace).

### The bootstrapping problem is universal

Every capability system must answer: who gets the first capabilities?

- seL4: root task gets everything via BootInfo
- Zircon: userboot receives handles via bootstrap channel
- Genode: core → init → recursive delegation via XML config
- EROS: orthogonal persistence sidesteps it entirely

The bootstrap design reveals the true relationship model. In seL4, the root task
is a de facto parent of everything — the flat model has a hierarchical
bootstrap. In Genode, the bootstrap IS the tree.

### What succeeds in practice

Every production microkernel has converged on:

1. Capabilities in the kernel (unforgeable, per-process)
2. A naming convention in userspace (directories, registries)
3. Some form of hierarchical structure (even if informal)
4. Both synchronous and asynchronous communication
5. A designated "initial task" with all authority

The disagreements are about where the hierarchy lives (kernel or userspace), how
rigid it is, and how much the kernel enforces.

---

## 8. References

### Systems

- seL4: [sel4.systems](https://sel4.systems), Whitepaper, Tutorials, Microkit
  documentation
- Zircon/Fuchsia: [fuchsia.dev](https://fuchsia.dev), Jobs/Exception docs,
  RFC-0064 Box Knox
- Genode: [genode.org](https://genode.org), Foundations docs, GitHub issues
  #1550, #2366
- EROS: Shapiro & Smith, "EROS: A Fast Capability System" (SOSP '99)
- KeyKOS: Hardy, "The Confused Deputies" (1988)
- QNX: [qnx.com](https://www.qnx.com), System Architecture docs
- Minix 3: Tanenbaum, USENIX Login status report
- Plan 9: Pike et al., "The Use of Name Spaces in Plan 9" (1995)
- Nemesis:
  [Cambridge Nemesis project](https://www.cl.cam.ac.uk/research/srg/netos/projects/archive/nemesis/),
  Hand, "Self-Paging in the Nemesis OS" (OSDI '99)
- Composite: [composite.seas.gwu.edu](https://composite.seas.gwu.edu), Parmer,
  "Temporal Capabilities" (RTSS '17)
- Singularity/Midori: Hunt & Larus (2007), Duffy blog posts
- Barrelfish: Baumann et al., "The Multikernel" (SOSP '09)

### Foundational papers

- Dennis & Van Horn, "Programming Semantics for Multiprogrammed Computations"
  (CACM 1966) — introduced capabilities
- Miller, "Robust Composition" (PhD thesis, Johns Hopkins, 2006) —
  object-capability model
- Miller, Yee & Shapiro, "Capability Myths Demolished" (2003) — refuted common
  objections
- Shapiro & Weber, "Verifying the EROS Confinement Mechanism" (IEEE S&P 2000) —
  constructor confinement proof
- Hewitt, Bishop & Steiger, "A Universal Modular ACTOR Formalism" (1973) — actor
  model
- Hoare, "Communicating Sequential Processes" (CACM 1978) — CSP
- Milner, Parrow & Walker, "A Calculus of Mobile Processes" (1992) — pi-calculus
- Elphinstone & Heiser, "From L3 to seL4: 20 Years of L4 Microkernels"
  (SOSP 2013) — retrospective
- Popov, "A Kernel Hacker Meets Fuchsia OS" (2022) — Zircon security analysis
- Lyons et al., "Scheduling-Context Capabilities" (EuroSys 2018) — seL4 MCS

### Analysis and retrospectives

- Heiser, "seL4 Design Principles" (microkerneldude.org, 2020)
- seL4 Summit 2022/2023 presentations (sDDF, Microkit migration)
- Feske, Genode Foundations (continuously updated)
- Duffy, "Objects as Secure Capabilities" (blog, 2015)
- HdM Stuttgart, "Fuchsia: Rethinking OS Security Design" (2023)
