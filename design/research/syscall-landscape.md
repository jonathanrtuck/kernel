# Syscall Landscape: Survey of System Call Interfaces Across Real Kernels

How do real, pressure-tested kernels define their system call interface? What
operations does each kernel expose, why, and what did they learn along the way?

This document surveys syscall interfaces across production microkernels,
formally verified kernels, research systems, monolithic kernels (for contrast),
and unusual architectures. "Pressure tested" means the interface was shaped by
actual deployment, formal verification, or sustained research use -- not just
theory.

---

## Table of Contents

1. [Production Microkernels](#1-production-microkernels)
2. [Research and Verified Kernels](#2-research-and-verified-kernels)
3. [Monolithic Kernels (Contrast)](#3-monolithic-kernels-contrast)
4. [Exokernels and Unusual Architectures](#4-exokernels-and-unusual-architectures)
5. [Analysis: Universal Syscalls](#5-analysis-universal-syscalls)
6. [Analysis: Common but Not Universal](#6-analysis-common-but-not-universal)
7. [Analysis: Philosophy-Specific Syscalls](#7-analysis-philosophy-specific-syscalls)
8. [Analysis: The Minimal Kernel](#8-analysis-the-minimal-kernel)
9. [Analysis: Lessons from Removals](#9-analysis-lessons-from-removals)
10. [Analysis: IPC as the Pivot Point](#10-analysis-ipc-as-the-pivot-point)
11. [References](#references)

---

## 1. Production Microkernels

### 1.1 seL4

**Source:** seL4 project, Data61/CSIRO (formerly NICTA). Formally verified.
Deployed in military, aerospace, automotive (HENSOLDT Cyber).

**Syscall count:** 8 core (master kernel), 11 (MCS kernel). Plus ~60 object
method invocations via capabilities.

**Core syscalls (master/classic kernel):**

| Syscall     | Purpose                                                        |
| ----------- | -------------------------------------------------------------- |
| `Send`      | Send message to capability (blocks until delivered)            |
| `NBSend`    | Non-blocking send (polling; succeeds only if receiver waiting) |
| `Recv`      | Block until message received on endpoint                       |
| `NBRecv`    | Non-blocking receive                                           |
| `Call`      | Send + wait for reply (the standard RPC pattern)               |
| `Reply`     | Reply to a received message                                    |
| `ReplyRecv` | Atomic reply + receive (server loop optimization)              |
| `Yield`     | Donate remaining timeslice to same-priority thread             |

**Additional MCS syscalls:**

| Syscall      | Purpose                                           |
| ------------ | ------------------------------------------------- |
| `Wait`       | Receive on endpoint (no reply cap)                |
| `NBWait`     | Non-blocking wait                                 |
| `NBSendRecv` | Non-blocking send on one cap + receive on another |
| `NBSendWait` | Non-blocking send on one cap + wait on another    |

**Object method invocations (via `Call` on typed capabilities):**

All kernel resource management happens through capability invocations, not
dedicated syscalls. The kernel decodes the capability type and dispatches to the
appropriate handler. Key object types and their methods:

- **CNode** (capability space): Copy, Mint, Move, Mutate, Delete, Revoke,
  Rotate, SaveCaller, CancelBadgedSends
- **TCB** (thread control block): Configure, SetSpace, SetIPCBuffer,
  SetPriority, SetMCPriority, SetSchedParams, SetAffinity, Resume, Suspend,
  ReadRegisters, WriteRegisters, CopyRegisters, BindNotification,
  UnbindNotification, SetTLSBase, SetBreakpoint, GetBreakpoint, UnsetBreakpoint,
  ConfigureSingleStepping
- **Untyped** (raw memory): Retype (create typed objects from raw memory)
- **IRQControl**: Get (create IRQ handler capability)
- **IRQHandler**: Ack, Clear, SetNotification
- **DomainSet**: Set (assign thread to scheduling domain)
- **SchedContext** (MCS): Bind, Unbind, UnbindObject, Consumed, YieldTo
- **SchedControl** (MCS): ConfigureFlags (budget, period, replenishment)
- **Arch-specific**: Page Map/Unmap, PageTable Map/Unmap, ASIDPool Assign,
  IOPort In/Out, VCPU operations

**Design philosophy:** Minimality for formal verification. The kernel provides
exactly the mechanisms needed to implement policy in userspace. There is no
policy in the kernel -- no filesystem, no device drivers, no networking. Even
memory allocation is explicit: userspace receives Untyped capabilities and
carves them into typed objects. This makes the kernel's resource consumption
fully deterministic and verifiable.

**IPC mechanism:** Synchronous message passing via endpoints. Messages are
passed in registers (a small number of message registers, architecture-
dependent). Larger transfers use shared memory mapped via capabilities.
Notifications provide asynchronous signaling (word-sized bitmask). The MCS
kernel adds reply objects as separate capabilities to prevent reply-cap abuse.

**Notable absences:**

- No `kill` or `exit` -- threads are stopped by revoking their scheduling
  context or TCB capability
- No memory allocation syscall -- memory comes from Untyped.Retype
- No clock/time syscalls -- time is provided via scheduling context
  configuration and the MCS kernel's time management
- No filesystem, networking, or device I/O syscalls of any kind

**Pain points and lessons:**

- IPC timeouts were abandoned. In practice only zero and infinity were useful;
  no good heuristics exist for choosing timeout values in non-trivial systems
  (see [Elphinstone & Heiser 2013]).
- Long IPC (transferring large messages in-kernel) was abandoned due to
  verification complexity. Shared memory is the transfer mechanism.
- The classic kernel's reply mechanism had a subtle vulnerability: a thread's
  reply cap could be stolen. The MCS kernel fixes this with explicit reply
  objects.

---

### 1.2 L4 Family (L4Ka::Pistachio / L4 X.2)

**Source:** University of Karlsruhe (L4Ka), UNSW. The L4 X.2 API is the
"standard" L4 specification implemented by Pistachio.

**Syscall count:** 7-12 depending on version.

**L4 X.2 syscalls:**

| Syscall             | Purpose                                              |
| ------------------- | ---------------------------------------------------- |
| `Ipc`               | Synchronous message passing (send, receive, or both) |
| `ThreadControl`     | Create, configure, delete threads                    |
| `SpaceControl`      | Configure address spaces                             |
| `Schedule`          | Set thread scheduling parameters                     |
| `ExchangeRegisters` | Read/write thread register state                     |
| `Unmap`             | Revoke virtual memory mappings (fpage_unmap)         |
| `ThreadSwitch`      | Donate timeslice to specific thread                  |
| `SystemClock`       | Read system clock                                    |
| `ProcessorControl`  | Control processor features                           |
| `MemoryControl`     | Set memory attributes (caching)                      |
| `LIPC`              | Lightweight IPC (local, optimized path)              |

**Design philosophy:** Jochen Liedtke's minimality principle -- the kernel
should contain only mechanisms that _must_ be in the kernel for correctness
(security, resource isolation). Everything else belongs in userspace. The IPC
path is the critical performance metric; L4 pioneered register-based message
passing and direct process switch (sender switches directly to receiver without
going through the scheduler).

**IPC mechanism:** Synchronous, register-based. The `Ipc` syscall combines send
and receive in a single trap. Messages are passed in virtual registers (mapped
to physical registers or a thread-local UTCB). Flexpage mappings allow memory
grant/map during IPC.

**Notable absences:**

- No process/task creation syscall (threads + address spaces compose into
  processes)
- No interrupt handling syscall (interrupts delivered as IPC messages to
  designated threads)
- No memory allocation (address spaces are configured; physical frames are
  mapped during IPC via flexpages)

**Pain points:**

- "Clans and chiefs" (hierarchical IPC routing) was abandoned in all modern L4
  variants -- too complex, not useful in practice.
- IPC timeouts: same lesson as seL4 (only 0 and infinity used).
- Lazy scheduling (deferring scheduler queue updates during IPC) worked well for
  ping-pong benchmarks but hurt real workloads. Replaced by "Benno scheduling."

---

### 1.3 L4/Fiasco.OC

**Source:** TU Dresden. Powers the L4Re operating system framework. Used in
Genode, DROPS, and commercial automotive systems.

**Syscall count:** The raw syscall count is very small (IPC is essentially the
only syscall), but kernel objects expose ~50+ operations through IPC-based
capability invocation.

**Kernel objects and their operations:**

- **Task**: map, unmap, unmap_batch, delete_obj, release_cap, cap_valid,
  cap_equal, add_ku_mem, vgicc_map
- **Thread**: ex_regs, yield, switch, stats_time, vcpu_control,
  vcpu_control_ext, vcpu_resume_start, vcpu_resume_commit, register_del_irq,
  register_doorbell_irq, modify_sender_start/add/commit, arm_set_tpidruro
- **Factory**: create (parameterized by object type -- can create Thread, Task,
  IPC-Gate, IRQ, Semaphore, Scheduler, etc.)
- **IPC-Gate**: bind_thread, get_infos, set_label
- **IRQ**: attach, detach, receive, trigger, chain, unmask
- **ICU**: bind, unbind, set_mode, info, msi_info
- **Scheduler**: info, run_thread, idle_time
- **Semaphore**: up, down
- **Vcon** (virtual console): read, write
- **VM**: (virtualization operations)

**Design philosophy:** Everything is an IPC to a kernel object. The raw kernel
has essentially one syscall (IPC), and all kernel operations are expressed as
IPC messages to capability-referenced kernel objects. This unifies the userspace
and kernel object invocation model.

**IPC mechanism:** Synchronous IPC through the `l4_ipc()` family of functions.
Messages use UTCBs (User Thread Control Blocks). Supports send-only, call
(send+wait), open wait, and closed wait. Flexpage mapping during IPC provides
memory transfer.

**Notable absences:**

- No dedicated memory management syscalls (memory mapped via Task::map and IPC
  flexpage grants)
- No signal mechanism
- No dedicated interrupt syscall (interrupts are IRQ objects invoked via IPC)

---

### 1.4 OKL4

**Source:** Open Kernel Labs (spun out from NICTA/UNSW). Billions of deployed
devices (wireless basebands, secure enclaves). Now part of General Dynamics.

**Syscall count:** ~7 raw syscalls, but >200 API entry points via heavy
overloading.

**Design philosophy:** Commercial L4 derivative. Started as L4-embedded (NICTA),
evolved into a "microvisor" -- a microkernel optimized for hosting virtual
machines and isolated subsystems on mobile devices. Added capability-based
security (first L4 to ship capabilities in production, v2.1, 2008).

**IPC mechanism:** L4-family synchronous IPC with extensions for virtual machine
guest communication.

**Notable features:**

- The gap between "7 syscalls" and "200+ APIs" reveals how L4 kernels overload
  syscalls through message encoding. The actual kernel trap count is minimal,
  but the effective operation count is much larger.
- OKL4's commercial pressure drove practical hardening that academic L4s didn't
  face.

---

### 1.5 QNX Neutrino

**Source:** QNX Software Systems (now BlackBerry). Certified for automotive (ISO
26262), medical (IEC 62304), industrial (IEC 61508), aviation (DO-178C).

**Syscall count:** ~90+ kernel calls across all categories.

**Kernel calls by category:**

_Thread Management:_

- ThreadCreate, ThreadDestroy, ThreadDetach, ThreadJoin, ThreadCancel, ThreadCtl

_Synchronization:_

- SyncTypeCreate, SyncDestroy, SyncMutexLock, SyncMutexUnlock, SyncCondvarWait,
  SyncCondvarSignal, SyncSemWait, SyncSemPost

_Scheduling:_

- SchedGet, SchedSet

_Signal Management:_

- SignalKill, SignalAction, SignalProcmask, SignalSuspend, SignalWaitinfo

_Message Passing:_

- MsgSend, MsgReceive, MsgReceivePulse, MsgReply, MsgError, MsgRead, MsgWrite,
  MsgInfo, MsgSendPulse, MsgDeliverEvent, MsgKeyData

_Channels and Connections:_

- ChannelCreate, ChannelDestroy, ConnectAttach, ConnectDetach,
  ConnectServerInfo, ConnectClientInfo, ConnectFlags

_Clock and Timer:_

- ClockTime, ClockAdjust, ClockCycles, ClockPeriod, ClockId, TimerAlarm,
  TimerCreate, TimerDestroy, TimerInfo, TimerSettime, TimerTimeout

_Interrupt Handling:_

- InterruptAttach, InterruptAttachEvent, InterruptDetach, InterruptWait,
  InterruptEnable, InterruptDisable, InterruptMask, InterruptUnmask,
  InterruptLock, InterruptUnlock, InterruptHookIdle, InterruptHookTrace

**Design philosophy:** POSIX compatibility with microkernel architecture. QNX
achieves POSIX conformance by implementing the full POSIX API in userspace
resource managers that communicate with the kernel via message passing. The
kernel itself provides threads, synchronization, message passing,
channels/connections, timers, and interrupt management. Everything else
(filesystems, networking, drivers) lives in userspace processes.

**IPC mechanism:** Synchronous message passing with the MsgSend/MsgReceive/
MsgReply trio. A thread calling MsgSend() blocks until the receiver calls
MsgReceive(), processes the message, and calls MsgReply(). This three-phase
protocol (send-receive-reply) is the fundamental building block. Pulses provide
lightweight asynchronous notification (small fixed-size messages that don't
require a reply). Channels are the receive endpoints; connections attach to
channels for sending.

**Notable features:**

- The channel/connection model separates the send and receive sides of IPC,
  enabling a clean client-server model where multiple clients connect to a
  server's channel.
- MsgDeliverEvent allows servers to asynchronously notify clients.
- Pulses are a critical addition to pure synchronous IPC: they allow interrupt
  handlers (which cannot block) to notify a thread.

**Pain points:**

- The large syscall surface (compared to L4-family kernels) is a direct
  consequence of POSIX compatibility requirements. QNX needs timer, signal, and
  synchronization primitives that pure microkernels push to userspace.
- Thread-level priority inheritance on mutexes is built into the kernel because
  priority inversion is safety-critical in QNX's domains (automotive, medical).

---

### 1.6 MINIX 3

**Source:** Vrije Universiteit Amsterdam (Andrew Tanenbaum). Designed for
reliability and self-healing. NetBSD userspace runs on top.

**Syscall count:** ~50 kernel calls.

**Kernel calls by category:**

_Process Management:_

- SYS_FORK, SYS_EXEC, SYS_CLEAR, SYS_EXIT, SYS_UPDATE, SYS_SCHEDULE,
  SYS_SCHEDCTL, SYS_PRIVCTL, SYS_TRACE, SYS_SETGRANT, SYS_RUNCTL,
  SYS_GETMCONTEXT, SYS_SETMCONTEXT

_Signal Handling:_

- SYS_KILL, SYS_GETKSIG, SYS_ENDKSIG, SYS_SIGSEND, SYS_SIGRETURN

_Memory Management:_

- SYS_MEMSET, SYS_VMCTL, SYS_PADCONF

_Data Copying:_

- SYS_UMAP, SYS_UMAP_REMOTE, SYS_VUMAP, SYS_VIRCOPY, SYS_PHYSCOPY,
  SYS_SAFECOPYFROM, SYS_SAFECOPYTO, SYS_VSAFECOPY, SYS_SAFEMEMSET

_Device I/O:_

- SYS_DEVIO, SYS_SDEVIO, SYS_VDEVIO, SYS_IRQCTL, SYS_IOPENABLE, SYS_READBIOS

_System Control:_

- SYS_ABORT, SYS_GETINFO, SYS_DIAGCTL

_Clock:_

- SYS_SETALARM, SYS_TIMES, SYS_STIME, SYS_SETTIME, SYS_VTIMER

_Profiling:_

- SYS_SPROF

**Design philosophy:** Self-healing through process isolation. Device drivers
run as separate userspace processes. If a driver crashes, the reincarnation
server restarts it. The kernel is deliberately larger than L4-family kernels (it
includes a scheduler, clock, and privilege management) because MINIX 3
prioritizes reliability over minimality. The kernel call interface is richer
because system servers (PM, VFS, RS, DS, VM) need fine-grained control over
process state for restart and recovery.

**IPC mechanism:** Synchronous message passing between processes. Fixed-size
messages. Asynchronous notification via SYS_KILL signals. The "grant" mechanism
(SYS_SETGRANT, SYS_SAFECOPYFROM, SYS_SAFECOPYTO) provides controlled data
copying between address spaces -- a critical mechanism for drivers and servers
that need to access user data without shared memory vulnerabilities.

**Notable absences:**

- No capability system (uses POSIX-style privilege bits and a privilege table)
- No shared memory IPC (deliberate -- forces isolation)

**Notable features:**

- SYS_UPDATE: live update of system processes (hot-swap a driver with a new
  version without rebooting)
- SYS_PRIVCTL: runtime privilege management for system processes

---

### 1.7 Zircon (Fuchsia)

**Source:** Google. Powers Fuchsia OS, deployed on consumer devices (Nest Hub).

**Syscall count:** ~170+ (as of the latest documentation). Target was ~100;
actual count grew significantly.

**Syscalls by category (selected -- full list has 170+ entries):**

_Handles:_

- handle_close, handle_close_many, handle_duplicate, handle_replace,
  handle_check_valid

_Objects:_

- object_get_child, object_get_info, object_get_property, object_set_property,
  object_signal, object_signal_peer, object_wait_async, object_wait_many,
  object_wait_one, object_set_profile

_Threads:_

- thread_create, thread_exit, thread_start, thread_read_state,
  thread_write_state, thread_legacy_yield, thread_raise_exception

_Processes:_

- process_create, process_create_shared, process_exit, process_start,
  process_read_memory, process_write_memory

_Jobs:_

- job_create, job_set_critical, job_set_policy

_Tasks (thread, process, or job):_

- task_create_exception_channel, task_kill, task_suspend, task_suspend_token

_Channels:_

- channel_create, channel_read, channel_read_etc, channel_write,
  channel_write_etc, channel_call, channel_call_etc

_Sockets:_

- socket_create, socket_read, socket_write, socket_set_disposition

_Streams:_

- stream_create, stream_readv, stream_readv_at, stream_writev, stream_writev_at,
  stream_seek

_FIFOs:_

- fifo_create, fifo_read, fifo_write

_Events:_

- event_create, eventpair_create

_Ports:_

- port_create, port_queue, port_wait, port_cancel, port_cancel_key

_Futexes:_

- futex_wait, futex_wake, futex_requeue, futex_wake_single_owner,
  futex_requeue_single_owner, futex_get_owner,
  futex_wake_handle_close_thread_exit

_VMOs:_

- vmo_create, vmo_create_child, vmo_create_contiguous, vmo_create_physical,
  vmo_read, vmo_write, vmo_get_size, vmo_set_size, vmo_op_range,
  vmo_replace_as_executable, vmo_set_cache_policy, vmo_transfer_data

_VMARs:_

- vmar_allocate, vmar_destroy, vmar_map, vmar_unmap, vmar_protect,
  vmar_op_range, vmar_map_iob

_Pagers:_

- pager_create, pager_create_vmo, pager_detach_vmo, pager_op_range,
  pager_supply_pages, pager_query_dirty_ranges, pager_query_vmo_stats

_Clocks and Time:_

- clock_create, clock_read, clock_get_details, clock_update,
  clock_get_monotonic, clock_get_boot, nanosleep, deadline_after, ticks_get,
  ticks_per_second

_Timers:_

- timer_create, timer_set, timer_cancel

_Interrupts and Drivers:_

- interrupt_create, interrupt_bind, interrupt_wait, interrupt_ack,
  interrupt_destroy, interrupt_trigger, bti_create, bti_pin,
  bti_release_quarantine, pmt_unpin, iommu_create, resource_create, smc_call,
  cache_flush, msi_allocate, msi_create

_Hypervisor:_

- guest_create, guest_set_trap, vcpu_create, vcpu_enter, vcpu_interrupt,
  vcpu_kick, vcpu_read_state, vcpu_write_state

_System:_

- system_get_page_size, system_get_num_cpus, system_get_physmem,
  system_get_version_string, system_get_features, system_get_event,
  system_mexec, system_powerctl, system_suspend_enter

_Debug:_

- debuglog_create, debuglog_read, debuglog_write, debug_read, debug_write,
  debug_send_command

_CPRNG:_

- cprng_draw, cprng_add_entropy

_Profiling:_

- sampler_create, sampler_start, sampler_stop, sampler_read, ktrace_control,
  ktrace_read, mtrace_control

**Design philosophy:** Object-oriented capability system. Every kernel resource
is an object referenced by a handle (capability). Syscalls are non-blocking by
default (exceptions: wait_one, wait_many, port_wait, nanosleep). The kernel
provides no file-related syscalls -- filesystems are entirely userspace. Zircon
deliberately includes more in the kernel than L4-family kernels: process
hierarchy (jobs), userspace pagers, clocks as first-class objects, sockets, and
FIFOs.

**IPC mechanism:** Channels provide bidirectional, asynchronous message passing.
Messages contain data bytes and handle transfers. Channels are the primary IPC
mechanism. Sockets provide streaming byte transfer. FIFOs provide fixed-size
element queues. Ports aggregate asynchronous events from multiple sources.

**Notable features:**

- Jobs provide hierarchical process containment with policy enforcement
- Userspace pagers (pager_create, pager_supply_pages) allow user-mode page fault
  handling
- VMOs (Virtual Memory Objects) as first-class objects separate memory identity
  from address space mapping -- memory can be shared without mapping
- Futexes in the kernel enable efficient userspace synchronization
- The handle/rights model supports fine-grained capability attenuation

**Pain points:**

- The syscall count (~170) significantly exceeds the original ~100 target. The
  team acknowledged temporary syscalls for "early bringup work" that would "be
  going away," but many persisted.
- Critics note that allowing remote thread creation in other processes
  (thread_create taking a process handle) undermines process isolation.
- The large syscall surface creates a significant attack surface compared to
  L4-family kernels.
- Obsolete PCI syscalls remain in the interface, marked as deprecated.

---

### 1.8 Mach / XNU

**Source:** Carnegie Mellon University (Mach), Apple (XNU). XNU is the macOS/iOS
kernel, a hybrid of Mach and BSD.

**Mach trap count:** ~40 traps (fast-path kernel entries). Plus ~150+ Mach
kernel API functions invoked via mach_msg RPC.

**Mach traps (from mach_traps.h):**

_Identity:_

- mach_reply_port, thread_self_trap, task_self_trap, host_self_trap,
  thread_get_special_reply_port

_Messaging:_

- mach_msg_trap, mach_msg_overwrite_trap

_Semaphores:_

- semaphore_signal_trap, semaphore_signal_all_trap,
  semaphore_signal_thread_trap, semaphore_wait_trap, semaphore_wait_signal_trap,
  semaphore_timedwait_trap, semaphore_timedwait_signal_trap

_Memory (fast-path):_

- \_kernelrpc_mach_vm_allocate_trap, \_kernelrpc_mach_vm_deallocate_trap,
  \_kernelrpc_mach_vm_protect_trap, \_kernelrpc_mach_vm_map_trap,
  \_kernelrpc_mach_vm_purgable_control_trap

_Port operations (fast-path):_

- \_kernelrpc_mach_port_allocate_trap, \_kernelrpc_mach_port_deallocate_trap,
  \_kernelrpc_mach_port_mod_refs_trap, \_kernelrpc_mach_port_move_member_trap,
  \_kernelrpc_mach_port_insert_right_trap,
  \_kernelrpc_mach_port_get_attributes_trap,
  \_kernelrpc_mach_port_construct_trap, \_kernelrpc_mach_port_destruct_trap,
  \_kernelrpc_mach_port_guard_trap, \_kernelrpc_mach_port_unguard_trap,
  \_kernelrpc_mach_port_type_trap,
  \_kernelrpc_mach_port_request_notification_trap

_Scheduling:_

- thread_switch, swtch_pri, swtch, clock_sleep_trap

_Other:_

- mach_generate_activity_id, task_dyld_process_info_notify_get,
  host_create_mach_voucher_trap, mach_voucher_extract_attr_recipe_trap,
  iokit_user_client_trap

**Mach kernel API (invoked via mach_msg RPC, not direct traps):**

- _Task:_ task_create, task_terminate, task_suspend, task_resume, task_info,
  task_threads, task_set_special_port, task_set_exception_ports, task_policy
- _Thread:_ thread_create, thread_create_running, thread_terminate,
  thread_suspend, thread_resume, thread_abort, thread_get_state,
  thread_set_state, thread_info, thread_set_exception_ports, thread_switch,
  thread_policy, thread_wire
- _Port:_ mach_port_allocate, mach_port_destroy, mach_port_deallocate,
  mach_port_insert_right, mach_port_extract_right, mach_port_mod_refs,
  mach_port_move_member, mach_port_request_notification, mach_port_names,
  mach_port_type, mach_port_get_refs, mach_port_get_set_status
- _VM:_ vm_allocate, vm_deallocate, vm_protect, vm_inherit, vm_read, vm_write,
  vm_copy, vm_map, vm_remap, vm_region, vm_wire, vm_msync, vm_machine_attribute,
  vm_behavior_set
- _External Memory Management:_ memory*object*\* (create, data_request,
  data_return, data_initialize, lock_request, synchronize, terminate, etc.)
- _Host:_ host_info, host_statistics, host_page_size, host_get_clock_service,
  host_reboot, host_set_time, host_processor_slots, host_processors
- _Processor:_ processor_info, processor_start, processor_control,
  processor_assign, processor_get_assignment
- _Processor Set:_ processor_set_create, processor_set_destroy,
  processor_set_info, processor_set_max_priority, processor_set_statistics,
  processor_set_policy_control
- _Clock:_ clock_get_time, clock_set_time, clock_get_attributes, clock_alarm,
  clock_sleep, clock_map_time
- _Lock:_ lock_set_create, lock_set_destroy, lock_acquire, lock_release,
  lock_try, lock_handoff, lock_make_stable
- _Ledger:_ ledger_create, ledger_terminate, ledger_transfer, ledger_read

**Design philosophy:** First-generation microkernel. Mach attempted to be a
universal substrate: any OS personality (BSD, OS/2, DOS) could run as a
userspace server on top of Mach's VM, IPC, and task/thread abstractions. This
led to a larger-than-necessary kernel API. Apple's XNU retains the Mach layer
for IPC, VM, and task management but layers BSD directly in-kernel (hybrid
approach) for performance.

**IPC mechanism:** Port-based asynchronous message passing via mach_msg.
Messages are queued in kernel port queues. A port is a unidirectional message
queue with send and receive rights. Port rights are capabilities. mach_msg is
the single most important Mach operation -- nearly all kernel services are
invoked via mach_msg RPC. The message format supports inline data, out-of-line
memory descriptors, and port right transfers.

**Pain points:**

- Mach's IPC performance was 5-10x slower than L4. This was the primary
  motivation for the L4 family.
- The large in-kernel API (task, thread, VM, port management) gave Mach a much
  larger TCB than second-generation microkernels.
- External memory management (the pager interface) was powerful but complex and
  poorly performing.
- Apple abandoned the pure Mach microkernel model by putting BSD in-kernel,
  acknowledging that Mach's IPC overhead was unacceptable for a general-purpose
  OS.
- The `_kernelrpc_*_trap` functions in XNU exist specifically as fast-path
  bypasses for common Mach operations that were too slow via mach_msg RPC.

---

## 2. Research and Verified Kernels

### 2.1 EROS / KeyKOS / CapROS / Coyotos

**Source:** University of Pennsylvania, Johns Hopkins (EROS/Coyotos, Jonathan
Shapiro). Key Logic (KeyKOS). CapROS continues EROS.

**Syscall count:** 3 (Coyotos). EROS/KeyKOS similarly minimal.

**Coyotos syscalls:**

| Syscall     | Purpose                                         |
| ----------- | ----------------------------------------------- |
| `InvokeCap` | Invoke a capability (optionally wait for reply) |
| `CopyCap`   | Copy a capability between register and memory   |
| `Yield`     | Relinquish processor                            |

**KeyKOS syscalls:**

| Syscall  | Purpose                                              |
| -------- | ---------------------------------------------------- |
| `CALL`   | Send message to key's domain, get reply cap, suspend |
| `FORK`   | Send message to key's domain and continue running    |
| `RETURN` | Send message to key and wait for next invocation     |

**Kernel-implemented capability types (Coyotos):**

- Process, Page, CapPage, GPT (guarded page table)
- Endpoint, Entry (IPC endpoints)
- Window, Background (address space mapping)
- Schedule, SchedCtl (scheduling)
- IrqCtl, IrqWait (interrupts)
- Range (capability fabrication/revocation)
- Null, KeyBits, Discrim (utility)
- Sleep (timer), Checkpoint (persistence), ObStore, SysCtl, KernLog

**Design philosophy:** Pure capability systems. The kernel provides one
fundamental operation: invoke a capability. All resources (memory, scheduling,
I/O, IPC endpoints) are accessed through capabilities. The kernel implements a
fixed set of object types, but user-level objects use the same invocation
interface, making the boundary between kernel and user services invisible to
callers.

KeyKOS messages contain: a parameter word, up to 4096 bytes of data, and exactly
4 keys (capabilities). This fixed message format simplifies the kernel and makes
formal analysis tractable.

**IPC mechanism:** Capability invocation IS the IPC mechanism. CALL sends a
message and waits for a reply (the system creates a resume key automatically).
FORK sends without waiting. RETURN completes a server invocation and waits for
the next one. Persistence is a core feature: the entire system state (including
all capabilities) can be checkpointed to disk and restored.

**Notable absences:**

- No thread creation syscall (processes are created by invoking the Range
  capability to fabricate a Process capability)
- No memory allocation (memory comes from Range-fabricated Page capabilities)
- No file/IO syscalls (everything is a capability invocation)
- No signals, no process IDs, no UIDs

**Notable features:**

- Persistence: the kernel periodically checkpoints all of memory and
  capabilities to disk. On restart, the system resumes from checkpoint.
- Confinement: formally proven that a confined process cannot leak information
  through covert channels.
- The 3-syscall design proves that a complete OS can be built on capability
  invocation alone.

---

### 2.2 Barrelfish

**Source:** ETH Zurich and Microsoft Research. Multikernel architecture.

**Syscall count:** ~15 core syscalls + capability operations.

**Core syscalls:**

| Syscall                         | Purpose                              |
| ------------------------------- | ------------------------------------ |
| `SYSCALL_INVOKE`                | Invoke capability operation          |
| `SYSCALL_YIELD`                 | Yield processor                      |
| `SYSCALL_NOP`                   | No-op (benchmarking)                 |
| `SYSCALL_PRINT`                 | Debug output                         |
| `SYSCALL_REBOOT`                | System reboot                        |
| `SYSCALL_SUSPEND`               | Suspend execution                    |
| `SYSCALL_DISPATCHER_SETUP`      | Configure dispatcher                 |
| `SYSCALL_DISPATCHER_PROPERTIES` | Set dispatcher scheduling properties |

**Capability operations (via SYSCALL_INVOKE):**

- retype, create, mint, copy, delete, revoke, map, unmap
- get_state, cap_identify, get_size, resize
- vnode_modify_flags, vnode_copy_remap, inherit
- clean_dirty_bits, mapping_destroy, mapping_modify
- io (port I/O), vmread/vmwrite/vmptrld/vmclear (virtualization)

**Design philosophy:** The multikernel. Each core runs its own kernel instance
(CPU driver). Kernels share no memory. Inter-core communication uses explicit
message passing, mirroring the distributed systems model. The OS state is
replicated across cores by user-level "monitor" processes. System calls go to
the local CPU driver; cross-core operations are monitor-to-monitor messages.

**IPC mechanism:** Two levels. Intra-core: capability invocations to the local
CPU driver. Inter-core: user-level message passing (UMP) using shared memory
regions, managed by monitors. The kernel itself does not provide inter-core IPC
-- that is a user-level concern.

**Notable absences:**

- No inter-core IPC in the kernel (by design -- treated as a distributed systems
  problem)
- No process abstraction (dispatchers are the execution unit)
- No filesystem or device driver support in the kernel

---

### 2.3 Composite

**Source:** George Washington University (Gabriel Parmer). Component-based OS.

**Syscall count:** ~5-8 core operations.

**Core interface:**

| Operation                       | Purpose                               |
| ------------------------------- | ------------------------------------- |
| `call_cap_op(cap, op, args...)` | Generic capability invocation         |
| `cos_thd_switch(thdcap)`        | Switch to another thread              |
| `cos_asnd(asndcap)`             | Asynchronous send (activate endpoint) |
| `CAPTBL_OP_COMPACTIVATE`        | Create/activate component             |
| `CAPTBL_OP_THDACTIVATE`         | Create/activate thread                |
| `copy(d, d', s, s')`            | Copy capability between tables        |
| `delete(d, d')`                 | Delete capability                     |

Higher-level wrappers: cos_captbl_alloc, cos_pgtbl_alloc, cos_comp_alloc.

**Design philosophy:** Components as the unit of isolation and composition. The
kernel provides capability-based access control and fast inter-component
invocation via thread migration (a thread can cross component boundaries without
context switching). Mutable Protection Domains (MPD) allow the system to
dynamically merge and split protection domains based on communication patterns,
reducing IPC overhead where isolation is not needed.

**IPC mechanism:** Synchronous invocation via thread migration. When component A
calls component B, the calling thread migrates into B's protection domain. This
avoids the scheduler entirely for the common synchronous call pattern.
Asynchronous activation via cos_asnd provides event notification.

**Notable features:**

- Thread migration eliminates IPC overhead for synchronous calls
- MPD allows runtime adjustment of isolation boundaries
- Hierarchical resource management (HiRes) for CPU, memory, and I/O

---

### 2.4 CertiKOS

**Source:** Yale University FLINT group (Zhong Shao). Formally verified
concurrent OS kernel, verified in Coq.

**Syscall count:** ~8-10 primitives (mCertiKOS / mC2).

**Known primitives:**

| Primitive                    | Purpose                                    |
| ---------------------------- | ------------------------------------------ |
| `sys_yield`                  | Yield remaining time quota                 |
| `sys_spawn` / `thread_spawn` | Create new thread                          |
| `sys_sleep`                  | Add thread to sleeping queue, run next     |
| `sys_wakeup`                 | Wake sleeping thread (local or remote CPU) |
| `container_get_quota`        | Query memory quota                         |
| Trap handlers                | System call dispatch entry points          |
| IPC primitives               | Inter-process communication                |
| VM operations                | Virtual memory management                  |

**Design philosophy:** Verified concurrency. CertiKOS is built from ~30 layers,
each adding one feature. The entire kernel (6500 lines of C and x86 assembly) is
formally verified for functional correctness including concurrent behavior
(interrupts, multicore). The syscall interface is deliberately minimal to make
verification tractable -- each primitive is verified at its layer.

**Notable features:**

- First OS kernel to achieve full functional correctness verification including
  concurrency
- Layer architecture: syscalls are organized as successive refinement layers,
  not a flat table
- Per-CPU ready/pending/sleeping queues with verified inter-CPU communication

---

### 2.5 Redox

**Source:** Jeremy Soller. Rust-based microkernel, aiming for Unix
compatibility.

**Syscall count:** ~35 syscalls.

**Syscalls:**

_File Operations:_

- openat, openat_with_filter, close, read, write, lseek, dup, dup2, fstat,
  fstatvfs, fchmod, fchown, fcntl, fsync, ftruncate, futimens, fpath, flink,
  frename, unlinkat, unlinkat_with_filter, getdents, sendfd

_Memory:_

- fmap, funmap, mremap, mprotect

_Process/Thread:_

- sched_yield, mkns (create scheme namespace)

_Synchronization:_

- futex

_Time:_

- clock_gettime, nanosleep

_Scheme Calls:_

- call_ro, call_wo, call_rw, std_fs_call

**Design philosophy:** "Everything is a URL." Redox routes nearly all syscalls
through "schemes" -- userspace services that handle a URL namespace. Opening
`disk:0/partition/1` sends a message to the disk scheme. The kernel's syscall
interface looks POSIX-like (open, read, write, close), but the kernel itself
only handles routing to schemes and basic process/memory management. The actual
service logic lives in userspace scheme handlers.

**IPC mechanism:** File-descriptor-based scheme calls. A process opens a scheme
path, receiving a file descriptor. read/write/close on that descriptor send
messages to the scheme handler. call_ro/call_wo/call_rw provide structured RPC
to schemes. This is philosophically similar to Plan 9's "everything is a file"
but with URL-based naming.

**Notable absences:**

- No dedicated IPC syscall (IPC is done through scheme file descriptors)
- No signals (events delivered through scheme notifications)
- No fork (processes created through scheme operations)
- Process management (getpid, waitpid, clone/exit) is handled through scheme
  operations, not listed as top-level syscalls in the number.rs constants

---

### 2.6 Genode (base-hw kernel)

**Source:** Genode Labs (Norman Feske). OS framework that runs on multiple
kernels; base-hw is its custom microkernel.

**Syscall count:** ~25 (split between public and core-private).

**Public syscalls:**

_IPC:_

- send_request_msg, await_request_msg, send_reply_msg

_Thread control:_

- stop_thread, restart_thread, yield_thread

_Signals:_

- await_signal, cancel_next_await_signal, submit_signal, pending_signal,
  ack_signal, kill_signal_context

_Time:_

- timeout, time, timeout_max_us

**Core-private syscalls (only callable by the root component):**

_Protection domains:_

- new_pd, update_pd, delete_pd

_Threads:_

- new_thread, start_thread, resume_thread, thread_quota, pause_thread,
  delete_thread, thread_pager, cancel_thread_blocking

_Signals:_

- new_signal_receiver, delete_signal_receiver, new_signal_context,
  delete_signal_context

_Objects and interrupts:_

- new_obj, delete_obj, new_irq, ack_irq, delete_irq

**Design philosophy:** Two-tier syscall interface. Public syscalls are available
to all components. Core-private syscalls are restricted to Genode's core process
(which acts as the root of the component tree). This avoids capability
complexity while still restricting privileged operations. The signal system is
Genode's asynchronous notification mechanism, distinct from IPC.

**Notable features:**

- Clean separation between IPC (request/reply messages) and signals
  (asynchronous notification)
- Dual scheduling model: "claims" for low-latency and "fills" for throughput

---

## 3. Monolithic Kernels (Contrast)

### 3.1 Linux

**Syscall count:** ~450+ (as of kernel 6.x).

**Categories and approximate counts:**

| Category             | Examples                                  | Approx. Count                          |
| -------------------- | ----------------------------------------- | -------------------------------------- |
| File operations      | open, read, write, close, stat, lseek     | ~50                                    |
| Directory operations | mkdir, rmdir, getdents, chdir             | ~15                                    |
| Extended attributes  | getxattr, setxattr, listxattr             | ~10                                    |
| I/O multiplexing     | select, poll, epoll*\*, io_uring*\*       | ~15                                    |
| Process management   | fork, clone, execve, exit, wait           | ~25                                    |
| Memory management    | mmap, munmap, mprotect, brk, madvise      | ~20                                    |
| Signals              | kill, sigaction, sigprocmask, rt_sigqueue | ~15                                    |
| IPC                  | pipe, shmget, semget, msgget, futex       | ~25                                    |
| Networking           | socket, bind, connect, send, recv, accept | ~25                                    |
| Time                 | clock_gettime, nanosleep, timer_create    | ~15                                    |
| Scheduling           | sched_setscheduler, nice, ioprio_set      | ~10                                    |
| Security             | prctl, seccomp, capget, capset            | ~20                                    |
| Namespace/cgroup     | unshare, setns, clone3                    | ~10                                    |
| Filesystem           | mount, umount, statfs, sync               | ~15                                    |
| Device/ioctl         | ioctl                                     | 1 (but ioctl is infinitely extensible) |
| Misc                 | uname, sysinfo, getrandom                 | ~30                                    |

**Design philosophy:** Everything in the kernel. The kernel provides complete
implementations of filesystems, networking, drivers, and IPC. The syscall
surface reflects every abstraction the kernel manages. The number grows
monotonically because Linux maintains backward compatibility -- syscalls are
essentially never removed.

**Notable features:**

- `ioctl` is a "meta-syscall" that provides access to unlimited driver-specific
  operations through a single syscall number
- `io_uring` (added ~5.1) provides a submission/completion ring interface that
  avoids syscall overhead for high-frequency I/O
- `seccomp` allows userspace to restrict which syscalls a process can make

---

### 3.2 Plan 9

**Source:** Bell Labs (Rob Pike, Ken Thompson, et al.). Deliberately minimal
monolithic kernel.

**Syscall count:** 39 (with some deprecated/renumbered entries up to index 51).

**Complete syscall table:**

| #   | Name       | Purpose                            |
| --- | ---------- | ---------------------------------- |
| 0   | SYSR1      | Reserved                           |
| 1   | \_ERRSTR   | Error string (old)                 |
| 2   | BIND       | Bind name to namespace             |
| 3   | CHDIR      | Change directory                   |
| 4   | CLOSE      | Close file descriptor              |
| 5   | DUP        | Duplicate file descriptor          |
| 6   | ALARM      | Set alarm                          |
| 7   | EXEC       | Execute program                    |
| 8   | EXITS      | Exit with status string            |
| 9   | \_FSESSION | (deprecated)                       |
| 10  | FAUTH      | Authentication on file descriptor  |
| 11  | \_FSTAT    | (old fstat)                        |
| 12  | SEGBRK     | Set segment boundary               |
| 13  | \_MOUNT    | (old mount)                        |
| 14  | OPEN       | Open file                          |
| 15  | \_READ     | (old read)                         |
| 16  | OSEEK      | Old seek                           |
| 17  | SLEEP      | Sleep for duration                 |
| 18  | \_STAT     | (old stat)                         |
| 19  | RFORK      | Fork with resource sharing control |
| 20  | \_WRITE    | (old write)                        |
| 21  | PIPE       | Create pipe                        |
| 22  | CREATE     | Create file                        |
| 23  | FD2PATH    | File descriptor to path            |
| 24  | BRK\_      | Set break                          |
| 25  | REMOVE     | Remove file                        |
| 26  | \_WSTAT    | (old wstat)                        |
| 27  | \_FWSTAT   | (old fwstat)                       |
| 28  | NOTIFY     | Set notification handler           |
| 29  | NOTED      | Notification acknowledged          |
| 30  | SEGATTACH  | Attach memory segment              |
| 31  | SEGDETACH  | Detach memory segment              |
| 32  | SEGFREE    | Free memory segment                |
| 33  | SEGFLUSH   | Flush segment caches               |
| 34  | RENDEZVOUS | Synchronization primitive          |
| 35  | UNMOUNT    | Unmount namespace                  |
| 36  | \_WAIT     | (old wait)                         |
| 37  | SEMACQUIRE | Acquire semaphore                  |
| 38  | SEMRELEASE | Release semaphore                  |
| 39  | SEEK       | Seek in file                       |
| 40  | FVERSION   | File version negotiation           |
| 41  | ERRSTR     | Error string                       |
| 42  | STAT       | File status                        |
| 43  | FSTAT      | File descriptor status             |
| 44  | WSTAT      | Write file status                  |
| 45  | FWSTAT     | Write fd status                    |
| 46  | MOUNT      | Mount server on namespace          |
| 47  | AWAIT      | Wait for child with status         |
| 50  | PREAD      | Positioned read                    |
| 51  | PWRITE     | Positioned write                   |

**Design philosophy:** Everything is a file. Plan 9 pushes the Unix "everything
is a file" metaphor to its logical conclusion. Networks, graphics, processes
(/proc), and even the window system are accessed through the file interface.
Per-process namespaces (BIND, MOUNT, UNMOUNT) allow each process to construct
its own view of the filesystem. The syscall set is minimal because the 9P
protocol (used for all file operations) provides the extensibility -- new
services are just new file servers.

**IPC mechanism:** File operations on mounted file servers. MOUNT attaches a
file server (which speaks the 9P protocol) to a namespace point. All
communication with that service then uses OPEN/READ/WRITE/CLOSE. PIPE creates a
bidirectional byte stream. RENDEZVOUS is the low-level synchronization primitive
(a single address-space meeting point).

**Notable features:**

- RFORK provides fine-grained control over resource sharing when forking
  (share/copy/none for: namespace, env, fd table, memory, notes, rendezvous)
- BIND/MOUNT/UNMOUNT provide per-process namespace manipulation -- the
  equivalent of capabilities via namespace control
- RENDEZVOUS is the only synchronization primitive; semaphores (SEMACQUIRE/
  SEMRELEASE) were added later
- 39 syscalls vs Linux's ~450 while providing comparable functionality through
  the file/9P abstraction

---

### 3.3 OpenBSD

**Source:** Theo de Raadt. Security-focused BSD derivative.

**Syscall count:** ~330 (BSD heritage, but actively managed).

**Security-notable syscalls:**

- `pledge(promises)` -- declare which syscall categories the process will use;
  subsequent violation kills the process with SIGABRT. Categories include:
  stdio, rpath, wpath, cpath, tmppath, inet, unix, dns, tty, proc, exec,
  prot_exec, settime, ps, vminfo, id, pf, route, wroute, audio, video, bpf,
  unveil, error, tape, disklabel, fattr, chown, flock, recvfd, sendfd
- `unveil(path, permissions)` -- restrict filesystem visibility to specified
  paths with specified permissions. Once called, all other paths become
  invisible.

**Design philosophy:** Defense in depth. OpenBSD does not have fewer syscalls
than other BSDs, but it uniquely provides syscalls that _reduce_ the effective
syscall surface at runtime. pledge() is essentially a syscall for restricting
syscalls -- a meta-level security mechanism. The combination of pledge() and
unveil() allows a program to drop privileges granularly after initialization.

---

## 4. Exokernels and Unusual Architectures

### 4.1 Aegis / Xok (MIT Exokernel)

**Source:** MIT (Dawson Engler, M. Frans Kaashoek). The foundational exokernel
research.

**Syscall count:** ~9 (Aegis).

**Aegis primitives:**

| Primitive     | Purpose                                 |
| ------------- | --------------------------------------- |
| `Yield`       | Yield processor to named process        |
| `Scall`       | Synchronous protected control transfer  |
| `Acall`       | Asynchronous protected control transfer |
| `Alloc`       | Allocate resource (e.g., physical page) |
| `Dealloc`     | Deallocate resource                     |
| `TLBwr`       | Insert mapping into TLB                 |
| `FPUmod`      | Enable/disable FPU                      |
| `CIDswitch`   | Install context identifier              |
| `TLBvadelete` | Delete virtual address from TLB         |

**Design philosophy:** Securely multiplex hardware. The exokernel exposes
hardware resources (CPU, memory, TLB, interrupts, network) directly to
applications. A "library OS" (libOS) in each application implements the OS
abstractions it needs. The kernel's role is limited to: (1) tracking resource
ownership, (2) ensuring isolation between applications, and (3) revoking
resources. Application-level TLB management (TLBwr, TLBvadelete) is the key
innovation -- virtual memory policy is fully in userspace.

**IPC mechanism:** Protected control transfer (Scall/Acall). The kernel verifies
the caller's right to transfer control to the target and performs the switch. No
message copying -- the caller and callee share (or exchange) state via registers
and shared memory.

**Notable features:**

- Application-level TLB management
- Packet filter for network demultiplexing (dynamic code generation)
- Secure bindings: application installs a binding (e.g., TLB entry), kernel
  verifies it on use
- "Abort protocol" for visible resource revocation -- applications get a chance
  to save state before a resource is reclaimed

---

### 4.2 Nemesis

**Source:** University of Cambridge, University of Glasgow, SICS, Citrix.
Single-address-space OS designed for multimedia QoS.

**Syscall count:** Very small (the NTSC -- Nemesis Trusted Supervisor Code --
provides ~5-10 primitives).

**Known NTSC primitives:**

- Event send (~50ns -- increment a 64-bit counter)
- Event wait / receive
- Activate domain (resume domain execution after event)
- Context save/restore
- Pseudo-opcode registration (trusted code can register new NTSC operations)

**Design philosophy:** Vertical structure. Most OS code runs in the
application's own address space as shared libraries. The NTSC (kernel) is
responsible only for: CPU scheduling (per-domain guaranteed CPU share), event
delivery, and context switching. Even page fault handling runs in userspace (the
NTSC sends an event to the faulting domain, which handles the fault in its own
activation handler).

**IPC mechanism:** Events. An event is an extremely lightweight notification
(incrementing a 64-bit counter). Combined with shared memory in the single
address space, events provide zero-copy IPC. There are no message-passing
syscalls because data sharing is direct (single address space) and coordination
uses events.

**Notable features:**

- Self-paging: page faults are delivered as events to the faulting domain, which
  handles them in userspace (100-200ns for activation)
- Per-domain CPU guarantees via the NTSC scheduler
- No address-space isolation (single address space) -- isolation via language
  safety or hardware protection keys

---

### 4.3 Singularity

**Source:** Microsoft Research (Galen Hunt, James Larus). Software Isolated
Processes (SIPs) using language safety.

**Syscall count:** 126 ABI entry points (version 1), reportedly grew to ~192.

**Key ABI characteristics:**

- All parameters are values, never pointers (kernel and process GCs are
  independent)
- The ABI maintains system-wide state isolation: a process cannot alter another
  process's state through the ABI
- Communication exclusively through typed, bidirectional channels

**Channel operations:**

- Send (with linear type discipline -- ownership of data transfers)
- Receive
- Channel endpoint creation/destruction

**SIP lifecycle:**

- Process creation, termination
- Contract verification at install time (not runtime)

**Design philosophy:** Language-level isolation instead of hardware-level. SIPs
are "closed object spaces" -- two processes cannot simultaneously access an
object. Channels enforce contract-specified protocols (verified at install
time). The exchange heap allows zero-copy data transfer by transferring
ownership (linear types prevent aliasing). No shared memory, no mutable state
sharing.

**IPC mechanism:** Typed channels with contract-verified protocols. Messages are
sent through channel endpoints. Contracts (defined in Spec#) specify the valid
message sequences. The kernel enforces in-order, lossless delivery. Zero-copy
transfer via the exchange heap with linear type ownership transfer.

**Notable absences:**

- No hardware address space isolation (single address space, software-isolated)
- No signals, interrupts exposed to processes
- No file descriptors (channels replace everything)

---

## 5. Analysis: Universal Syscalls

Every kernel surveyed, regardless of philosophy, provides these operations:

### 5.1 IPC / Communication

Every kernel has a mechanism for inter-component communication. The form varies
enormously (message passing, capability invocation, channel operations, file
operations, events), but the function is universal.

| Kernel       | IPC Primitive                            |
| ------------ | ---------------------------------------- |
| seL4         | Send/Recv/Call/Reply on endpoints        |
| L4 family    | Ipc syscall (send+receive in one trap)   |
| QNX          | MsgSend/MsgReceive/MsgReply              |
| MINIX 3      | Fixed-size message send/receive          |
| Zircon       | channel_read/channel_write/channel_call  |
| Mach         | mach_msg_trap                            |
| EROS/Coyotos | InvokeCap (CALL/FORK/RETURN)             |
| Barrelfish   | SYSCALL_INVOKE + UMP (inter-core)        |
| Composite    | call_cap_op / cos_asnd                   |
| Redox        | read/write on scheme file descriptors    |
| Plan 9       | read/write on mounted file servers       |
| Aegis        | Scall/Acall (protected control transfer) |
| Nemesis      | Event send/receive + shared memory       |
| Singularity  | Channel send/receive                     |

### 5.2 Processor Yield

Every kernel provides a way to voluntarily relinquish the CPU:

| Kernel       | Yield Primitive      |
| ------------ | -------------------- |
| seL4         | Yield                |
| L4           | ThreadSwitch         |
| QNX          | (via SchedSet/yield) |
| Zircon       | thread_legacy_yield  |
| EROS/Coyotos | Yield                |
| Barrelfish   | SYSCALL_YIELD        |
| Composite    | cos_thd_switch       |
| Genode       | yield_thread         |
| Plan 9       | SLEEP(0)             |

### 5.3 Thread/Execution Context Management

Every kernel provides a way to create and manage execution contexts, though the
abstraction varies (threads, domains, dispatchers, processes):

- seL4: TCB capability operations (Configure, Resume, Suspend)
- L4: ThreadControl
- QNX: ThreadCreate, ThreadDestroy
- Zircon: thread_create, thread_start, thread_exit
- Barrelfish: SYSCALL_DISPATCHER_SETUP
- Composite: CAPTBL_OP_THDACTIVATE
- Genode: new_thread, start_thread

---

## 6. Analysis: Common but Not Universal

### 6.1 Time / Clock Access

Present in most but deliberately omitted by some:

- **Present:** QNX (ClockTime, TimerCreate), Zircon (clock_get_monotonic,
  timer_create), Mach (clock_get_time), MINIX 3 (SYS_TIMES), Plan 9 (SLEEP with
  duration), Redox (clock_gettime, nanosleep), L4 (SystemClock)
- **Absent from kernel:** seL4 (time is a scheduling context property, not a
  syscall), EROS/Coyotos (time accessed via Sleep capability invocation),
  Barrelfish (no kernel clock -- user-level), Aegis (no clock primitive)

Why absent: strict microkernels argue that time is a resource managed by
userspace. seL4's approach is that the kernel provides scheduling budgets
(temporal isolation) but does not provide a "what time is it" service.

### 6.2 Interrupt Management

Present in most kernels that target bare-metal deployment:

- **In-kernel:** QNX (InterruptAttach, InterruptWait), Zircon (interrupt_create,
  interrupt_wait), MINIX 3 (SYS_IRQCTL), Mach (via task ports), Genode (new_irq,
  ack_irq)
- **Via IPC:** seL4 (IRQHandler.Ack, IRQControl.Get -- interrupts delivered as
  notifications), L4 (interrupts delivered as IPC messages), EROS
  (IrqCtl/IrqWait capabilities)
- **Not applicable:** Nemesis (kernel handles interrupts internally, delivers
  events), Singularity (kernel handles interrupts)

The L4 family's approach of delivering interrupts as IPC messages to designated
threads is widely adopted. It unifies the event handling model: a server can
wait for both IPC messages and interrupts with the same receive operation.

### 6.3 Memory Management

Present in all but with wildly different scope:

- **Full VM in kernel:** Linux (mmap, mprotect, brk), QNX (via mmap in process
  manager), Zircon (vmar_map, vmo_create)
- **Capability-based:** seL4 (Untyped.Retype, Page.Map), Barrelfish (cap retype,
  vnode_map), L4 (flexpages mapped during IPC), Composite (cos_pgtbl_alloc)
- **User-level paging:** Aegis (TLBwr -- application manages own TLB), Nemesis
  (self-paging via event delivery)

### 6.4 Signals / Asynchronous Notification

Present in POSIX-compatible systems, absent from capability systems:

- **Present:** QNX (SignalKill, SignalAction), MINIX 3 (SYS_KILL), Linux (kill,
  sigaction), Plan 9 (NOTIFY, NOTED)
- **Replaced by:** seL4 (notifications -- word-sized bitmask signals), Zircon
  (object_signal, eventpair_create), Genode (signal system), L4 (no signals --
  use IPC), Barrelfish (no signals -- use events), Composite (cos_asnd)
- **Absent:** EROS/Coyotos (no signals at all), Singularity (no signals), Aegis
  (no signals)

Why absent: POSIX signals have complex semantics (signal masks, handlers,
default actions, restartable syscalls) that capability-system designers consider
both unnecessary and harmful. Simpler notification primitives (seL4
notifications, Genode signals) provide the necessary functionality without the
complexity.

---

## 7. Analysis: Philosophy-Specific Syscalls

### 7.1 Capability Manipulation (capability systems only)

Only present in kernels with explicit capability models:

- seL4: CNode.Copy, CNode.Mint, CNode.Move, CNode.Delete, CNode.Revoke,
  CNode.Rotate, CNode.Mutate
- L4/Fiasco.OC: Task.map (capability transfer during IPC), Factory.create
- Zircon: handle_duplicate, handle_replace, handle_close
- Barrelfish: cap copy, mint, delete, revoke, retype
- EROS/Coyotos: CopyCap, Range (fabrication/revocation)
- Composite: copy, delete (capability table operations)

### 7.2 Userspace Paging

Only present in systems that push page fault handling to userspace:

- Zircon: pager_create, pager_supply_pages, pager_query_dirty_ranges
- Mach: memory*object*\* (external memory management interface)
- Aegis: TLBwr, TLBvadelete (application-level TLB management)
- Nemesis: self-paging via event delivery
- seL4: page faults delivered as IPC to fault handler thread
- L4: page faults delivered as IPC to pager thread

### 7.3 Persistence (checkpoint systems only)

Only EROS/KeyKOS/CapROS: the kernel periodically checkpoints all memory and
capabilities to disk. No other surveyed kernel provides this.

### 7.4 Job/Process Hierarchy (Zircon only in the microkernel world)

Zircon's job_create, job_set_policy, job_set_critical provide hierarchical
process containment. No other microkernel surveyed provides this in-kernel
(others implement it in userspace).

### 7.5 Namespace Manipulation (Plan 9, Redox)

- Plan 9: BIND, MOUNT, UNMOUNT -- per-process namespace construction
- Redox: mkns -- create scheme namespace

These are unique to "everything is a file/URL" systems where namespace
manipulation replaces capability manipulation.

### 7.6 Synchronization Primitives in Kernel

- **In kernel:** QNX (SyncMutexLock, SyncCondvarWait, SyncSemWait), Zircon
  (futex_wait, futex_wake), Redox (futex), Plan 9 (SEMACQUIRE, SEMRELEASE,
  RENDEZVOUS), L4/Fiasco.OC (Semaphore.up, Semaphore.down)
- **Userspace only:** seL4, EROS/Coyotos, Barrelfish, Composite

The divide: kernels targeting POSIX or real-time certification need in-kernel
synchronization for priority inheritance. Pure capability kernels push
synchronization to userspace (built from IPC).

---

## 8. Analysis: The Minimal Kernel

If designing an absolute minimum viable microkernel syscall set, what must be
included?

### The irreducible set: 5-7 operations

| Operation                       | Justification                                                                                                                                                                                    | Who has it                                                   |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------ |
| **IPC send+receive**            | Communication is the raison d'etre of a microkernel. Without IPC, nothing outside the kernel can function. Must support synchronous call/reply for RPC and ideally a non-blocking variant.       | All                                                          |
| **Thread/context create**       | Must be able to create new execution contexts. Could be a capability operation rather than a dedicated syscall.                                                                                  | All                                                          |
| **Thread/context switch/yield** | Must be able to relinquish the CPU. Without this, cooperative multitasking is impossible and the scheduler cannot function.                                                                      | All                                                          |
| **Memory map/grant**            | Must be able to share memory between address spaces. Could be piggybacked on IPC (L4 flexpages) or a dedicated operation. Without this, data transfer requires copying through the kernel.       | All                                                          |
| **Capability/resource create**  | Must be able to create new resources. seL4 does this via Untyped.Retype; L4 via ThreadControl/SpaceControl; Zircon via explicit create syscalls. The mechanism varies but the need is universal. | All                                                          |
| **Capability/resource revoke**  | Must be able to revoke access to resources. Without revocation, there is no way to reclaim resources or enforce security policy changes.                                                         | All (capability systems explicitly; others via kill/destroy) |

### Arguably required (6th-7th)

| Operation              | Justification                                                                                                                                 |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| **Interrupt delivery** | Bare-metal kernels must deliver hardware interrupts to userspace drivers. Can be unified with IPC (L4 model: interrupts are IPC messages).    |
| **Scheduling control** | Setting thread priority/budget. Could be a capability operation. Without it, the kernel's scheduler cannot be configured by userspace policy. |

### The proof: EROS/Coyotos achieves a complete OS with 3 syscalls

Coyotos (InvokeCap, CopyCap, Yield) proves that the theoretical minimum is 3,
but only because InvokeCap is infinitely overloaded -- every kernel object type
responds to different messages. The effective operation count is ~20-30 when
counting distinct capability types and their methods.

### The pragmatic minimum: ~8-12 effective operations

Looking at what seL4 provides:

- 8 syscalls (Send, NBSend, Recv, NBRecv, Call, Reply, ReplyRecv, Yield)
- ~15 kernel object types with ~60 methods

The 8 syscalls are the trap-level interface. The ~60 methods are the effective
API. Both counts matter: the trap count determines hardware/verification
complexity; the method count determines the actual API surface developers use.

A pragmatic minimum for a new microkernel would be:

1. **IPC** (send, receive, call, reply -- 4 variants or 1 with flags)
2. **Yield**
3. **Capability operations** (create, copy, delete, revoke -- 4 operations or 1
   generic invoke)
4. **Thread control** (create, start, stop -- 3 operations or 1 with
   subcommands)
5. **Memory mapping** (map, unmap -- 2 operations, could be via IPC flexpages)
6. **Interrupt acknowledgment** (1 operation, could be unified with IPC)

This gives 6-15 operations depending on whether they're separate syscalls or
subcommands of a generic invoke.

---

## 9. Analysis: Lessons from Removals

### 9.1 IPC Timeouts (L4 -> seL4/OKL4)

All modern L4 derivatives abandoned IPC timeouts. Original L4 allowed specifying
timeout values for IPC operations. In practice, only two values were ever used:
zero (non-blocking) and infinity (blocking). No good theory or heuristics exist
for choosing intermediate timeout values in non-trivial systems. The
simplification: provide blocking and non-blocking variants, nothing in between.

### 9.2 Clans and Chiefs (L4)

Original L4 restricted IPC routing through a hierarchical model where messages
could only go to siblings or parents ("clans and chiefs"). Abandoned in all
modern L4 variants -- too restrictive, too complex, and unnecessary when
capabilities control communication rights.

### 9.3 Long IPC (L4 -> seL4)

Original L4 supported transferring large messages (beyond register size) through
kernel-mediated copy. seL4 abandoned this in favor of shared memory. Long IPC
complicated the kernel (nested page faults during copy), and shared memory
performs better for large transfers anyway.

### 9.4 Lazy Scheduling (L4)

Original L4 deferred updating scheduler run queues during IPC (since the sending
thread would block and the receiving thread would run). This optimized the IPC
fast path at the cost of worst-case latency. Abandoned in favor of "Benno
scheduling" -- always keep run queues consistent, accept the small fast-path
cost for predictable behavior.

### 9.5 Mach's External Memory Management

Mach's external memory manager interface (memory*object*\*) was a powerful
abstraction that allowed userspace to implement custom paging policies. In
practice, it was too complex, too slow, and rarely used for anything beyond the
default pager. Apple's XNU retains a simplified version but the full generality
was abandoned.

### 9.6 Mach -> L4 (the entire first generation)

The move from Mach to L4 was itself a massive "removal." Mach had ~130 kernel
API functions. L4 reduced this to 7 syscalls. The lesson: a microkernel should
provide mechanisms, not policies. Mach's mistake was implementing too many
abstractions (tasks, threads, ports, port sets, port rights, port notifications,
memory objects, lock sets, semaphores, ledgers, processor sets, ...) inside the
kernel.

### 9.7 Zircon's PCI Syscalls

Zircon accumulated ~14 PCI-related syscalls (pci_config_read, pci_init, etc.)
that are now marked obsolete. They were added for early bringup and replaced by
userspace driver infrastructure. This illustrates the risk of "temporary" kernel
interfaces becoming permanent.

### 9.8 C++ in Kernels (L4Ka -> seL4, OKL4)

Not a syscall removal but relevant: L4Ka::Pistachio was written in C++. Both
seL4 and OKL4 abandoned C++ for C (and assembly). C++ features (exceptions,
RTTI, virtual dispatch) created hidden control flow and memory allocation that
complicated both verification and reasoning about kernel behavior.

---

## 10. Analysis: IPC as the Pivot Point

IPC design is the single decision that most shapes a microkernel's syscall
surface. Every other design decision follows from how IPC works.

### 10.1 Synchronous vs. Asynchronous

**Synchronous** (seL4, L4, QNX, EROS): The sender blocks until the receiver
processes the message. Advantages: simple semantics, no kernel buffering,
natural flow control. Disadvantages: requires careful design to avoid deadlock,
needs an asynchronous notification mechanism for events that cannot block
(interrupts, timeouts).

**Asynchronous** (Mach, Zircon): Messages are queued in kernel buffers. The
sender continues immediately. Advantages: no deadlock risk, natural for
event-driven systems. Disadvantages: kernel must manage message buffers (memory
allocation in the kernel), harder to reason about ordering, potential for
unbounded queue growth.

**Hybrid** (QNX: synchronous MsgSend + asynchronous pulses; seL4: synchronous
endpoints + asynchronous notifications; Zircon: asynchronous channels +
synchronous channel_call): Most practical systems combine both.

### 10.2 How IPC Shapes the Syscall Surface

When IPC is synchronous and register-based (L4, seL4):

- Memory transfer must be explicit (shared memory, flexpages)
- Interrupt delivery is naturally unified with IPC (interrupts = messages)
- Thread switching during IPC enables "direct process switch" optimization
- The kernel needs no memory allocator for message buffers
- The syscall set stays small (IPC + capability management)

When IPC is asynchronous and buffered (Mach, Zircon):

- The kernel needs memory management for message queues
- Port/channel lifecycle management becomes necessary (create, destroy)
- Handle/right management becomes complex (send rights, receive rights)
- The kernel accumulates more syscalls for buffer management, handle operations,
  and waiting mechanisms (ports, wait sets)
- Memory transfer can piggyback on messages (Mach out-of-line, Zircon handle
  transfer) but adds complexity

### 10.3 IPC and the Capability Model

The relationship between IPC and capabilities determines whether the kernel has
a unified or fragmented syscall interface:

- **Unified** (seL4, EROS, Composite): capability invocation IS IPC. There is
  one mechanism (invoke a capability) that does everything. The syscall count is
  tiny because the capability type determines the operation.
- **Fragmented** (Zircon, QNX): IPC and resource management are separate
  mechanisms. Channels for IPC, separate syscalls for process/thread/memory
  management. The syscall count is larger but each syscall has a clear purpose.

The unified model produces a smaller kernel and simpler formal properties. The
fragmented model produces a more familiar API and easier learning curve.

### 10.4 IPC Performance as Forcing Function

L4's contribution was proving that IPC performance determines microkernel
viability. The performance evolution:

| System       | IPC Round-trip | Year  |
| ------------ | -------------- | ----- |
| Mach         | ~100 us        | 1990  |
| L4/x86       | ~5 us          | 1993  |
| L4/Pistachio | ~0.5 us        | 2003  |
| seL4/ARM64   | ~0.2 us        | 2013+ |

This 500x improvement came from: register-based message passing (no copying),
direct process switch (no scheduler invocation), minimal kernel path (no
unnecessary work), and hardware-specific optimization (ARM fast-path).

The implication for syscall design: the IPC syscall will be the most-executed
kernel entry point by orders of magnitude. Every cycle on that path matters.
Design the IPC syscall first, optimize it ruthlessly, and build everything else
around it.

---

## Syscall Count Summary

| Kernel         | Raw Syscalls | Effective Operations      | Philosophy             |
| -------------- | ------------ | ------------------------- | ---------------------- |
| Coyotos        | 3            | ~25 (cap types x methods) | Pure capability        |
| seL4           | 8-11         | ~60 (object methods)      | Verified capability    |
| L4 X.2         | 7-12         | ~12                       | Minimal IPC            |
| Aegis          | 9            | 9                         | Exokernel              |
| Composite      | ~6           | ~15                       | Component capability   |
| CertiKOS       | ~8-10        | ~10                       | Verified minimal       |
| Barrelfish     | ~8 + cap ops | ~25                       | Multikernel            |
| Genode/base-hw | ~25          | ~25                       | Tiered microkernel     |
| Redox          | ~35          | ~35                       | Unix-compatible micro  |
| MINIX 3        | ~50          | ~50                       | Reliable microkernel   |
| Plan 9         | 39           | 39                        | Minimal monolithic     |
| QNX            | ~90+         | ~90+                      | POSIX microkernel      |
| Mach/XNU       | ~40 traps    | ~190 (API functions)      | First-gen micro/hybrid |
| Singularity    | 126          | 126-192                   | Language-isolated      |
| Zircon         | ~170+        | ~170+                     | Object-capability      |
| OpenBSD        | ~330         | ~330                      | Security monolithic    |
| Linux          | ~450+        | ~450+ (+ ioctl)           | Monolithic             |

---

## References

### Primary Sources

- seL4 API Reference: https://docs.sel4.systems/projects/sel4/api-doc.html
- seL4 syscall.xml:
  https://github.com/seL4/seL4/blob/master/libsel4/include/api/syscall.xml
- L4 X.2 Reference Manual: https://www.l4ka.org/l4ka/l4-x2-r7.pdf
- L4Re/Fiasco.OC Kernel Objects:
  https://l4re.org/doc/group__l4__kernel__object__api.html
- QNX Neutrino Microkernel Architecture:
  https://www.qnx.com/developers/docs/6.4.1/neutrino/sys_arch/kernel.html
- MINIX 3 Kernel API:
  https://wiki.minix3.org/doku.php?id=developersguide:kernelapi
- Zircon System Calls: https://fuchsia.dev/reference/syscalls
- Zircon Kernel Concepts: https://fuchsia.dev/fuchsia-src/concepts/kernel
- XNU mach_traps.h:
  https://github.com/apple/darwin-xnu/blob/main/osfmk/mach/mach_traps.h
- Mach Kernel Interface Reference:
  https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/
- Redox syscall numbers:
  https://github.com/redox-os/syscall/blob/master/src/number.rs
- Redox syscall wrappers:
  https://github.com/redox-os/syscall/blob/master/src/call.rs
- Plan 9 syscall source: https://plan9.io/sources/plan9/sys/src/cmd/ki/syscall.c
- Barrelfish syscall.c:
  https://github.com/BarrelfishOS/barrelfish/blob/master/kernel/arch/x86_64/syscall.c
- Genode base-hw:
  https://genode.org/documentation/genode-foundations/19.05/under_the_hood/Execution_on_bare_hardware_(base-hw).html
- Composite capability design:
  https://www2.seas.gwu.edu/~parmer/posts/2016-04-06-capability-based-design.html

### Papers

- Elphinstone, K. and Heiser, G. "From L3 to seL4: What Have We Learnt in 20
  Years of L4 Microkernels?" SOSP 2013.
  https://sigops.org/s/conferences/sosp/2013/papers/p133-elphinstone.pdf
- Engler, D., Kaashoek, M.F., and O'Toole, J. "Exokernel: An Operating System
  Architecture for Application-Level Resource Management." SOSP 1995.
  https://pdos.csail.mit.edu/6.828/2008/readings/engler95exokernel.pdf
- Shapiro, J. "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- Baumann, A. et al. "The Multikernel: A New OS Architecture for Scalable
  Multicore Systems." SOSP 2009.
  https://people.inf.ethz.ch/troscoe/pubs/sosp09-barrelfish.pdf
- Gu, R. et al. "CertiKOS: An Extensible Architecture for Building Certified
  Concurrent OS Kernels." OSDI 2016.
  https://www.usenix.org/system/files/conference/osdi16/osdi16-gu.pdf
- Hunt, G. and Larus, J. "Singularity: Rethinking the Software Stack."
  https://www.microsoft.com/en-us/research/wp-content/uploads/2005/10/tr-2005-135.pdf
- Hardy, N. "The KeyKOS Architecture."
  https://dl.acm.org/doi/pdf/10.1145/858336.858337
- Hand, S. "Self-Paging in the Nemesis Operating System." OSDI 1999.
  https://www.usenix.org/legacy/events/osdi99/full_papers/hand/hand.pdf
- MIT Microkernel Lecture: https://pdos.csail.mit.edu/archive/6.097/lec/l14.html

### Limitations

- OKL4: Proprietary; detailed syscall list not publicly available. Known to
  follow L4 X.2 API with extensions.
- Nemesis: NTSC interface reconstructed from papers and source code fragments;
  no single authoritative reference found.
- CertiKOS: Syscall list reconstructed from papers and Coq artifacts; the
  verified kernel is primarily an academic artifact.
- Singularity: The full 126/192 ABI entry point list is in the source code
  distribution; not available online in a single reference.
- Some kernels (particularly Barrelfish and Composite) have evolved since the
  referenced sources; the interfaces described reflect the versions documented
  in the cited sources.
