# 040 — Pager fault resolution protocol

**Date:** 2026-04-21 **Starting point:** D12 opened "Pager reply/resume
mechanism" as a downstream question. D14 settled resume(observer_handle) as a
typed kernel syscall. D35 named install_cap as the fault resolution mechanism.
D28 settled the fault message format (label + 4 data words + 1 cap). The
remaining question: what is the pager's complete syscall sequence when resolving
a fault? Is resume() alone sufficient, or must the pager perform memory
operations?

---

## The question

When an Observer faults, the kernel delegates to the handler (D12). The handler
receives a fault message (D28: fault type, Space identity, offset, access type,
Observer handle). How does the handler resolve the fault and resume the
Observer? What operations, in what order?

The spec listed three related open questions: pager reply/resume mechanism,
pager unavailability protocol, and D7 classification of fault traffic. This
exploration addresses the first.

---

## D26 transforms the question

In seL4, L4, Mach, and Zircon, the pager controls VA placement — it maps pages
at the faulting address. The faulting instruction retries at the same VA and
succeeds because the pager explicitly placed memory there.

Under D26 (capability-addressed memory), the landscape shifts:

1. **Holding a Space cap = having access** (D26). The page table is a
   materialized view of cap state (D24). There is no separate map() or unmap().
2. **The kernel assigns VA bases** (D26). A Space's VA base is set at creation
   time and is a property of the Space. The pager doesn't control it.
3. **install_cap IS the mapping operation** (D26 + D24 + D35). When the handler
   installs a Space cap into an Observer's table, the kernel updates the page
   table as a consequence.

Consequence (3) means the "memory operation" question from the spec is answered
by D26's model: install_cap IS the memory operation. No separate mapping syscall
exists or is needed.

But consequence (2) creates a structural constraint that no surveyed system
shares: **a new Space cannot cover the faulting VA.** The faulting instruction
was at `base_of(S) + offset` where offset exceeds S's size. A new Space T gets
its own VA base. The instruction retries at the same VA, which T's range doesn't
cover. The fault is unresolved.

This means D26 structurally limits traditional demand paging. The pager can
provide new Spaces, but they don't resolve out-of-bounds faults on existing
Spaces. Resolution would require Space resize (D9 open question: "specific
operations on Spaces — split, COW/clone, resize") or pager-influenced VA
assignment (D26 does not provide).

---

## Per-fault-type resolution

### Resource requests (D31): install_cap + resume

An Observer that needs more Space or Time invokes a resource request syscall.
The kernel routes it to the handler as a fault message (D31). The handler
provides the requested resource:

```
[handler receives resource request → obs_handle, request info]
observer_install_cap(obs_handle, resource_cap)   -- provides Space or Time
observer_resume(obs_handle)                      -- Observer's syscall completes
```

This works cleanly because the Observer explicitly asked for resources. When it
resumes, it learns the new cap's slot from the syscall return value and adapts
to the new VA base. No VA placement problem — the Observer adjusts.

The same path serves the D8 pattern for non-table-full cap provisioning: the
Observer needs a cap installed, the handler installs it, the Observer resumes.

### Cap-table-full (D8): reserved growth slot + resume

When the Observer's cap table is full and a new capability must be stored (D8
line 400), the kernel faults the Observer. The handler must provide a Space the
kernel consumes for table growth (D32: type conversion into cap table backing).

install_cap to a regular slot cannot work — the table is full. The solution uses
the same reserved-slot pattern as D21 (fault handler at kernel-reserved slot):
the kernel reserves a second cap-table slot for table growth backing.

```
[handler receives table-full fault → obs_handle]
observer_install_cap(obs_handle, growth_space)  -- installs at reserved growth slot
observer_resume(obs_handle)                     -- kernel consumes Space, retries
```

The reserved growth slot is always writable (reserved slots are not counted
against "table full"). The kernel consumes the Space for table growth (D32 type
conversion) and retries the original operation that triggered the table-full
condition.

This also handles the interleaved case: if the handler encounters table-full
while resolving another fault (e.g., trying to install_cap for a resource
request), install_cap returns an error. The handler installs growth Space at the
reserved slot, the kernel grows the table, and the handler retries the original
install_cap.

D32 (line 1667) explicitly notes that optimizations (designated growth Space at
creation time) can be added behind the same interface. A pre-arranged growth
Space avoids the fault roundtrip entirely for common table growth.

### VM page faults (out-of-bounds): error notification

An Observer that accesses a VA outside any of its Space caps' ranges generates a
hardware page fault (ARM64 Data Abort or Instruction Abort). Under D12, the
kernel dispatches this to the handler.

The handler cannot resolve the fault by providing a new Space. The faulting
instruction will retry at the same VA (`base_of(S) + offset`), and a new Space T
has its own kernel-assigned VA base (D26) that doesn't cover that address. Space
resize would resolve it (grow S to cover the offset), but resize is unsettled.

The handler's options:

1. **Destroy** the Observer — the access was a bug.
2. **Cooperative recovery** — change the Observer's PC via write-registers (D39)
   to a pre-arranged trampoline. The trampoline requests Space via D31, updates
   the Observer's internal bookkeeping, and retries the operation using the new
   Space's VA base. This is analogous to Unix signal handlers, but the "kernel"
   role is played by the handler Observer using existing operations
   (write-registers + resume).

Memory growth for Observers is through the explicit D31 resource request path.
The Observer knows when it needs more memory (bounds checking against known
Space sizes), requests it, receives a new Space cap, and adapts to the new VA
base.

### Lazy PTE population: kernel-internal

D9 says Spaces are always physically backed. If the kernel lazily populates page
table entries (D26 open sub-question: demand fault vs. eager), a fault within an
owned Space means: "Observer holds the cap, physical pages exist, PTE not yet
populated." The kernel has all information needed and no policy decision to
make. This is D12's preserved "kernel-internal fast path for trivial faults" —
the kernel populates the PTE and resumes without involving the pager.

---

## Kernel validation

The kernel does not validate fault resolution before honoring resume(). If the
handler calls resume() without resolving the fault condition, the Observer
re-faults. This is self-correcting (the handler gets another chance) and
consistent with D12's trust model — the handler IS the designated fault handler.
A malicious handler can already withhold resume() entirely; validation provides
no additional safety.

---

## Archive convergence

The archive (journal/011-reply-routing-and-fault-resume.md) unified IPC reply
and fault resume from the sender's perspective: both were "send to a
reply/control endpoint." The current chain diverges: D14 decoupled fault resume
as a typed kernel syscall (resume(observer_handle)), and D16 settled IPC reply
as send-to-field with send-once cap. Different mechanism families per D7.

The archive's fault resolution was IPC-based (handler sends "resume" to a
per-Context control Endpoint). The current chain's resolution is syscall-based
(handler calls install_cap + resume). The divergence is explained by D7 (typed
kernel syscalls for Observer operations) and D14 (resume is not IPC), both
settled after the archive.

The archive did not have D26 (capability-addressed memory), so it did not
encounter the VA placement constraint that limits demand paging. The archive's
external pager model implicitly assumed VA-controlled mapping.

---

## The decision

**Pager fault resolution is per-fault-type via typed kernel syscalls:**

1. **Resource requests (D31):** `observer_install_cap(obs, cap)` +
   `observer_resume(obs)`. The Observer asked explicitly; the handler provides;
   the Observer adapts.

2. **Cap-table-full (D8):** `observer_install_cap(obs, growth_space)` to
   reserved growth slot + `observer_resume(obs)`. Kernel consumes Space for
   table growth (D32) and retries the original operation. Growth slot follows
   D21's reserved-slot pattern.

3. **VM page faults (out-of-bounds):** Dispatched to handler per D12 (error
   notification). Handler destroys or performs cooperative recovery (PC surgery
   via write-registers + resume). A new Space cannot resolve the faulting
   instruction under D26 (VA placement is kernel-assigned). Transparent demand
   paging is not supported without Space resize.

4. **Lazy PTE population:** Kernel-internal per D12's preserved fast-path
   optimization. Pager never sees these.

5. **No kernel validation** of fault resolution before resume(). Kernel trusts
   the handler (D12's trust model).

**install_cap + resume is the general-purpose resolution pattern** for faults
where the handler provides resources. D35's structural reuse holds: the same
operations serve Observer creation, resource request resolution, and cap-table
growth. No new kernel surface is introduced.

**The reserved growth slot** extends D21's pattern (one more kernel-reserved
cap-table slot per Observer). The growth slot is consumed on use (D32 type
conversion) and becomes empty again.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Rust's type system will shape the
implementation (e.g., the Option<SpaceCap> parameter on the growth slot) but the
mechanism choice does not depend on the implementation language.

**A2 (ARM64)** provides the hardware fault mechanism (ESR_EL1, FAR_EL1,
exception vectors) but does not choose between resolution models.

---

## What remains open

- **Space resize.** If settled, transparent demand paging becomes possible — the
  handler grows the faulting Space to cover the offset, and the retried
  instruction succeeds. The pager protocol (install_cap + resume) does not need
  to change; resize is a new Space operation, not a new fault resolution
  mechanism. This is the highest-leverage open question for enabling traditional
  demand paging.

- **Observer handle rights in fault message.** The kernel creates the Observer
  handle in the fault message. What rights it carries (minimum: install-cap +
  resume for resource requests) is downstream of the fault message content
  question.

- **Fault message content per type.** D28 settled the format (4 data words + 1
  cap) but not the specific content for each fault type (VM fault,
  cap-table-full, resource request). The handler needs to distinguish these to
  choose the correct resolution path.

- **Pager unavailability protocol.** What happens when the handler is destroyed,
  blocked, or unresponsive while an Observer is faulting. Separate exploration.

---

## Status

**Settled.** The pager fault resolution protocol follows mechanically from D26
(install_cap is the mapping operation), D14 (resume as typed syscall), D35
(install_cap as general-purpose primitive), D32 (type conversion for table
growth), and D12 (delegation without kernel policy). The reserved growth slot
extends D21's pattern with no new kernel surface.

Revisit if D9 is revised (Space resize would enable transparent demand paging —
the OOB fault path changes from error notification to resolution), if D26 VA
assignment policy allows pager-influenced placement, or if a downstream
derivation reveals that the error-notification model for OOB faults creates
essential complexity that transparent demand paging would have avoided.
