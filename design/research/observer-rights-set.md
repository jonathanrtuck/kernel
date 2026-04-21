# Observer Capability Rights Set

## The Question

What is the complete rights set that should be defined on an Observer
capability? A minimum is settled (resume, destroy, install-cap, write-registers,
clone), but the following candidates remain open: suspend (external pause),
read-registers (debugging), scheduling modification, change-fault-handler, and
duplicate-control (a Zircon-style per-handle clonability flag). This document
surveys how real systems gate operations on their execution unit objects.

---

## Survey of Existing Systems

### seL4 — TCB Capability Rights

seL4 TCB capabilities carry the standard seL4 access rights bits: **Read**,
**Write**, **Grant**, **GrantReply**. The kernel enforces which right is needed
for each TCB method invocation.

**Operation-to-right mapping:**

| Operation                  | Required right                         | Notes                                                                  |
| -------------------------- | -------------------------------------- | ---------------------------------------------------------------------- |
| `TCB_ReadRegisters`        | Read                                   | Inspect register state                                                 |
| `TCB_WriteRegisters`       | Write                                  | Modify registers; resume flag available                                |
| `TCB_CopyRegisters`        | Write                                  | Copy registers between two TCBs; covers both directions                |
| `TCB_Suspend`              | Write                                  | Halt a running or blocked thread                                       |
| `TCB_Resume`               | Write                                  | Make a suspended or faulted thread runnable                            |
| `TCB_Configure`            | Write + Grant                          | Sets fault endpoint, CSpace, VSpace; Grant needed to install caps      |
| `TCB_SetSpace`             | Write + Grant                          | Change fault EP, CSpace, VSpace individually                           |
| `TCB_SetPriority`          | Write (target) + Write (authority_tcb) | The priority cannot exceed the authority TCB's max-controlled priority |
| `TCB_SetSchedParams` (MCS) | Write (target) + Write (authority_tcb) | Same pattern; also binds a SchedContext                                |
| `TCB_SetMCPriority`        | Write                                  | Only reduces max-controlled priority                                   |
| `TCB_SetTLSBase`           | Write                                  | Mutable like other GPRs                                                |
| `TCB_BindNotification`     | Write + Grant                          | Binds a notification cap to the TCB                                    |

**The scheduling authority cap pattern.** `seL4_TCB_SetPriority` and
`seL4_TCB_SetSchedParams` take two capability arguments: the target TCB cap and
a second "authority" TCB cap. The priority that can be assigned is bounded above
by the authority TCB's current max-controlled priority. This prevents a holder
of the target TCB cap from granting the thread arbitrary priority — they can
only set priority up to what they themselves are authorized for. The authority
cap is a separate invocation argument, not a rights bit.

**Read vs. Write split.** The distinction between `Read` (ReadRegisters) and
`Write` (WriteRegisters, Suspend, Resume, Configure) is deliberate: it enables a
"read-only inspector" capability. A debugger can be given a TCB cap with only
the Read right, granting register inspection without the ability to modify
state, suspend, or reconfigure the thread.

**Source:** seL4 Reference Manual 14.0.0 §4, §9; seL4 kernel source
`src/object/tcb.c`. https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
https://github.com/seL4/seL4/blob/master/src/object/tcb.c

---

### Zircon (Fuchsia) — Thread Handle Rights

Zircon uses a per-handle rights bitmask. Each thread handle carries an explicit
set of rights at creation; rights can only be reduced (never amplified) by
`zx_handle_duplicate`.

**Operation-to-right mapping for thread handles:**

| Syscall                            | Required right       | Purpose                             |
| ---------------------------------- | -------------------- | ----------------------------------- |
| `zx_thread_read_state`             | `ZX_RIGHT_READ`      | Read register state                 |
| `zx_thread_write_state`            | `ZX_RIGHT_WRITE`     | Write register state                |
| `zx_task_suspend`                  | `ZX_RIGHT_WRITE`     | Externally pause a thread           |
| `zx_task_kill`                     | `ZX_RIGHT_DESTROY`   | Terminate a thread (irreversible)   |
| `zx_object_get_info` (thread info) | `ZX_RIGHT_INSPECT`   | Query metadata without state access |
| `zx_handle_duplicate`              | `ZX_RIGHT_DUPLICATE` | Create an additional handle         |
| (transfer via channel)             | `ZX_RIGHT_TRANSFER`  | Move the handle to another process  |

**Explicit Destroy right.** Zircon gives `ZX_RIGHT_DESTROY` its own distinct
bit, separate from `ZX_RIGHT_WRITE`. This means: an attenuated handle with
`ZX_RIGHT_WRITE` but not `ZX_RIGHT_DESTROY` can suspend but cannot kill. The
inverse (kill-only without suspend) is also expressible.

**Suspension token model.** `zx_task_suspend` does not take a resume parameter.
Instead, it returns a **suspend token** handle. The thread remains suspended
until all outstanding suspend token handles are closed. Multiple holders can
independently suspend the same thread; the thread resumes only when _all_ tokens
are released. This decouples "authority to suspend" from "authority to resume":
the latter is implicit in token destruction.

**`ZX_RIGHT_DUPLICATE` as meta-right.** This right controls whether the handle
holder can produce additional handles via `zx_handle_duplicate`. It is not about
operations on the thread object; it is about the authority to widen the set of
handle holders. Stripping this right before transferring a handle creates a
unique-ownership model for an otherwise duplicatable type.

**Source:** Fuchsia syscall reference.
https://fuchsia.dev/fuchsia-src/reference/syscalls/thread_read_state
https://fuchsia.dev/fuchsia-src/reference/syscalls/thread_write_state
https://fuchsia.dev/reference/syscalls/task_suspend
https://fuchsia.dev/fuchsia-src/reference/syscalls/task_kill
https://fuchsia.dev/fuchsia-src/concepts/kernel/handles

---

### Mach / XNU — Thread Port Model

Mach expresses authority over threads through port rights. The thread's "kernel
port" is a Mach port; holding a send right to that port allows any thread
operation.

**Thread operations and their authorization:**

| Operation                         | Authorization                                             |
| --------------------------------- | --------------------------------------------------------- |
| `thread_suspend`                  | Send right to thread kernel port                          |
| `thread_resume`                   | Send right to thread kernel port                          |
| `thread_get_state`                | Send right to thread kernel port                          |
| `thread_set_state`                | Send right to thread kernel port                          |
| `thread_terminate`                | Send right to thread kernel port                          |
| `thread_set_priority` (base)      | Send right to thread kernel port                          |
| `thread_max_priority` (raise max) | Send right to **processor-set control port** (privileged) |
| `thread_policy`                   | Send right to thread kernel port                          |

Mach does not sub-divide the thread send right by operation. Any holder of a
send right to the thread's kernel port can call any thread operation — including
suspend, read state, write state, and terminate. There is no "read-only
observer" role at the Mach kernel level.

**The processor-set control port for scheduling authority.** Raising a thread's
maximum priority requires a send right to the processor set's _control port_,
not merely the thread port. The control port for the default processor set is
privileged (held only by the kernel initially). This is the Mach analog to
seL4's authority-cap argument: scheduling ceiling changes require proof of
authority not contained in the thread cap itself.

**Suspend count semantics.** `thread_suspend` is cumulative: calling it N times
increments the suspend count by N. The thread only resumes when `thread_resume`
has been called N times (count returns to zero). This allows multiple
independent holders to each "hold" a thread suspended without explicit
coordination, similar to Zircon's token model but expressed as a count.

**Source:** GNU Mach Reference Manual.
http://gnu.ist.utl.pt/software/hurd/gnumach-doc/mach_7.html
https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/thread_suspend.html
https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/thread_get_state.html

---

### QNX Neutrino — No Capability-Gated Thread Rights

QNX has no kernel capability model for thread authority. A process owns its
threads; interprocess thread manipulation is not a first-class kernel concept.
External debugging is mediated by the `procnto` resource manager and `/proc`
filesystem (a POSIX-ambient model, not capability-gated).

QNX threads carry per-thread scheduling state (priority, policy) that can be
changed by any thread in the same process, or by processes with the
`PROCMGR_AID_SCHEDULE` ability for cross-process changes.

**Source:** QNX Neutrino System Architecture documentation.
https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/kernel_THREADSANDPRO.html

---

### EROS / Coyotos — Keys on Processes

EROS has several key types for process control. The relevant types for the
authority question:

- **Start key:** Allows resuming the process by invoking the key. The invoker
  passes a message; the process wakes from wait at the invocation point.
- **Resume key:** Similar to start key but used for fault resumption.
- **Fetch/store register keys** (in some EROS derivatives): separate authority
  for reading vs. writing process register state.
- **Suspend / domain key** variants for different authority levels.

The EROS design explicitly separates "can invoke (resume)" from "can read
registers" from "can write registers" — the three are distinct key types or
capabilities. This allows a debugger to hold read-without-write authority.

**Source:** Shapiro, J.S. "EROS: A Fast Capability System." SOSP 1999.
https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf

---

### Genode — CPU Session and Thread Authority

Genode wraps execution units behind CPU session capabilities. A component
interacts with threads through CPU session objects; direct manipulation of
another component's threads is not possible without holding the CPU session cap.

The CPU session capability provides a coarse-grained authority: a holder can
create, pause, resume, and configure threads within that session. Genode does
not expose a per-thread read-registers vs. write-registers right split at the
framework level; the CPU session is either full-authority or inaccessible.

**Source:** Genode Foundations documentation.
https://genode.org/documentation/genode-foundations-25-05.pdf

---

## Cross-System Patterns

### Pattern 1: Read and write register rights are commonly separated

seL4 (`Read` vs `Write` on TCB cap) and Zircon (`ZX_RIGHT_READ` vs
`ZX_RIGHT_WRITE`) independently separated read-register authority from
write-register authority. EROS similarly distinguished register-read from
register-write key types.

The rationale is consistent across sources: a debugger needs to observe but not
modify; a checkpoint service needs to read but not alter execution; a profiler
needs sample state but must not interfere.

Systems that do not separate them (Mach, QNX) push the distinction to higher
layers (the debug server holds full authority; it internally decides whether to
use read or write operations).

---

### Pattern 2: Suspend and destroy are often distinct rights

Zircon explicitly separates `ZX_RIGHT_WRITE` (suspend) from `ZX_RIGHT_DESTROY`
(terminate/kill). This allows expressing: "can pause this thread temporarily but
cannot kill it."

seL4 and Mach do not enforce this distinction at the capability level: the Write
right or the send right covers both suspend and terminate. The caller decides
which to invoke.

---

### Pattern 3: Scheduling authority requires a second capability in two systems

seL4 MCS and Mach both require a second authority object (authority TCB cap;
processor-set control port) to raise a thread's scheduling ceiling. The thread
cap/port alone is not sufficient. This prevents a holder of a thread cap from
inflating the thread's priority beyond what the cap-holder is themselves
authorized for.

Systems without this pattern (Zircon profiles, QNX process-owned scheduling)
express scheduling authority through separate object types, not through the
thread handle.

---

### Pattern 4: Suspend is distinct from the fault/blocked state in all surveyed systems

Every surveyed system with external thread control (seL4, Zircon, Mach) treats
external suspension as a state separate from blocking on IPC or faulting:

- **seL4:** `Suspended` is a TCB state distinct from `BlockedOnReceive`,
  `BlockedOnSend`, `Restart` (faulted-awaiting-reply), `Running`, `Idle`.
- **Zircon:** `ZX_THREAD_STATE_SUSPENDED` is a distinct thread signal, separate
  from `ZX_THREAD_STATE_BLOCKED_EXCEPTION` (faulted).
- **Mach:** Suspend count is independent of IPC wait state; a thread can have
  suspend count > 0 while simultaneously waiting on a port.

All three systems allow a debugger to externally suspend a thread regardless of
whether it is currently running, blocked on IPC, or in a faulted state. This
requires a kernel state machine that combines suspend count and other block
states.

Zircon's token model creates an interesting property: suspend authority can be
held by multiple parties independently. The thread resumes only when all parties
release their tokens, which prevents race conditions where one debugger resumes
a thread that another debugger is still inspecting.

---

### Pattern 5: Fault handler modification is gated by the same right as general configuration

In seL4, changing the fault endpoint (the fault handler) is part of
`TCB_Configure` and `TCB_SetSpace`, both requiring the Write+Grant right. The
fault handler is not protected by a separate right. If you can write-configure
the TCB, you can change its fault handler.

Zircon uses a different mechanism: exception channels are registered via
`zx_task_create_exception_channel` and treated as separate objects. The thread
handle is not directly involved in fault handler installation — exception
channels have their own lifecycle.

---

## Rights-per-Operation Matrix (All Systems)

| Right / Operation    | seL4                  | Zircon                       | Mach                               | EROS            |
| -------------------- | --------------------- | ---------------------------- | ---------------------------------- | --------------- |
| Resume / restart     | Write                 | Write                        | send right                         | start key       |
| Suspend              | Write                 | Write                        | send right                         | (no equivalent) |
| Destroy / terminate  | Write                 | Destroy                      | send right                         | —               |
| Read registers       | **Read**              | **Read**                     | send right                         | read-reg key    |
| Write registers      | Write                 | Write                        | send right                         | write-reg key   |
| Change scheduling    | Write + authority_cap | profile object               | send right + proc-set control port | —               |
| Change fault handler | Write + Grant         | exception channel (separate) | send right                         | —               |
| Duplicate handle     | Grant                 | Duplicate                    | —                                  | —               |
| Inspect metadata     | Read                  | Inspect                      | send right                         | —               |

---

## Tradeoffs

**Coarse-grained (single right covers many operations, Mach model):**

- Simpler rights model: fewer rights to enumerate, easier to understand.
- No way to delegate "read-only inspector" authority — any holder can write,
  suspend, and terminate.
- Suitable when all operations on a thread are expected to require the same
  trust level. Debugging components must hold full control authority.

**Fine-grained operation separation (seL4/Zircon model):**

- Read-only inspector caps are expressible: a debugger gets Read (seL4) or
  ZX_RIGHT_READ (Zircon) without getting Write.
- Suspend-without-destroy is expressible (Zircon only).
- The set of rights grows with the number of distinct operations, increasing the
  design surface.
- Rights attenuation to "debug-only" is natural: mint a copy of the thread cap
  with Write stripped.

**Separate scheduling authority (seL4 MCS / Mach proc-set model):**

- Prevents a thread-cap holder from inflating scheduling priority above their
  own authorization.
- Requires an additional capability argument for the scheduling change call.
- The delegating party must hold both the thread cap and an adequate authority
  cap to grant scheduling changes.
- Absent this: holding a thread cap implies unconstrained scheduling changes
  (can set any priority up to the system maximum).

**Suspension token model vs. count model:**

- Token model (Zircon): each suspending party holds an independent handle;
  releasing it is a natural lifecycle event. Dropped tokens (e.g., debugger
  crash) automatically resume the thread.
- Count model (Mach): explicit resume calls must match suspend calls; a debugger
  crash with outstanding suspend leaves the thread permanently suspended.
- The token model is more resilient to caller failures.

**Fault handler change as separate right:**

- If the fault handler can be changed by any Write-authorized holder, then
  granting "debug write" authority implicitly grants "redirect fault handling"
  authority. These are distinguishable operations with different security
  implications.
- Zircon avoids this by separating exception channels from the thread handle
  entirely.
- seL4 does not separate them — Configure covers fault endpoint installation.

**Duplicate-control right (meta-right):**

- Enables enforcing unique ownership of a cap without changing the object type.
- Zircon's `ZX_RIGHT_DUPLICATE` is the canonical example: strip it before
  transfer and the receiver cannot produce additional copies.
- Without this right, the only way to enforce unique ownership is to use a
  non-clonable type at the kernel level (Mach receive-right approach).
- With this right, clonability is a per-instance property, not a per-type
  property.

---

## Measured Data

No published benchmarks isolate the cost of rights checking per operation. The
design dimensions are primarily about expressiveness, not performance.

From adjacent data:

- seL4 capability lookup on a cached path is a small fraction of total IPC
  overhead (two memory accesses for a two-level CSpace walk). Rights bit-testing
  adds no perceptible overhead on top of this.
- Zircon: per-handle rights checking is a bitmask AND before each syscall. Cost
  is bounded by syscall entry overhead, not rights checking itself.

The suspension token model (Zircon) introduces an object handle for each
outstanding suspend token. In a system with N debugger attachments, N handles
exist. Closing all N triggers thread resumption, each requiring a handle close
operation. This is O(tokens) work on resume, not O(1).

---

## References

- seL4 Reference Manual, Version 14.0.0.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf

- seL4 kernel source — `src/object/tcb.c`.
  https://github.com/seL4/seL4/blob/master/src/object/tcb.c

- seL4 API Reference. https://docs.sel4.systems/projects/sel4/api-doc.html

- Fuchsia syscall reference — `zx_thread_read_state`.
  https://fuchsia.dev/fuchsia-src/reference/syscalls/thread_read_state

- Fuchsia syscall reference — `zx_thread_write_state`.
  https://fuchsia.dev/fuchsia-src/reference/syscalls/thread_write_state

- Fuchsia syscall reference — `zx_task_suspend`.
  https://fuchsia.dev/reference/syscalls/task_suspend

- Fuchsia syscall reference — `zx_task_kill`.
  https://fuchsia.dev/fuchsia-src/reference/syscalls/task_kill

- Fuchsia documentation — Zircon Handles.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/handles

- Fuchsia documentation — Exception Handling.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/exceptions

- GNU Mach Reference Manual — Thread Execution.
  http://gnu.ist.utl.pt/software/hurd/gnumach-doc/mach_7.html

- Apple / XNU — `thread_suspend` man page (MIT mirror).
  https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/thread_suspend.html

- Apple / XNU — `thread_get_state` man page (MIT mirror).
  https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/thread_get_state.html

- Shapiro, J.S., Smith, J.M., Farber, D.J. (1999). "EROS: a fast capability
  system." SOSP '99.
  https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf

- Genode OS Framework Foundations (25.05).
  https://genode.org/documentation/genode-foundations-25-05.pdf
