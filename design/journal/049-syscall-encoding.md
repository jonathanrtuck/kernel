# 049 — Syscall ABI encoding: error signaling, cap-present, SVC assignments, typed op codes, large return values

Date: 2026-04-23

## Starting point

D47 settled the ABI framework (SVC immediate, register convention, two-level
numbering). D48 settled the 25-operation enumeration. Five encoding details were
explicitly deferred by both derivations: error signaling convention, cap-present
indicator, SVC number assignments, typed operation code assignments, and large
return value convention.

These are interdependent — error signaling and cap-present share register
budget, and the convention must work across both IPC and typed operations.

## Exploration

### Error signaling

The central constraint: D28's IPC receive uses all 8 registers on success (x0–x3
data, x4 label, x5 badge, x6 cap, x7 reply). No spare register for error status.
Data words (x0–x3) are arbitrary 64-bit values, so a negative return convention
(Linux) cannot distinguish error codes from data.

Four approaches evaluated:

1. **Condition flag (SPSR modification).** The kernel modifies SPSR_EL1 before
   eret — setting or clearing the carry flag (NZCV.C). Carry set = error, carry
   clear = success. On error, x0 = error code. On success, all 8 registers carry
   normal payload. XNU uses this exact mechanism on ARM64.

   The D47 exploration's derive.md incorrectly foreclosed this: "ARM64's eret
   restores SPSR_EL1 to PSTATE. The kernel cannot use NZCV condition flags..."
   This conflated "hardware restores saved flags" with "kernel can't modify
   saved flags." The kernel writes SPSR_EL1 before eret — that IS the restored
   PSTATE. NZCV are caller-saved per AAPCS64; clobbering them on syscall return
   is legitimate.

   Cost: one BIC or ORR on the saved SPSR value. The kernel already
   saves/restores SPSR (exception.rs line 29: `pub spsr: u64`). Piggybackable on
   existing restore. ~1 cycle on a ~400-cycle fast path.

2. **Dual-purpose x7.** On receive: x7 >= 0 = reply handle, x7 = 0 = no reply,
   x7 < 0 = error code. Three-state encoding. No SPSR manipulation. But: no
   precedent for three-state register encoding in any surveyed system. And: cap
   slot 0 is a valid index, so x7 = 0 as "no reply" requires either reserving
   slot 0 or using a different sentinel. Fragile.

3. **Dedicated register outside x0–x7 (x8 or x9).** Breaks D47's "x0–x7 carry
   the message" convention. Requires saving/restoring an additional register.

4. **Status in x0, shift data words.** Breaks D47's register convention
   entirely. Destroys fast-path x0–x3 pass-through.

Approaches 3 and 4 are incompatible with D47 as settled.

**Decision: split convention — condition flag for IPC, negative-x0 for typed
operations.**

IPC uses the carry flag. This is the only approach that preserves all 8
registers for message data while being unambiguous. The ~1 cycle SPSR cost is
negligible on the fast path.

Typed operations use x0 (negative = error, non-negative = success/return value).
Typed operation return values are bounded non-negative integers (cap-table slot
indices, timestamps, zero-for-success). Negative values are unambiguous errors.
This is the Zircon convention.

The two families having different error conventions is consistent with D7's
split — they already have different register semantics (x4 = label vs. opcode,
x5 = field vs. badge vs. target). Forcing uniform condition-flag checking on
typed operations would be gratuitous — x0 already carries the return value, and
checking its sign is the simplest possible convention.

### Cap-present indicator

D28 allows 0 or 1 user caps per message. On IPC Send, x6 carries a cap handle or
indicates "no cap." On IPC Receive return, x6 carries a received cap handle or
"no cap." On Receive after Call-originated messages, x7 carries a reply handle
or "no reply."

Three approaches:

1. **Sentinel value in x6/x7.** u64::MAX = no cap present. Cap-table slot
   indices are small non-negative integers. u64::MAX cannot be a valid slot (D8
   tables are bounded by typed-memory backing — practically limited to thousands
   of slots).

2. **Flag bit in x7 (send side).** One bit of x7 (flags) indicates x6 cap
   presence. Asymmetric — receive needs a different mechanism.

3. **Implicit from structure.** Reserve slot 0 as "no cap." Couples cap-table
   layout to ABI.

**Decision: sentinel u64::MAX.** Self-contained in one register. Uniform for x6
(user cap) and x7 (reply cap) on both send and receive sides. Trivial check. No
flags consumed. No cap-table layout coupling.

### SVC number assignments

Five IPC operations need nonzero SVC immediates. No technical constraint on
assignment — all are equality-checked against ESR_EL1[15:0]. Assigned by logical
grouping:

| SVC # | Operation |
| ----- | --------- |
| #1    | Send      |
| #2    | Receive   |
| #3    | Call      |
| #4    | ReplyRecv |
| #5    | Yield     |

Primitives before compounds, IPC before scheduling. The assignment is a
convention, not a structural decision.

### Typed operation code assignments (x4)

20 operations need codes in x4. Grouped sequential — contiguous range per object
type. The kernel already knows the target type from the cap in x5, so type
information in x4 is not required for dispatch but makes the codes
self-documenting and enables per-type range-check validation.

| Range | Type              | Operations                                                                                    |
| ----- | ----------------- | --------------------------------------------------------------------------------------------- |
| 0–6   | Observer          | resume, install_cap, write_registers, read_registers, suspend, change_handler, set_scheduling |
| 7–10  | Generic cap       | destroy, clone, close, mint                                                                   |
| 11–12 | Space             | split, merge                                                                                  |
| 13–14 | Field             | create, split                                                                                 |
| 15    | Time              | split                                                                                         |
| 16–17 | Pulsar            | create, clock_read                                                                            |
| 18    | Observer creation | create_observer                                                                               |
| 19    | Resource          | resource_request                                                                              |

Dispatch: bounds check (0–19) + table jump. Dense table, no gaps. Future
operations from rights mask derivations (Space, Field, Pulsar) append to their
respective type groups — the kernel dispatch table grows but stays dense.

### Large return value convention

observer_read_registers returns a full register dump (~248 bytes: 31 GPRs + PC

- PSTATE + scheduling state). observer_write_registers takes the same as input.
  These exceed the 4-register (32-byte) argument budget.

Three approaches:

1. **Userspace buffer pointer.** The caller provides (pointer, length) in two
   registers. The kernel validates the VA and reads/writes. Under D26, Observers
   know their own VA layout (VA bases are communicated on Space cap
   acquisition).

2. **Kernel-allocated per-Observer region.** Fixed region in each Observer's
   address space. Similar to seL4 IPC buffer. Adds a "well-known address"
   concept that D26 explicitly avoids.

3. **Space cap + offset.** Cap resolution on the data path. Three registers.
   More principled but costlier.

**Decision: userspace buffer pointer.** The caller provides x0 = buffer pointer
and x1 = buffer length. The kernel validates the VA falls within mapped Spaces
(D24 cap-mapping invariant guarantees access for any valid VA). On ARM64 v8.1+,
the kernel uses LDTR/STTR instructions (or PAN disable/enable bracket) for user
memory access.

This creates no new kernel concept. No per-Observer allocation. Works
symmetrically for read_registers (kernel writes to buffer) and write_registers
(kernel reads from buffer). D26's "no well-known addresses" principle is
preserved.

For operations using the buffer convention, the typed operation register layout
is: x0 = buffer pointer, x1 = buffer length, x5 = target handle, x4 = operation
code. x2–x3 are available for additional arguments if needed.

### Per-IPC-operation register details (settled by this derivation)

**Send (SVC #1) entry:** x0–x3 = data words, x4 = label, x5 = field handle, x6 =
cap handle (u64::MAX if no cap), x7 = flags.

**Send (SVC #1) return:** Carry clear = success. Carry set = error, x0 = error
code.

**Receive (SVC #2) entry:** x5 = field handle. Other registers are don't-care.

**Receive (SVC #2) return, success:** Carry clear. x0–x3 = data words, x4 =
label, x5 = badge, x6 = cap handle (u64::MAX if no cap), x7 = reply handle
(u64::MAX if no reply cap).

**Receive (SVC #2) return, error:** Carry set. x0 = error code. x1–x7 undefined.

**Call (SVC #3) entry:** Same as Send. x0–x3 = data, x4 = label, x5 = field
handle, x6 = cap handle (u64::MAX if no cap), x7 = flags.

**Call (SVC #3) return, success:** Carry clear. x0–x3 = reply data, x4 = reply
label, x5 = reply badge, x6 = reply cap (u64::MAX if none), x7 = u64::MAX (no
reply-to-reply cap).

**Call (SVC #3) return, error:** Carry set. x0 = error code. x1–x7 undefined.

**ReplyRecv (SVC #4) entry:** x0–x3 = reply data, x4 = reply label, x5 = receive
field handle (next receive), x6 = reply cap (user cap to include in reply;
u64::MAX if none), x7 = send-once reply cap handle (from previous receive's x7).

**ReplyRecv (SVC #4) return:** Same as Receive return.

**Yield (SVC #5) entry:** No arguments. All registers are don't-care.

**Yield (SVC #5) return:** Carry clear (always succeeds). All registers
undefined (the kernel has no value to return).

### Typed operation register details

**Entry (SVC #0):** x0–x3 = operation-specific args (or buffer pointer + length
for large ops), x4 = operation code (0–19), x5 = target handle.

**Return, success:** x0 = return value (cap slot index, 0 for void operations).
Non-negative.

**Return, error:** x0 = negative error code.

## Status

Settled. The five encoding details are:

1. **Error signaling:** condition flag (carry) for IPC; negative-x0 for typed
   operations.
2. **Cap-present indicator:** sentinel u64::MAX in x6 and x7.
3. **SVC assignments:** #1=Send, #2=Receive, #3=Call, #4=ReplyRecv, #5=Yield.
4. **Typed op codes:** grouped sequential 0–19 by object type.
5. **Large return values:** userspace buffer pointer (x0=pointer, x1=length).
