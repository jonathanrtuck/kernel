# IPC Message Format — 2026-04-20

**Starting point:** D13 (queued fields) listed message format as a downstream
open question. Five journal entries (013, 016, 017, 023, 025) reference it; none
derive it. D25 (cap-mapping invariant) dissolved the last coupling concern
(ownership-transfer IPC), confirming message format independence. All parent
decisions are settled: D8, D13, D15, D16, D17, D18, D24.

---

## The format

A message is a fixed-size control packet with structurally separate fields:

```text
Sender provides:              Receiver sees:
  label      (header)           badge       (kernel-injected from sending cap)
  data[0..3] (4 words)          label       (header, unchanged)
  cap        (0 or 1 handle)    data[0..3]  (4 words, unchanged)
                                cap         (0 or 1 handle, remapped to receiver's table)
                                reply_cap   (kernel-injected, present only on Call())
```

- **4 data words** (32 bytes). Untyped machine words.
- **1 user capability slot.** Structurally separate from data words.
- **Label** in a dedicated header field (not a data word).
- **Badge** in a dedicated kernel-injected field (not a data word).
- **Reply cap** in a dedicated kernel-injected field (not a user cap slot).
  Present only when the message was sent via Call().
- **Fixed size.** No length field, no variable layout.

---

## Derivation

### Data word count: 4

Three independent constraints converge on 4 as the natural size:

**Fault descriptor completeness.** Under D12 (fault delegation), the kernel
generates fault messages. A VM fault under D26 (capability-addressed memory)
carries: fault type, Space identity, offset within Space, access type. That is 4
words. No fewer suffices; more does not help — the gap between a fault
descriptor (4 words) and full Observer state (~98 words for GP registers alone,
~400+ bytes with FP/SIMD) is too large for any reasonable message size to
bridge. The fault handler holds the Observer handle (via the cap slot) and calls
inspect(observer_handle) — a D7 typed kernel operation — for full state. This
decomposition is architecturally clean: IPC carries notification (D13); register
inspection is a resource operation (D7).

**Control-plane sufficiency.** Bulk data goes through shared Spaces (D26, D9,
landscape §3.2: "IPC should never carry bulk data"). Messages carry control
metadata only. With label in a dedicated header field, 4 data words cover: most
RPC request/reply pairs (4 arguments), interrupt messages (badge carries IRQ
identity; data carries minimal state), and badge-closure notifications (1 word
at most).

**Queue memory bounds.** D13 estimates ~48 bytes per queued message. A 4-word
message with header, badge, cap slot, reply cap, and queue linkage fits within
this budget. Larger formats increase per-slot cost for every field in the system
— cold-path waste that compounds.

**Rejected: variable-length (seL4 model).** Length validation on every message,
two copy paths (register vs. memory spill), variable queue slot sizes. The
flexibility serves workloads that under D26 are already better served by
shared-Space bulk transfer. The two-path complexity would damage D1's hot-path
simplicity and verification tractability.

**Rejected: 6 words.** Comfortable register-fit on ARM64 (A2), but no settled
decision requires more than 4 data words. The extra 2 words would be unused in
fault messages (the richest kernel-generated type), most RPC pairs, and all
interrupt/closure messages. Wasted queue memory with no structural benefit.

### Cap slot count: 1 user slot + kernel-injected reply cap

**1 user slot.** Three independent settled decisions each require exactly 1 cap
in a message:

- D16: Call() includes a send-once reply cap.
- D14/D12: fault messages include the faulting Observer's handle (for resume).
- D22: interrupt messages include a send-once ack cap (for unmask).

No settled decision requires 2 simultaneous user-provided caps.

**Reply cap as a dedicated kernel-injected field.** D16 settles that the kernel
creates the reply cap — the sender does not provide it. This makes the reply cap
structurally parallel to badge: kernel-injected, outside the sender's control.
Placing it in a dedicated field (rather than consuming the user's cap slot)
follows from this parallel. The sender's 1 cap slot remains free for payload
caps during Call(). On non-Call sends, the reply cap field is absent.

This resolves the tension identified in Phase 5: without the dedicated reply-cap
field, Call() could never transfer a user cap (the reply cap would consume the
only slot). With it, the common "request + delegated authority" RPC pattern fits
in one message: label + 4 data words + 1 user cap + kernel-injected reply cap.

**Rejected: 2 user cap slots.** The second slot adds per-message validation and
allocation cost on every cap-bearing message. The only pattern requiring 2 user
caps is multi-cap transfer (e.g., handing over an Observer handle and a Space
cap simultaneously) — a cold-path operation absorbable by multi-message
protocols or separate typed kernel operations (D7). Each additional cap slot is
expensive: rights validation, destination table allocation, ABA tag management,
table-full error handling. 1 slot covers all structurally motivated patterns.

**Rejected: 3 cap slots (seL4).** No settled decision requires 3 concurrent cap
transfers. Additional format complexity with no structural demand.

### Cap transfer encoding: dedicated fields

Capability slots are structurally separate from data words. The message has 4
data words + 1 cap slot. The sender fills them independently. The kernel knows
the message shape from the cap-slot presence indicator (zero or one) — a single
field check that gates the fast path.

**Rejected: bitmask over unified slots (archive's cap_mask).** Under the archive
model, data and caps share the same 4 slots; a bitmask indicates which carry
handles. This conflates two structurally different kernel operations: data
copying (memcpy-speed) and cap transfer (validation + allocation + ABA tag).
D8's flat cap table with typed entries makes cap transfer a categorically
different operation from data movement. Dedicated fields reflect this structural
distinction. Additionally, the bitmask model requires inspecting the mask before
knowing the message shape, even for zero-cap messages — the dedicated-fields
model makes zero-cap detection trivial (one count/flag check), which is the
fast-path gate.

### Badge placement: outside data words

Badge is a dedicated kernel-injected field, not a data word. Sender fills 4
words; receiver reads 4 words + badge. No positional shift between sender and
receiver data layouts.

D17 settles that badge is kernel-injected — the sender never provides it. Making
it a separate field (not overwriting a data word) preserves 1:1 positional
alignment between sender and receiver data words. The alternative (badge
replaces data word 0, shifting sender's words) creates a format transform every
sender and receiver must account for — accidental complexity.

### Label: in header

A label field in the message header serves receiver-side dispatch (analogous to
seL4's label in MessageInfo_t). It does not consume a data word. Under D7's
split model, the kernel does not dispatch on IPC message labels — that is the
receiver's job — so the label is pass-through metadata.

### Fault message decomposition

Fault messages use the standard format. The kernel generates:

```text
badge:     from fault handler cap (D21) — identifies which Observer faulted
label:     fault type (VM fault, cap-table-full, invalid syscall, etc.)
data[0]:   Space identity (for VM faults)
data[1]:   offset within Space
data[2]:   access type (read/write/execute)
data[3]:   reserved / fault-type-specific
cap:       Observer handle (for resume via D14)
reply_cap: absent (kernel deposit, not Call())
```

Full Observer state (registers, PC, SP, PSTATE) is accessible via
inspect(observer_handle) — a D7 typed kernel operation. The fault message is the
notification; state inspection is a separate operation. This decomposition is
correct under D7: IPC is one mechanism family; resource operations are another.

---

## Archive convergence

Archive journal 010 ("Message Shape") arrived at a similar position through
independent reasoning:

**Converges on:**

- 4-slot payload size (32 bytes for data)
- Badge as kernel-injected, minter-assigned, per-capability
- Fixed-size format (no variable-length messages)
- Bulk data through shared memory, not in-message
- Register-sized messages as a ceiling

**Diverges on:**

1. **Cap encoding: bitmask (archive) vs. dedicated fields (current).** The
   archive's cap_mask model treats 4 slots as a shared budget between data and
   caps. This derivation separates them. Explanation: D8's flat cap table with
   typed entries (settled after the archive) makes cap transfer a structurally
   distinct operation. Dedicated fields reflect D8's structural distinction; the
   archive lacked D8's clarity when it chose cap_mask.

2. **Reply cap placement: payload slot (archive) vs. dedicated field
   (current).** The archive's RPC request uses 2 data + 2 caps (reply field
   - Time capability) from the 4 shared slots. This derivation gives the reply
     cap its own kernel-injected field. Explanation: D16 (settled after the
     archive) establishes that the kernel creates the reply cap, making it
     structurally parallel to badge. The archive treated the reply cap as
     sender-provided because D16 didn't exist yet.

3. **Cross-architecture ceiling: x86-64 (archive) vs. ARM64-only (current).**
   The archive derived 4 slots from x86-64's register limit (6 arg registers
   - 2 overhead = 4). A2 (ARM64 target) removes this bottleneck. This derivation
     arrives at 4 words from fault-descriptor completeness and control-plane
     sufficiency, not register count.

4. **Type field: kernel-set (archive) vs. label (current).** The archive has a
   kernel-set type field (IPC/fault/interrupt/system). This derivation uses a
   label in header — a pass-through tag for receiver dispatch. For
   kernel-generated messages (faults, interrupts), the label carries the fault
   or interrupt type. The distinction matters less than it appears: both provide
   a receiver dispatch mechanism. The label model is more uniform — the same
   field serves both userspace and kernel messages.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. The message format is a data layout decision;
Rust implements any of the considered formats equally well. A1 becomes relevant
one level down when defining the Rust types for message structs.

**A2 (ARM64)** provides the register file but does not choose the format. ARM64
has 8 argument registers (more than needed for 4 data words + metadata). The
format would fit on architectures with fewer registers (at the cost of memory
spill), but A2 means we don't need to optimize for that case.

**A3 (generic)** creates mild pressure toward larger/variable formats (more
workload flexibility). Bounded by D26: bulk data goes through Spaces, so message
payloads are inherently small control packets regardless of workload. The
pressure is real but dominated.

**A4 (reactive)** confirms no background message assembly or fragmentation — the
format must be self-contained per syscall. Consistent with fixed-size but not
load-bearing (variable-size is also per-syscall).

---

## What this derivation does NOT settle

- **Fault message content details.** The specific fields for each fault type (VM
  fault, cap-table-full, invalid syscall) need formal derivation. This entry
  establishes the format and budget (4 data words + 1 cap); the content layout
  is one level down.
- **Badge-closure notification content.** Kernel-generated message for D17
  opt-in lifecycle tracking. Format budget established; content layout deferred.
- **Interrupt message content.** D22 delivers badge + ack cap. Whether
  additional data words carry interrupt state is deferred.
- **inspect() syscall shape.** D7 typed kernel operation for reading Observer
  state. This entry establishes the decomposition (fault message = descriptor,
  inspect = full state); the inspect interface is part of the Observer rights
  model derivation.
- **Sender-side syscall encoding.** How the send() and Call() syscalls encode
  the message (which registers carry what) is an A2 implementation detail behind
  this format.
- **Send-right gating of cap transfer.** Whether the field's rights (D15) or a
  separate Grant right gates cap slot usage. seL4 uses a Grant right; this
  kernel may or may not need one.
- **IPC fast-path conditions.** Whether the fast path handles 0-cap messages
  only or also covers the 1-cap case. Implementation optimization.

---

## Status

**Settled.**

Revisit if D13 is revised (different IPC model changes the queue and fast-path
assumptions), if D16 is revised (changes the reply-cap mechanism that motivated
the dedicated field), if D26 is revised (removing capability-addressed memory
would reopen bulk-data-in-message), or if a downstream derivation reveals that 4
data words are insufficient for a structurally required kernel message type.
