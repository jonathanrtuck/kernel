# IPC Fast-Path Conditions: Receiver State, Scheduler State, and Message Shape

## Question

Under what conditions does a kernel take the direct-switch fast path on IPC?
Specifically, what must be true about:

1. **The receiver** — what state must the receiving thread (or endpoint) be in?
2. **The scheduler** — what priority and runnable-thread constraints must hold?
3. **The message** — what size and content constraints must the message satisfy?
4. **Hardware readiness** — address-space validity, ASID, and core affinity?

The question arises because kernels with queued IPC objects (endpoints,
channels, ports) typically implement two code paths: a "fast path" that performs
a direct context switch to an already-waiting receiver, and a "slow path" that
enqueues the sender when no receiver is waiting. The fast path achieves
rendezvous speed (~150–500 cycles on ARM64) by skipping queue insertion,
scheduler invocation, and address-space setup that would otherwise be needed.

---

## Survey

### seL4 — Classic Configuration

seL4's fast path is implemented in `src/fastpath/fastpath.c` and covers both
`seL4_Call` (initial synchronous send) and `seL4_ReplyRecv` (reply + next
receive). The conditions are checked in sequence; any failure drops to the slow
path.

#### Message checks

- `fastpath_mi_check(msgInfo)` passes: message length ≤
  `seL4_FastMessageRegisters` (currently 4 machine words on AArch64), no extra
  capabilities embedded (`capsUnwrapped == 0`, `extraCaps == 0`).
- The current thread has no pending saved fault:
  `seL4_Fault_get_seL4_FaultType(ksCurThread->tcbFault) == seL4_Fault_NullFault`.
  A pending fault redirects the thread to a fault handler rather than the
  call-site endpoint.

#### Receiver state check

- Endpoint state is `EPState_Recv`: at least one thread is blocked waiting to
  receive on this endpoint. The waiting receiver is the head of the endpoint's
  receive queue. If no receiver is waiting (`EPState_Idle` or `EPState_Send`),
  the sender enqueues itself — no fast path.

#### Capability check

- The invoked cap is a valid endpoint cap with `capCanSend` set.
- For `seL4_Call`: the sender must have at least `Write` rights; no `Grant` or
  `GrantReply` rights are needed unless the message contains capability
  references — and the fast path requires that it does not. If the capability
  includes `Grant` and the message includes cap references, the fast path is
  skipped.

#### Scheduler state check

- Destination thread's scheduling priority satisfies: **destination thread is
  the highest-priority runnable thread in the current domain** (no
  higher-priority thread is currently runnable). This means
  `dest->tcbPriority == maxDomainPrio` (for the domain), checked as
  `isHighestPrio(dest_dom, dest_prio)`.
- Older seL4 (before 8.0.0): the check was
  `dest->tcbPriority >= ksCurThread->tcbPriority` — destination must be at least
  as high-priority as the sender. This disallowed fast-path to a lower-priority
  receiver even if nothing higher was runnable. seL4 8.0.0 relaxed this to allow
  fast-path when the destination is the highest runnable regardless of sender's
  priority.
- Destination thread is in the current scheduling domain
  (`dest->tcbDomain == ksCurDomain`). Multi-domain configurations limit
  fast-path to intra-domain calls.

#### Address-space / hardware checks

- Destination thread has a valid virtual address space root:
  `isValidVTableRoot_fp(dest->tcbVTable)` — the destination's page table is
  loaded and not null.
- **AArch64**: ASID mapping must exist for the destination vspace, and the
  stored ASID must match the vspace root: `armv_contextIDIsValid(asid)` and
  `asid == stored_asid`.
- **AArch32**: The hardware ASID allocated for the destination vspace must be
  valid (not exhausted from the ASID pool).
- **IA-32**: Destination thread must not be under hardware single-step debug
  (`!dest->tcbArch.tcbContext.breakpointState.singleStepping`).

#### SMP check (seL4 SMP configurations)

- Both the current thread and the destination thread must have identical core
  affinity: `ksCurThread->tcbAffinity == dest->tcbAffinity`. Cross-core IPC
  cannot use the fast path — the receiver's CPU would need an inter-processor
  interrupt to resume, which is not on the fast path.

**Source:** `src/fastpath/fastpath.c` in the seL4 kernel source
(github.com/seL4/seL4); seL4 Reference Manual 14.0.0 §4.2–4.3; seL4 8.0.0
release notes.

---

### seL4 — MCS (Mixed-Criticality Scheduling) Configuration

The MCS configuration introduces scheduling contexts (SC) as explicit kernel
objects. The fast path carries all classic conditions plus additional SC checks.

#### Additional receiver checks (MCS)

- A `Reply` object cap must be provided by the caller to the `seL4_Call` syscall
  (the kernel deposits the reply entry into the `Reply` object rather than in
  the callee's TCB slot).
- `fastpath_reply_cap_check(reply_ptr)`: the `Reply` object must be valid.
- For `seL4_ReplyRecv` replay: the Reply object must be at the head of the call
  chain (`reply_ptr->replyNext` is the correct caller), and the caller must be
  in `BlockedOnReply` state.
- Caller's fault type must be null or VM fault — no pending hard fault blocks
  the reply fast path.
- The caller's scheduling context must match what the callee's call chain
  records:
  `SC_PTR(call_stack_get_callStackPtr(reply_ptr->replyNext)) == ksCurThread->tcbSchedContext`.
  If the scheduling context has been transferred or is mismatched, the fast path
  is skipped.

**Source:** seL4 MCS pre-release 10.1.1 release notes; Lyons et al., "Mixed
Criticality Systems with seL4" (RTSS 2018); RFC-13 mailing list discussion on
GrantReply rights in MCS.

---

### L4 Version 2 (Liedtke, 1993) and L4Ka::Pistachio

The original L4 (Liedtke, i386, 1993) established the template for the IPC fast
path. L4Ka::Pistachio (KIT, ~2003) refined it for the L4 Version 4 API.

#### Conditions in L4v2 / Pistachio

- **Receiver state**: Receiver thread must be in `recv_blocked` state (already
  executing a receive operation, waiting at an endpoint/thread ID).
- **Message type — untyped words only**: The fast path covers only untyped
  (register) message words. Pistachio's fast path is documented as covering
  "only untyped message registers." Any typed items (memory/IO descriptors) go
  to the slow path.
- **Short message**: Message fits in the available hardware registers (or UTCB
  virtual registers). Pistachio's fast path maps UTCB virtual registers to
  physical registers when the message is short enough.
- **SMP**: Pistachio's fast path is **incompatible with SMP**. The fast path
  assumes single-CPU; SMP configurations must use the slow path for all IPC.
  This is explicitly documented in Pistachio architecture-specific notes.
- **Priority and scheduling**: Liedtke's original design transferred the
  sender's remaining timeslice directly to the receiver ("time donation" or
  "lazy scheduling"). Later L4 variants moved to "Benno scheduling" — keeping
  run queues consistent at all times, accepting a small overhead on the fast
  path to avoid the complexity of deferred scheduler updates.

**Source:** Liedtke, "Improving IPC by Kernel Design" (SOSP 1993); L4Ka
Pistachio architecture performance notes (l4ka.org/english/121.php); L4Ka source
code comments.

---

### Fiasco.OC (L4.re)

Fiasco.OC implements an IPC fast path similar in structure to seL4's.

#### Conditions

- **Receiver state**: Receiving thread must be `Receive_wait` (IPC-blocked
  waiting for a sender).
- **Direct switch**: The kernel performs a direct context switch from sender to
  receiver if: (a) the receiver is waiting, and (b) no higher-priority runnable
  thread exists. The run queue is not consulted if the direct switch is legal.
- **Message**: Short message only — the fast path uses register-passed words. No
  typed items or capability transfers on the fast path.
- **Priority**: After the context switch, the scheduler's run queue is updated
  consistently ("Benno scheduling" model — deferred lazy-scheduling was
  abandoned because it complicated worst-case latency analysis and the fast-path
  overhead savings did not justify the complexity).
- **MCS/transactional**: Fiasco.OC has a transactional IPC extension (Fiasco.OC
  POSIX-IPC paper, OSPERT 2015) that adds additional checks around transaction
  state, but the base fast path retains the receiver-waiting + priority
  conditions above.

**Source:** Fiasco.OC source (github.com/kernkonzept/fiasco); "Transactional IPC
in Fiasco.OC" (OSPERT 2015, Bruns, Böttger, Backes).

---

### QNX Neutrino

QNX uses synchronous message passing (`MsgSend` / `MsgReceive`). There is no
explicitly labeled "fast path" in the seL4sense, but the equivalent optimization
is the **direct-switch** that occurs when the receiver is already
RECEIVE-blocked.

#### Conditions

- **Receiver state**: The server thread must be in `STATE_RECEIVE` (blocked on
  `MsgReceive()`) when the client calls `MsgSend()`. In this case the client
  transitions directly from RUNNING to `STATE_REPLY` (REPLY-blocked), skipping
  the `STATE_SEND` (SEND-blocked/queued) state entirely. The data transfer
  occurs immediately into the receiver's address space.
- **Priority inheritance**: QNX performs priority inheritance on the direct
  switch — if the sender has higher priority than the receiver, the receiver
  inherits the sender's effective priority for the duration of the message
  handling. There is no requirement for the receiver to be higher or equal
  priority before the switch; the priority relationship is resolved by
  inheritance, not by a precondition.
- **No explicit message-size fast-path threshold**: QNX does not distinguish a
  "fast" vs. "slow" path based on message size in the way seL4 does. All
  synchronous messages use the same `MsgSend`/`MsgReceive` interface; large
  messages perform a kernel-mediated copy.
- **No address-space validity check listed**: QNX's synchronous model assumes
  the server's address space is valid (it is already resident and scheduled by
  the OS). There is no ASID-check equivalent in the documented fast path.

**Source:** QNX Neutrino System Architecture Guide 7.0 §"Synchronous Messaging"
(qnx.com/developers/docs/7.0.0); QNX `MsgSend` reference (qnx.com/developers/
docs/8.0).

---

### EROS / Coyotos

EROS uses gate invocation as its IPC primitive. The "hot path" is the case where
the called domain (callee) is already waiting at a gate.

#### Conditions

- **Receiver state**: The callee domain must be waiting at a gate (the
  equivalent of "blocked on receive"). The EROS caller invokes a capability to a
  gate; if the callee is waiting, the call rendezvousing immediately.
- **Time donation**: EROS transfers the caller's remaining timeslice to the
  callee on the hot path — analogous to L4's time donation. This is part of the
  fast-path mechanism: no scheduler invocation is needed because the callee
  receives the caller's time budget directly.
- **Message**: Short capability invocation messages (4 data words + keys). EROS
  messages are fixed-size by design, so there is no size-check disqualifier —
  all invocations are the same size.
- **Coyotos** added endpoints (as separate kernel objects) and multiple-receiver
  support. The Coyotos endpoint model allows multiple callers to queue if the
  callee is not waiting, with the direct-switch semantics preserved for the
  receiver-waiting case.

**Source:** Shapiro, "EROS: A Fast Capability System" (SOSP 1999); Coyotos
Microkernel Specification (Jonathan Shapiro, 2007,
coyotos.org/coyotos-spec.pdf).

---

### Systems Without a Kernel-Level Fast Path

**Zircon (Fuchsia):** Channels are asynchronous. There is no equivalent of
`EPState_Recv` — receivers poll or use `zx_object_wait_*`. The kernel never
performs a direct context switch on the send path. Benchmarks show Zircon IPC at
approximately 9× seL4's latency (seL4 whitepaper, 2019).

**Mach / XNU:** Port-based message passing through `mach_msg`. No equivalent of
seL4's fast path — every send involves port queue insertion and a scheduler
call. Liedtke (1993) measured Mach at 100+ µs vs. L4's ~5 µs on the same i486
hardware. The `_kernelrpc_*_trap` shortcuts in XNU reduce syscall overhead for
common port operations but do not implement a receiver-waiting direct switch.

---

## Measured Data

| System                      | Platform       | Latency (approx.)   | Notes                                      |
| --------------------------- | -------------- | ------------------- | ------------------------------------------ |
| seL4 classic (fast path)    | ARM Cortex-A57 | ~400–700 cycles     | Round-trip; same-priority, hot cache       |
| seL4 classic (fast path)    | ARM Cortex-A15 | ~190 cycles one-way | From seL4 2013 SOSP paper                  |
| seL4 classic (fast path)    | x86-64         | ~280–400 cycles     | From seL4 whitepaper                       |
| seL4 slow path (no FP)      | ARM Cortex-A57 | ~3–5× fast path     | Scheduler invocation + queue ops           |
| L4Ka::Pistachio (fast path) | x86 (Pentium4) | ~0.5 µs             | UTCB fast path, 2003 benchmarks            |
| Liedtke L4 (fast path)      | i486 / 100 MHz | ~5 µs               | Liedtke SOSP 1993; Mach was 100+ µs        |
| Zircon (no fast path)       | x86-64         | ~9× seL4            | seL4 whitepaper comparative table          |
| Mach (no fast path)         | various        | 10–20× L4           | Liedtke 1993 critique paper                |
| QNX direct-switch           | ARM / x86      | <5 µs (typical)     | No published cycle count for direct switch |

Cycle counts vary with cache warmth. The seL4 fast path is designed to fit in L1
cache (~4 KB of kernel code). A cache-cold fast path is approximately 2–3×
slower than the hot-cache measurements above.

---

## Tradeoffs

### Receiver-waiting check: endpoint queue vs. per-thread state

- **Endpoint queue state** (seL4, EROS, Fiasco.OC): The kernel checks the
  endpoint/gate object's state field (`EPState_Recv`). This requires one load
  from the endpoint object, but that object is likely in cache after the
  capability lookup that preceded it.
- **Per-thread state** (QNX): The channel's receive queue is scanned for a
  waiting thread. Cost is similar but the data structure differs (thread queue
  vs. endpoint state enum).

### Priority check: strict ≥ vs. "highest in domain"

- **Strict ≥ (old seL4):** Simpler check, but disallows fast-path to
  lower-priority receivers even when nothing higher is runnable. Penalizes
  server designs where the server runs at lower priority than its clients.
- **Highest-in-domain (seL4 8.0.0+):** Allows fast-path to a lower-priority
  receiver if nothing higher is runnable. More permissive, but requires reading
  the domain's max-priority runnable state — one additional check.

### SMP: same-core requirement vs. IPI-assisted switch

- **Same-core requirement (seL4, Pistachio):** The fast path is only available
  for threads with identical core affinity. Cross-core sends always use the slow
  path (enqueue + IPI). Simpler, avoids cross-core synchronization on the fast
  path.
- **IPI-assisted fast path (not implemented in any surveyed kernel):** Would
  allow a direct wake-on-remote-core path, but requires synchronization between
  cores on the fast path, negating most of the latency benefit.

### Message size threshold: register count vs. fixed-size

- **Register count threshold (seL4, Pistachio, Fiasco.OC):** Fast path handles
  messages up to N register words. Messages exceeding N use the slow path with a
  kernel-mediated IPC buffer copy. Tradeoff: threshold is a design parameter;
  increasing it increases register pressure in fast-path code.
- **Fixed-size always (EROS):** EROS messages are fixed-size by design (4 data
  words + 4 keys). No size check needed; all invocations are candidates for the
  hot path. Simpler fast path but restricts message flexibility.
- **No threshold (QNX):** No fast-path downgrade by size; all synchronous
  messages go through the same path. The kernel copy handles large messages
  without a separate slow path, at the cost of non-trivial per-message overhead
  for large payloads.

### ASID / address-space check: verify-or-skip

- **Verify on fast path (seL4):** The fast path re-validates the destination's
  VTable and ASID on every invocation. Cost: ~2–3 memory loads (likely in
  cache). Benefit: guarantees the switch is safe; avoids stale-ASID faults after
  a context switch.
- **Lazy (not implemented in any surveyed kernel):** Skip the ASID check and
  handle ASID faults as deferred exceptions. Would save cycles on the fast path
  but introduces a fault recovery path for what is currently a guaranteed-valid
  path.

### Capability transfer: exclusion vs. slow-path fallback

- All surveyed kernels that have a fast path (seL4, L4, Fiasco.OC) exclude
  capability transfer from the fast path. Capability transfer requires
  capability space manipulation (derivation, badge copying, reference counting),
  which cannot be done in the ~100-instruction fast-path budget.
- The tradeoff is not "allow cap transfer on fast path" vs. "exclude it" — no
  kernel has achieved cap transfer at fast-path cost. The real question is
  whether the fast path predicate should check for cap transfer (and fall back
  cleanly) or require it to be structurally impossible (e.g., fixed-width
  messages with no cap field). seL4 uses the former (runtime check); EROS uses
  the latter (fixed schema, no inline cap transfer — caps are separate key slots
  in the fixed message format).

---

## References

1. seL4 kernel source, `src/fastpath/fastpath.c`.
   https://github.com/seL4/seL4/blob/master/src/fastpath/fastpath.c

2. seL4 Reference Manual, Version 14.0.0, §4.2–4.3.
   https://sel4.systems/Info/Docs/seL4-manual-latest.pdf

3. seL4 8.0.0 release notes — fastpath priority-check relaxation.
   https://docs.sel4.systems/releases/sel4/8.0.0.html

4. Liedtke, J. "Improving IPC by Kernel Design." SOSP 1993. (Classic paper
   establishing the receiver-waiting direct-switch model.)

5. Elphinstone, K. and Heiser, G. "From L3 to seL4: What Have We Learnt in 20
   Years of L4 Microkernels?" SOSP 2013.
   https://flint.cs.yale.edu/cs428/doc/L3toseL4.pdf

6. Lyons, A. et al. "Mixed Criticality Systems with seL4." RTSS 2018. (MCS fast
   path and scheduling-context conditions.)

7. Bruns, B. et al. "Transactional IPC in Fiasco.OC." OSPERT 2015.
   https://people.mpi-sws.org/~bbb/events/ospert15/pdf/ospert15-p19.pdf

8. Shapiro, J.S. "EROS: A Fast Capability System." SOSP 1999.

9. QNX Neutrino System Architecture Guide 7.0, §"Synchronous Messaging."
   http://www.qnx.com/developers/docs/7.0.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Sync_messaging.html

10. L4Ka::Pistachio architecture performance notes.
    https://www.l4ka.org/english/121.php

11. seL4 IPC fastpath mailing list question.
    https://devel.sel4.systems.narkive.com/fJE3ecgZ/sel4-sel4-ipc-fastpath-question
