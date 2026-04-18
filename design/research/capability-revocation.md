# Capability Revocation

## 1. The Question

Once a capability has been granted, how can that authority be taken back?
Specifically: what mechanism allows the kernel (or an authorized entity) to
invalidate outstanding capabilities before the last holder has voluntarily
released them?

Four classes of mechanism appear in the literature and deployed systems:

1. **Close-only / reference counting** — no third-party revocation; an object
   survives until the last holder drops it.
2. **Authoritative destroy** — an entity with appropriate rights destroys the
   underlying object; all outstanding capabilities become dead.
3. **Derivation tracking** — the kernel tracks a tree of all extant
   capabilities; revoking a capability also revokes all descendants.
4. **Generation numbers** — each object carries a version counter; capabilities
   encode the version at issue time; incrementing the counter invalidates all
   outstanding capabilities at once without any traversal.

---

## 2. Survey of Existing Systems

### 2.1 Close-Only / Reference Counting

**POSIX.** File descriptors are reference-counted. Closing your own fd revokes
your access; other processes' fds remain valid. No mechanism exists for a third
party to forcibly invalidate another process's fd (short of killing that
process). This is intentional — POSIX does not express revocation.

**Zircon (most objects).** Handles are reference-counted. An object (VMO,
Channel, etc.) lives until the last handle is closed. Zircon does not have a
per-handle revocation primitive. The system's own "revocation" for most
resources is: the object owner holds an extra handle; closing that handle does
not revoke others' handles. The object persists for all remaining holders.
Revocation of authority requires either object destruction (§2.2) or userspace
interposition.

**Tradeoff context.** Close-only is O(1) for all operations — no kernel
bookkeeping beyond a reference count. It provides no mechanism for a third party
to enforce security policy changes or reclaim resources from misbehaving
holders. All revocation must be cooperative (holder drops its reference).

---

### 2.2 Authoritative Destroy

**Zircon.** An entity that holds a handle with `ZX_RIGHT_DESTROY` (or the last
handle is closed) destroys the kernel object. All other handles to it become
"dead handles." Dead handles return `ZX_ERR_BAD_HANDLE` on any subsequent
syscall. There is no per-handle revocation; the granularity is the entire
object. Holders can detect a dead handle by waiting on it
(`ZX_OBJECT_PEER_CLOSED`-style signals, or via `zx_object_wait_*`).

Source: Fuchsia Kernel Concepts documentation (handle.md); Zircon syscall API.

**Mach / XNU.** Mach port rights: destroying the receive right kills the port
itself. All send-right holders receive a DEAD_NAME notification in their port
namespace; their send right is replaced by a dead name. This is object-level
revocation — O(holders) for notification delivery. Fine-grained revocation of a
specific send right held by a specific task is not a native operation; the
application must arrange this through intermediaries.

Source: Mach Interface Generator (MIG) documentation; Carnegie Mellon Mach
technical reports.

**Genode.** Authority flows strictly parent-to-child in Genode's session
hierarchy. To revoke a child's capability, the parent destroys the session or
the child component. All capabilities granted within that session become
invalid. Revocation is hierarchical and top-down; siblings cannot directly
revoke each other's capabilities. No CDT or link-chain is maintained — the
session tree itself is the authority structure.

Source: Genode OS Framework design documentation (genode.org).

**Tradeoff context.** Authoritative destroy is simple to implement — no extra
kernel bookkeeping beyond reference counts and object state. Granularity is the
entire object: you cannot selectively revoke one holder's access without
affecting all holders. If the revoker still needs the resource themselves, they
must reconstruct it after destruction.

---

### 2.3 Derivation Tracking (CDT)

**seL4.** The Capability Derivation Tree (CDT) is a kernel-side data structure
that records parent-child relationships among capabilities. When a capability is
copied with `seL4_CNode_Copy` or derived (with rights attenuation) with
`seL4_CNode_Mint`, the new capability becomes a child of the source in the CDT.
The CDT entries are stored within kernel objects themselves (avoiding dynamic
allocation), forming an intrusive linked structure.

`seL4_CNode_Revoke` walks all descendants in the CDT subtree rooted at the
target capability and nulls each one out. Cost is O(number of derived
capabilities). For untyped memory, `seL4_UntypedRetype` first checks that all
children in the CDT have been removed before the untyped can be reused —
enforcing safe memory reclamation.

**Preemption concern.** In the baseline seL4 kernel, revocation is a
non-preemptible kernel operation. A server that has minted one capability per
connected client (N clients) causes N CDT nodes to be visited during revocation
without a preemption point. This is a known WCET (worst-case execution time)
concern in real-time contexts.

The seL4 MCS (Mixed Criticality System) kernel adds preemption within the
revocation traversal, allowing the operation to be interrupted and resumed.
Source: seL4 Reference Manual §2.3; Lyons et al., "Mixed-Criticality Support in
seL4" (RTAS 2018).

**Cost:** O(derived caps) in kernel mode. No published benchmark in isolation;
the seL4 Reference Manual notes the linear relationship explicitly.

**Barrelfish.** Barrelfish uses a CDT-style derivation tree, but capability
management is delegated to user-mode Monitor processes (one per core). The
kernel on each core maintains a local capability tree; cross-core copies are
tracked by the monitors using a two-phase acknowledgment protocol.

Revocation requires: (1) the revoking monitor sends revocation requests to all
monitors that hold copies; (2) each remote monitor traverses its local subtree
and acknowledges; (3) once all acknowledgments are received, the object is
reclaimed.

Measured cost (Nevill, 2012 master's thesis, ETH Zürich): local capability
operations ~100 µs; cross-core revoke (2-core round-trip) ~1 ms; scales with the
number of cores holding copies. The thesis concludes that fine-grained
cross-core capability revocation is expensive enough to be an architectural
concern, suggesting coarse-grained ownership epochs or batching as mitigations.

Source: Baumann et al., "The Multikernel: A new OS architecture for scalable
multicore systems" (SOSP 2009); Nevill, "Capability-Based Operating Systems for
Future Hardware" (ETH Zürich, 2012).

**SemperOS.** SemperOS (USENIX ATC 2019, Hille and Asmussen) extends
Barrelfish's approach to non-cache-coherent heterogeneous cores. Capabilities
between VPEs (virtual processing elements) are interlinked in a capability tree.
Revocation traverses the subtree rooted at the target, notifying each node.
Group-local revocation (same kernel domain) is roughly 2× the cost of M3
baseline; group-spanning revocation (cross-kernel domain) is roughly 3× local,
because messages must be sent between kernels and acknowledgments awaited.

Source: Hille and Asmussen, "SemperOS: A Distributed Capability System" (USENIX
ATC 2019).

**NOVA Microhypervisor.** NOVA maintains capability tables per protection domain
(PD) with `delegate` and `revoke` operations. `revoke` removes a capability from
a target PD's table. NOVA does not publish a CDT; revocation is targeted (caller
must know which PD holds the capability). Cross-PD revocation requires the
revoking entity to track delegations.

Source: Steinberg and Kauer, "NOVA: A Microhypervisor-Based Secure
Virtualization Architecture" (EuroSys 2010).

**Tradeoff context.** CDT gives authoritative, transitive revocation: revoking a
capability revokes everything derived from it, regardless of how many
intermediate hops occurred. The cost is proportional to the derivation depth and
breadth. Storing CDT linkage within kernel objects avoids dynamic allocation but
requires careful memory layout. In distributed settings, CDT traversal becomes a
distributed protocol with network-RTT-scale costs.

---

### 2.4 Generation Numbers (Allocation Count)

**Coyotos.** Coyotos (Shapiro, Johns Hopkins, ~2003–2007; successor to EROS)
uses an **allocation count** embedded in each kernel object and mirrored in
every capability that names that object. A capability is valid if and only if
the allocation count stored in the capability matches the current allocation
count of the object.

Revocation is performed by `coyotos.range.rescind`: the kernel increments the
object's allocation count. This operation is O(1) — no traversal, no
notification, no list of holders needed. Outstanding capabilities become stale
immediately (their stored count no longer matches) but are not actively nulled
out.

Stale capabilities are discovered lazily: when a stale capability is accessed
(during the "prepare" phase that resolves token to pointer), the kernel detects
the count mismatch and rewrites the capability slot to Null in place, without
marking the containing object as modified. This deferred write avoids a
dirty-page cascade.

**Overflow handling.** The Coyotos specification acknowledges the finite width
of the allocation count field and describes mitigation strategies to prevent
overflow.

Source: Shapiro, "Coyotos Microkernel Specification" (available at coyotos.org);
Shapiro and Doerrie, "Towards a Verified Trustworthy Kernel" (Johns Hopkins
technical report).

**EROS.** EROS (Shapiro et al., SOSP 1999) — Coyotos's predecessor — used a
different mechanism: **capability link chains**. Each object maintained a
doubly-linked list of all capabilities that named it ("holders list").
Revocation traversed this list and nulled each entry. Cost: O(number of
holders). EROS prepared capabilities (resolved to pointers for performance);
revocation converted them back to unprepared form by walking the holder chain.

The transition from EROS to Coyotos replaced the eager O(holders) link-chain
traversal with lazy O(1) allocation-count invalidation. The allocation count
approach eliminates the need to maintain the holders list, removing a
significant bookkeeping burden and making revocation-time cost independent of
the number of outstanding capabilities.

Source: Shapiro et al., "EROS: A Fast Capability System" (SOSP 1999); Shapiro,
"Differences Between Coyotos and EROS" (cap-lore.com).

**Storage systems (NASD / SCARED).** Some distributed storage systems (NASD —
Network-Attached Secure Disk; SCARED) use object version numbers to provide
revocation for storage capabilities. A capability authorizes access to a
specific version of a storage object; incrementing the object's version number
revokes all existing capabilities for that object instantly, without any
capability-list traversal. This is the same semantic as Coyotos's allocation
count, applied to the storage domain.

Source: Gobioff et al., "Security for Network Attached Storage Devices" (CMU
technical report, 1997); Gibson et al., "File Server Scaling with
Network-Attached Storage" (SIGMETRICS 1997).

**L4.Sec / versioned thread IDs.** Some L4 variants (notably L4.Sec, explored in
the seL4 research lineage) experimented with version-tagged thread identifiers,
where an IPC endpoint encodes a version number for the target thread. Destroying
and recreating a thread bumps the version, invalidating all outstanding endpoint
references. This applies the generation-number pattern to IPC endpoints rather
than arbitrary objects.

Source: Elphinstone et al., "L4 on the Raw Iron" (working notes); seL4 design
history documents.

**Tradeoff context.** Generation numbers make revocation O(1) at revocation time
— no traversal, no lock on a holder list, no notification. The cost is pushed to
the capability holder: every use of a capability requires a generation-number
check (typically one memory comparison). Stale capabilities are not eagerly
cleaned up; they occupy table slots until next access. If a capability is never
used after revocation, its slot remains occupied (wasted). Large-scale lazy
cleanup may require explicit GC passes if slot pressure is a concern.

---

## 3. Mechanism Comparison Table

| Mechanism             | Who can revoke        | Cost at revoke time        | Cost at use time         | Stale cap discovery  | Systems                       |
| --------------------- | --------------------- | -------------------------- | ------------------------ | -------------------- | ----------------------------- |
| Close-only (refcount) | Last holder only      | O(1)                       | O(1)                     | N/A (no revocation)  | POSIX fds, Zircon (most objs) |
| Authoritative destroy | Object owner          | O(1) destroy + O(h) notify | O(1) (dead check on use) | Eager (notification) | Zircon, Mach, Genode          |
| CDT traversal         | Any CDT ancestor      | O(derived caps)            | O(1) (no check per use)  | Eager (nulled)       | seL4, Barrelfish, SemperOS    |
| Link chain            | Any authorized entity | O(holders)                 | O(1)                     | Eager (nulled)       | EROS                          |
| Generation numbers    | Any authorized entity | O(1)                       | O(1) + comparison        | Lazy (on next use)   | Coyotos, NASD, L4.Sec         |

---

## 4. Tradeoffs

**Revocation scope.** CDT-based revocation is transitive: revoking a capability
revokes all capabilities derived from it, at any depth, across all holders.
Generation-number and object-destroy approaches are object-scoped: all
capabilities to a given object are invalidated together, but there is no concept
of "derived from" — the unit of revocation is the object, not the delegation
chain.

**Revocation time vs. use time.** CDT and link-chain mechanisms front-load the
cost: revocation time is proportional to the number of outstanding capabilities.
Generation numbers and object-destroy back-load or eliminate per-revocation cost
by deferring discovery to first use (generation numbers) or notifying lazily
(object-destroy notifications).

**Slot management.** Generation numbers do not reclaim table slots at revocation
time. Stale entries occupy capability-table slots until next access or an
explicit GC scan. In systems where table slots are scarce, this can create slot
pressure between revocation and the next use of each stale capability. CDT and
link-chain mechanisms null the slot immediately on revocation, freeing it for
reuse without waiting for the holder to access it.

**Distributed settings.** Generation numbers are attractive in distributed or
multi-core settings because revocation requires no cross-core communication —
the generation counter lives with the object, and each holder checks it locally
on use. CDT-based revocation in distributed settings (Barrelfish, SemperOS)
requires a cross-core protocol with acknowledgment rounds, incurring
millisecond-scale costs.

**Transitive delegation tracking.** CDT tracks the full derivation graph; any
intermediate holder can be revoked and all downstream capabilities vanish.
Without CDT, systems must either restrict delegation (Genode's hierarchical
model) or accept that revokers must know all direct holders (link chain,
object-destroy). Generation numbers do not provide transitive tracking — all
capabilities to an object share a single generation, regardless of how they were
derived.

**WCET / real-time.** CDT traversal in seL4's baseline kernel is
non-preemptible, creating unbounded execution time proportional to CDT size.
seL4 MCS addresses this with preemption points. Link chains share this property.
Generation numbers have constant revocation cost, making them WCET-friendly.

**Verification.** seL4's CDT-based authority model is formally verified (the
seL4 Access Control proof). Coyotos was designed with formal verification in
mind but the project was not completed. The correctness of generation-number
schemes depends on the counter being updated atomically with respect to
capability creation — a simpler invariant to verify than CDT structural
integrity.

**Bootstrapping and authority.** In CDT systems, the root capability is the root
of trust; revocation cascades downward through delegation. In generation-number
systems, the authority to call `rescind` on an object must be governed by some
separate mechanism (another capability, or kernel policy). The revocation
authority itself must be tracked — who holds the right to revoke?

---

## 5. References

- Shapiro et al., "EROS: A Fast Capability System," SOSP 1999.
- Shapiro, "Coyotos Microkernel Specification," coyotos.org (archived).
- Shapiro, "Differences Between Coyotos and EROS," cap-lore.com.
- seL4 Reference Manual v14.0.0, seL4.systems.
- Lyons et al., "Mixed-Criticality Support in seL4," RTAS 2018.
- Baumann et al., "The Multikernel: A New OS Architecture for Scalable Multicore
  Systems," SOSP 2009.
- Nevill, "Capability-Based Operating Systems for Future Hardware," Master's
  Thesis, ETH Zürich, 2012.
- Hille and Asmussen, "SemperOS: A Distributed Capability System," USENIX
  ATC 2019.
- Steinberg and Kauer, "NOVA: A Microhypervisor-Based Secure Virtualization
  Architecture," EuroSys 2010.
- Gobioff et al., "Security for Network-Attached Storage Devices," CMU Tech
  Report CMU-CS-97-185, 1997.
- Fuchsia Kernel Concepts Documentation — handles.md, zircon.googlesource.com.
- Mach Interface Generator (MIG) documentation, Carnegie Mellon.
- Genode OS Framework — design documentation, genode.org.
