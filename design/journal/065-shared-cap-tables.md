# Journal 065 — Shared Capability Tables: Per-Observer Only (D8 Confirmed)

Discharges D8's sharing revisit condition. Closes the open question "Can
Observers share capability tables?"

## Context

D8 settled per-Observer flat capability tables with typed-memory backing and
explicitly deferred table sharing: "revisit if the capability-addressed memory
model (D26) reveals that per-Observer tables force essential sharing complexity
into userspace." D26 has now settled (capability-addressed memory with page
table subtree sharing). The revisit condition is discharged.

## The question

Should the kernel support shared capability tables between Observers that occupy
the same protection domain, or is D8's per-Observer table sufficient for all use
cases?

The primary motivation for sharing is ergonomic: multi-threaded userspace
expects threads to share open handles (the POSIX fd table model). Per-Observer
tables require explicit cap transfer via observer_install_cap (D35) for every
cap a sibling thread needs.

## Design space

Four options were evaluated:

1. **Per-Observer only (D8 as-is).** Each Observer has its own private table.
   Sharing requires explicit cap transfer.
2. **Shared CSpace.** Multiple Observers reference the same physical table. One
   Observer's mutations are immediately visible to siblings.
3. **Hybrid.** Per-Observer default, opt-in sharing. The kernel supports both
   modes.
4. **Copy-on-reference.** Shared reads, private writes. On first cap mutation,
   the kernel forks the table. Novel — no prior art in any surveyed kernel.

## Why per-Observer only

Three independent criteria converge on the same option:

**Performance.** The cap table sits on the hot path (D43 marks it "Hot (syscall
entry)"). A private table is exclusively per-core — zero locking, zero cache
invalidation from sibling writes, zero contention. Every sharing option
introduces synchronization: Option 2 requires a reader-writer lock on every cap
access; Option 3 pays that cost for the shared-table case; Option 4 pays an
O(table-size) copy on first write.

**Security.** Each Observer is an independent trust domain. A compromised
Observer can only use caps it was explicitly given — it cannot read, modify, or
close caps in a sibling's table. Confused-deputy protection operates at thread
granularity, which is stronger isolation than any mainstream OS provides.
Options 2 and 3 (shared mode) reduce this to POSIX-style intra-process exposure.

**Structural alignment.** Per-Observer tables satisfy D1 (no shared mutable
state on hot path), D4 (designation = authority at Observer granularity), D8
(typed-memory backing — each table backed by its Observer's Space), D33 (destroy
cascade works unmodified), and D26 (memory sharing already solved at page-table
level, not cap-table level).

## Why sharing is not essential complexity

The D8 revisit condition asks whether per-Observer tables "force essential
sharing complexity into userspace." Two findings resolve this:

1. **D26 already handles the most common sharing use case.** Observers sharing a
   Space share the page table subtree — they see the same virtual addresses.
   Memory sharing does not require cap-table sharing.

2. **Authority propagation is userspace-library complexity.** A threading
   library wrapping Observer creation can install a standard set of caps into
   each new Observer via observer_install_cap (D35). Runtime cap changes require
   the supervisor to push new caps to sibling Observers — O(N x M) for N threads
   and M cap changes. This is the EROS/KeyKOS discipline. It is real work for a
   userspace library, but it is not essential kernel complexity under A5: the
   kernel's interface (observer_install_cap) is already simple and sufficient.
   The complexity of "which caps does each thread get?" is policy, not
   mechanism.

## Rejected alternatives

**Shared CSpace (Option 2):** Violates D1 (shared mutable state on hot path),
breaks D8 typed-memory backing (ambiguous ownership — which Observer's Space
backs the shared table?), complicates D33 cascade (shared table outlives
individual Observer destruction, requiring reference counting or new cascade
semantics), reduces isolation. seL4 and Zircon both use shared tables
successfully, but their design constraints differ: seL4 offers it as optional
(and POSIX emulation always uses it); Zircon mandates per-process sharing (but
has a process concept this kernel lacks per D6).

**Hybrid (Option 3):** Two code paths for every cap-table operation — two lookup
patterns, two destroy protocols, two accounting models. A5 cuts against this:
two modes where one suffices is accidental complexity. seL4's experience shows
that when given the choice, userspace always chooses sharing for threads, making
the private option dead code in practice.

**Copy-on-reference (Option 4):** No prior art in any surveyed kernel. Solves
spawn-time convenience but not runtime sharing — degenerates to per-Observer
after first write. Novel semantics with no established mental model. The
spawn-time convenience is achievable via a userspace library without a new
kernel primitive.

## Prior art

- **EROS/KeyKOS:** Per-domain authority as architectural principle. Domains
  typically single-threaded; multi-threaded computation expressed as cooperating
  servers.
- **seL4 (strict per-TCB):** Used in safety-critical contexts where
  intra-process thread isolation is desired. Per-TCB tables with userspace
  authority management.
- **seL4 (shared CSpace):** Optional via shared root CNode. POSIX thread
  emulation layers always use it. Locking overhead not reported as a bottleneck.
- **Zircon:** Mandatory per-process sharing with per-table reader-writer lock.
  Shipped on billions of devices without production issues. But Zircon has a
  process concept; this kernel does not (D6).
- **NOVA:** Per-PD sharing. Genode layers authority structure above.
