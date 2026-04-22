# Userspace Timer Syscall Interface

## The Question

When a userspace thread wants to be woken at a specific future time, what does
it ask the kernel for, how does it ask, and what does the kernel deliver when
the time arrives?

Three sub-questions drive the survey:

1. **What does the Observer provide?** — absolute deadline vs. relative
   duration; slack/tolerance; one-shot vs. periodic; which timer object or
   scheduling context is invoked.
2. **How does the Observer express the request?** — standalone timer syscall;
   timeout parameter inline in a blocking call; budget exhaustion on a
   scheduling context.
3. **What does the kernel deliver?** — pure wakeup (thread resumes with error
   code), signal, message/pulse injected into a queue, or a fault delivered to a
   fault handler endpoint.

Parent design context (from question): D2 (per-core schedulers), D13 (queued
fields, delivery mechanism), D22 (interrupt model), D28 (fixed-size message
format), D42 (scheduling profile: responsiveness, throughput, precision), A4
(purely reactive kernel).

---

## Survey of Existing Systems

### Zircon / Fuchsia: Timer Kernel Object

Zircon exposes a `Timer` as a first-class kernel object type. The lifecycle
involves three distinct syscalls.

**Creation:**

```c
zx_status_t zx_timer_create(uint32_t options,
                             zx_clock_t clock_id,
                             zx_handle_t* out);
```

- `clock_id`: `ZX_CLOCK_MONOTONIC` or `ZX_CLOCK_BOOT`. Determines which clock
  the deadline is measured against. Specified at creation; immutable.
- `options`: one of `ZX_TIMER_SLACK_CENTER` (default), `ZX_TIMER_SLACK_EARLY`,
  `ZX_TIMER_SLACK_LATE` — controls whether the coalescing window allows the
  timer to fire before, after, or symmetrically around the deadline.

**Arming:**

```c
zx_status_t zx_timer_set(zx_handle_t handle,
                          zx_time_t  deadline,
                          zx_duration_t slack);
```

- `deadline`: absolute time value on the clock the timer was created with. To
  compute an absolute deadline from a relative duration, callers use
  `zx_deadline_after(duration)` which returns
  `zx_clock_get_monotonic() + duration`.
- `slack`: size of the coalescing window in nanoseconds (non-negative). The
  kernel may fire anywhere in `[deadline - slack, deadline + slack]` (or half
  the window depending on creation options). Zero = exact.
- If a previous `zx_timer_set` was pending, it is atomically replaced. There is
  no "already armed" error; replace is always valid.
- No periodic mode. Repeating timers must call `zx_timer_set` again after each
  firing.

**Cancellation:**

```c
zx_status_t zx_timer_cancel(zx_handle_t handle);
```

De-asserts `ZX_TIMER_SIGNALED`. Safe to call even if the timer has not fired.

**Delivery:** When the timer fires, the kernel asserts the `ZX_TIMER_SIGNALED`
signal bit on the timer object. It does not inject a message anywhere. The
caller must wait for this signal explicitly:

```c
zx_object_wait_one(handle, ZX_TIMER_SIGNALED, ZX_TIME_INFINITE, &observed);
```

or `zx_object_wait_many` to multiplex multiple waitables (channels, timers,
events) on a single thread. The signal remains asserted until the next
`zx_timer_set` or `zx_timer_cancel` clears it.

**What the caller receives:** The wait syscall returns with `observed`
containing the asserted signal bits. No time-of-firing data is delivered; the
caller re-reads the clock if it needs elapsed time. No overrun count is exposed.

**Required right:** The handle must carry `ZX_RIGHT_WRITE` for `zx_timer_set`.

**Reference:**
[Zircon zx_timer_set](https://fuchsia.dev/fuchsia-src/reference/syscalls/timer_set);
[Zircon System Calls](https://fuchsia.dev/reference/syscalls).

---

### QNX Neutrino: POSIX Timers with Pulse Delivery

QNX implements POSIX `timer_create` / `timer_settime` but its preferred delivery
mechanism is `SIGEV_PULSE` — delivering a lightweight pulse message to a channel
connection rather than a UNIX signal.

**Creation:**

```c
timer_t timerid;
struct sigevent event;
SIGEV_PULSE_INIT(&event, connection_id, priority, code, value);
timer_create(CLOCK_MONOTONIC, &event, &timerid);
```

- The `sigevent` specifies _how_ the timer fires, not when. It is set once at
  creation.
- `connection_id`: a QNX connection (result of `ConnectAttach()`), typically
  back to a channel the same thread is about to `MsgReceive()` on.
- `code`: a 1-byte discriminator (`_PULSE_CODE_MINAVAIL..MAXAVAIL`) that
  identifies the timer among other pulse sources.
- `value`: a 4-byte integer payload delivered with the pulse.
- `priority`: the scheduling priority at which the pulse is delivered
  (`SIGEV_PULSE_PRIO_INHERIT` = inherit the thread's current priority).

**Arming:**

```c
struct itimerspec spec = {
    .it_value.tv_sec = ..., .it_value.tv_nsec = ...,  /* first expiry */
    .it_interval.tv_sec = 0, .it_interval.tv_nsec = 0 /* one-shot: 0 */
};
timer_settime(timerid, flags, &spec, NULL);
```

- `flags`: 0 = relative (from now), `TIMER_ABSTIME` = absolute.
- `it_interval` non-zero = periodic: auto-rearms after each expiry.
- `it_value` = zero = disarm (cancel).

**Delivery:** When the timer fires, the kernel delivers a `struct _pulse` to the
channel:

```c
struct _pulse { int16_t type; int16_t subtype; int8_t code;
                uint8_t zero[3]; union sigval value; int32_t scoid; };
```

The pulse has a 5-byte header and a 4-byte value payload. Total fixed size. The
receiving thread calls `MsgReceive()` on its channel, which unblocks and returns
the pulse. The pulse is distinguishable from regular IPC messages by
`type == _PULSE_TYPE` (−1). The code identifies which timer fired.

**Overrun:** If the timer fires multiple times before the pulse can be delivered
(e.g., the thread was blocked), the kernel sets an overrun counter. The receiver
calls `timer_getoverrun(timerid)` after receiving the pulse to learn how many
additional expirations were missed. At most one pulse is queued per timer at any
time (additional firings are counted as overruns, not queued).

**What the caller receives:** A fixed-size pulse message with the code and
4-byte value. No timestamp of firing. Overrun count available via separate call.

**Reference:**
[QNX timer_create](https://www.qnx.com/developers/docs/7.0.0/com.qnx.doc.neutrino.lib_ref/topic/t/timer_create.html);
[QNX Timing](https://www.qnx.com/developers/docs/6.5.0SP1/neutrino/prog/timing.html);
[QNX Pulse notification](http://www.qnx.com/developers/docs/qnxcar2/topic/com.qnx.doc.neutrino.getting_started/topic/s1_timer_sigevent_pulse_notification.html).

---

### POSIX / Linux: Per-Process Timers

POSIX specifies a general-purpose timer API with multiple delivery mechanisms.

**Creation:**

```c
timer_t timerid;
struct sigevent sev = { .sigev_notify = SIGEV_SIGNAL,
                        .sigev_signo = SIGRTMIN };
timer_create(CLOCK_MONOTONIC, &sev, &timerid);
```

Delivery mechanisms:

- `SIGEV_SIGNAL`: deliver a real-time signal to the process.
- `SIGEV_THREAD`: invoke a thread function (kernel spawns or unblocks a thread).
- `SIGEV_THREAD_ID` (Linux): deliver a signal to a specific thread (not POSIX
  standard; Linux extension). Used internally by NPTL.
- `SIGEV_NONE`: no notification; caller polls with `timer_gettime()`.

**Arming:**

```c
struct itimerspec spec = { .it_value = {secs, nsecs}, .it_interval = {0, 0} };
timer_settime(timerid, 0, &spec, &old);
```

- Flags: `TIMER_ABSTIME` for absolute time.
- One-shot if `it_interval == 0`.
- Returns previous arm state in `old`.

**Delivery:** Signal arrives at the thread. The `siginfo_t` passed to the signal
handler contains `si_value` (the `sigev_value` set at creation) and the timer ID
(`si_timerid`). The signal number carries no timing information directly.

**Overrun:** `timer_getoverrun(timerid)` returns the count of expirations that
occurred after the last signal delivery but before that signal was handled. At
most one signal is queued per timer. Linux limits the accumulated overrun count
to `DELAYTIMER_MAX` (typically INT_MAX).

**What the caller receives:** A signal (not a message). With `SIGEV_THREAD`, a
userspace function is invoked. With `SIGEV_THREAD_ID`, the signal targets a
specific thread's signal mask.

**Reference:**
[timer_create(2) man page](https://man7.org/linux/man-pages/man2/timer_create.2.html).

---

### seL4 MCS: Budget Exhaustion as Timer Delivery

seL4 MCS does not provide a standalone "set a timer" syscall. Instead, a
thread's SchedContext is the timer: the thread runs until its budget is
exhausted, at which point the kernel raises a **timeout fault**.

**Configuration (timer setup):**

```c
seL4_SchedControl_Configure(schedcontrol_cap, sc_cap,
                             budget_us, period_us,
                             extra_refills, badge);
```

The SchedContext acts as a periodic timer when `budget < period`. The thread
executes for at most `budget` microseconds per period, and the period is the
timer interval. For a one-shot delay, `budget` = desired delay and `period` =
desired delay (or a large value if the thread should not auto-repeat).

**Registering the timeout handler:**

```c
seL4_TCB_SetTimeoutEndpoint(tcb_cap, endpoint_cap);
```

When a timeout fault fires, the kernel delivers a message to `endpoint_cap`.
This is optional: if no endpoint is set, the thread simply blocks at budget
exhaustion and resumes at the next replenishment.

**Timeout fault message:**

```text
word 0: fault_type (seL4_Fault_Timeout)
word 1: badge (from SchedControl_Configure, identifies which SC fired)
word 2: consumed (time consumed since last fault or Consumed call, in ticks)
```

The message is fixed-size (3 words). The handler thread receives it via a
standard `seL4_Recv` on the endpoint. To resume the faulted thread:

```c
seL4_TimeoutReply_new(reply_cap);
```

**Querying elapsed time:**

```c
uint64_t consumed;
seL4_SchedContext_Consumed(sc_cap, &consumed);
```

Resets the internal consumed counter and returns the elapsed ticks since the
last reset or fault.

**What the caller receives:** A fault message with a badge (identifying the SC)
and a consumed-time field. No wall-clock timestamp of firing. Overrun is
implicit: if the fault handler is slow and the thread has accumulated additional
budget, the consumed counter reflects total elapsed time including any
undelivered expirations.

**Key distinction from other models:** The "timer" is the scheduling context
itself. Setting a timer = configuring a SchedContext. There is no separate timer
kernel object. The delivery is a fault delivered to a pre-registered endpoint,
not a notification signaled on a new object.

**Reference:**
[seL4 MCS Tutorial](https://docs.sel4.systems/Tutorials/mcs.html);
[seL4 MCS 10.1.1-mcs release notes](https://docs.sel4.systems/releases/sel4/10.1.1-mcs.html);
Lyons et al., EuroSys 2018.

---

### L4 (Liedtke / Pistachio): Inline IPC Timeout

Classical L4 does not have a timer object. Timeouts are expressed as parameters
to the IPC syscall itself. There is no separate "set a timer" step — the timeout
is part of the blocking operation.

**Encoding:** L4 specifies a 32-bit timeout field containing:

- `send_timeout` (16 bits): how long to wait for a sender to accept the message
- `recv_timeout` (16 bits): how long to wait for a message to arrive

Each 16-bit field uses a 10-bit mantissa and 6-bit exponent encoding, giving a
range from 0 (poll: return immediately) to ∞ (block forever), with microsecond
granularity at short intervals and coarser at longer ones.

**Delivery:** If the timeout expires before the IPC completes, the syscall
returns with an error code in the message tag (`IPC_ERROR_Timeout`). No separate
delivery occurs; the thread simply wakes up with the error condition.

**What the caller receives:** The IPC call return value indicates timeout vs.
success. No payload. No fired-at timestamp. No overrun.

**Key distinction:** The timeout is fused with the blocking IPC call. There is
no standalone timer; every blocking receive implicitly carries a deadline. This
is minimal but forces every wait site to encode its own timeout, and there is no
"arm now, wait later" pattern.

**Reference:** L4 Specification Version X.2; Liedtke, "On µ-Kernel
Construction," SOSP 1995.

---

### Plan 9: Alarm and Rendezvous

Plan 9 provides two mechanisms: `sleep` (relative delay) and `alarm` (interrupt
after a duration).

**`sleep(ms)` — blocking relative wait:**

```c
int sleep(long milliseconds);
```

Blocks the calling process for the specified number of milliseconds. Analogous
to POSIX `nanosleep`. No object created, no message delivered. The thread
resumes from sleep with no data.

**`alarm(ms)` — schedule a note:**

```c
ulong alarm(ulong milliseconds);
```

Schedules delivery of a `"alarm"` note (like a UNIX signal) after `ms`
milliseconds. Returns the time remaining on any previously set alarm. When it
fires, the process receives the note and can handle it via `atnotify()`. If not
handled, the note terminates the process. Zero cancels a pending alarm.

**What the caller receives:** A note (string: `"alarm"`) delivered via the note
mechanism. Not a message. No timing data. One alarm per process at a time.

**Reference:** Plan 9 manual page `alarm(2)`, `sleep(2)`.

---

### MINIX 3: Alarm-to-IPC Conversion

MINIX 3 has no timer kernel object. The system clock driver manages timers on
behalf of other servers. User processes interact with timing via:

1. `alarm()` (POSIX) — converted to a message to the clock server
2. `SIGALRM` delivered back when the clock server fires the timer
3. Server components use asynchronous message passing to the clock driver
   (`CLOCK_setitimer`) to register timers

**Delivery:** The clock driver sends an IPC notification to the registered
process/server. For user processes, it raises `SIGALRM`. For system servers, it
delivers a `NOTIFY` IPC message.

**What the caller receives:** Either a signal (user) or a fixed-format
notification IPC message (server). No timestamp. No overrun count in the
message; the process can query with `getitimer`.

**Reference:** MINIX 3 design documentation; Herder et al., "MINIX 3: A Highly
Reliable, Self-Repairing Operating System," ACM Operating Systems Review, 2006.

---

## Design Dimensions and Observed Tradeoffs

### 1. How the timer is expressed

| Approach                              | Systems                 | Properties                                                                            |
| ------------------------------------- | ----------------------- | ------------------------------------------------------------------------------------- |
| Standalone timer object + arm syscall | Zircon                  | Timer is persistent; can be rearmed many times; object lifespan independent of thread |
| POSIX timer (two-step: create/arm)    | QNX, Linux              | Timer is persistent, referenced by ID; delivery mechanism chosen at creation          |
| Budget exhaustion on SchedContext     | seL4 MCS                | No separate object; the scheduling context IS the timer                               |
| Inline IPC timeout parameter          | L4                      | No persistent timer; each blocking call carries its own deadline                      |
| Blocking sleep syscall                | Plan 9, POSIX nanosleep | No object; no asynchronous delivery; thread is suspended                              |

**Object-based vs. inline:** Object-based timers (Zircon, QNX) allow a thread to
arm a timer, then do other work, and later wait for or receive the firing.
Inline timeout (L4) couples the timer duration to a single blocking operation;
there is no "arm now, wait later" pattern.

**Budget-as-timer (seL4 MCS):** The SC budget is simultaneously a scheduling
guarantee and a timer. Setting `budget=5ms, period=5ms` creates both a 5ms
execution quantum and a 5ms timer. This unifies two concepts (how long the
thread may run, when the thread is woken) that other systems separate. The
consequence is that the "timer" always carries scheduling semantics even if the
intent is purely temporal notification.

---

### 2. What the Observer specifies

**Absolute deadline vs. relative duration:**

| System       | Deadline type                         | Helper for relative           |
| ------------ | ------------------------------------- | ----------------------------- |
| Zircon       | Absolute (`ZX_CLOCK_MONOTONIC`)       | `zx_deadline_after(relative)` |
| QNX/POSIX    | Either (flag `TIMER_ABSTIME`)         | Default is relative           |
| seL4 MCS     | Implicit (budget = duration from now) | None; period is the interval  |
| L4           | Relative (mantissa/exponent)          | Native                        |
| Plan 9 alarm | Relative (milliseconds)               | Native                        |

**Tolerance / slack:**

| System   | Slack exposed? | Semantics                                                  |
| -------- | -------------- | ---------------------------------------------------------- |
| Zircon   | Yes (`slack`)  | Firing window `[deadline-slack, deadline+slack]` (or half) |
| QNX      | No             | Timer fires at specified resolution (clock granularity)    |
| POSIX    | No             | Fires at or after specified time                           |
| seL4 MCS | No             | Fires at budget boundary (scheduler tick granularity)      |
| L4       | No             | Fires at or after timeout (scheduler tick granularity)     |

Zircon is the only surveyed system that exposes timer slack as a first-class API
parameter. The slack is used by the kernel for coalescing: multiple timers with
overlapping windows may fire together, reducing timer interrupt frequency and
improving power efficiency and cache behavior. Zircon's creation-time `options`
(`ZX_TIMER_SLACK_EARLY/LATE/CENTER`) control whether the coalescing window is
asymmetric.

---

### 3. Delivery mechanism

Three distinct delivery models appear:

**Model A — Signal / flag on kernel object (Zircon):** The kernel asserts a
signal bit on the timer object. The thread waits for this signal with a separate
wait syscall. Decoupled: arming and waiting are two separate operations.
Multiple threads could wait on the same timer object. No message data delivered
— the signal is a pure wakeup. The fired-at time is not delivered; the caller
re-reads the clock if needed.

**Model B — Message / pulse injected into queue (QNX SIGEV_PULSE, MINIX 3):**
The kernel delivers a fixed-size message into the thread's receive queue when
the timer fires. The thread learns of the firing when it calls `MsgReceive()`
(or equivalent). The message contains a small discriminator (code + value in
QNX) identifying which timer fired. Overrun count is available via a separate
call after receiving the pulse. The receiving thread does not need to be
explicitly waiting on the timer object at the moment of firing — the pulse
accumulates in the queue.

**Model C — Fault message to pre-registered endpoint (seL4 MCS):** The kernel
delivers a fault message to an endpoint registered ahead of time with
`seL4_TCB_SetTimeoutEndpoint`. The fault message carries a badge (SC identity)
and consumed time. The handler thread — which may be a _different thread_ from
the one whose budget expired — receives the fault. This is the only model where
the recipient of the timer event can be a separate thread from the timer owner.

**Model D — IPC call return with error (L4):** The IPC call itself returns with
a timeout error code. No separate delivery event. The thread learns the timeout
expired because the operation it was waiting on returned with
`IPC_ERROR_Timeout`. No queue, no persistent object.

**Model E — Thread resume from sleep (Plan 9 sleep, POSIX nanosleep):** The
blocking call returns. The thread resumes execution normally. No signal, no
message, no fault. The return value of `sleep()` indicates any remaining time
(on Linux `nanosleep` with a signal interrupt) but Plan 9 `sleep` has no return
value conveying timing.

---

### 4. What the kernel delivers

| System             | Delivery payload                                           | Overrun info         |
| ------------------ | ---------------------------------------------------------- | -------------------- |
| Zircon             | `ZX_TIMER_SIGNALED` bit; no data                           | No                   |
| QNX SIGEV_PULSE    | 1-byte code + 4-byte value from `sigevent`                 | `timer_getoverrun()` |
| POSIX SIGEV_SIGNAL | Signal + `siginfo_t` with `si_value`                       | `timer_getoverrun()` |
| seL4 MCS fault     | badge word + consumed-time word (2 data words + fault tag) | Implicit in consumed |
| L4 timeout         | IPC return: error tag only                                 | No                   |
| Plan 9 alarm       | Note: string `"alarm"`                                     | No                   |
| Plan 9 sleep       | Nothing (pure wakeup)                                      | No                   |

**Overrun:** QNX and POSIX both provide `timer_getoverrun()`. The pattern is: at
most one notification is in-flight at a time; additional expirations while the
first is pending are counted, not queued. The receiver can detect how far behind
the timer is but cannot reconstruct individual missed firings.

seL4 MCS does not have an explicit overrun count; the `consumed` field in the
fault message accumulates execution time across all missed periods, from which
the receiver can compute how many periods were missed as `consumed / period`.

---

### 5. Cancellation

| System    | Cancellation mechanism                           |
| --------- | ------------------------------------------------ |
| Zircon    | `zx_timer_cancel(handle)` — de-asserts signal    |
| QNX/POSIX | `timer_settime()` with `it_value = 0`            |
| seL4 MCS  | `seL4_SchedContext_Bind` / `Unbind` (SC-level)   |
| L4        | No cancellation; timeout is part of the IPC call |
| Plan 9    | `alarm(0)` cancels pending alarm                 |

---

### 6. One-shot vs. periodic

| System    | Periodic support                                      |
| --------- | ----------------------------------------------------- |
| Zircon    | No native periodic. Caller re-arms in signal handler. |
| QNX/POSIX | Yes: `it_interval != 0` auto-rearms                   |
| seL4 MCS  | Yes: `period` field in SchedContext; budget refills   |
| L4        | No: timeout is per-IPC-call                           |
| Plan 9    | No native periodic. Caller re-issues alarm.           |

---

### 7. Timer object lifecycle

**Persistent, reusable (Zircon, QNX/POSIX):** A timer object is created once,
armed and disarmed multiple times, and destroyed independently of the thread.
Multiple timers can be created and managed simultaneously by one thread. A
thread can hold ten timer handles and arm them independently.

**SC-coupled (seL4 MCS):** The timer's lifetime is tied to the SchedContext
object, which is tied to the thread. Creating a new "timer duration" means
creating or reconfiguring a SchedContext. The number of simultaneous independent
timers is limited by the number of SchedContexts a thread holds.

**Ephemeral (L4):** No timer object at all. The timeout lives only for the
duration of the IPC call.

---

### 8. Measured data

**Zircon timer overhead:** No published per-timer-set latency.
`zx_object_wait_one` round-trip on Zircon x86 is approximately 9× seL4 endpoint
cost (SJTU XPC, TOCS 2022), but this is the wait path, not the timer-set path
specifically.

**QNX pulse delivery:** QNX documentation caps timer event generation at 50
events per clock tick to prevent overload. No published latency for pulse
delivery from timer firing to `MsgReceive` return.

**seL4 MCS timeout fault:** Not separately benchmarked from the scheduling
context overhead. Replenishment operation (period boundary): ~50 cycles on ARM
Cortex-A9 (Lyons et al., EuroSys 2018).

**POSIX `clock_nanosleep` on Linux (x86):** Typical resolution is clock tick
granularity (1 ms by default; 250 µs with `CONFIG_HZ=4000`). With `hrtimer`
(high-resolution timers enabled): resolution reduces to tens of microseconds or
below on modern hardware. `nanosleep` measured overhead: ~1–2 µs for the syscall
round-trip itself (kernel entry + timer arm + reschedule path).

---

## What Is Not Settled in the Literature

1. **Slack in non-Zircon systems:** Only Zircon exposes a programmable slack
   parameter. Whether per-timer slack improves power efficiency enough to
   justify API complexity is not studied in microkernel literature specifically.
   The Linux `timerfd_settime` with `TFD_TIMER_ABSTIME` and `clock_nanosleep`
   with `CLOCK_REALTIME_ALARM` expose no slack parameter.

2. **Delivery of fired-at timestamp:** No surveyed system delivers the actual
   clock value at which the timer fired in the notification itself. Callers must
   re-read the clock after waking. This means the caller cannot know exactly how
   late the delivery was without a second syscall.

3. **Multiple simultaneous timers per thread:** All surveyed systems allow a
   thread to hold multiple timer objects (Zircon handles, POSIX timer IDs) and
   arm them independently. No system imposes a hard per-thread timer count limit
   in the API (resource limits aside). Whether a capability-based system should
   bound timer count via the authority system is not analyzed in published
   literature.

4. **Timer as field message:** No surveyed system integrates timer firing
   directly with a structured message queue in the way D13 (queued fields)
   implies. QNX `SIGEV_PULSE` comes closest: a pulse is delivered to a channel,
   and `MsgReceive` returns it alongside regular IPC messages. The timer firing
   is indistinguishable at the receive point from any other pulse source — the
   code field discriminates. Whether timer delivery via a structured message
   (like D28 fixed-size format) creates composability advantages over a separate
   signal/flag model is not analyzed in the literature.

---

## References

- [Zircon zx_timer_set](https://fuchsia.dev/fuchsia-src/reference/syscalls/timer_set).
  Fuchsia documentation.
- [Zircon System Calls reference](https://fuchsia.dev/reference/syscalls).
  Fuchsia documentation.
- [QNX Neutrino timer_create](https://www.qnx.com/developers/docs/7.0.0/com.qnx.doc.neutrino.lib_ref/topic/t/timer_create.html).
  QNX developer documentation, v7.0.0.
- [QNX Timing concepts](https://www.qnx.com/developers/docs/6.5.0SP1/neutrino/prog/timing.html).
  QNX developer documentation, v6.5.0SP1.
- [QNX Pulse notification](http://www.qnx.com/developers/docs/qnxcar2/topic/com.qnx.doc.neutrino.getting_started/topic/s1_timer_sigevent_pulse_notification.html).
  QNX Getting Started guide.
- [timer_create(2) man page](https://man7.org/linux/man-pages/man2/timer_create.2.html).
  Linux man-pages.
- Lyons, A., McLeod, K., Almatary, H., Heiser, G. (2018). "Scheduling-context
  capabilities: a principled, light-weight operating-system mechanism for
  managing time." EuroSys 2018.
  https://trustworthy.systems/publications/abstracts/Lyons_MAH_18.abstract
- [seL4 MCS Tutorial](https://docs.sel4.systems/Tutorials/mcs.html). seL4
  documentation.
- [seL4 MCS Release 10.1.1-mcs](https://docs.sel4.systems/releases/sel4/10.1.1-mcs.html).
  seL4 documentation.
- Liedtke, J. (1995). "On µ-Kernel Construction." SOSP '95.
  https://os.inf.tu-dresden.de/papers_ps/sosp95.ps
- L4 Specification Version X.2. http://l4ka.org/projects/pistachio/l4-x2-r7.pdf
- Plan 9 manual: `alarm(2)`, `sleep(2)`. https://plan9.io/magic/man2html/2/alarm
- Herder, J., Bos, H., Gras, B., Homburg, P., Tanenbaum, A.S. (2006). "MINIX 3:
  A Highly Reliable, Self-Repairing Operating System." ACM OSR 40(3).
  https://doi.org/10.1145/1151374.1151391
- Du, D. et al. (2022). "Boosting Inter-Process Communication with Architectural
  Support." TOCS 2022.
  https://ipads.se.sjtu.edu.cn/_media/publications/2022_-_a_-_tocs_-_xpc.pdf
