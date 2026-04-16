# Trust and Authority Models: Survey Across Real Kernels

How do kernels answer the question "is this operation permitted?" — and what
mechanism do they use to prove authority? This document surveys the design space
from ambient authority (Unix/POSIX) through pure capability systems (seL4, EROS)
to hybrids, covering how authority is granted, held, transferred, attenuated,
amplified, and revoked.

---

## Table of Contents

1. [Framing the Question](#1-framing-the-question)
2. [Ambient Authority (ACL-based) Systems](#2-ambient-authority-acl-based-systems)
3. [Capability-Based Systems](#3-capability-based-systems)
4. [System-by-System Survey](#4-system-by-system-survey)
5. [Tradeoffs](#5-tradeoffs)
6. [Measured Data](#6-measured-data)
7. [References](#references)

---

## 1. Framing the Question

A kernel enforces authority by answering, at every operation boundary, whether
the caller is allowed to perform that operation on that object. Two
fundamentally different mechanisms exist:

**Ambient authority:** The kernel checks who the caller is (process identity,
role, UID) against a policy stored separately from the invocation. The caller
names the object and the kernel looks up the ACL. Authority travels with the
subject, not the request.

**Capability:** The caller presents an unforgeable token that simultaneously
names the object and encodes the permitted operations. No separate lookup
needed. The possession of the token is the proof of authority.

These are not merely implementation variants — they have different expressivity
properties, different vulnerability surfaces, and different composability
properties.

### The Confused Deputy Problem

Named by Norm Hardy (1988), this is the canonical motivation for capabilities
over ACLs. When a privileged program (the "deputy") acts on behalf of a less
privileged caller, ambient authority means the deputy executes with _its own_
authority, not the caller's. If the caller can choose what object the deputy
operates on (via a filename, path, or identifier), it can trick the deputy into
misusing its authority — e.g., overwriting a protected billing file when
instructed to write to what appears to be a user-specified output path.

In a capability system, the caller must supply the capability to the target
object. The deputy can only operate on objects the caller already has authority
over, eliminating the attack surface structurally.

### Authority Operations

Across all systems, four operations on authority recur:

- **Grant:** Initial creation — who first receives authority over a new object?
- **Delegate:** Transfer or share authority with another subject.
- **Attenuate:** Pass authority with reduced rights (read-only, count-limited).
- **Revoke:** Withdraw previously delegated authority.

The difficulty and overhead of each operation differs significantly between
models.

---

## 2. Ambient Authority (ACL-based) Systems

### POSIX (Linux, BSD, macOS user space)

Authority is ambient: a process has a set of credentials (UID, GID, supplemental
groups) and the kernel checks these against per-object permission bits or
extended ACLs. No token is passed; the kernel resolves identity from the process
table.

**Grant:** `chown`, `chmod`, or setuid bits — out-of-band, stored in filesystem
metadata.

**Delegate:** Fork + exec. A child inherits the parent's credentials. Privilege
can be elevated via setuid executables (an amplification mechanism the kernel
provides explicitly because ambient authority cannot do it compositionally).

**Attenuate:** Not native. Security wrappers (`seccomp`, Linux namespaces,
`pledge`/`unveil` on OpenBSD) layer on top. These are ambient authority
_reduction_ mechanisms, not capability delegation.

**Revoke:** Drop UID/GID via `setuid`. Irrevocable in the sense that an already
delegated fork cannot have authority withdrawn by the parent after the fork.

**Confused deputy vulnerability:** Structural. TOCTOU (time-of-check/
time-of-use) races are the filesystem manifestation. Symlink attacks, path
injection, and privilege escalation via setuid deputies are recurring classes.

### Windows (NT kernel)

Authority model: security tokens attached to processes and threads, checked
against discretionary ACLs (DACLs) on every securable object. Each token
contains a SID (Security Identifier) for user identity and group memberships,
plus privilege bits (e.g., SeDebugPrivilege).

Token impersonation allows a server thread to temporarily assume a client's
lower-privileged token. This is a directed confused-deputy mitigation, but
impersonation is still ambient — it's a credential swap, not capability passing.

**Integrity Levels (Vista+):** Mandatory Integrity Control adds an ordered label
(Untrusted → Low → Medium → High → System) with a mandatory policy that prevents
low-integrity processes from writing to higher-integrity objects. Closer to a
type system than capabilities, but reduces some deputy attack surface.

---

## 3. Capability-Based Systems

A capability is an unforgeable token held by a subject that designates a kernel
object and a set of allowed operations on it. "Unforgeable" means: the kernel
mediates all operations on capabilities; user code cannot manufacture one from
an integer or memory address.

### Core Properties (from Miller, Yee, Shapiro 2003)

"Capability Myths Demolished" defines capability systems against three common
misconceptions:

1. **Equivalence Myth:** "ACL systems and capability systems are formally
   equivalent." False at the level of _usable_ security: capabilities compose
   and attenuate without kernel involvement; ACL modification requires
   out-of-band coordination and a shared namespace.

2. **Confinement Myth:** "Capabilities cannot enforce confinement." False: a
   capability system can confine a component by simply not granting it
   capabilities outside its designated scope.

3. **Irrevocability Myth:** "Capability-based access cannot be revoked." False:
   revocation is implementable via indirection (proxy objects), CDTs, or
   kernel-tracked derivation.

### Authority Amplification and Attenuation

**Attenuation:** Create a proxy or mint a copy of the capability with a subset
of rights. The holder of the attenuated copy can only invoke the permitted
subset. In seL4, `seL4_CNode_Mint` creates a derived capability with access
rights masked. In Zircon, `zx_handle_duplicate` with a subset of rights.

**Amplification:** A subject holding a "seal" capability can expand the rights
of an attenuated token. This is explicit and controlled — amplification does not
happen implicitly as in setuid. seL4 does not provide a kernel mechanism for
amplification; it is expressed by the creator retaining the unattenuated
capability and acting as a proxy.

---

## 4. System-by-System Survey

### 4.1 seL4

**Authority model:** Pure capability system. No ambient authority. Every kernel
object (thread, address space, endpoint, interrupt, memory frame) is accessed
only through a capability held in user space.

**Capability storage:** Capability nodes (CNodes) — kernel-managed arrays of
capability slots. A CNode is itself a kernel object, accessed via a capability.
CNodes compose into a tree forming a _capability space (CSpace)_. Each thread
has a root CNode; the kernel resolves capability paths by walking this tree.
Slot 0 of the root CNode is the thread's own TCB capability by convention.

**Types:** Capabilities are strongly typed. Each type encodes which operations
are valid: Endpoint caps support Send/Recv; TCB caps support
Resume/Suspend/Configure; Untyped caps support Retype. The kernel enforces type
at invocation — invoking an Endpoint cap with a TCB method is a type error, not
a privilege escalation.

**Grant mechanism:** The initial thread receives all untyped memory as a set of
Untyped capabilities. It reifies kernel objects (TCBs, CNodes, Endpoints,
Frames) by invoking `seL4_Untyped_Retype`. New objects have no capabilities; the
bootstrap process mints them. No ambient "root" authority — even the initial
thread must explicitly build its capability tree.

**Delegation:** Capabilities can be copied or moved between CNodes via
`seL4_CNode_Copy` / `seL4_CNode_Move`. The kernel records no provenance for
copies at the CNode level — all copies look equivalent to the kernel.

**Attenuation:** `seL4_CNode_Mint` produces a derived capability with a masked
rights field. For Endpoints, this yields send-only or receive-only caps.

**Revocation:** The Capability Derivation Tree (CDT) tracks parent-child
relationships at the level of untyped memory regions. When `seL4_CNode_Revoke`
is called on a capability, the kernel removes all capabilities derived from it
across all CSpaces in the system. The CDT is stored within the kernel objects
themselves (avoiding dynamic allocation), making traversal proportional to the
number of derived caps. For endpoints and non-memory objects, revocation (via
the CDT) effectively destroys all handles the server minted for a client
session.

**Formal verification status:** seL4's authority model is formally verified via
its Access Control proof. The C implementation is proven to correctly enforce
the specified access policy — the proof rules out entire classes of privilege
escalation by construction.

**Deployment:** HENSOLDT Cyber MCS (military), DARPA MORPHEUS, automotive
embedded systems (seL4 on microcontrollers for AUTOSAR).

### 4.2 EROS and KeyKOS

**Background:** KeyKOS (mid-1980s, Key Logic) is the direct predecessor of EROS
(Extremely Reliable Operating System, Shapiro et al., SOSP 1999). KeyKOS ran on
IBM System/370 hardware; EROS retargeted to x86.

**Authority model:** Pure capability system. All objects accessed through
capabilities (called "keys" in KeyKOS). Both address translation data structures
and process state are stored in nodes (the EROS equivalent of seL4 CNodes).

**Revocation:** KeyKOS introduced "rescinding" of capabilities. EROS uses a
prepared/unprepared capability distinction: on-disk capabilities are
"unprepared" (pure token), in-memory capabilities are "prepared" (resolved to
pointers for performance). Revocation converts prepared capabilities back to
unprepared form by walking capability link chains. Because EROS maintains these
chains per-object, revocation is O(references-to-object), bounded by the number
of holders.

**Performance emphasis:** EROS's primary contribution was demonstrating that a
capability system need not be slow. The SOSP 1999 paper measured:

- Cold IPC (seL4-generation baseline): ~342 cycles on Pentium II
- EROS hot IPC path: ~112 cycles on Pentium II

KeyKOS was notable for never having been successfully penetrated in production
use.

### 4.3 Coyotos

**Background:** Successor to EROS, begun ~2003 when synchronous IPC problems
were identified. Work halted on EROS; Coyotos aimed for formal verification.
Source: Jonathan Shapiro, Johns Hopkins University.

**Key change from EROS:** Addressed the "IPC security issues" discovered by
Shapiro in 2003 regarding synchronous rendezvous IPC primitives and
capability-passing races. Coyotos introduced a restricted IPC model to prevent
certain timing-channel and confused-deputy attacks that EROS (and L4) were
vulnerable to in theory.

**Authority model:** Same core capability approach as EROS. Coyotos added
explicit "opaque" capabilities that could not be delegated (preventing
transitive leakage).

### 4.4 Zircon (Fuchsia)

**Authority model:** Handle-based capability system. A _handle_ is an integer
token (process-local) that references a kernel object and carries a rights mask.
Multiple handles to the same object with different rights can coexist in one
process.

**Rights mask:** Per-handle bitmask. For example: ZX_RIGHT_READ, ZX_RIGHT_WRITE,
ZX_RIGHT_EXECUTE, ZX_RIGHT_DUPLICATE, ZX_RIGHT_TRANSFER. Invocations are checked
against the handle's rights mask — passing a read-only handle to a write syscall
fails.

**Delegation:** Handles are transferred via channels (`zx_channel_write` with
`handles` array; `zx_channel_read` receives them). On transfer, the sender loses
the handle (move semantics). `zx_handle_duplicate` creates an additional
reference with equal or fewer rights before transfer if sharing is needed.

**Attenuation:** `zx_handle_duplicate` takes a rights mask — the duplicate may
only reduce rights (never increase). This is kernel-enforced monotonic
attenuation.

**No ambient authority:** The Fuchsia design documentation is explicit: "No
application has ambient authority." The Component Framework passes handles for
specific capabilities (protocols, directories) from parent to child component.
Components cannot acquire capabilities except through what they receive from
their parent environment.

**Revocation:** No kernel-level revocation primitive. Revocation is accomplished
at the user-space component level — the intermediary that granted the handle can
refuse to act on messages, or the kernel object itself can be destroyed,
invalidating all handles (they become "dead handles"). There is no seL4-style
CDT traversal to forcibly reclaim handles in other processes.

**Source:** Fuchsia Kernel Concepts documentation, handle.md.

### 4.5 Genode

**Authority model:** Hierarchical pure capability system. Components form a
tree; each parent creates children from its own resources. The parent's
authority over children is absolute — a parent can destroy a child at any time
to reclaim resources.

**Initial capability:** At creation, a child receives a single _parent
capability_, enabling RPC to its parent only. All further capabilities must be
explicitly granted by the parent (or obtained via the parent from the broader
system).

**Delegation without diminishment:** A capability holder that delegates a
capability to a child retains full authority itself. The child gains a copy, not
a transfer. Authority in Genode is monotonically addable (within the tree), not
transferred.

**Authority structure:** Service sessions are capabilities. A child requests a
service by sending a session request up the tree; the parent decides whether to
route it to a sibling server, create a local server, or deny it. Policy is
applied at each level — the parent acts as an authority broker.

**Revocation:** The parent can close sessions (destroying the capability) or
destroy the child entirely. No global CDT. Revocation propagates top-down
through the parent's explicit action.

**Implementation detail:** On seL4 and NOVA microkernels, Genode's capability
model is implemented atop the underlying kernel capabilities. On Genode's own
"base-hw" kernel, capabilities are kernel-native. The framework's authority
model is consistent regardless of the underlying mechanism.

**Source:** Genode Foundations documentation (25.05), "Capability-based
security" section; "Recursive system structure" section.

### 4.6 Mach / XNU (macOS, iOS)

**Authority model:** Port-rights system. A _port_ is a message queue. Rights to
a port are:

- **Receive right:** Dequeue messages; create send rights. At most one holder.
- **Send right:** Enqueue messages. Many holders.
- **Send-once right:** Enqueue exactly one message, then extinguished.

Port rights are capabilities: unforgeable, held per-task in a port-name table
(the task's namespace). They are passed in Mach messages.

**Authority semantics:** Holding a send right to a port is authority to invoke
the service listening on that port. Services interpret messages and enforce
their own authority rules on top. The kernel only enforces that the sender
_holds_ a send right — it does not validate what the message means.

**Delegation:** `mach_port_insert_right` transfers a right into another task's
namespace. `mach_msg` can carry inline port rights (in the message descriptor).

**Revocation:** Destroying the receive right kills the port — all send rights to
a dead port transition to "dead names", and senders receive dead-name
notifications. Fine-grained revocation (invalidating specific send rights
without destroying the port) is not kernel-native; the service must refuse
connections.

**XNU additions:** Vouchers (iOS 8+, macOS 10.10+) — metadata tokens that carry
bank account identity, attribution, and QoS information alongside messages.
Vouchers are not authority tokens in the capability sense, but they extend the
message to carry context that services can use for fine-grained policy.

**Historical note:** Mach's port model influenced nearly every later microkernel
IPC design, though most moved away from Mach's multiprocessing complexity.

### 4.7 Barrelfish

**Authority model:** Per-core capability spaces. Each core has its own local
capability table managed by its CPU driver. Capabilities are not shared across
cores; cross-core capability operations are handled by per-core user-mode
_Monitor_ processes.

**Monitors are trusted:** They can serialize a capability to bits and
reconstruct it on the remote core. This is a significant deviation from strict
kernel enforcement — trust is placed in a user-mode process (though the Monitor
is part of the trusted computing base).

**Revocation in a distributed setting:** The SOSP 2009 Barrelfish paper and the
work by Nevill ("Capabilities in Barrelfish") describe a protocol:

- Each capability has an _owner core_.
- Revoke sends notifications to all cores that hold copies.
- Concurrent retypes on other cores must be coordinated with the revoke — the
  protocol uses two-phase messaging to handle races.
- Cost: proportional to the number of cores that hold a copy; involves message
  round-trips.

**Performance implication:** Cross-core capability operations are significantly
more expensive than local operations. The design trades revocation simplicity
for per-core isolation and absence of shared data structures.

### 4.8 L4 Family (L4.x86, OKL4, NOVA)

**L4 (Jochen Liedtke era, 1993-2001):** Original L4 had minimal authority model
— threads were identified by their thread ID (a global integer), and authority
was implicit in knowing a thread ID. No formal capability system. Memory
mappings conferred authority over address spaces (sigma0 "grant" protocol).

**OKL4 (Open Kernel Labs, 2008+):** First L4-family kernel to ship capabilities
in production (v2.1). Added a capability table per thread controlling kernel
object access. Deployed in billions of mobile SoCs (Qualcomm baseband).

**NOVA (Udo Steinberg, 2010):** Research hypervisor/microkernel with a
capability system based on "portals" (typed endpoints with protection domains).
Capabilities are typed; invocation checks type. NOVA uses a capability
delegation model similar to seL4's Mint.

**seL4 lineage:** seL4 is derived from L4 and EROS. It replaced L4's implicit
authority with an explicit, formally specified capability system.

### 4.9 QNX Neutrino

**Authority model:** Hybrid — POSIX ambient authority (UID/GID) plus a
capability table for privileged operations. QNX uses _abilities_ (called
`procmgr_ability` since QNX 7.0) which are per-process rights for operations
that would otherwise require root (e.g., bind to privileged ports, change
scheduling priority). This is capability-like but not a full object-capability
system: abilities are coarse-grained bit flags, not object-specific tokens.

**Resource manager model:** QNX's primary IPC mechanism (messages to resource
managers) uses POSIX credentials for authorization — the receiving server
receives the sender's UID/GID and applies its own policy. Not a capability
passing model.

**Summary:** QNX occupies a position between ambient authority and capabilities:
it fragments root privilege into discrete abilities, but retains ambient
credential-based authorization as the primary mechanism.

---

## 5. Tradeoffs

### 5.1 Confused Deputy Immunity

| System       | Immune? | Mechanism                                        |
| ------------ | ------- | ------------------------------------------------ |
| seL4         | Yes     | Caller must hold cap to target object            |
| EROS/Coyotos | Yes     | Same: cap must be passed in invocation           |
| Zircon       | Yes     | Handle must be in caller's handle table          |
| Genode       | Yes     | Session caps grant only what parent authorized   |
| Mach/XNU     | Partial | Service interprets message; port rights are caps |
| QNX          | No      | POSIX credentials are ambient                    |
| POSIX/Linux  | No      | UID/GID are ambient; setuid is the deputy vector |

### 5.2 Revocation Complexity

| System     | Revocation mechanism                          | Cost            |
| ---------- | --------------------------------------------- | --------------- |
| seL4       | CDT traversal; `seL4_CNode_Revoke`            | O(derived caps) |
| EROS       | Capability link chain traversal               | O(holders)      |
| Zircon     | Destroy kernel object (invalidates handles)   | O(1) per object |
| Zircon     | Per-handle fine-grained revocation            | Not native      |
| Genode     | Parent destroys child or closes session       | Top-down        |
| Mach       | Destroy receive right; dead-name notification | O(holders)      |
| Barrelfish | Cross-core notification protocol              | O(cores × RTT)  |

### 5.3 Delegation Semantics: Move vs. Copy

| System | Delegation semantics                                    |
| ------ | ------------------------------------------------------- |
| seL4   | Copy (Mint/Copy) or Move                                |
| Zircon | Move (transfer removes from sender); Duplicate for copy |
| Genode | Copy (parent retains authority)                         |
| Mach   | Copy (multiple send rights to same port)                |
| EROS   | Copy (keys duplicated freely)                           |

Move semantics (Zircon) make capability accounting exact — a given token exists
in exactly one place at a time, simplifying revocation. Copy semantics (seL4,
EROS) require tracking all copies for revocation via the CDT.

### 5.4 Typed vs. Untyped Capabilities

| System       | Typed? | Enforcement                                     |
| ------------ | ------ | ----------------------------------------------- |
| seL4         | Yes    | Kernel enforces type at invocation              |
| EROS         | Yes    | Type encoded in key                             |
| Zircon       | Yes    | Object type determines valid syscalls           |
| Genode       | Yes    | Session type determines valid interface         |
| Mach         | No     | Port is untyped queue; service defines protocol |
| L4 (classic) | No     | Thread IDs are untyped names                    |

Typed capabilities prevent type confusion attacks at the kernel boundary. Mach's
untyped ports push the burden of type enforcement into service code.

### 5.5 Capability Namespace: Flat vs. Tree vs. Per-Process

| System     | Namespace shape                    | Implications                                    |
| ---------- | ---------------------------------- | ----------------------------------------------- |
| seL4       | Tree of CNodes (per-thread CSpace) | Hierarchical; lookup walks the tree             |
| Zircon     | Flat handle table (per-process)    | O(1) lookup; no hierarchy                       |
| EROS       | Flat node (c-list per process)     | Shallow                                         |
| Genode     | Tree (session hierarchy)           | Authority structure mirrors component hierarchy |
| Mach       | Flat port-name table (per-task)    | O(1) lookup                                     |
| Barrelfish | Per-core flat tables               | Replicated; cross-core is RPC                   |

Tree-shaped namespaces (seL4 CSpaces) allow capability spaces to be recursively
composed and support delegation of sub-spaces. Flat tables (Zircon, Mach) have
simpler lookup but provide no structural relationship between capabilities.

### 5.6 Amplification: Is Privilege Escalation Expressible?

In ambient authority systems, amplification is provided via setuid (Linux) or
impersonation (Windows) — explicit kernel mechanisms needed because the
authority model cannot express it compositionally.

In capability systems, amplification requires holding a more privileged
capability and choosing to invoke it. A process cannot amplify its own authority
— it can only use what it holds. Escalation without explicit grant is impossible
by construction (in formally verified systems, provably so).

The tradeoff: capability systems require explicit initial distribution of
authority (bootstrapping problem). All authority must be derived from the
initial grant; there is no "root" ambient fallback. This is by design but
requires careful bootstrap engineering.

### 5.7 IPC Integration

Most capability-based microkernels integrate capability passing with IPC:

- **seL4:** Capabilities are passed in message registers alongside data.
  `seL4_CNode_Copy` is a syscall; capability transfer via IPC embeds delegation
  into the communication act.
- **Zircon:** Handles transferred as an array field in `zx_channel_write`.
- **Mach:** Port rights carried in message descriptors inline with message data.
- **EROS/KeyKOS:** Keys in a fixed-size "keyrings" array of the invocation.

Systems that separate IPC from capability transfer (hypothetically: passing
integer tokens that the receiver must look up) introduce TOCTOU windows and lose
the atomicity guarantees.

---

## 6. Measured Data

### 6.1 IPC Round-Trip with Capability Passing

| System         | Hardware          | Round-trip (cycles) | Notes                            |
| -------------- | ----------------- | ------------------- | -------------------------------- |
| EROS           | Pentium II 300MHz | ~112                | SOSP 1999 hot path               |
| seL4 (AArch64) | Cortex-A57        | ~460                | seL4 benchmark suite (fast path) |
| seL4 (x86-64)  | Core i7           | ~340                | seL4 benchmark suite (fast path) |
| Zircon         | Cortex-A53        | ~1400               | Fuchsia syscall benchmark        |
| L4 (Jochen's)  | Pentium 120MHz    | 115                 | L4 1993 benchmark                |
| Mach 3         | i486 25MHz        | ~2500               | Mach IPC benchmark (Bershad)     |

Mach's high IPC cost was a primary motivation for the L4 redesign. EROS
demonstrated that a capability system need not add overhead relative to L4
baselines, countering the perception that capability checking was expensive.

Note: These numbers are not directly comparable (different hardware generations,
different benchmarking methodologies). They indicate order-of-magnitude
relationships, not absolute latencies.

### 6.2 seL4 Capability Lookup

seL4 capability resolution walks a CNode tree on every syscall. For a typical
two-level CSpace (root CNode + leaf CNode), this is two memory accesses per
capability lookup. The seL4 team measured capability lookup as a minority of
total IPC cost on cached paths; the dominant cost is context switch and message
copy, not capability resolution.

### 6.3 CDT Revocation Cost in seL4

No published benchmark for CDT revocation traversal in isolation. The seL4
Reference Manual notes that revocation is O(number of derived capabilities),
which can be O(1) if only one copy was ever made, or O(clients) if a server
minted one capability per connected client. For a server with 1000 clients,
revocation requires visiting ~1000 CDT nodes. This is kernel-mode work with no
preemption points (in the non-MCS kernel), meaning revocation of heavily-shared
objects can cause unbounded kernel execution time — a known WCET concern in
real-time contexts.

The seL4 MCS (Mixed Criticality System) kernel adds preemption to revocation to
bound kernel execution time.

### 6.4 Barrelfish Cross-Core Capability Revocation

Nevill (2012 master's thesis, ETH Zürich) measured:

- Local capability operations: O(100µs)
- Cross-core revoke (2-core round-trip): O(1ms)
- Cross-core revoke scales with number of cores that hold copies

The paper concludes that fine-grained capability revocation across many cores is
expensive enough to be an architectural concern — suggesting coarse-grained
ownership epochs or batching revocations.

---

## References

- Norman Hardy. "The Confused Deputy (or, Why Capabilities Might Have Been
  Invented)." _Operating Systems Review_, 22(4), October 1988.
  https://css.csail.mit.edu/6.858/2015/readings/confused-deputy.html

- Mark S. Miller, Ka-Ping Yee, Jonathan Shapiro. "Capability Myths Demolished."
  Technical Report SRL2003-02, Johns Hopkins University, 2003.
  https://srl.cs.jhu.edu/pubs/SRL2003-02.pdf

- Jonathan Shapiro, Jonathan Smith, David Farber. "EROS: A Fast Capability
  System." _Proceedings of the 17th ACM SOSP_, 1999.
  https://courses.cs.washington.edu/courses/cse551/19wi/readings/eros-sosp99.pdf

- Gerwin Klein et al. "seL4: Formal Verification of an OS Kernel." _SOSP 2009_.
  https://www.sigops.org/s/conferences/sosp/2009/papers/klein-sosp09.pdf

- Gerwin Klein et al. "Comprehensive Formal Verification of an OS Microkernel."
  _ACM Transactions on Computer Systems_, 32(1), 2014.
  https://sel4.systems/Research/pdfs/comprehensive-formal-verification-os-microkernel.pdf

- Toby Murray. "Verified Protection Model of the seL4 Microkernel."
  http://flint.cs.yale.edu/cs428/doc/seL4cap.pdf

- seL4 Reference Manual (latest).
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf

- Fuchsia. "Zircon Handles."
  https://fuchsia.dev/fuchsia-src/concepts/kernel/handles

- Norman Feske. _Genode OS Framework Foundations_ (25.05).
  https://genode.org/documentation/genode-foundations-25-05.pdf

- Genode. "Capability-based security."
  https://genode.org/documentation/genode-foundations/22.05/architecture/Capability-based_security.html

- Andrew Baumann et al. "The Multikernel: A New OS Architecture for Scalable
  Multicore Systems." _SOSP 2009_. (Barrelfish)

- Simon Nevill. "Capabilities in Barrelfish." Master's thesis, ETH Zürich, 2012.
  https://barrelfish.org/publications/nevill-master-capabilities.pdf

- Apple Developer. "Mach Overview." Kernel Programming Guide.
  https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html

- Udo Steinberg, Bernhard Kauer. "NOVA: A Microhypervisor-Based Secure
  Virtualization Architecture." _EuroSys 2010_.

- Wikipedia. "Confused deputy problem."
  https://en.wikipedia.org/wiki/Confused_deputy_problem

- Wikipedia. "Ambient authority."
  https://en.wikipedia.org/wiki/Ambient_authority
