# Observer Creation API

## The Question

When the kernel must create a new schedulable execution unit, what shape does
the API take? Four specific sub-questions:

1. **Create-then-configure vs. all-params-upfront.** Does creation take a rich
   parameter set, or is a newly-created unit in a bare/unconfigured state that
   requires subsequent operations before it can run?
2. **Initial PC/SP provision.** How and when does the caller specify the
   instruction pointer and stack pointer for first execution?
3. **Initial capability set.** How does a newly-created unit receive its first
   capabilities — and how many can it receive atomically at creation?
4. **Create vs. start as separate operations.** Is there an observable kernel
   state of "created but not started," and is that state useful?

---

## Survey of Existing Systems

### seL4

**API sequence (4–5 syscalls):**

```c
// 1. Allocate a TCB object from untyped memory
seL4_Untyped_Retype(untyped_cap, seL4_TCBObject, seL4_TCBBits,
                    cnode, 0, 0, tcb_slot, 1);

// 2. Assign CSpace, VSpace, IPC buffer
seL4_TCB_Configure(tcb_slot, fault_ep, cspace_root, cspace_root_data,
                   vspace_root, vspace_data, ipc_buffer_addr, ipc_buffer_cap);

// 3. Set scheduling parameters (priority, scheduling context in MCS)
seL4_TCB_SetPriority(tcb_slot, authority_tcb, priority);

// 4. Set initial registers (PC, SP, argument registers)
seL4_TCB_WriteRegisters(tcb_slot, 0, 0, reg_count, &regs);

// 5. Make runnable
seL4_TCB_Resume(tcb_slot);
```

Steps 4 and 5 can be merged: the `resume_target` parameter of
`seL4_TCB_WriteRegisters` can be set to 1 to resume immediately.

**PC/SP provision:** `seL4_TCB_WriteRegisters` takes a `seL4_UserContext` struct
containing PC, SP, and general-purpose registers. The struct is
architecture-specific but always includes at minimum PC and SP.

**Initial capabilities:** The new TCB starts with an empty CSpace slot.
`seL4_TCB_Configure` installs a CNode as the CSpace root — the caller supplies
the root. That CSpace must be populated by the creator _before_ calling
`seL4_TCB_Resume`, because the new thread will reference it immediately. The new
thread receives whatever the creator pre-loaded into its CSpace.

There is no atomic "here are N capabilities for you" primitive. The creator uses
normal capability derivation and copy operations to build the child's CSpace,
then attaches it.

**Create vs. start:** Always separate. The TCB exists as a kernel object from
the moment of `Untyped_Retype`. It can be inspected (`TCB_ReadRegisters`) and
modified during the configure/write-registers phase without any concern about
the thread executing prematurely.

The fault endpoint is part of `TCB_Configure` (second argument). A null cap is
valid; the TCB still starts, but any fault causes a kernel error rather than
delivery to a handler.

**Source:** seL4 Reference Manual 14.0.0 §4 (Thread Control Block), §9 (System
Calls). https://sel4.systems/Info/Docs/seL4-manual-latest.pdf Tutorial:
https://docs.sel4.systems/Tutorials/threads.html

---

### Zircon (Fuchsia)

**API sequence:**

```c
// 1. Create thread object within a process (does not start it)
zx_thread_create(process_handle, name, name_size, 0, &thread_out);

// 2. Start execution (for threads after the first in a process)
zx_thread_start(thread, pc, sp, arg1, arg2);
// OR for the first thread of a new process:
zx_process_start(process, thread, entry, stack, arg1_handle, arg2);
```

**PC/SP provision:** Both start calls take PC (`entry`) and SP (`stack`) as
explicit parameters. The thread starts executing at `pc` with `sp` set. There is
no register-configuration step; registers not explicitly set default to zero.

Fuchsia API level 31 adds `zx_thread_start_regs` which also takes `tp` (thread
pointer) and `abi_reg` (shadow-call-stack pointer):

```c
zx_thread_start_regs(handle, pc, sp, arg1, arg2, tp, abi_reg);
```

**Initial capabilities — single bootstrap handle:** A brand-new process has an
empty handle table. `zx_process_start` takes a single `arg1_handle` which is
_transferred_ from the caller's handle table into the new process's handle
table. The new thread receives this handle as its first argument register.

> "The first argument (arg1) is a handle, which will be transferred from the
> process of the caller to the process being started." — Fuchsia documentation,
> zx_process_start

If `arg1_handle` is `ZX_HANDLE_INVALID`, the process starts with zero handles.
Obtaining additional handles requires the first thread to use that single
bootstrap handle to communicate with a provider (typically the ELF loader or
component framework).

For `zx_thread_start` (creating additional threads in an existing process),
there is no handle transfer — the new thread shares the process's existing
handle table automatically.

**Create vs. start:** Always separate. `zx_thread_create` produces a stopped
thread handle. It can be debugged, have exception channels attached, or be
examined before starting.

**Source:** Fuchsia API reference:
https://fuchsia.dev/fuchsia-src/reference/syscalls/thread_create
https://fuchsia.dev/fuchsia-src/reference/syscalls/thread_start
https://fuchsia.dev/fuchsia-src/reference/syscalls/process_start

---

### Mach / XNU

Mach offers _both_ the multi-step and the atomic forms.

**Multi-step form:**

```c
thread_create(parent_task, &child_thread);    // creates suspended thread
thread_set_state(child_thread, flavor,        // sets PC, SP, registers
                 state, state_count);
thread_resume(child_thread);                  // makes runnable
```

**Atomic form:**

```c
thread_create_running(parent_task, flavor, state, &child_thread);
```

`thread_create_running` is explicitly documented as "an optimized form of the
sequence: `thread_create`, `thread_set_state` and `thread_resume`." The created
thread is immediately runnable; it is not suspended.

**PC/SP provision:** The `state` parameter is a machine-specific struct (e.g.,
`arm_thread_state64_t` on ARM64) containing all general registers including PC
(`__pc`) and SP (`__sp`). The `flavor` parameter selects the register set.

**Initial capabilities:** Threads do not carry authority in Mach — the _task_
holds the port-right namespace. A new thread in an existing task inherits full
access to the task's port namespace immediately. There is no per-thread
capability initialization step.

This design choice (authority at task, not thread) eliminates the initialization
problem but means all threads within a task are mutually indistinguishable in
terms of authority.

**Create vs. start:** Optional separation. `thread_create` → `thread_set_state`
→ `thread_resume` preserves the separated model. `thread_create_running`
collapses it.

**Source:** Mach Man Pages (MIT mirror):
https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/thread_create.html
https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/thread_create_running.html
Apple Kernel Programming Guide: Mach Scheduling and Thread Interfaces:
https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/scheduler/scheduler.html

---

### QNX Neutrino

```c
int ThreadCreate(pid_t pid,
                 void* (*func)(void*),
                 void* arg,
                 const struct _thread_attr* attr);
```

**PC/SP provision:** Neither PC nor SP is passed directly. The thread entry
point is a C function pointer (`func`); the kernel sets PC to it and
allocates/assigns a stack internally (or uses the stack from `attr`). This is a
higher-level model than the other systems — the caller never sees raw register
values.

The attributes structure can specify a pre-allocated stack base and size, but
not an arbitrary SP value.

**Initial capabilities:** QNX does not have a kernel capability model. Authority
is mediated by the message-passing system (channels and connections). A new
thread inherits its parent process's pulse/message connections. No explicit
capability transfer at creation time.

**Create vs. start:** The thread starts executing immediately upon creation.
There is no "created but not started" state at the kernel API level.

**Source:** QNX Neutrino Library Reference — ThreadCreate:
https://www.qnx.com/developers/docs/7.0.0/com.qnx.doc.neutrino.lib_ref/topic/t/threadcreate.html

---

### EROS

EROS processes unify execution and authority: the process _is_ both the
scheduling unit and the capability namespace. A process has 32 **capability
registers** plus general registers (PC and data registers).

**Creation via capability invocation:** There is no separate "create process"
syscall; process creation is mediated through a _constructor_ capability (a
kernel object that, when invoked, fabricates a new process). The constructor is
given a "prototype" space.

**PC/SP provision:** The initial PC and data registers are set as part of the
constructor protocol. The EROS "start capability" (a subtype of process
capability) both encapsulates the entry state and allows the holder to resume
the process. Possession of a start capability to a process controls when and how
it executes.

**Initial capabilities:** The 32 capability registers are populated before the
process is made runnable. The constructor typically pre-loads a standard set
(memory capability, scheduler capability, etc.) before issuing the start
capability.

**Create vs. start:** EROS separated the "has been fabricated" state from the
"is runnable" state via the start capability. A process fabricated but not yet
given a start capability invocation is quiescent.

**Source:** Shapiro, J.S., Smith, J.M., Farber, D.J. "EROS: a fast capability
system." SOSP '99.
https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf

---

### Coyotos

Coyotos succeeded EROS and changed the capability model from per-process
registers to a capability address space. Process capabilities have subtypes;
there is no separate "start capability." A process capability subtype that
carries the authority to resume the process replaces both the EROS start
capability and the restart capability.

The creation model evolved away from direct kernel construction. In Coyotos the
constructor concept (a user-space pattern) was formalized: a trusted constructor
service receives a prototypical image and an initial set of capabilities, then
fabricates a sealed process that it cannot subsequently inspect.

**Initial capabilities:** Provided through the capability address space before
the process is made runnable, analogous to seL4's CSpace pre-population.

**Source:** Shapiro, J.S. "Coyotos Microkernel Specification" (v0.6+).
https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html

---

### L4 (original / Liedtke)

L4 lacked a formal capability model. Thread creation used a single call that
specified the thread's initial state directly:

```c
l4_thread_ex_regs(thread_id, eip, esp, pager, ...);
```

PC (`eip`) and SP (`esp`) were passed directly. The `pager` field specified the
thread's pager (the server that handles its page faults). This is close to the
"all-params-upfront" model — a single invocation configured the thread's entry
state, though memory allocation was handled separately.

Later L4 descendants (Fiasco, L4.KeST) introduced typed capabilities and
multi-step configuration closer to seL4's model.

**Source:** Liedtke, J. "On µ-kernel construction." SOSP '95.

---

## Design Dimensions and Observed Tradeoffs

### 1. Create-then-configure vs. all-params-upfront

| Approach             | Systems               | Properties                                                                                  |
| -------------------- | --------------------- | ------------------------------------------------------------------------------------------- |
| Multi-step configure | seL4, Coyotos         | Maximum flexibility; parent can fully prepare child's namespace before start; more syscalls |
| Create + start       | Zircon, L4 (original) | Two phases; start bundles PC/SP/initial-caps in one call                                    |
| Optional collapse    | Mach                  | Both forms available; `thread_create_running` for common case                               |
| Single call          | QNX, pthreads         | Highest-level; hides PC/SP; no pre-start window                                             |

The multi-step model's key benefit is the **pre-start window**: between
`Untyped_Retype` and `TCB_Resume` (seL4) or between `zx_thread_create` and
`zx_thread_start` (Zircon), the parent can install capabilities, attach
debuggers, and configure exception handlers without risk of the child executing
prematurely.

The single-call model's benefit is atomicity and simplicity: no intermediate
observable state, and no API surface for configuring a half-formed object.

### 2. Initial PC/SP provision

All systems require PC and SP to be specified before a thread can run. The
variation is _when_ in the creation sequence this happens:

- **At start time** (Zircon, Mach/`thread_create_running`, original L4): PC and
  SP are parameters of the "go" call. This makes them impossible to inspect or
  modify after the fact.
- **As a separate configuration step** (seL4 `TCB_WriteRegisters`, Mach
  `thread_set_state`): PC and SP are written in a distinct call before resume.
  They can be read back with `TCB_ReadRegisters` (seL4) or `thread_get_state`
  (Mach).
- **Indirectly via function pointer** (QNX, pthreads): the kernel derives PC
  from a function pointer and SP from a stack descriptor. Raw PC/SP are not
  exposed.

The function-pointer model is incompatible with loading programs from ELF
images, where the entry point is a raw virtual address.

### 3. Initial capability set

This is the most varied dimension:

| Approach                              | Systems                  | Properties                                                                       |
| ------------------------------------- | ------------------------ | -------------------------------------------------------------------------------- |
| No per-thread caps; inherit task      | Mach, QNX                | Thread immediately has full access; no init step                                 |
| Pre-populate CSpace before resume     | seL4, Coyotos            | Parent builds child's cap namespace before child can execute; any number of caps |
| Single-handle bootstrap               | Zircon (`process_start`) | Exactly one handle transferred; child bootstraps rest via that channel           |
| Capability registers set before start | EROS                     | Fixed number (32) of cap registers pre-loaded                                    |
| No formal cap model                   | Original L4              | Pager specified as thread-ID; no other per-thread caps                           |

**Single-handle bootstrap** (Zircon) is notable: it enforces a clean handoff
protocol. The creator cannot "accidentally" give a new process N handles
atomically at creation; it must hand over one. If the new process needs more, it
must receive them through the established channel. This is a designed
constraint, not a limitation.

**Pre-populate model** (seL4) is the most powerful: the parent can install any
number of capabilities in any arrangement into the child's CSpace before the
child executes. The downside is that the child is not self-contained until the
parent finishes populating its CSpace.

**Inherited task namespace** (Mach) is the simplest but eliminates per-unit
authority isolation: all threads in a task are authority-equivalent.

### 4. Create vs. start as separate operations

Systems that separate create from start enable:

1. **Debugger attachment before first instruction.** The child can have a
   debug/exception handler installed while it is still stopped.
2. **Atomic capability pre-population.** For systems where the child's cap space
   is set up by the parent, the separation guarantees the child cannot run
   before the parent is done.
3. **Verified-before-start patterns.** A capability-safe constructor can create,
   inspect/validate, and only then resume — the sealed constructor pattern in
   EROS and Coyotos depends on this.

Systems that merge create and start (QNX's `ThreadCreate`, Mach's
`thread_create_running`) sacrifice this window for simplicity. The capability
initialization problem does not arise for them because authority is either
inherited or passed as the start arguments.

---

## Measured Data

**seL4 TCB creation overhead:** seL4 benchmarks report that `Untyped_Retype` for
a TCB on ARM takes approximately the same order of magnitude as a fastpath IPC
(hundreds of nanoseconds). The multi-step configure/resume sequence adds several
syscall round-trips. No published figure for full TCB creation + configure +
resume vs. single-call alternatives on the same hardware.

**Zircon process start latency:** Fuchsia engineering documentation notes that
process startup (from `zx_process_create` through first instruction execution)
includes several kernel round-trips for thread creation, VMAR setup, and handle
transfer. No single published figure is available for the `zx_thread_start` call
itself.

**Mach thread creation:** Liedtke's 1993 analysis placed Mach thread/process
creation significantly higher than L4's, primarily due to per-task
port-namespace overhead. L4's `thread_ex_regs` on i486 was measured at ~5 µs vs.
Mach's 100+ µs for comparable operations, though these cover different scopes
(Mach task creation vs. L4 thread creation). Modern XNU `thread_create`
performance is not independently published in equivalent benchmarks.

---

## References

- seL4 Reference Manual, Version 14.0.0.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4 Threads Tutorial. https://docs.sel4.systems/Tutorials/threads.html
- Fuchsia API reference — zx_thread_create.
  https://fuchsia.dev/fuchsia-src/reference/syscalls/thread_create
- Fuchsia API reference — zx_thread_start.
  https://fuchsia.dev/fuchsia-src/reference/syscalls/thread_start
- Fuchsia API reference — zx_process_start.
  https://fuchsia.dev/fuchsia-src/reference/syscalls/process_start
- Apple/XNU — thread_create_running man page (MIT mirror).
  https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/thread_create_running.html
- Apple Kernel Programming Guide: Mach Scheduling and Thread Interfaces.
  https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/scheduler/scheduler.html
- QNX Neutrino Library Reference — ThreadCreate.
  https://www.qnx.com/developers/docs/7.0.0/com.qnx.doc.neutrino.lib_ref/topic/t/threadcreate.html
- Shapiro, J.S., Smith, J.M., Farber, D.J. (1999). "EROS: a fast capability
  system." SOSP '99.
  https://sites.cs.ucsb.edu/~chris/teaching/cs290/doc/eros-sosp99.pdf
- Shapiro, J.S. "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- Liedtke, J. (1993). "Improving IPC by kernel design." SOSP '93.
- Liedtke, J. (1995). "On µ-kernel construction." SOSP '95.
