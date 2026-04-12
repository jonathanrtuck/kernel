# Component Exploration — 2026-04-09

First exploration of the kernel's internal structure after the design process
restart.

## Approach

Instead of imposing a top-down decomposition (the previous "memory isolation |
CPU scheduling | communication" attempt had fuzzy interfaces), we enumerated the
hardware-imposed interfaces and looked for components emerging bottom-up.

## Hardware-imposed interfaces

The hardware presents these interfaces to the kernel:

1. **Exception delivery** — the CPU saves PC and PSTATE, masks interrupts, jumps
   to the vector table. On `eret`, restores state. This is the entry/exit
   protocol for ALL kernel activity — faults, interrupts, and syscalls all enter
   through the same mechanism.

2. **MMU** — the kernel points it at a page table (TTBR0 for user, TTBR1 for
   kernel). The MMU translates every virtual address access and enforces r/w/x
   permissions per mapping. Faults on failed translation are delivered through
   exception delivery.

3. **Timer** — a counter that fires an interrupt when it reaches a programmed
   value. Delivered through exception delivery.

4. **GIC (interrupt controller)** — device interrupt lines with configurable
   priority and core routing. Delivered through exception delivery.

5. **Physical memory** — RAM ranges discovered at boot (DTB).

6. **Cores** — independent execution units, each with own registers, TLB, timer.
   Coordinated by IPI (inter-processor interrupts).

**Key observation:** Exception delivery is the funnel. Timer, GIC, and MMU
faults all signal the kernel through the same exception mechanism. It's a
hardware-imposed calling convention, not a kernel component.

## The kernel is purely reactive

The kernel never runs proactively. It only executes in response to hardware
exceptions. The exception handler is the event loop:

```text
exception → classify → produce effects → eret
```

Analogy: like a React useEffect — a reactive function triggered by external
events that produces side effects. The antipattern parallel: useEffect updating
state triggers re-renders (infinite loop); the kernel equivalent is an exception
handler that triggers another exception (page fault inside page fault handler).

## Exception entry/exit flow

1. Exception occurs
2. Hardware: saves PC → ELR_EL1, PSTATE → SPSR_EL1, masks interrupts, jumps to
   vector
3. Kernel: immediately saves all GP registers + ELR + SPSR to stack (must happen
   before any nested exception can clobber them)
4. Kernel: classify (read ESR_EL1), handle, possibly reschedule
5. Kernel: restore registers (same or different Context)
6. `eret` — hardware restores PSTATE (which unmasks interrupts)

Important: the hardware only saves TWO registers automatically. If a nested
exception fires before the kernel pushes them to the stack, the saved state is
overwritten and lost. This is why the first instructions in the exception vector
must save state unconditionally.

Masking: hardware masks interrupts on exception entry. Most simple kernels leave
them masked for the entire handler. Unmasking during handling enables nesting
(for latency) but requires careful stack management. Masked interrupts aren't
lost — the GIC latches them as pending and delivers when unmasked.

## Contexts are the data, not a component

Explored whether Contexts could be abstracted like cores (hidden behind a
uniform interface). Conclusion: no.

Cores are fungible — any core is interchangeable. The kernel can hide them
behind "total ns/s of capacity." Contexts are NOT fungible — each has unique
state (registers, page table, Time allocation, identity). Almost everything the
kernel does is in relation to a specific Context.

Contexts are the central entity the kernel operates on — the data, not a
component. The kernel's components are defined by which aspect of Context state
they manage.

## Emerging components

**Space allocator** — tracks physical pages (free/used). Pure bookkeeping. Leaf
node with a simple interface: allocate → physical address, free.

**Time allocator + Core abstraction** — same component. The hardware-facing side
discovers total CPU capacity from cores and exposes it as ns/s. The bookkeeping
side tracks how capacity is subdivided among Contexts. Leaf node.

The "ns/s" abstraction (instead of "N cores") was explored. Potential leaks:
cache affinity (requires knowing which core is which), heterogeneous cores
(big.LITTLE — same ns/s, different throughput). Tentatively: these are leaf-node
optimization concerns below the abstraction, not fundamental leaks. Risk noted.

**Space manager** — programs page tables, manages per-Context address spaces,
handles mappings and permissions. Uses Space allocator.

**Scheduler** — decides which Context runs, programs timer, triggers context
switches. Uses Time allocator. Scheduling algorithm is a leaf node inside the
scheduler — swappable without interface changes.

**Space manager vs. Scheduler — one thing or two?** Unresolved. They manage
clearly separable state (address space vs. CPU time) and have substantial
independent work. But the context switch entangles them: save registers, switch
TTBR, load registers, program timer, `eret` is one operation touching both. Also
interact on blocking faults (page fault → deschedule). Whether the entanglement
makes them one component or two with a narrow interface is an open question.

## Structural observation: third allocator?

Space and Time each have allocator/manager pairs. If Communication involves
authority/governance (routing, permissions), there may be a parallel Capability
or Route allocator/manager. Or authority may be distributed across existing
managers rather than being a separate component. Connected to previous
exploration where capabilities were "governance encoded in existing data
structures."

## Open threads

- Communication design: "the kernel created the wall, so it provides the door."
  What does the door look like?
- Unrecoverable fault delivery: who finds out when a Context breaks? Crosses
  kernel | userspace boundary.
- Device interrupt delivery: kernel receives all interrupts, some are for
  userspace Contexts.
- One-shot timer: inside the Scheduler, but does it constrain interfaces at this
  level?
- Time measurement: hardware timer counts wall-clock time, not EL0 execution
  time. If Time is ns/s, is that wall-clock or actual execution? Affects
  fairness.
