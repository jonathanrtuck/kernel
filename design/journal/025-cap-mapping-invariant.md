# 025 — Cap-mapping invariant: no cap → no mapping

**Date:** 2026-04-19. **Starting point:** The open question "Is ownership-
transfer IPC in scope?" (raised by journal 023's timing-sensitive flag on the
PLOS 2023 ownership-transfer work). The question aimed to determine whether the
message format and page size exposure decisions are coupled or independent.

---

## The question

Should the kernel provide ownership-transfer IPC — Rust move semantics extended
across process boundaries, where the sender physically cannot access transferred
memory after send?

---

## Exploration path

### Ownership transfer as an IPC mechanism

Four options were evaluated as IPC mechanisms:

**A. Full ownership transfer** — every message can move memory objects. Maximum
A1 consistency. Costs: variable-cost send path (~60-14,000+ cycles of page-table
work on top of ~400-cycle IPC), DoS vector (sender-controlled kernel work
proportional to object size in the send path), D3 cold-path revisit trigger
fires if frequent, D5 CHERI note tension (page-granularity enforcement), D7
classification ambiguity, D13 non-uniform queue cost.

**B. Dedicated kernel operation** — separate transfer syscall, not IPC. Clean D7
classification, but two-step coordination (IPC + transfer), no atomic send-with-
data, no production precedent.

**C. No ownership transfer** — shared memory + signaling for all bulk data
(landscape §3.2 universal pattern). Message format and page size independent.
Zero kernel complexity. The "no aliasing after send" invariant is a userspace
convention.

**D. Optional/hybrid** — available but not default. Closest to L4 flexpage
grant. Tensions proportional to usage frequency.

### Performance analysis

A key finding: ownership transfer provides no performance benefit over shared
memory + signaling. Shared memory is already zero-copy — both sender and
receiver access the same physical pages through the same mappings. Ownership
transfer ADDS page-table manipulation overhead (TLBI, PTE writes, potential
IPI). The "zero-copy" framing is misleading — it compares ownership transfer to
a copying IPC model, but this kernel's alternative (Option C, D18) is shared
memory, which is already zero-copy.

The benefit of ownership transfer is safety (kernel-enforced non-aliasing), not
performance.

### Security analysis

Three concrete scenarios where kernel-enforced non-aliasing matters:

1. **TOCTOU / double-fetch prevention.** Under shared memory, validated data can
   be modified by the sender between check and use. Standard defense: copy
   before validate. Ownership transfer makes in-place validation safe.

2. **Post-handoff isolation.** A compromised sender (e.g., driver) retains
   shared-memory access to buffers it already handed off. Ownership transfer
   removes access at handoff.

3. **Clean failure on use-after-send.** Silent data corruption becomes a page
   fault.

All achievable through userspace discipline (copy + voluntary unmap) except
post-handoff isolation against adversarial senders — a compromised sender won't
voluntarily unmap, and the receiver can't modify the sender's page tables.

### The reframe: cap-table/MMU inconsistency

The exploration surfaced that the real question is not about IPC. In the current
model, capability transfer and MMU mappings are independent:

- **Cap removed, mapping persists:** An Observer closes its cap to a memory
  object, but the virtual address mappings remain. The Observer can still
  read/write the memory through the MMU despite having no capability authority.
- **Cap present, no mapping:** An Observer holds a cap but hasn't mapped the
  object. The Observer can perform syscalls on the object but can't access the
  memory.

The second state is natural (D9 two-step: create, then bind). The first state is
a tension with D4 ("designation = authority") — the MMU grants access that the
capability system does not authorize.

Every surveyed system has this same disconnect (Mach, Zircon, seL4, L4). It is
the standard model. But this kernel's D4 commitment is stronger than most, and
the tension is real.

### The invariant

Rather than adding an IPC mechanism, the kernel maintains a system-wide
invariant:

**When an Observer's last capability to a mapped memory object is removed (via
close, move, or destroy), the kernel automatically unmaps that object from the
Observer's address space.**

This makes the capability table the source of truth for memory access. The MMU
follows the cap state. The Observer's experience: if you hold a cap to a mapped
object, you can access the memory. If you lose the cap, you lose the access.

Ownership transfer falls out naturally: "move" is clone-to-receiver +
close-on-sender. The close triggers auto-unmap. No special IPC mechanism needed.

### Why this is better than IPC-level ownership transfer

The same safety property (sender can't access after send), achieved as a cap-
system invariant instead of an IPC mechanism:

- **No IPC-path page-table operations.** The unmap happens at cap-close time
  (cold path under D1), not at send time (hot path). The D1/D3 tensions from
  Option A dissolve.
- **No message-format changes.** The IPC message carries caps as normal. Whether
  a cap is cloned or moved is a cap-table operation, not a message attribute.
- **No D7 classification ambiguity.** Cap operations are typed kernel ops. IPC
  is IPC. No hybrid.
- **No DoS vector.** The page-table work happens in the close() syscall, which
  is the Observer's own voluntary action, not in a send() triggered by an
  arbitrary sender.
- **No D13 queue cost disruption.** Queue throughput remains uniform.
- **Stronger than IPC-level ownership transfer.** The invariant applies to ALL
  cap removal, not just IPC sends. Close, destroy, any path that removes a cap
  triggers the unmap. The safety property is systemic.

### Cross-core analysis

For single-Observer address spaces (the common case under D6): the auto-unmap is
always local to the Observer's own core. The Observer's syscall (close, or send
with move) runs on its core; the TLBI is local. No cross-core broadcast.

For shared address spaces (D10): broadcast is needed when the last per-AS cap-
holder closes, because other cores may have TLB entries. But this is the same
broadcast that any shared-address-space page-table modification requires —
explicit unmap() has the same cost. The invariant adds no cross-core
coordination beyond what the same operations would cost if done explicitly.

Memory object destroy (D11) requires cross-core broadcast regardless of the
invariant — you can't leave page-table entries pointing to freed pages.

### D10 shared address space: cascade behavior

If Observers A and B share an address space, and only A holds a cap to mapped
memory object M: when A closes the cap, the per-AS counter hits zero, and the
kernel unmaps M. B loses access and gets a page fault on next use.

This is correct behavior under the invariant (B had no authority, so B shouldn't
have access). But it requires cap discipline: every Observer that uses a shared
memory object in a shared address space must hold its own cap. Sharing is
through capability transfer (D9). A library function absorbs this — trivial
userspace complexity, not essential complexity pushed outward.

### Implementation requirements

**Reverse mapping list (per memory object):** Track which (address-space,
virtual-address-range) tuples reference each memory object. Needed for destroy
cleanup regardless of invariant. Map adds an entry; unmap removes one.

**Per-(address-space, memory-object) cap counter:** Count how many Observers
sharing an address space hold caps to each mapped object. Updated on every cap-
table add/remove for memory-object caps. Small in practice — bounded by the
number of mapped objects per address space.

### Rejected alternatives

**IPC-level ownership transfer (Options A/B/D):** All place page-table work on
the IPC path or require IPC-level mechanism changes. The invariant achieves the
same property at a better layer.

**No invariant (Option C):** Leaves the cap-table/MMU inconsistency. Shared
memory + signaling works, but a compromised sender retains MMU access to
transferred data indefinitely. Acceptable by landscape standards, but
inconsistent with D4's "designation = authority" commitment.

**Opt-in invariant (per-object flag):** "Some memory objects auto-unmap, others
don't." Creates two classes of memory objects with different cap/mapping
behavior. Rejected for interface complexity and the principle that the invariant
should be systemic — D4 doesn't apply selectively.

### Convergence check

The archive (restart-1) chose byte-addressed memory objects with kernel-internal
page backing but did not address the cap/mapping relationship. The archive's
"ownership-transfer IPC" concept was an IPC-level mechanism. This derivation
reaches a different answer (cap-system invariant vs. IPC mechanism) by a path
the archive did not explore: the reframe from "how should IPC transfer memory?"
to "what is the relationship between caps and mappings?"

The archive's IPC-level approach is dominated: the cap-system invariant provides
the same safety property with strictly less cost (cold-path vs. hot-path, no IPC
changes, no D7 ambiguity).

---

## Decision

**The kernel maintains the cap-mapping invariant: when an Observer's last
capability to a mapped memory object is removed, the kernel automatically unmaps
that object from the Observer's address space.** Ownership-transfer IPC is not a
separate mechanism — it falls out naturally from cap move + auto-unmap.

---

## What this settles

- **Ownership-transfer IPC:** dissolved. Not a separate mechanism. Falls out of
  cap move + auto-unmap via the invariant.
- **Message format independence:** the message format is fully independent of
  ownership transfer. No IPC changes needed.
- **Page size exposure independence:** the invariant operates on whatever pages
  back the memory object internally (D9: "which physical pages back an object is
  a kernel-internal concern"). Page size exposure remains a fully independent
  question.

## What this does NOT settle

- **Cap/mapping relationship details:** Does explicit unmap() still exist?
  (Likely yes — remap at a different address requires unmap + map. The cap is
  retained; only the mapping moves.) Map remains explicit (Observer chooses
  address); unmap is available both explicitly and automatically.
- **Sub-page packing:** If the kernel packs two small objects onto one physical
  page, the invariant means closing the last cap to one object requires either:
  no packing (each object gets its own page), or the kernel handles sub-page
  objects without auto-unmap (breaks invariant), or the kernel copies the
  co-located object to a new page (expensive). This is a kernel-internal
  implementation concern that the memory-object implementation must resolve.
- **Space budget transfer on cap move:** When a memory-object cap is moved, does
  the budget charge transfer? Kernel-internal accounting question.
- **D9 memory object operations:** The invariant adds auto-unmap as a new
  behavior of memory objects. D9's "specific operations" remain open.

## What this dissolves from the open questions list

- ~~Ownership-transfer IPC~~ — dissolved by this invariant
- Message format — now fully independent (no IPC-level ownership transfer)
- Page size exposure — now fully independent

## Derivations that need notes

- **D9:** The memory object model gains a new property (auto-unmap on last-cap-
  close). D9's conclusion stands; the invariant builds on it.
- **D10:** Shared-address-space cascade behavior should be documented. D10's
  conclusion stands.
- **D11:** Close gains auto-unmap as a consequence for mapped memory-object
  caps. D11's conclusion stands.
