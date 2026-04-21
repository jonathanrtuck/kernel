# Destroy Cascade Protocol

## 1. The Question

When a kernel object (process, thread, capability container) is destroyed, what
sequence of events follows? Specifically:

1. **Cascade ordering.** What order are held capabilities closed? What happens
   if a notification cannot be delivered (queue full)?
2. **Depth bounding.** Can cascade be unbounded? How do kernels prevent infinite
   recursion or unbounded kernel execution?
3. **Freed backing routing.** When an object is destroyed and frees physical
   memory, who receives that memory? When there is no explicit "destroyer" (the
   object was destroyed transitively, not directly), where do freed pages go?
4. **Preemptibility.** Can destroy be interrupted mid-cascade? How do kernels
   maintain consistency if interrupted?

---

## 2. Survey of Existing Systems

### 2.1 seL4 — Capability Container Deletion and Zombie Protocol

**Cascade ordering.** In seL4, deleting the last capability to a TCB or CNode
triggers destruction of the object. Before the container is freed, every
capability slot it holds is deleted in turn (`cteDelete` on each slot). If a
slot holds the last cap to another object, that object is destroyed too — a
cascade.

The cascade is bounded by a loop-prevention rule: if a CNode being deleted
contains a capability to _another_ CNode, that inner CNode is not recursively
destroyed in-place. Instead, the capability is _moved_ so the inner CNode
becomes unreachable (an orphan CNode). Orphaned CNodes are not immediately
recovered; they can be cleaned up later by revoking the covering Untyped
capability.

Source: seL4 Reference Manual, §2.3 Deletion and §3 CNode Objects; seL4 tutorial
"Untyped Memory."

**Preemptibility — baseline kernel.** In the baseline seL4 kernel, deletion is
not preemptible. The run time of `cteDelete` is unbounded under certain
circumstances (O(number of held capabilities), which may cascade). This creates
a known worst-case execution time (WCET) concern for real-time use.

**Preemptibility — MCS kernel.** The seL4 Mixed-Criticality System (MCS) kernel
adds preemption points within deletion traversal. The design pattern used is
called _incremental consistency_: each sub-operation transforms the kernel from
one consistent state to another. The kernel checks for pending interrupts at
each preemption point and may abort the current sub-operation.

**Zombie capabilities.** When deletion is preempted, the progress state must
survive. seL4 stores this state in the _last capability referencing the object
being destroyed_, called a zombie capability. The zombie holds enough
information to resume deletion from the point of interruption. If a second
thread then attempts to destroy the same zombie, it continues where the first
was preempted — effectively a form of priority inheritance — rather than
blocking or failing.

The correctness of this mechanism does not depend on user-accessible registers;
the zombie capability is a kernel-side data structure within the capability
table slot.

Source: seL4 Reference Manual §2.3; seL4 whitepaper "The seL4 Microkernel";
Blackham et al., "Protected Hard Real-time: The Next Frontier" (APSYS 2011);
seL4 MCS documentation (docs.sel4.systems).

**Freed backing routing.** seL4 does not have a kernel-managed physical memory
pool. All kernel objects are backed by user-owned Untyped capabilities. When
objects are destroyed, their physical memory does not automatically return to
anyone — it stays "in" the covering Untyped. The Untyped's internal watermark
(bump allocator) cannot move backward while any derived capabilities exist.

To reuse the backing memory, a user-level entity must: (1) revoke all
capabilities derived from the Untyped (clearing all CDT children), then (2)
reset the Untyped. Until both steps are completed, the physical pages are inert
— held by the Untyped capability but usable for nothing new.

This means that in a cascade destruction where the Untyped's holder is a third
party (not the destroyed object), that third party retains the Untyped and can
eventually reclaim it after the cascade completes. If the Untyped itself has no
surviving holder (because it was held only by the destroyed entity), it becomes
unreachable and its pages are effectively leaked until a covering Untyped is
revoked by a surviving ancestor.

**Notification during cascade.** seL4 has no built-in notification mechanism for
capability deletion itself. Notification objects (seL4 Notification) can send
signals, but the kernel does not automatically signal observers when a
capability is deleted. It is the responsibility of user-level to arrange
teardown notifications. If a badge-close notification cannot be delivered (e.g.,
the notification object's recipient is not waiting), the delivery fails silently
unless the application has arranged flow control.

---

### 2.2 Zircon (Fuchsia) — Handle Reference Counting

**Cascade ordering.** In Zircon, each kernel object has a handle reference
count. The object is destroyed when the last handle is closed. There is no
explicit "delete everything I hold" cascade for generic objects; the cascade
arises naturally from handle reference counting.

On _process_ termination, the kernel iterates over the process's handle table
and closes each handle in turn. If a handle's closure causes the object's count
to reach zero, the object is destroyed. The ordering within the handle table
iteration is implementation-defined.

For _channels_: destroying a channel endpoint also destroys all pending messages
in the channel's queue, which closes any handles embedded in those messages.
This is a bounded cascade (bounded by message queue depth).

Source: Fuchsia Kernel Concepts documentation (fuchsia.dev); Zircon Handles
documentation (fuchsia.dev).

**Preemptibility.** Zircon's destruction paths are not described as a WCET
concern in the open literature, because the cascade is bounded per object (the
handle table iteration is O(open handles), which is bounded by kernel policy).
No MCS-style preemption protocol is described for Zircon destruction.

**Freed backing routing.** Zircon maintains a kernel-managed physical page pool.
When a VMO (Virtual Memory Object) is destroyed (all handles closed, all VMAR
mappings detached), its pages return to the kernel pool automatically. There is
no user-space-level tracking of which pages went where. This makes the "no
caller" case simple: freed pages go to the kernel pool unconditionally.

**Peer closure ordering.** Zircon provides a synchronous guarantee for peered
objects (channels, sockets, FIFOs, event pairs): when the last handle to one
peer is closed, the `ZX_*_PEER_CLOSED` signal is asserted on the surviving peer
_before_ `zx_handle_close` returns to the caller. This means the notified peer
can observe the closure synchronously — the signal is not deferred or dropped.

Source: Zircon Kernel Objects documentation (fuchsia.googlesource.com); Fuchsia
kernel concepts.

---

### 2.3 Mach / XNU — Port Destruction and Dead-Name Cascade

**Cascade ordering.** In Mach, the primitive for IPC is a port. Destroying a
port (`mach_port_destroy` or dropping the last receive right) produces:

1. All queued messages to the port are destroyed.
2. All send rights and send-once rights to that port become _dead names_.
3. If a dead-name request was registered for a right that becomes a dead name, a
   `MACH_NOTIFY_DEAD_NAME` notification is generated and sent to the registered
   send-once right.
4. If a name denotes a receive right that is a member of a port set, it is
   implicitly removed from the port set.

The ordering is: message queue destruction first, then dead-name generation.
Notifications are sent to the registered send-once right; if the notification
cannot be delivered (the target port is itself dead), the notification is
dropped.

On task exit, all port rights held by the task are released. The release order
within the task's port namespace is implementation-defined. Each released port
generates notifications for any holders of send rights to that port (DEAD_NAME).
This can generate O(holders × ports) notifications from a single task exit.

Source: GNU Mach Reference Manual, "Port Destruction" and "Request
Notifications"; Mach 3 Kernel Principles (CMU); `mach_port_destroy` man page
(web.mit.edu/darwin).

**Freed backing routing.** Mach manages physical memory through an external
pager interface. Ports themselves are kernel-managed; freed port memory returns
to the kernel's internal allocator. Task memory (vm_objects) is managed by the
kernel with demand paging; on task exit, all vm_objects backed by the task's
address space are freed to the kernel pool.

---

### 2.4 Genode — Hierarchical Session Destruction

**Cascade ordering.** Genode's component model is strictly hierarchical. Every
component has a parent. When a parent decides to destroy a child (by withdrawing
its session), the cascade proceeds top-down: the parent closes the child's
sessions, which causes the kernel to invalidate all capabilities associated with
those sessions' RPC objects. The kernel-level capability revocation is
object-level: destroying an RPC object invalidates all capabilities that name
it, regardless of how many intermediate delegation steps occurred.

Dataspaces are freed hierarchically: the server frees a dataspace at its PD
session; Genode's `core` component (the initial kernel-level process) then
implicitly detaches the dataspace from all region maps that had attached it.
This "push" model means the server-side free propagates to clients without
client cooperation.

**Freed backing routing.** Resources in Genode are always held by the parent
that created the session. When a child is destroyed, the resources the parent
allocated _on behalf of_ that child (RAM quota, CPU time, capabilities) return
to the parent. This is structurally guaranteed: the child component was created
out of the parent's quota, so quota returns to the parent on child destruction.

This sidesteps the "no caller" problem: the caller is always the parent, which
survives the destruction.

Source: Genode Foundations (Feske); Genode documentation "Recursive System
Structure" (genode.org); "Core — the root of the component tree" (genode.org).

---

### 2.5 EROS and KeyKOS — Space Bank Page Return

**Cascade ordering.** EROS and KeyKOS do not have a concept of "process destroy"
that the kernel handles directly in a cascade. Process structure is managed by
user-level. A process dying (e.g., a "keeper" invoking destruction) must
explicitly sell pages back to the space bank.

When a page is sold (destroyed in KeyKOS terminology), the kernel:

1. Traverses the depend table to find all prepared capabilities (pointer
   resolutions) to the page and converts them back to unprepared
   (disk-reference) form.
2. Invalidates all page table entries that map the page (also via depend table).
3. Removes all capability keys to the page (nulls them out), walking the holder
   list.
4. Frees the physical frame to the kernel.

The depend table walk is bounded by the number of current mappings + prepared
caps for that page. The holder list walk is O(number of caps to the page).

**Freed backing routing.** In KeyKOS/EROS, the Space Bank is a user-level
service that holds all free physical pages. Selling a page back to the bank
returns it to the bank's free pool. The space bank hierarchy propagates freed
pages upward: each bank's net-allocation counter decrements, making that quota
available to the bank's superiors. There is no kernel pool; all physical page
tracking is userspace accounting through the bank tree.

Source: cap-lore.com "KeyKOS Space Banks"; Shapiro et al., "EROS: A Fast
Capability System" (SOSP 1999); capros.org "Space Banks — The New Generation."

---

### 2.6 QNX Neutrino — Pulse Delivery on Channel Destroy

**Cascade ordering.** In QNX, when a process exits, the kernel terminates all
its threads, closes all file descriptors, and destroys all channels and
connections. For each connection a client has open to a server channel: when the
server's channel is destroyed, the kernel delivers a `_PULSE_CODE_COIDDEATH`
pulse to the client's channel (if the client registered for it via
`_NTO_CHF_DISCONNECT`).

**Notification ordering guarantee.** If a server exits or closes its channel at
approximately the same time that a client closes its connection, the kernel _may
or may not_ deliver the death pulse. The QNX documentation explicitly states
this is a race. Applications must handle both cases.

**Pulse pool overflow.** QNX channels can be created with a fixed-size pulse
pool (`ChannelCreatePulsePool`). If a pulse cannot be delivered because no
thread is available and the pulse pool is exhausted, the channel owner is sent a
`SIGKILL` by default. This is an overflow policy: the kernel does not drop the
pulse silently; it terminates the misbehaving component instead.

**Freed backing routing.** QNX is not a capability kernel; physical memory
management is handled by the kernel via the process manager (procmgr). On
process exit, all mapped memory is released to the kernel's physical memory
pool. No user-level accounting or routing is required.

Source: QNX Neutrino documentation — "Process Termination" and "Detecting Client
Termination" and "ChannelCreate" (qnx.com/developers/docs/8.0).

---

## 3. Mechanism Comparison

### 3.1 Cascade depth and bounding

| System      | Cascade mechanism                  | Depth bound                                  |
| ----------- | ---------------------------------- | -------------------------------------------- |
| seL4        | Recursive cap-container deletion   | Loop prevention: inner CNodes become orphans |
| Zircon      | Handle table iteration on exit     | O(handles), bounded by per-process limits    |
| Mach        | Dead-name notification per port    | O(ports × holders), not bounded by kernel    |
| Genode      | Top-down session hierarchy         | Tree depth; parent always survives           |
| EROS/KeyKOS | Explicit sell-back via space bank  | O(holders per page), bounded by depend table |
| QNX         | Kernel reclaim, pulse notification | O(connections), bounded by system limits     |

### 3.2 Preemptibility of destroy

| System        | Preemptible?                       | Mechanism                                    |
| ------------- | ---------------------------------- | -------------------------------------------- |
| seL4 baseline | No                                 | Non-preemptible; WCET unbounded              |
| seL4 MCS      | Yes                                | `preemptionPoint()` + zombie capability      |
| Zircon        | Effectively yes (short paths)      | Reference counting; O(1) per object          |
| Mach          | Implicit (notification is async)   | DEAD_NAME sent as message; async delivery    |
| Genode        | Yes (session close is cooperative) | Server acknowledgment before cap invalidated |
| QNX           | Yes (pulse is async)               | Pulse delivery is asynchronous               |

### 3.3 Freed backing routing

| System    | Where freed pages go                       | Who decides                       |
| --------- | ------------------------------------------ | --------------------------------- |
| seL4      | Stay in Untyped; Untyped holder reclaims   | Untyped holder (survives death)   |
| Zircon    | Kernel pool (automatic, no user tracking)  | Kernel                            |
| Mach      | Kernel pool (vm_object freed on task exit) | Kernel                            |
| Genode    | Return to parent component's quota         | Hierarchy; parent always survives |
| EROS/KKOS | Space bank free pool (user-level)          | Space bank (user policy)          |
| QNX       | Kernel pool (procmgr)                      | Kernel                            |

### 3.4 Notification during cascade — dropped message handling

| System | Notification type       | Drop policy on overflow / missing target    |
| ------ | ----------------------- | ------------------------------------------- |
| seL4   | None (kernel-level)     | N/A; user arranges notifications            |
| Zircon | Signal (PEER_CLOSED)    | Synchronous; delivered before close returns |
| Mach   | DEAD_NAME message       | Dropped if target port is dead              |
| Genode | Capability invalidation | Synchronous via RPC object destruction      |
| QNX    | Pulse (`COIDDEATH`)     | Race if exit concurrent with disconnect     |

---

## 4. Tradeoffs

**Recursion vs. iteration.** Recursive cascade (seL4) is natural for tree-like
capability structures but risks unbounded kernel execution and stack depth. The
seL4 loop-prevention rule (orphan inner CNodes) trades cascade completeness for
bounded per-step execution, pushing the cleanup of orphans to a separate
user-level operation. Iterative approaches (Zircon handle table walk) are
naturally bounded and non-recursive but apply only to flat structures.

**Preemptibility and consistency.** Making destruction preemptible requires
storing intermediate state somewhere. seL4 MCS stores it in the zombie
capability (kernel-side), making restartability independent of user register
state. The alternative — not allowing preemption — creates unbounded kernel
latency, problematic for real-time systems. The seL4 WCET analysis is the only
published formal analysis of kernel deletion latency in the open literature.

**Zombie priority inheritance.** seL4's zombie design has a non-obvious
property: a second thread attempting to destroy the zombie _continues the work_
rather than blocking or failing. This avoids priority inversion (a low-priority
thread holds the zombie and a high-priority thread must wait) but means the
second thread inherits the cleanup work. This is a form of work-stealing.

**Freed backing — kernel pool vs. user accounting.** Kernel-pool return (Zircon,
Mach, QNX) is simple and handles the "no caller" case automatically: freed pages
go to the pool regardless of how they were freed. User-accounting systems (seL4,
EROS) require surviving entities to hold the "container" capability (Untyped,
space bank) that can reclaim pages. If the container capability is itself lost
in the cascade, pages become unreachable. This is a design constraint: the
memory accounting structure must outlive the objects it tracks.

**Notification synchrony.** Zircon's synchronous PEER_CLOSED guarantee (signal
asserted before close returns) is the strongest notification model found in this
survey. Mach's DEAD_NAME is asynchronous (sent as a message) — the notified task
may not have processed it before attempting to use the now-dead right. QNX
explicitly documents a race between server exit and client disconnect death
notification. seL4 provides no kernel-level deletion notification at all.

**Hierarchical destruction (Genode) as a pattern.** Genode's strict parent-child
hierarchy sidesteps most cascade problems: the parent always survives, so it
always receives back its resources; the parent initiates destruction top-down so
there is always an explicit destroyer. This structure rules out the "no caller"
problem by construction. The tradeoff is inflexibility: sibling components
cannot destroy each other without going through a common parent.

---

## 5. References

- seL4 Reference Manual v14.0.0. seL4 Foundation.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 Microkernel Whitepaper. seL4 Foundation.
  https://sel4.systems/About/seL4-whitepaper.pdf
- Blackham, B. et al. "Protected Hard Real-time: The Next Frontier." APSYS 2011.
  https://apsys11.ucsd.edu/papers/apsys11-blackham.pdf
- seL4 MCS Tutorial. seL4 Foundation.
  https://docs.sel4.systems/Tutorials/mcs.html
- Heiser, G. "The formally verified seL4 microkernel — a high-assurance
  foundation for MCS." RTCSA 2020.
  https://trustworthy.systems/publications/papers/Heiser_20:rtcsa.abstract
- Zircon Kernel Objects. Google / Fuchsia team.
  https://fuchsia.dev/fuchsia-src/reference/kernel_objects/objects
- Zircon Handles. Google / Fuchsia team.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/handles
- Fuchsia Kernel Concepts.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
- GNU Mach Reference Manual — Port Destruction.
  https://www.gnu.org/software/hurd/gnumach-doc/Port-Destruction.html
- GNU Mach Reference Manual — Request Notifications.
  https://www.gnu.org/software/hurd/gnumach-doc/Request-Notifications.html
- `mach_port_destroy` man page (Darwin/XNU).
  https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/mach_port_destroy.html
- Genode Foundations (Feske). Genode Labs.
  https://genode.org/documentation/genode-foundations/
- Genode — "Recursive System Structure."
  https://genode.org/documentation/genode-foundations/22.05/architecture/Recursive_system_structure.html
- Shapiro, J. et al. "EROS: A Fast Capability System." SOSP 1999.
  https://flint.cs.yale.edu/cs428/doc/eros.pdf
- cap-lore.com — "KeyKOS Space Banks."
  http://www.cap-lore.com/CapTheory/KK/KKBank.html
- capros.org — "Space Banks: The New Generation."
  http://www.capros.org/design-notes/SpaceBank.html
- QNX Neutrino — "Process Termination."
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.prog/topic/process_PROCTERM.html
- QNX Neutrino — "Detecting Client Termination."
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.prog/topic/process_Client_termination.html
- QNX ChannelCreate documentation.
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.lib_ref/topic/c/channelcreate.html
