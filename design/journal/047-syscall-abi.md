# 047 — Syscall ABI: trap mechanism, register convention, numbering

Date: 2026-04-23

## Starting point

D7 settled two mechanism families (IPC + typed kernel operations). D28 settled
the fixed-size message format (4 data words + 1 cap + label/badge/reply). D13
established the direct-switch fast path (~400 cycles ARM64) as the IPC
performance target. Multiple derivations (D35, D39) explicitly deferred
"specific syscall encoding — register conventions, error codes" as one level
down.

The question: how does userspace invoke the kernel at the instruction level?
Three sub-questions: (1) trap mechanism, (2) register calling convention, (3)
operation numbering scheme.

## Exploration

### Register budget analysis

The central constraint is D28's message format mapped to ARM64's 8 argument
registers (x0–x7).

IPC send provides: label + 4 data words + field handle + cap handle = 7 values,
plus a syscall discriminator = 8 total. IPC receive returns: badge + label + 4
data words + cap handle + reply handle = 8 values. Both directions exactly fill
8 registers.

This means the discriminator's placement directly determines whether IPC can be
fully register-resident. If the discriminator occupies a register, one message
value must go to memory or be packed.

### Trap mechanism: SVC #imm16

ARM64's SVC instruction has a 16-bit immediate field. The kernel already reads
ESR_EL1 to confirm EC=0x15 (SVC from AArch64); the immediate is in ESR_EL1[15:0]
— the same register read. Using the immediate for the operation number costs
zero additional cycles vs. reading a GPR.

seL4 uses SVC #0 with the syscall number in x7. This is the industry precedent.
However, it consumes one of the 8 argument registers, which under D28's tight
register budget means one IPC value cannot be register-resident.

Decision: **syscall number in SVC #imm16.** Frees all 8 argument registers for
payload. The cost — syscall number is an instruction-embedded constant — is
acceptable because syscall wrappers are per-operation functions;
runtime-variable syscall numbers are not needed.

### Register convention: IPC-optimized, uniform

Three options were considered:

**AAPCS64-aligned:** x0 = return/error, x1–x7 = args. Follows the C calling
convention. On receive return, x0 carries error status, leaving 7 registers for
message data — losing one data word or the reply handle.

**IPC-optimized:** Registers mapped to D28's message format. x0–x3 = 4 data
words, x4 = label (send) / label (receive), x5 = field handle (send) / badge
(receive), x6 = cap handle, x7 = flags (send) / reply handle (receive).

**Split convention:** IPC-optimized for IPC, AAPCS64 for typed operations.

The IPC-optimized layout enables a critical fast-path optimization: on direct
switch (D13), the kernel can leave x0–x3 in the physical registers without
saving or restoring them. Data words pass through from sender to receiver with
zero memory operations — saving ~40–60 cycles per crossing (~80–120 per
round-trip). On a ~400-cycle baseline, this is a ~20–30% improvement.

This optimization requires the kernel's fast-path assembly to never use x0–x3 as
scratch registers (use x9–x15 instead). This is an invariant that must be
maintained in the exception entry code.

The AAPCS64 "ergonomic advantage" (trivial C wrappers) was found to be a
non-issue: both conventions require the same amount of inline assembly for the
SVC instruction. Rust's `asm!` macro with explicit register operands handles
either layout identically.

Decision: **IPC-optimized, uniform for both families.** Data words in x0–x3.
Metadata in x4–x7. Typed operations reuse the same register positions with
different semantic interpretation: x0–x3 = operation-specific args, x4 =
operation code, x5 = target handle, x6–x7 = additional args or unused.

### Numbering: two-level

D7's split interaction model has two families with different performance
profiles. IPC is hot-path; typed operations are cold-path (D1).

Three options: flat namespace, family-prefixed, two-level.

The two-level scheme exploits the SVC immediate: each IPC operation gets its own
SVC number (SVC #1 = send, #2 = receive, #3 = Call, #4 = ReplyRecv, #5 =
NBSend). Typed operations share SVC #0, with the specific operation encoded in
x4.

This means the kernel can dispatch IPC operations from ESR_EL1 alone — before
reading any GPR. For the fast path, the first branch (IPC vs. typed) is resolved
from the exception syndrome register, which the kernel reads anyway.

Decision: **two-level.** IPC operations encoded as nonzero SVC immediates. Typed
operations use SVC #0 with the operation code in x4.

### Error signaling

Not fully settled by this derivation. The IPC-optimized layout uses all 8
registers for message data on receive. Error signaling for receive requires a
design choice — e.g., x7 (reply handle position) is unused on plain receive
success, making it available for error status. For Call, the reply handle is
always valid on success, so x7 = 0 could indicate error. For typed operations
(SVC #0), x0 carries the return value and can serve as error status. Deferred to
implementation.

### Cap-present indicator

D28's "0-1 cap handle" requires the kernel to know whether a cap is present. A
sentinel value in x6 (e.g., u64::MAX = no cap), or a flag bit in x7 (flags
register). Deferred to implementation.

## What this settles

The syscall ABI framework:

1. **Trap mechanism:** SVC #imm16. The operation is encoded in the SVC
   instruction's 16-bit immediate field.
2. **Register convention:** IPC-optimized, uniform. x0–x3 = primary payload
   (data words for IPC, operation-specific args for typed ops). x4 = label (IPC)
   / operation code (typed ops). x5 = field handle (IPC send) / badge (IPC
   receive) / target handle (typed ops). x6 = cap handle / secondary arg. x7 =
   flags (IPC send) / reply handle (IPC receive) / additional arg (typed ops).
3. **Numbering:** Two-level. IPC operations are nonzero SVC immediates (SVC
   #1–#N). Typed kernel operations are SVC #0 with the operation code in x4.

The kernel's dispatch logic:

1. Read ESR_EL1[15:0] (the SVC immediate).
2. If nonzero → IPC. The immediate identifies the specific IPC operation. x0–x7
   carry the message per the register convention above.
3. If zero → typed operation. x4 = operation code, x5 = target handle, x0–x3 =
   operation-specific arguments.

The fast-path optimization: on direct switch (D13, receiver waiting on same
core), x0–x3 pass through in physical registers without save/restore. The
kernel's fast-path code uses only x4–x15 and system registers.

## What this does NOT settle

- Error signaling convention (how failed receive/typed ops report errors).
- Cap-present indicator (sentinel value vs. flag bit).
- Specific SVC numbers for each IPC operation (the assignment #1 = send, etc. is
  illustrative, not binding).
- Typed operation code assignments within x4.
- Large return value convention (e.g., read_registers returning a buffer).
- IPC fast-path conditions (when direct switch occurs — separate from ABI).

## Status

Settled.
