# D104 — ResourceRequest dispatch: dual-path implementation

**Date:** 2026-04-26

**Question:** How does the kernel handle a ResourceRequest fault? D31 defines
two acquisition paths (pager chain and root allocation), but the dispatch
mechanics were not settled.

**Rests on:** D12 (fault delegation — faults route to handler), D21 (fault
handler at slot 0), D31 (resource acquisition through pager chain — root holds
pool, allocates or denies; non-root faults upward), D61 (four fault types —
ResourceRequest carries resource type + quantity), D80 (fault delivery protocol
— deliver_fault composes Message, enqueues to handler Field), D100 (fault
delivery mechanics — handler unavailable path).

**Status:** settled.

---

## Settles

### Dual-path dispatch

ResourceRequest has two paths, forced by D31's architecture. The paths are
structurally distinct, not a policy choice:

**Path 1 — Non-root Observer:** The kernel treats the ResourceRequest
identically to any other fault. `deliver_fault()` constructs a fault Message
(D61: x0 = resource type, x1 = quantity, x4 = ResourceRequest label), installs a
fault Observer cap in the handler's cap table, and enqueues the message into the
handler Field at slot 0. The handler — a supervisor Observer — receives the
request, decides whether to grant it (by splitting its own Space or Time and
installing the result into the faulting Observer via ObserverInstallCap), and
resumes the faulting Observer. This is the standard pager chain: the request
propagates upward through handlers until someone can satisfy it.

No new mechanism is needed. ResourceRequest for non-root Observers is a fault,
and fault delivery (D80, D100) already handles it.

**Path 2 — Root Observer:** The root Observer has no userspace handler. D31
states: "the root Observer's handler is the kernel — allocate from pool or
deny." The kernel detects the root case and allocates directly from the
SpaceManager pool without constructing a fault message or invoking
deliver_fault.

Root detection: the kernel checks whether the handler cap at slot 0 in the
faulting Observer's cap table is empty or invalid. An empty handler slot means
the Observer's fault chain terminates at the kernel. For the root Observer at
boot, no handler is installed — slot 0 is empty by construction.

Root allocation mechanics:

1. The kernel reads the ResourceRequest parameters (resource type, quantity)
   from the faulting Observer's saved registers (x0, x1 per D61).
2. For Space requests: the kernel calls `SpaceManager::allocate()` to obtain
   backing pages from the kernel pool. On success, it creates a Space object,
   installs the Space cap into the faulting Observer's cap table, places the new
   cap handle in x0 of the faulting Observer's saved registers, and resumes the
   Observer. On failure (pool exhausted): the kernel treats this as an
   unrecoverable root fault — log and PSCI SYSTEM_OFF (D100 root fault path).
3. For Time requests: same pattern — allocate from the kernel's Time pool,
   install the Time cap, resume.

### Why not always use fault delivery

The root Observer cannot fault-deliver to itself. D80's deliver_fault requires a
handler Field to enqueue into. The root Observer at boot has no handler Field —
it IS the top of the supervision hierarchy. D31 explicitly designates the kernel
as the root's resource provider: "allocate from pool or deny." The dual path is
not a design choice but a structural consequence of D31's boot architecture.

---

## Rejected alternatives

**Fault delivery to a kernel-internal Field:** The kernel could create an
internal Field and "deliver" the ResourceRequest to itself, then process it in
the scheduling loop. This adds unnecessary indirection — the kernel is already
in the fault handling path, has access to the pool, and can allocate
synchronously. A kernel-internal Field would create a message that the kernel
immediately dequeues and processes, adding allocation, enqueue, schedule,
dequeue, and processing steps where a direct allocation suffices.

**Always fault upward, even for root:** Requires someone above root to handle
resource requests. There is no one above root. D31 explicitly places the kernel
in this role for the root Observer.

**Root Observer pre-allocated with all resources:** Eliminates the need for root
ResourceRequest entirely. But D31 settled that resource acquisition is dynamic
through the pager chain — pre-allocation would require knowing the root
Observer's resource needs at kernel compile time, violating A3 (generic kernel —
workload-independent).

**Unified path with handler-presence check at delivery time:** Merging the paths
into deliver_fault with a "if no handler, allocate directly" branch inside fault
delivery conflates two distinct concerns: fault message construction (non-root)
and resource pool management (root). The dual path keeps each concern in its own
code path, making the root allocation logic testable independently of the fault
delivery infrastructure.

---

## Does NOT settle

- Resource type enumeration (which types beyond Space and Time can be requested
  via ResourceRequest — D61 leaves the type field generic)
- Root allocation failure policy beyond SYSTEM_OFF (whether the kernel should
  attempt partial allocation, or whether a more graceful degradation path
  exists)
- Resource quota enforcement (D31 says "allocate or deny" — the denial criteria
  for the root path are not specified beyond pool exhaustion)
