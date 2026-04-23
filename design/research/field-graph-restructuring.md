# Field Graph Restructuring: Split and Combine Operations

## 1. The Question

At runtime a set of kernel IPC/delivery objects may need to be restructured: one
object split into two (e.g., separating a subset of IRQ routes or badge ranges
onto their own field), or multiple objects combined into one (e.g., aggregating
interrupt and RPC messages onto a single receive point). This question covers:

1. **What operations are available?** How do real systems express split and
   combine?
2. **What happens to existing capabilities** (send caps held by other parties)
   when the topology changes?
3. **Crash recovery.** If the holder of a newly split or combined object
   crashes, what happens to the source objects and their capabilities?
4. **Generalization.** Do these mechanisms in practice serve only the use case
   that introduced them, or do they generalize to broader routing
   reorganization?

---

## 2. Survey

### 2.1 Mach / XNU — Port Sets (Combine, Receive-Side Only)

**Mechanism.** A _port set_ aggregates receive rights into a single receive
point. `mach_port_move_member(task, port_name, set_name)` moves a named receive
right into the set. The receive right is no longer directly receivable — all
messages deposited to that port are retrieved via `mach_msg` on the set.

**Effect on send rights.** Send rights held by other tasks are **not affected**.
Senders continue to name the individual port; they have no visibility into the
set membership. From the sender's perspective nothing has changed: messages sent
to the individual port appear at the set's receive call, identified by
`msgh_local_port` in the message header.

**Effect on the original receive right.** The receive right is "absorbed" into
the set. It cannot be used for direct receive while it is a set member. It can
be extracted from the set (moved out) by calling `mach_port_move_member` with
`set_name = MACH_PORT_NULL`, which restores it to a standalone receive right.

**Dynamic membership.** Members can be added and removed while a receive from
the set is in progress. `mach_port_extract_member` removes a named port from a
set.

**Port set destruction.** Destroying a port set (via `mach_port_destroy` on the
set name) causes all member receive rights to be removed from the set and then
destroyed. Outstanding send rights to those ports become _dead names_. Holders
receive `MACH_NOTIFY_DEAD_NAME` if they registered for dead-name notification.
Messages queued on member ports at destruction time are dropped.

**No split primitive.** Mach has no kernel-level operation to split a port into
two. To route a subset of senders to a new port, the server must revoke the
relevant send rights from clients and re-distribute new send rights pointing to
the new port. This is a userspace-level operation.

**Sources:**

- [GNU Mach Reference Manual — Port Sets](https://www.gnu.org/software/hurd/gnumach-doc/Port-Sets.html)
- [GNU Mach Reference Manual — Port Destruction](https://www.gnu.org/software/hurd/gnumach-doc/Port-Destruction.html)
- Apple Kernel Programming Guide — Mach IPC

---

### 2.2 Zircon (Fuchsia) — Port Object (Fully Non-Destructive Registration)

**Mechanism.** `zx_object_wait_async(handle, port, key, signals, options)`
registers an object's signals with a port. The original handle is **not moved or
consumed** — it remains fully valid in the caller's handle table with all
original rights. Multiple ports can have the same object registered
simultaneously. The port receives a `zx_port_packet_t` containing the caller's
`key` when the registered signals fire.

**Packet types aggregated.** A single port can aggregate: user packets (explicit
`zx_port_queue`), signal packets from kernel objects (channels, VMOs, timer
objects, process objects), interrupt packets from hardware IRQ objects
(`zx_interrupt_bind(irq, port, key)`), and pager page-request packets.

**Interrupt binding.** `zx_interrupt_bind(irq_handle, port_handle, key, 0)`
routes a hardware interrupt object's delivery to a port. The interrupt object
itself is not consumed — it can be unbound and re-bound to a different port
later. If the interrupt fires while unbound, no packet is delivered.

**Effect on existing handles.** Because the original handle is not moved, all
existing operations on it continue normally. Destroying the port does **not**
destroy any registered objects. Pending async wait registrations are cancelled
when the port is destroyed.

**Crash recovery.** If the process holding the port handle crashes, the port is
destroyed. Registered wait_async operations are cancelled; any queued but unread
packets in the port are dropped. The underlying kernel objects (channels,
interrupt objects, etc.) remain intact and continue to function independently —
any process holding handles to them can continue using them.

**No split primitive.** Zircon has no "split an interrupt object by IRQ range"
primitive. Interrupt objects are created individually, one per hardware
interrupt, via `zx_interrupt_create`. Routing a subset of IRQs to a new handler
requires creating new interrupt objects and binding them to a new port.

**Sources:**

- [Fuchsia `zx_port_wait` syscall reference](https://fuchsia.dev/fuchsia-src/reference/syscalls/port_wait)
- [Fuchsia Zircon kernel objects](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/objects)
- [Zircon `zx_interrupt_bind`](https://fuchsia.dev/fuchsia-src/reference/syscalls/interrupt_bind)

---

### 2.3 Windows NT — I/O Completion Ports (Non-Destructive Association)

**Mechanism.**
`CreateIoCompletionPort(file_handle, completion_port, key, concurrency)`
associates an existing file/socket handle with a completion port. The original
handle is not moved or consumed. When an I/O operation completes on the
associated handle, a completion packet (with the caller's `key` value) is posted
to the port. `GetQueuedCompletionStatus(port, ...)` dequeues packets.

**Effect on existing handles.** The file/socket handle remains fully usable
after association. I/O operations on it continue normally; completions are
delivered to the port rather than inline.

**Destruction.** Closing the completion port does not close or destroy
associated file handles. Pending but undelivered completion packets are dropped.

**Multiple ports.** A single handle cannot be associated with multiple
completion ports simultaneously in the original Win32 API. Attempting to
re-associate moves the association.

**Sources:**

- Windows I/O Completion Ports — Microsoft documentation
- _Windows Internals_ (Russinovich et al.), Part 2, Chapter on I/O system

---

### 2.4 Barrelfish — RAM Capability Split (Destructive on Original)

**Mechanism.** Barrelfish represents physical memory as typed capabilities. A
RAM capability covers a contiguous range. The `cnode_split` or equivalent
operation splits a RAM capability into two equal halves (each covering half the
address range). The original capability is **consumed** — it ceases to exist
after the split. The two new capabilities are children of the original in the
kernel's Capability Derivation Tree (CDT).

**No merge.** Barrelfish does not support directly merging two adjacent
capabilities back into one. Merging requires tracking whether the two halves
originate from the same parent and whether all descendants of both halves have
been freed. In practice this tracking is maintained by a user-level memory
manager, which waits until both halves' descendant trees are clean before
recombining. There is no single kernel operation for merge.

**Cross-core revocation.** If copies of the original capability exist on other
cores, those copies must be revoked before the split can proceed. The revoking
monitor sends revocation requests to each core holding a copy; each core
acknowledges. Measured cost: local operations ~100 µs; cross-core round-trip ~1
ms (Nevill, ETH Zürich thesis, 2012).

**Crash recovery.** Because the split consumes the original, the CDT records the
parent-child relationship. If the holder of one split half crashes (dropping
their capability), the other half remains valid. To recombine, a surviving
entity must hold (or be able to revoke) both halves' CDT subtrees.

**Sources:**

- Nevill, "Capability-Based Operating Systems for Future Hardware," ETH Zürich
  Master's Thesis, 2012.
  [barrelfish.org](https://barrelfish.org/publications/nevill-master-capabilities.pdf)
- Baumann et al., "The Multikernel," SOSP 2009.
  [PDF](https://people.inf.ethz.ch/troscoe/pubs/sosp09-barrelfish.pdf)

---

### 2.5 seL4 — Untyped Retype (Non-Destructive Parent, Watermark Allocation)

**Mechanism.**
`seL4_Untyped_Retype(untyped_cap, type, size, root, node, depth, offset, num_objects)`
creates child kernel objects backed by a sub-range of the untyped memory region.
The **parent untyped capability remains valid** and continues to cover the full
original range.

**Watermark semantics.** Each untyped capability maintains a single watermark
pointer. Retype calls advance the watermark; memory before the watermark is
allocated (children exist for it). Memory after the watermark is free. You
cannot retype the same sub-range twice. There is no "split" that subdivides the
untyped capability itself — the untyped is not consumed; it is a parent that
creates children.

**Reclaiming allocated ranges.** To reuse a sub-range, all capabilities derived
from that untyped (all CDT descendants) must be revoked via `seL4_CNode_Revoke`.
After revocation, the watermark can be reset, freeing the range. This is
O(number of derived capabilities) in the baseline kernel (non-preemptible), and
has preemption points in the MCS kernel.

**Effect on existing capabilities.** The untyped capability itself is not
affected by retype — it can still be copied and delegated. Children's
capabilities can be further copied and delegated; the CDT tracks all of them for
future revocation.

**Split summary.** seL4 has no `split_untyped` operation in the sense of "divide
the parent into two independent sub-range parents." The parent always covers the
full original range; children are the sub-allocations. Partitioning authority
over sub-ranges requires delegating the untyped to sub-managers who each retype
their assigned ranges, but they all hold the same parent untyped cap — no
one-to-two split of authority at the kernel level.

**Sources:**

- seL4 Reference Manual v14.0.0 §2 (Untyped Memory).
  [seL4.systems](https://sel4.systems/Info/Docs/seL4-manual-latest.pdf)
- [seL4 Untyped tutorial](https://docs.sel4.systems/Tutorials/untyped.html)

---

### 2.6 Plan 9 — Namespace Union Mount (Non-Destructive Name Aliasing)

**Mechanism.** Plan 9's `bind(2)` and `mount(2)` modify the _namespace_ — a
per-process-group mapping from names to file servers (channels). Union mounts
cause one name to resolve to multiple servers in priority order.
`bind /new /old` causes `/old` to also resolve through `/new`'s server. The
underlying servers are not affected; both remain independently accessible via
their own names.

**Combine analogue.** A union mount makes reads/writes to a single name dispatch
to multiple underlying servers. This is namespace-level fan-in. The underlying
server objects (channels to the 9P server processes) are not consumed; they
survive the bind and are accessible via their original paths.

**Split analogue.** Plan 9's `unmount(2)` removes a layer from a union. The
underlying server is still live; it simply no longer appears at that union name.
Its original name remains valid.

**Crash recovery.** If a server process in a union dies, its entry in the union
becomes dead; reads/writes through it return errors. The other servers in the
union continue to function. No kernel-side recombination occurs automatically.

**No capability effect.** Plan 9 does not use a capability model in the
seL4/Mach sense. The "capabilities" (file descriptors to channels) to the
underlying servers are unaffected by bind/unbind.

**Sources:**

- Plan 9 `bind(1)` manual page.
  [man.cat-v.org](http://man.cat-v.org/plan_9/1/bind)
- "The Plan 9 Namespace for Dummies," darknedgy.net.

---

### 2.7 QNX Neutrino — Channel/Connection Model (No Structural Reconfiguration)

QNX has no native split or combine primitive for channels. A server creates a
channel once; multiple clients connect to it via connections. The many-to-one
aggregation is structural (the channel aggregates all connections by design),
not a runtime combine of previously independent channels. There is no
`combine_channels` syscall and no `split_channel_by_id` operation.

To route a subset of clients to a new server, the server must direct those
clients to call `ConnectDetach` + `ConnectAttach` to the new channel. This is
cooperative client migration, not a kernel restructuring operation.

---

### 2.8 EROS / Coyotos — Domain Splitting (Userspace Factory Pattern)

EROS/Coyotos do not expose split or combine primitives at the kernel level.
Authority subdivision is handled by user-level services:

- A _space bank_ holds a pool of physical pages as capabilities. Splitting the
  bank's authority means creating a sub-bank object (user-level) and selling a
  range of pages to it. The sub-bank then acts as the authority for its subset.
- Combining authority is performed by transferring capabilities back to the
  parent bank.
- The kernel has no direct notion of "split this kernel object into two." It
  only has the capability table manipulation primitives (copy, mint, revoke,
  delete).

This pattern shifts structural reconfiguration entirely to user-level domain
managers.

**Sources:**

- Shapiro et al., "EROS: A Fast Capability System," SOSP 1999.
- cap-lore.com, "KeyKOS Space Banks."

---

## 3. Mechanism Comparison

### 3.1 Combine: effect on send-side capabilities

| System        | Combine mechanism               | Effect on existing send caps           | Original receive right |
| ------------- | ------------------------------- | -------------------------------------- | ---------------------- |
| Mach port set | Move receive right into set     | **Unaffected** — senders see no change | Absorbed into set      |
| Zircon port   | Register object with port       | **Unaffected** — handle unchanged      | Original handle intact |
| Windows IOCP  | Associate handle with port      | **Unaffected** — handle unchanged      | Original handle intact |
| Plan 9 union  | Bind name to union              | **N/A** (no send-cap model)            | Servers remain live    |
| QNX           | None (topology fixed at create) | N/A                                    | N/A                    |
| EROS/Coyotos  | Userspace bank merge            | Depends on bank implementation         | Depends                |

All surveyed combine mechanisms leave existing send-side access rights intact.
No deployed system surveyed invalidates existing sender authority as a
consequence of a combine operation.

### 3.2 Split: effect on the original capability

| System         | Split mechanism                     | Original capability after split | New capabilities      |
| -------------- | ----------------------------------- | ------------------------------- | --------------------- |
| Barrelfish RAM | `cnode_split` (binary half-split)   | **Consumed / destroyed**        | Two halves (children) |
| seL4 Untyped   | `Untyped_Retype` (child carve)      | **Survives** (parent intact)    | Child objects         |
| Mach           | None at kernel level                | N/A                             | N/A                   |
| Zircon         | Interrupt objects created 1-per-IRQ | N/A (no split primitive)        | N/A                   |
| EROS/Coyotos   | Space bank sub-bank creation        | Bank survives (userspace)       | Sub-banks (userspace) |

For resources that _are_ split, Barrelfish consumes the parent (gives the two
halves independent existence) while seL4 retains the parent (keeps centralized
authority and uses a child carve-out model).

### 3.3 Crash recovery comparison

| Scenario                      | Mach port set                           | Zircon port               | Barrelfish cap split                 |
| ----------------------------- | --------------------------------------- | ------------------------- | ------------------------------------ |
| Set/port holder crashes       | Member receive rights destroyed         | Registered objects intact | Split halves intact                  |
| Existing send caps to members | Become dead names (notifications sent)  | Unaffected                | Unaffected (send caps gone anyway)   |
| Resource recovery             | Pages/rights go with member destruction | Objects remain usable     | CDT tracks; recoverable if CDT clean |
| Holder recovers (restart)     | Must recreate set and re-acquire rights | Recreate port only        | Must recover cap from CDT            |

The asymmetry is significant: Mach's combine-with-absorption means the set
holder's crash propagates as dead-name notifications to all send-right holders.
Zircon's non-destructive registration means the aggregator can crash silently
and sources remain unaffected.

---

## 4. Tradeoffs

### Combine: destructive (absorption) vs. non-destructive (registration)

**Destructive absorption** (Mach port set receive-side):

- The aggregate has sole receive authority; direct receive on the original is
  disabled. This prevents split-brain: messages cannot be missed by one of two
  competing receivers.
- If the holder of the aggregate crashes, all absorbed originals are destroyed,
  and send-right holders are notified via dead names. This is a strong
  propagation guarantee — the failure is observable by senders.
- Restoring the original topology requires re-acquiring receive rights (either
  surviving copies or a fresh distribution). No automatic reversion.

**Non-destructive registration** (Zircon port, Windows IOCP):

- The aggregate is an additional listener, not the sole receiver. The original
  object retains all its independent semantics.
- Aggregate crash is silent from the source objects' perspective — they continue
  functioning. This makes recovery simpler (just recreate the port and
  re-register) but means failure is not automatically propagated to peers.
- Two active receivers (original handle + port) can coexist — the application
  must ensure only one path is actually used, or accept that events may be
  consumed on either path.

### Split: consume-parent vs. keep-parent

**Consume-parent (Barrelfish)**:

- After split, no capability covers the full original range. Both halves exist
  as independent peers. Neither subsumes the other. This prevents ambiguity
  about which entity has authority over the full range.
- Recombination requires both halves to be free (CDT subtrees clean). This is
  strictly ordered.
- If one split half's holder crashes and drops its capability, the half becomes
  unreachable (until the CDT is cleaned up by a surviving ancestor). The other
  half is unaffected.

**Keep-parent (seL4 Untyped)**:

- The parent always has potential authority over the full range, even while
  sub-ranges are allocated as children. This makes centralized authority clear:
  the untyped holder is the ultimate resource owner.
- Reclaiming a child's range requires revoking all CDT descendants of the
  untyped, which resets the entire watermark — you cannot selectively reclaim
  one child's range without reclaiming all subsequent children too.
- The parent surviving makes crash recovery of child objects simpler: the
  untyped holder (who typically is a resource manager that outlives its
  allocations) has the authority to revoke and reallocate.

### Badge-range split: no deployed system provides this

None of the surveyed systems provides a kernel-level operation to split an IPC
endpoint by the badge value of sending capabilities. The routing of "send caps
with badge ≥ N go to field A; badge < N go to field B" does not appear as a
kernel primitive in seL4, Mach, Zircon, QNX, EROS, or Barrelfish.

Badge routing in existing systems is entirely a userspace pattern: the server
dispatches incoming messages by badge after receiving them, or uses multiple
distinct endpoints created for distinct sender sets. seL4's many-to-one endpoint
with badged caps provides a single receive point; subdividing that endpoint by
badge requires retiring old send caps and issuing new ones to different
endpoints — a cooperative reorganization, not a kernel operation.

### Generalization beyond the introducing use case

Mach port sets were designed for multiplexing across IPC ports and are general
across all port types. Zircon ports aggregate across all signal-bearing kernel
object types (channels, VMOs, processes, interrupt objects). Windows IOCP
aggregates across all I/O handles.

In all three cases the combine mechanism generalized well beyond the original
use case. The generalizing property: the mechanism does not care about the
_content_ of what the objects deliver — it cares only about the _event signal_.
Any object that can fire a signal can be registered with the aggregator.

Barrelfish's memory split is specific to typed memory capabilities and does not
generalize to IPC endpoints. The seL4 untyped retype is specific to memory
objects and does not generalize to IPC endpoint splitting.

The asymmetry: **aggregation (combine) generalizes readily in all systems that
have it; splitting does not generalize and is largely absent from IPC primitives
in all surveyed systems.**

---

## 5. References

1. GNU Mach Reference Manual — Port Sets.
   https://www.gnu.org/software/hurd/gnumach-doc/Port-Sets.html
2. GNU Mach Reference Manual — Port Destruction.
   https://www.gnu.org/software/hurd/gnumach-doc/Port-Destruction.html
3. Fuchsia `zx_interrupt_bind` syscall reference.
   https://fuchsia.dev/fuchsia-src/reference/syscalls/interrupt_bind
4. Fuchsia `zx_port_wait` syscall reference.
   https://fuchsia.dev/fuchsia-src/reference/syscalls/port_wait
5. Fuchsia Zircon kernel objects reference.
   https://fuchsia.dev/fuchsia-src/reference/kernel_objects/objects
6. Nevill, M. "Capability-Based Operating Systems for Future Hardware." Master's
   Thesis, ETH Zürich, 2012.
   https://barrelfish.org/publications/nevill-master-capabilities.pdf
7. Baumann, A. et al. "The Multikernel: A new OS architecture for scalable
   multicore systems." SOSP 2009.
   https://people.inf.ethz.ch/troscoe/pubs/sosp09-barrelfish.pdf
8. Bodun Hu, "A Little Review on Barrelfish Memory Managements."
   https://www.bodunhu.com/blog/posts/a-little-review-on-barrelfish-memory-managements/
9. seL4 Reference Manual v14.0.0 §2.
   https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
10. seL4 Untyped tutorial. https://docs.sel4.systems/Tutorials/untyped.html
11. Plan 9 `bind(1)` manual page. http://man.cat-v.org/plan_9/1/bind
12. Shapiro, J. et al. "EROS: A Fast Capability System." SOSP 1999.
    https://flint.cs.yale.edu/cs428/doc/eros.pdf
13. Russinovich, M. et al. _Windows Internals_, 7th ed., Part 2, Chapter 8 (I/O
    system). Microsoft Press.
14. Hille, N. and Asmussen, N. "SemperOS: A Distributed Capability System."
    USENIX ATC 2019. https://www.usenix.org/system/files/atc19-hille.pdf
