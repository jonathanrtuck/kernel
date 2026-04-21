# Scheduling Donation Mechanism for Synchronous IPC

## The Question

When a caller makes a synchronous call to a server, the server must have some
source of scheduling capacity to process the request. Five approaches appear in
deployed systems:

1. **Passive server (kernel-implicit donation):** the kernel transfers the
   caller's time/scheduling capability to the server as a side-effect of the
   Call syscall; no user-level cap transfer in the message.
2. **Call-chain SC propagation (NOVA model):** the kernel forwards the caller's
   scheduling context along the call chain; the server always runs on the
   caller's SC because it has no SC of its own by construction.
3. **Thread migration:** the calling thread physically migrates into the
   server's protection domain carrying its own scheduling context; no "donation"
   occurs because the same thread + same SC runs the server code.
4. **Priority-only inheritance:** the server keeps its own time allocation but
   has its scheduling priority boosted to the caller's priority for the duration
   of the call; the time budget remains the server's own.
5. **Server pre-assigned its own scheduling context:** the server has a
   statically or dynamically configured time allocation independent of any
   caller; no donation occurs.

This document surveys each mechanism as implemented in real systems. The
question of whether a time capability is a separate kernel object at all
(surveyed in `time-as-kernel-object.md`) and what it contains (surveyed in
`time-object-content.md`) are treated as settled context here.

---

## Survey

### Mechanism 1: Passive Server — Kernel-Implicit Donation (seL4 MCS)

**Reference:** Lyons, McLeod, Almatary, Heiser. "Scheduling-context
capabilities: a principled, light-weight operating-system mechanism for managing
time." EuroSys 2018.

#### What triggers the donation

In seL4 MCS the donation is **triggered entirely by kernel-internal state** at
Call time. No capability transfer appears in the IPC message. The kernel checks
whether the server TCB has a bound SchedContext
(`tcb->tcbSchedContext == NULL`); if it does not, the server is "passive" and
the kernel initiates donation:

1. The kernel unbinds the caller's SchedContext from the caller's TCB.
2. The kernel binds that SchedContext to the server's TCB
   (`server_tcb->tcbSchedContext = caller_sc`).
3. The server is placed on the scheduler run queue; the caller blocks
   (`BlockedOnReply` state — it no longer has an SC, so it cannot be scheduled).

The caller's TCB is momentarily unbound from any SchedContext, which is safe
because the caller is blocked.

#### How the donation chain is tracked

The **Reply object** (introduced by MCS; previously a slot in the server's TCB)
carries the donation metadata. The server supplies a Reply object cap to
`seL4_Recv()`; the kernel populates it on call arrival with:

- A link back to the blocked caller (to enable direct wakeup at reply time)
- A pointer to the donated SchedContext (so the kernel knows which SC to return
  and to whom)

This is why MCS moved from a TCB-resident reply slot to an explicit Reply
object: the SC donation metadata had to live in a user-managed object to support
deferred replies (if the server replies asynchronously, it needs the Reply
object's donation record to restore the SC correctly).

#### What is donated

The full SchedContext object — including its `(budget, period, refill_list)`
triple. The server runs against the exact same budget and refill state the
caller had accumulated. The caller's SC is not copied; the same kernel object is
moved to the server's TCB.

#### How the SC is returned

On `seL4_ReplyRecv()` the kernel:

1. Looks up the SC reference in the Reply object.
2. Unbinds the SC from the server's TCB.
3. Rebinds it to the caller's TCB.
4. Transitions the caller from `BlockedOnReply` to `Running`.

Because `seL4_ReplyRecv` atomically combines reply-send and next-receive, the SC
is never in an unbound state visible between user-mode operations.

**Fast-path cost:** Zero additional cycles for the donation path vs. non-MCS
IPC, per Lyons et al. benchmark on ARM Cortex-A9. The reply-and-rebind is part
of the same context-switch sequence that the fastpath already performs.

#### Structural enforcement

The passive state is structural: the server has no SC bound to its TCB. Any
thread entering `seL4_Recv` with no bound SC is implicitly passive. There is no
"passive flag" — absence of an SC binding is the condition.

---

### Mechanism 2: Call-Chain SC Propagation (NOVA / Genode)

**Reference:** Steinberg, Kauer. "NOVA: A Microhypervisor-Based Secure
Virtualization Architecture." EuroSys 2010. Also: Genode Foundations 20.05,
"Execution on the NOVA microhypervisor."

#### Local EC vs. Global EC

NOVA enforces the donation model through execution context taxonomy:

- **Global EC (gEC):** Can be associated with an SC; can make IDC calls; cannot
  directly receive IDC calls.
- **Local EC (lEC):** Has no SC of its own; can receive IDC calls (via portals);
  cannot run without an incoming call.

A Genode entrypoint is a local EC. A Genode background thread is a global EC.
The structural separation means a local-EC server cannot have its own SC — it is
impossible by construction.

#### What triggers the propagation

When a global EC invokes a portal bound to a local EC, the kernel **passes the
caller's SC along the call chain**:

1. The calling gEC's SC is recorded as the "current" SC for the call chain.
2. The lEC runs using that SC — it is scheduled on the caller's time budget.

If the lEC itself makes a further IDC call (to another lEC), the same SC
continues propagating. The call chain — potentially spanning many protection
domains — runs on a single SC belonging to the originating gEC.

The Genode documentation describes this as: "the SCs of both client and server
reside within the server" during the call, noting that this "breaks the
invariant that all threads of one component share the same CPU priority."

#### What is donated

The full SC and its budget state — the same object propagates through the chain.
Unlike seL4's explicit bind/unbind, NOVA's propagation is described as the SC
pointer "following" the call chain; the original gEC is blocked and cannot
consume its own SC while the call is in flight.

#### How the SC is returned

When the lEC returns (via IDC return / portal return), the kernel restores the
SC to the calling gEC. Because lECs have no SC of their own, there is no
competing binding to resolve.

#### Key difference from seL4 passive server

In seL4 MCS the donation is opt-in (a server can have its own SC or be passive).
In NOVA the local EC / global EC distinction is architectural: servers that
receive calls are always lECs, lECs always run on the caller's SC. There is no
choice.

**Priority inversion in Genode:** Because the SC propagates along the call chain
without aggregation, if a low-priority client calls into a component that
currently serves a high-priority chain (multi-threaded server), the priorities
interact. Genode implemented priority-inheriting spinlocks to handle this, but
the fundamental tension remains a documented tradeoff.

---

### Mechanism 3: Thread Migration (Composite OS)

**Reference:** Parmer and West. "The Case for Thread Migration: Predictable IPC
in a Customizable OS." OSPERT 2010. Parmer and West. "Predictable and
Configurable Component-Based Scheduling in the Composite OS." ACM TECS 2013.

#### What "donation" means here

Composite OS uses thread migration as its IPC mechanism. When component A calls
component B, the **same kernel thread migrates** into B's protection domain. The
thread carries its SchedContext with it because the thread and SC are bound.

This is not donation in the seL4/NOVA sense: there is no unbind/rebind of the
SC. The SC follows the thread because they are the same scheduling unit. The
server code in B executes on the calling thread's execution context, including
its SC.

#### Trigger

The migration is triggered by the invocation syscall. The kernel:

1. Saves the caller's register state.
2. Changes the current thread's protection domain to B's domain (address space,
   capability space).
3. Begins executing B's handler in the thread's context.

The thread's SC has never left the thread; no SC pointer manipulation occurs.

#### Return

When B's handler returns (via the return invocation), the kernel changes the
thread's domain back to A's domain. The SC has remained bound throughout.

#### Concurrency model

Because the calling thread migrates into the server, the server's handler code
runs in the caller's thread. This means:

- The server has no independent threads of its own for this invocation.
- If the server needs to serve multiple simultaneous callers, multiple caller
  threads each migrate in; the server code must be re-entrant or protected by
  locks.
- Each migrated call carries its own thread + SC, so there is no "which SC
  governs?" ambiguity.

#### Comparison to seL4 MCS passive server

| Property                       | Composite migration                         | seL4 MCS passive server                                                                           |
| ------------------------------ | ------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| SC movement                    | SC stays on thread (thread moves)           | SC unbound from caller, bound to server                                                           |
| Server's own SC                | Not applicable (no server threads)          | Optional (passive = no own SC)                                                                    |
| Multi-call concurrency         | Each caller's thread migrates independently | Server thread processes one call at a time; must use separate server threads for concurrent calls |
| SC mismatch possible           | No (SC always matches the migrated thread)  | No (SC donated before server runs)                                                                |
| Required state in Reply object | Not needed (migration carries context)      | Yes (Reply object must track SC for return)                                                       |

---

### Mechanism 4: Priority-Only Inheritance (QNX Neutrino)

**Reference:** QNX Neutrino System Architecture, "Priority inheritance and
messages," 7.0.0. QNX MsgSend() reference manual, 8.0.

#### What is transferred

In QNX, the server **does not receive the caller's time capability or scheduling
budget**. The server has its own scheduling parameters (priority +policy). When
a client calls `MsgSend()`, and the server thread processes the request via
`MsgReceive()`:

1. The server thread's **effective priority is raised** to the caller's priority
   for the duration of the call.
2. The server's scheduling **policy** is not inherited — only the priority
   level.
3. The server's own time budget (timeslice) is unchanged.

On `MsgReply()`, the server thread's priority returns to its base value.

#### Mechanism

This is purely kernel-internal: the kernel tracks which `MsgReceive()` call
corresponds to which `MsgSend()`, and applies the priority adjustment when the
server receives the message. No capability transfer, no SC unbind/rebind, no
Reply object.

**Priority inversion prevention:** If a higher-priority sender arrives while the
server is processing a lower-priority request, the server's priority is bumped
to the higher sender's priority. The system ensures the server always runs at
the priority of the highest-priority blocked sender.

#### What is NOT transferred

- Scheduling policy (FIFO, round-robin, sporadic remain the server's own)
- Scheduling budget (the server's timeslice/period remain unchanged)
- CPU affinity
- Any notion of the caller's time capability

This is important: QNX does not have time-as-capability. Scheduling parameters
are per-thread attributes, so there is nothing to "transfer" beyond priority.
Priority inheritance is the entire mechanism.

**Flag:** `ChannelCreate(_NTO_CHF_FIXED_PRIORITY)` disables priority inheritance
entirely, giving the server a fixed priority regardless of callers.

---

### Mechanism 5: Server Has Own Scheduling Context (Classical L4, Mach, Zircon)

#### Description

The server has a pre-assigned scheduling context (priority, timeslice, or
profile) independent of any caller. When the caller makes a synchronous call:

1. The caller blocks waiting for the server.
2. The server is scheduled according to its own scheduling parameters.
3. No transfer of any kind occurs at call time.

#### Priority inversion risk

If the server's priority is below the caller's priority, the caller blocks
waiting for a server that may not run, while higher-priority threads preempt the
server. This is the classic priority-inversion problem.

#### Workarounds deployed in systems without donation

**Classical L4 / OKL4:** Servers are typically assigned static high priority to
reduce inversion risk. The kernel provides `ExchangeRegisters` to change thread
priority, which a monitor could use to implement software-level priority
boosting, but no automatic mechanism exists.

**Mach / XNU:** The Mach IPC path includes priority propagation: the kernel
propagates the calling thread's effective priority to the server thread when a
call is in flight. XNU's `IPC_port_importance` mechanism tracks this for chain
propagation. From the XNU source (`osfmk/ipc/ipc_importance.c`): a "importance
donation" travels through the port, boosting the server thread's priority for
the duration of the call. This is closer to QNX-style priority inheritance than
seL4-style SC donation — no budget transfer, only priority propagation.

**Zircon:** Zircon does not have automatic priority inheritance over channels.
Server components are assigned profiles appropriate to their expected load. For
latency-sensitive work, deadline profiles ensure the server is scheduled on
time. No call-time donation mechanism exists.

**MINIX 3:** IPC uses fixed-priority scheduling with servers at static
priorities. The server scheduler runs at higher priority than user tasks to
prevent inversion.

---

### Mechanism Comparison by Dimension

| Dimension                             | seL4 MCS passive                   | NOVA local EC                 | Composite migration        | QNX inherit                   | Server-own SC |
| ------------------------------------- | ---------------------------------- | ----------------------------- | -------------------------- | ----------------------------- | ------------- |
| What transfers                        | Full SC (budget + refills)         | Full SC (chain follow)        | N/A (same thread)          | Priority only                 | Nothing       |
| Trigger                               | Kernel implicit on Call()          | Kernel implicit on portal IDC | Thread migration syscall   | Kernel implicit on MsgReceive | None          |
| Server has own SC?                    | Optional (passive = none)          | Never (lEC has none)          | N/A                        | Yes                           | Yes           |
| Reply object needed?                  | Yes (MCS Reply)                    | No (portal return path)       | No                         | No                            | No            |
| Budget tracked across IPC?            | Yes                                | Yes                           | Yes (same SC, same budget) | No (only priority)            | No            |
| Fastpath donation cost                | Zero (Lyons 2018)                  | Not separately measured       | N/A                        | Zero                          | N/A           |
| Priority inversion possible?          | No (caller's SC runs server)       | No                            | No                         | Reduced but not eliminated    | Yes           |
| Server blocks caller during donation? | Yes (caller has no SC until reply) | Yes (caller is blocked)       | Yes (thread is migrated)   | Yes (blocked on MsgReply)     | Yes           |

---

## Tradeoffs

### Full SC donation vs. priority-only inheritance

**Full SC donation (seL4 MCS, NOVA):**

- Hard-RT guarantees propagate across IPC boundaries: the caller's deadline
  budget is what the server runs against. If the caller's budget expires, the
  server is preempted.
- Priority inversion is structurally impossible: the server runs at exactly the
  caller's scheduling parameters.
- Budget consumption is attributed to the caller's SC, not the server's.
  Accounting reflects what the call actually cost the caller.
- The server cannot run "extra" work on donated SC — once the call returns, the
  SC returns to the caller.

**Priority-only inheritance (QNX, Mach IPC importance):**

- Simpler kernel mechanism: no SC unbind/rebind, no Reply object, no chain
  tracking.
- The server continues consuming its own budget, so a server with limited
  timeslice may still exhaust it even while running at elevated priority.
- Budget accounting is not call-attributed: the server's time shows up in the
  server's accounting, not the caller's.
- Reduces but does not eliminate priority inversion: if the server's timeslice
  runs out, it is preempted even at elevated priority.

### Kernel-implicit vs. explicit cap transfer in message

No surveyed system implements scheduling donation via explicit capability
transfer in the IPC message at call time. The reasons are structural:

- If the caller included their SC cap in the message payload, the server would
  need to: (a) receive the cap, (b) unbind the caller's own SC from the caller's
  TCB (requiring authority over the caller), (c) bind it to the server's TCB.
  Steps (b) and (c) are IPC-round-trip-separable from (a) — there is a window
  where neither party has the SC bound. The kernel-implicit approach has no such
  window because the unbind+rebind is atomic with the IPC context switch.
- The caller should not trust the server to properly manage the donated SC.
  Kernel-implicit donation means the kernel, not the server, controls when the
  SC is returned. If the server crashes or misbehaves, the kernel returns the SC
  to the caller via the Reply object's cleanup path.
- Explicit message-level cap transfer is useful for persistent delegation (e.g.,
  handing off an SC to a child permanently), but synchronous IPC donation is
  ephemeral — it is tied to one call-reply pair and must be automatically
  revocable.

### Structural enforcement vs. policy opt-in

**seL4 MCS:** Passive state is opt-in. A server can choose to have its own SC
(runs independently) or be passive (runs on donor SCs). This is flexible but
requires the system designer to make the right choice for each server.

**NOVA:** Local EC / global EC is structural. Servers (lECs) cannot have their
own SC. This eliminates a class of configuration errors at the cost of
flexibility (a server component cannot do background work without a separate
global-EC thread).

**Composite:** Thread migration makes the question moot — the server has no
independent thread to schedule.

### Donation vs. independent server for concurrent servers

A passive/donated model assumes the server processes one call at a time per
client thread. For high-throughput servers with many concurrent calls:

- seL4 MCS: each concurrent call is handled by a separate server thread, each
  with its own (or donated) SC. If the server is passive, it needs multiple
  server threads each capable of receiving donations — or it can use a pool of
  server threads, some passive and some with their own SCs.
- NOVA: concurrent calls migrate into the same component via separate caller
  threads; each caller's SC propagates independently.
- Composite: each migrating thread brings its own SC; concurrency is natural.
- QNX: the server's channel receives multiple concurrent senders; server threads
  pick up messages and inherit priorities. Thread pool size limits concurrency.

---

## Measured Data

**seL4 MCS IPC donation overhead** (Lyons et al., EuroSys 2018, ARM Cortex-A9):

- Passive server round-trip: approximately equal to non-MCS direct IPC.
- Donation tracking adds zero cycles to the fastpath.
- Reply object SC tracking: included in the reply-path overhead; not separately
  benchmarked.

**NOVA IDC latency** (from Steinberg, Kauer, EuroSys 2010 and Udo Steinberg's
FOSDEM 2020 slides on NOVA for ARMv8-A):

- Base IDC round-trip on NOVA: comparable to seL4 on similar hardware (order
  hundreds of cycles), not separately benchmarked for SC propagation cost.
- SC propagation is described as part of the kernel's call-chain tracking, with
  no additional overhead claimed.

**QNX priority inheritance** (from QNX documentation, no published benchmark
numbers for the priority adjustment itself):

- Priority inheritance is described as happening at message-receive time, not as
  a separate operation; it is folded into the `MsgReceive()` scheduler path.

**Composite OS thread migration vs. rendezvous IPC** (Parmer 2010):

- Thread migration latency claimed lower than two-thread rendezvous IPC because
  it eliminates one scheduling decision and one context switch: the calling
  thread continues executing (now in the callee's domain) instead of blocking
  and waking a separate server thread.

---

## What Is Not Settled in the Literature

1. **Explicit cap transfer for donation:** No deployed system uses explicit
   message-level SC cap transfer for synchronous IPC donation. Whether there is
   a semantic advantage (more explicit authority chain) or only disadvantages
   (window between unbind and rebind, server malice) is not analyzed.

2. **Multi-hop donation chain:** seL4 MCS and NOVA both support chains (server A
   calls server B while itself serving a donation from client C). The depth
   limit on such chains and the overhead of tracking multi-hop Reply objects
   (seL4) or multi-hop SC propagation (NOVA) is not benchmarked in published
   literature.

3. **Donation with multi-Time capability (additive model):** All surveyed
   systems assume donation = one SC at a time. If an execution unit can hold
   multiple Time capabilities (surveyed in `time-capability-cardinality.md`),
   the question of which SC to donate (or whether donation uses a subset vs. the
   active SC) is not addressed in any published system.

4. **Budget exhaustion during donation:** If the caller's SC budget runs out
   while the server is executing on the donated SC, the server is preempted.
   What happens to the call-in-progress (partial reply, server state in middle
   of operation) is system-specific. seL4 MCS: the server is descheduled; it
   continues when the SC is replenished on the next period boundary. Whether
   this creates correctness issues for servers that assume bounded execution is
   not analyzed.

---

## References

- Lyons, A., McLeod, K., Almatary, H., Heiser, G. (2018). "Scheduling-context
  capabilities: a principled, light-weight operating-system mechanism for
  managing time." EuroSys 2018.
  https://trustworthy.systems/publications/abstracts/Lyons_MAH_18.abstract

- seL4 MCS Tutorial. https://docs.sel4.systems/Tutorials/mcs.html

- seL4 MCS Reference Manual, Version 10.1.1-MCS.
  https://sel4.systems/Info/Docs/seL4-manual-10.1.1-mcs.pdf

- seL4 MCS pre-release notes (10.1.1-mcs).
  https://docs.sel4.systems/releases/sel4/10.1.1-mcs.html

- Steinberg, U., Kauer, B. (2010). "NOVA: A Microhypervisor-Based Secure
  Virtualization Architecture." EuroSys 2010.
  https://hypervisor.org/eurosys2010.pdf

- Genode Foundations 20.05, "Execution on the NOVA microhypervisor (base-nova)."
  https://genode.org/documentation/genode-foundations/20.05/under_the_hood/Execution_on_the_NOVA_microhypervisor_(base-nova).html

- Parmer, G. and West, R. (2010). "The Case for Thread Migration: Predictable
  IPC in a Customizable OS." OSPERT 2010.
  https://www2.seas.gwu.edu/~gparmer/publications/ospert10.pdf

- Parmer, G. and West, R. (2013). "Predictable and Configurable Component-Based
  Scheduling in the Composite OS." ACM TECS 12(3).
  https://www2.seas.gwu.edu/~gparmer/pubs.html

- Parmer, G. (2016). "Composite Component Invocations."
  https://www2.seas.gwu.edu/~gparmer/posts/2016-01-17-composite-component-invocation.html

- QNX Neutrino System Architecture, "Priority inheritance and messages" (7.0.0).
  https://www.qnx.com/developers/docs/7.0.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Priority_inheritance_messages.html

- QNX MsgSend() reference manual (8.0).
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.lib_ref/topic/m/msgsend.html

- Steinberg, U. (2020). "NOVA Microhypervisor on ARMv8-A." FOSDEM 2020.
  https://archive.fosdem.org/2020/schedule/event/uk_nova/attachments/slides/3995/export/events/attachments/uk_nova/slides/3995/slides.pdf
