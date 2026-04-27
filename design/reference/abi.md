# Syscall ABI Specification

This document specifies the exact register conventions for crossing the EL0/EL1
boundary. It is the reference for anyone writing a language binding, assembly
shim, or userspace test against this kernel.

Derivation trail: D47 (trap mechanism, register convention, two-level
numbering), D48 (operation enumeration), D49 (error signaling, cap-present
sentinel, SVC/operation-code assignments), D74 (register save/restore flow), D84
(EL0 exception entry mechanics), D85 (context switch restore sequence).

Source of truth: `src/syscall.rs` (types and encoding constants),
`src/frame/arch/aarch64/exception.S` (assembly entry/exit), `src/frame/cores.rs`
(register read/write helpers), `src/frame/arch/aarch64/register_state.rs`
(RegisterState layout).

---

## 1. Trap Mechanism

Userspace invokes the kernel with the ARM64 `SVC` (Supervisor Call) instruction.
The 16-bit immediate field embedded in the instruction selects the operation
class:

| SVC immediate | Operation class                               |
| ------------- | --------------------------------------------- |
| `#0`          | Typed kernel operation (operation code in x4) |
| `#1`          | IPC Send                                      |
| `#2`          | IPC Receive                                   |
| `#3`          | IPC Call                                      |
| `#4`          | IPC ReplyRecv                                 |
| `#5`          | Yield                                         |

On `SVC` from EL0, the processor takes a synchronous exception to EL1 through
vector offset `0x400` (Lower EL, AArch64, Synchronous). The kernel reads
`ESR_EL1` to confirm the exception class is `0x15` (SVC from AArch64) and
extracts the immediate from `ESR_EL1[15:0]`.

**Dispatch rule:** if the immediate is nonzero, the operation is an IPC syscall
dispatched from `ESR_EL1` alone, before reading any general-purpose register. If
the immediate is zero, the kernel reads `x4` for the typed operation code and
`x5` for the target capability handle (D47).

This two-level scheme means IPC dispatch never touches general-purpose registers
on the decode path, which is critical for the direct-switch fast path where
`x0`-`x3` pass through in physical registers.

---

## 2. IPC Register Layout

### 2.1 Entry Registers (Userspace to Kernel)

All IPC operations (SVC `#1`-`#5`) use the same entry register layout. Registers
not used by a given operation are "don't-care" on entry.

| Register | Field           | Description                                                                                      |
| -------- | --------------- | ------------------------------------------------------------------------------------------------ |
| `x0`     | `data[0]`       | First data word                                                                                  |
| `x1`     | `data[1]`       | Second data word                                                                                 |
| `x2`     | `data[2]`       | Third data word                                                                                  |
| `x3`     | `data[3]`       | Fourth data word                                                                                 |
| `x4`     | `label`         | Message label (arbitrary 64-bit value)                                                           |
| `x5`     | `target_handle` | Field capability handle (target endpoint)                                                        |
| `x6`     | `user_cap`      | User capability handle to transfer, or sentinel `0xFFFF_FFFF_FFFF_FFFF` if absent                |
| `x7`     | `reply_info`    | Send/Call: flags (0). ReplyRecv: send-once reply capability handle from previous receive's `x7`. |

**Per-operation entry register usage:**

| Operation            | Registers used on entry                                                                                                                                                                                 |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Send (SVC `#1`)      | `x0`-`x3`, `x4`, `x5`, `x6`, `x7`                                                                                                                                                                       |
| Receive (SVC `#2`)   | `x5` only; all others are don't-care                                                                                                                                                                    |
| Call (SVC `#3`)      | `x0`-`x3`, `x4`, `x5`, `x6`, `x7`                                                                                                                                                                       |
| ReplyRecv (SVC `#4`) | `x0`-`x3` (reply data), `x4` (reply label), `x5` (receive Field handle for the next receive), `x6` (user cap to include in reply, or sentinel), `x7` (send-once reply cap handle from previous receive) |
| Yield (SVC `#5`)     | None; all registers are don't-care                                                                                                                                                                      |

### 2.2 Exit Registers (Kernel to Userspace)

On IPC return, the register layout carries the received message. Exact contents
depend on the operation and success/failure.

**Successful return** (carry clear):

| Register | Field           | Description                                                                                     |
| -------- | --------------- | ----------------------------------------------------------------------------------------------- |
| `x0`     | `data[0]`       | First data word                                                                                 |
| `x1`     | `data[1]`       | Second data word                                                                                |
| `x2`     | `data[2]`       | Third data word                                                                                 |
| `x3`     | `data[3]`       | Fourth data word                                                                                |
| `x4`     | `label`         | Message label                                                                                   |
| `x5`     | `badge`         | Sender's badge (from capability)                                                                |
| `x6`     | `user_cap_slot` | Slot index of received user capability, or sentinel `0xFFFF_FFFF_FFFF_FFFF` if none transferred |
| `x7`     | `reply_cap`     | Reply capability handle, or sentinel `0xFFFF_FFFF_FFFF_FFFF` if no reply capability             |

**Error return** (carry set):

| Register  | Field        | Description                |
| --------- | ------------ | -------------------------- |
| `x0`      | `error_code` | Error code (see Section 6) |
| `x1`-`x7` | undefined    | Contents are unspecified   |

**Per-operation exit register usage:**

| Operation            | Success return                                                                | Error return                  |
| -------------------- | ----------------------------------------------------------------------------- | ----------------------------- |
| Send (SVC `#1`)      | Carry clear. No data returned (all registers undefined).                      | Carry set. `x0` = error code. |
| Receive (SVC `#2`)   | Carry clear. Full message in `x0`-`x7`.                                       | Carry set. `x0` = error code. |
| Call (SVC `#3`)      | Carry clear. Reply message in `x0`-`x7`. `x7` = sentinel (no reply-to-reply). | Carry set. `x0` = error code. |
| ReplyRecv (SVC `#4`) | Carry clear. Next received message in `x0`-`x7` (same layout as Receive).     | Carry set. `x0` = error code. |
| Yield (SVC `#5`)     | Carry clear (always succeeds). All registers undefined.                       | Never fails.                  |

---

## 3. Typed Operation Register Layout

### 3.1 Entry Registers (Userspace to Kernel)

Typed operations use SVC `#0`. The operation code and target handle occupy fixed
register positions; `x0`-`x3` carry operation-specific arguments.

| Register | Field           | Description                            |
| -------- | --------------- | -------------------------------------- |
| `x0`     | `args[0]`       | Operation-specific argument 0          |
| `x1`     | `args[1]`       | Operation-specific argument 1          |
| `x2`     | `args[2]`       | Operation-specific argument 2          |
| `x3`     | `args[3]`       | Operation-specific argument 3          |
| `x4`     | `op_code`       | Operation code (0-19, see table below) |
| `x5`     | `target_handle` | Capability handle of the target object |

Registers `x6` and `x7` are unused on entry for typed operations.

### 3.2 Operation Code Assignments

Codes are assigned in dense sequential blocks grouped by object type (D49).

| Code | Operation              | Target type      | Arguments                                                        |
| ---- | ---------------------- | ---------------- | ---------------------------------------------------------------- |
| 0    | ObserverResume         | Observer         | (none)                                                           |
| 1    | ObserverInstallCap     | Observer         | `x0` = source handle, `x1` = destination slot                    |
| 2    | ObserverWriteRegisters | Observer         | `x0` = PC, `x1` = SP, `x2` = x0 value, `x3` = PSTATE (NZCV mask) |
| 3    | ObserverReadRegisters  | Observer         | (none)                                                           |
| 4    | ObserverSuspend        | Observer         | (none)                                                           |
| 5    | ObserverChangeHandler  | Observer         | `x0` = new handler Field handle, `x1` = new badge                |
| 6    | ObserverSetScheduling  | Observer         | `x0` = R (rate), `x1` = T (period)                               |
| 7    | Destroy                | Any (generic)    | (none)                                                           |
| 8    | Clone                  | Any (generic)    | (none)                                                           |
| 9    | Close                  | Any (generic)    | (none)                                                           |
| 10   | Mint                   | Any (generic)    | `x0` = rights mask                                               |
| 11   | SpaceSplit             | Space            | `x0` = size in bytes                                             |
| 12   | SpaceMerge             | Space            | `x0` = source Space handle                                       |
| 13   | CreateField            | Space (consumed) | `x0` = capacity hint                                             |
| 14   | FieldSplit             | Field            | (none)                                                           |
| 15   | TimeSplit              | Time             | `x0` = amount                                                    |
| 16   | CreatePulsar           | Space (consumed) | `x0` = handler Field handle, `x1` = badge                        |
| 17   | ClockRead              | Observer (self)  | (none)                                                           |
| 18   | CreateObserver         | Space (consumed) | `x0` = handler Field handle, `x1` = handler badge                |
| 19   | ResourceRequest        | Space            | `x0` = resource type (0 = Space), `x1` = quantity                |

### 3.3 Exit Registers (Kernel to Userspace)

| Register  | Field    | Description                                                       |
| --------- | -------- | ----------------------------------------------------------------- |
| `x0`      | `result` | Non-negative = success value. Negative (bit 63 set) = error code. |
| `x1`-`x3` | varies   | Operation-specific. Undefined unless documented.                  |

**Success return values by operation:**

| Operation              | `x0` on success                    | `x1`-`x3` on success                            |
| ---------------------- | ---------------------------------- | ----------------------------------------------- |
| ObserverResume         | 0                                  | undefined                                       |
| ObserverInstallCap     | 0                                  | undefined                                       |
| ObserverWriteRegisters | 0                                  | undefined                                       |
| ObserverReadRegisters  | PC of target Observer              | `x1` = SP, `x2` = x0, `x3` = PSTATE (NZCV only) |
| ObserverSuspend        | 0                                  | undefined                                       |
| ObserverChangeHandler  | 0                                  | undefined                                       |
| ObserverSetScheduling  | 0                                  | undefined                                       |
| Destroy                | 0                                  | undefined                                       |
| Clone                  | New capability slot index          | undefined                                       |
| Close                  | 0 or backing Space handle          | undefined                                       |
| Mint                   | New capability slot index          | undefined                                       |
| SpaceSplit             | New Space capability slot index    | undefined                                       |
| SpaceMerge             | 0                                  | undefined                                       |
| CreateField            | 0                                  | undefined                                       |
| FieldSplit             | New Field capability slot index    | undefined                                       |
| TimeSplit              | New Time capability slot index     | undefined                                       |
| CreatePulsar           | 0                                  | undefined                                       |
| ClockRead              | Current counter value (ticks)      | undefined                                       |
| CreateObserver         | New Observer capability slot index | undefined                                       |
| ResourceRequest        | New capability slot index          | undefined                                       |

---

## 4. Error Signaling

The kernel uses two distinct error signaling conventions, one for each syscall
family (D49).

### 4.1 IPC Errors: Carry Flag in SPSR

IPC operations signal errors through the ARM64 carry flag (PSTATE.C), which is
stored in `SPSR_EL1` bit 29 and restored to `PSTATE` on `eret`.

- **Carry clear** (PSTATE.C = 0): success. Registers `x0`-`x7` carry message
  data as described in Section 2.2.
- **Carry set** (PSTATE.C = 1): error. `x0` contains the error code. `x1`-`x7`
  are undefined.

The kernel manipulates the carry flag by modifying `RegisterState.pstate` (which
is loaded into `SPSR_EL1` during restore). The relevant bit position is:

```text
SPSR_EL1 bit 29 = NZCV.C (carry flag)

  Bit 31: N (negative)
  Bit 30: Z (zero)
  Bit 29: C (carry)     <-- IPC error indicator
  Bit 28: V (overflow)
```

Implementation: `SPSR_CARRY_BIT = 1 << 29` in `src/frame/cores.rs`.

**Checking carry in userspace (assembly):**

```asm
svc     #1              // Send
b.cs    .error          // Branch if carry set (error)
// ... success path ...
```

**Checking carry in userspace (Rust inline assembly):**

```rust
asm!(
    "svc #1",
    "mrs {result}, NZCV",
    result = out(reg) result,
    // ... register operands ...
);
let is_error = (result & (1 << 29)) != 0;
```

The carry flag is part of NZCV (bits 31:28 of PSTATE). NZCV flags are
caller-saved per AAPCS64, so clobbering them on syscall return is legitimate
(D49).

### 4.2 Typed Operation Errors: Negative x0

Typed operations signal errors through the sign of `x0`:

- **`x0` >= 0** (bit 63 clear): success. The value is the operation's return
  value (a capability slot index, a counter value, or zero for void operations).
- **`x0` < 0** (bit 63 set): error. The value is a negative error code.

**Checking in userspace (assembly):**

```asm
svc     #0              // Typed operation
tbnz    x0, #63, .error // Branch if bit 63 set (negative = error)
// ... success path (x0 = result) ...
```

**Checking in userspace (Rust):**

```rust
let result = typed_syscall(op_code, target, args);
if result.0 < 0 {
    // error: result.0 is the negative error code
} else {
    // success: result.0 is the return value
}
```

### 4.3 Error Code Tables

**Typed operation error codes** (negative `x0` values, from
`SyscallError::error_code()`):

| `x0` value | Constant               | Description                                                                                                       |
| ---------- | ---------------------- | ----------------------------------------------------------------------------------------------------------------- |
| -1         | `InvalidCap`           | Invalid or empty capability handle. Also covers slot-tag mismatch (stale handle to reused slot).                  |
| -2         | `StaleCap`             | Revoked capability (generation mismatch). The object still exists but the caller's access was explicitly revoked. |
| -3         | `NoRight`              | Insufficient rights for this operation.                                                                           |
| -4         | `WrongType`            | Wrong object type for this operation.                                                                             |
| -5         | `QueueFull`            | Field queue is full (Send error-to-sender).                                                                       |
| -6         | `TableFull`            | Observer's capability table is full.                                                                              |
| -7         | `AlreadyConsumed`      | Send-once capability already consumed.                                                                            |
| -8         | `CloneForbidden`       | Clone forbidden for linear types (Time).                                                                          |
| -9         | `InvalidState`         | Invalid state transition for the Observer.                                                                        |
| -10        | `InvalidProfile`       | Invalid scheduling profile (R + T > 128).                                                                         |
| -11        | `ZeroSize`             | Zero-size Space split.                                                                                            |
| -12        | `InsufficientResource` | Insufficient resource for the requested operation.                                                                |
| -13        | `NotAdjacent`          | Merge requires adjacent virtual address Space.                                                                    |

**IPC error codes** (carry set, non-negative value in `x0`, from `SyscallError`
enum discriminant via `error as u64`):

| `x0` value | Constant               | Description                                    |
| ---------- | ---------------------- | ---------------------------------------------- |
| 0          | `InvalidCap`           | Invalid or empty capability handle.            |
| 1          | `StaleCap`             | Revoked capability (generation mismatch).      |
| 2          | `NoRight`              | Insufficient rights for this operation.        |
| 3          | `WrongType`            | Wrong object type for this operation.          |
| 4          | `QueueFull`            | Field queue is full.                           |
| 5          | `TableFull`            | Observer's capability table is full.           |
| 6          | `AlreadyConsumed`      | Send-once capability already consumed.         |
| 7          | `CloneForbidden`       | Clone forbidden for linear types.              |
| 8          | `InvalidState`         | Invalid state transition.                      |
| 9          | `InvalidProfile`       | Invalid scheduling profile.                    |
| 10         | `ZeroSize`             | Zero-size Space split.                         |
| 11         | `InsufficientResource` | Insufficient resource.                         |
| 12         | `NotAdjacent`          | Merge requires adjacent virtual address Space. |

The two families use different numeric encodings for the same logical errors.
For IPC, the carry flag is the primary error indicator -- userspace checks carry
first, then reads `x0` for the specific code. For typed operations, the sign of
`x0` is the indicator (bit 63 set = error).

---

## 5. Capability-Absent Sentinel

The sentinel value `0xFFFF_FFFF_FFFF_FFFF` (`u64::MAX`) means "no capability in
this register position" (D49).

This sentinel is used in three positions:

| Register | Direction | Meaning when sentinel                       |
| -------- | --------- | ------------------------------------------- |
| `x6`     | Entry     | No user capability included in this message |
| `x6`     | Exit      | No user capability was transferred          |
| `x7`     | Exit      | No reply capability (Receive/ReplyRecv)     |

The sentinel is safe because capability table slot indices are small
non-negative integers. The table is bounded by typed-memory backing and cannot
reach `2^64 - 1` slots.

Defined as `CAP_ABSENT` in `src/capability.rs` and `tests/src/lib.rs`.

---

## 6. Register Preservation

### 6.1 Syscall Register Convention

Across a syscall (SVC), the kernel treats registers as follows:

| Registers   | Preservation | Notes                                                                                                     |
| ----------- | ------------ | --------------------------------------------------------------------------------------------------------- |
| `x0`-`x7`   | Caller-saved | Overwritten with syscall return values. Contents on return are defined per-operation (Sections 2.2, 3.3). |
| `x8`        | Callee-saved | Preserved across the syscall.                                                                             |
| `x9`-`x15`  | Callee-saved | Preserved across the syscall.                                                                             |
| `x16`-`x17` | Callee-saved | Preserved across the syscall (intra-procedure-call scratch in AAPCS64, but preserved by the kernel).      |
| `x18`       | Callee-saved | Preserved across the syscall (platform register in AAPCS64, but preserved by the kernel).                 |
| `x19`-`x28` | Callee-saved | Preserved across the syscall.                                                                             |
| `x29` (FP)  | Callee-saved | Preserved across the syscall.                                                                             |
| `x30` (LR)  | Callee-saved | Preserved across the syscall.                                                                             |
| `SP`        | Callee-saved | `SP_EL0` is preserved.                                                                                    |
| `PC`        | Callee-saved | `ELR_EL1` is set to the instruction following the `SVC` (hardware behavior).                              |
| `TPIDR_EL0` | Callee-saved | Preserved across the syscall.                                                                             |
| `q0`-`q31`  | Callee-saved | All FP/SIMD registers are preserved (eager save/restore).                                                 |
| `FPCR`      | Callee-saved | Preserved across the syscall.                                                                             |
| `FPSR`      | Callee-saved | Preserved across the syscall.                                                                             |

**Summary:** the kernel preserves all registers except `x0`-`x7` and the NZCV
condition flags. `x0`-`x7` are the syscall argument/result registers. Everything
else (`x8`-`x30`, `SP_EL0`, `TPIDR_EL0`, all FP/SIMD state) is saved to
`RegisterState` on exception entry and restored before `eret`.

### 6.2 Rationale

The kernel saves and restores the full register context (including `x8`-`x30`
and FP/SIMD) on every EL0 exception, even though AAPCS64 designates `x8`-`x15`
as caller-saved. This is necessary because context switches may occur during any
syscall: the kernel may block the caller (Receive, Call) and resume a different
Observer. When the original Observer is eventually resumed, it must find all its
non-argument registers intact.

---

## 7. SPSR and PSTATE Handling

### 7.1 What SPSR Contains

When an exception is taken from EL0, the processor saves `PSTATE` into
`SPSR_EL1`. The EL0 exception entry assembly then stores `SPSR_EL1` into
`RegisterState.pstate` at byte offset 264.

On `eret`, the processor loads `SPSR_EL1` into `PSTATE`, restoring the EL0
execution state. The kernel writes `RegisterState.pstate` into `SPSR_EL1` during
the restore sequence.

### 7.2 NZCV Flags After Syscall Return

The NZCV condition flags (PSTATE bits 31:28) are **not preserved** across
syscalls. They are explicitly used for error signaling:

- **IPC operations:** the kernel may set or clear the carry flag (bit 29) to
  signal error or success.
- **Typed operations:** NZCV flags are undefined on return (the kernel does not
  modify them, but does not guarantee preservation either).

Userspace must not depend on NZCV values surviving a syscall. This is consistent
with AAPCS64, which classifies NZCV as caller-saved.

### 7.3 PSTATE Bits Preserved

The following PSTATE bits ARE preserved across syscalls (assuming no context
switch to a different Observer modifies them):

| Bit(s) | Field           | Notes                                    |
| ------ | --------------- | ---------------------------------------- |
| 31:28  | NZCV            | NOT preserved (used for error signaling) |
| 9      | DAIF.D (debug)  | Preserved (kernel does not modify)       |
| 7      | DAIF.A (SError) | Preserved (kernel does not modify)       |
| 6      | DAIF.I (IRQ)    | Preserved (always 0 for EL0)             |
| 5      | DAIF.F (FIQ)    | Preserved (kernel does not modify)       |
| 4      | M[4] (nRW)      | Always 0 (AArch64)                       |
| 3:0    | M[3:0] (EL/SP)  | Always 0b0000 (EL0, SP_EL0)              |

In practice, the kernel preserves the full `SPSR_EL1` value except for the carry
flag (bit 29), which it may modify for IPC error signaling. All other NZCV bits
(N, Z, V) are also formally clobbered — userspace should treat all four
condition flags as undefined after any syscall.

---

## 8. RegisterState Memory Layout

The `RegisterState` structure (816 bytes, `#[repr(C)]`) stores the complete EL0
register context. It is the save/restore target for all EL0 exceptions.

| Byte offset | Size (bytes) | Field            | Description                                                                                                              |
| ----------- | ------------ | ---------------- | ------------------------------------------------------------------------------------------------------------------------ |
| 0           | 248 (31x8)   | `gprs[0..31]`    | General-purpose registers `x0`-`x30`. Each 8 bytes. `x0` at offset 0, `x1` at offset 8, ..., `x30` at offset 240.        |
| 248         | 8            | `sp`             | `SP_EL0` (user stack pointer)                                                                                            |
| 256         | 8            | `pc`             | `ELR_EL1` (program counter / resume address)                                                                             |
| 264         | 8            | `pstate`         | `SPSR_EL1` (saved processor state)                                                                                       |
| 272         | 8            | `tpidr`          | `TPIDR_EL0` (thread-local storage pointer)                                                                               |
| 280         | 8            | (padding)        | Alignment padding for 16-byte FP/SIMD block                                                                              |
| 288         | 512 (32x16)  | `fp_regs[0..32]` | FP/SIMD registers `q0`-`q31`. Each 128-bit (16 bytes). `q0` at offset 288, `q1` at offset 304, ..., `q31` at offset 784. |
| 800         | 8            | `fpcr`           | Floating-point control register                                                                                          |
| 808         | 8            | `fpsr`           | Floating-point status register                                                                                           |

Total: 816 bytes. Defined in `src/frame/arch/aarch64/register_state.rs` with
compile-time offset assertions.

The assembly code in `exception.S` uses these offsets as hard-coded immediates.
The offset constants (`RS_GPRS`, `RS_SP`, `RS_PC`, `RS_PSTATE`, `RS_TPIDR`,
`RS_FP_REGS`, `RS_FPCR`, `RS_FPSR`) and compile-time `offset_of!` assertions
ensure the Rust struct layout and assembly immediates agree.

---

## 9. Exception Entry and Exit Sequences

### 9.1 EL0 Exception Entry (Save)

When an exception arrives from EL0, the processor switches to `SP_EL1` (the
kernel stack) and vectors through VBAR_EL1 + `0x400` (synchronous), `0x480`
(IRQ), `0x500` (FIQ), or `0x580` (SError).

The vector entry macro (`VECTOR_ENTRY_EL0` in `exception.S`) performs a
7-instruction bootstrap (D84):

1. `stp x0, x1, [sp, #-16]!` -- park `x0` and `x1` on the kernel stack.
2. `mrs x0, tpidr_el1` -- load the `PerCoreData` pointer from `TPIDR_EL1`.
3. `ldr x0, [x0, #0]` -- dereference `PerCoreData.register_state_ptr` (offset 0)
   to get the `RegisterState` pointer.
4. `str x1, [x0, #8]` -- save user `x1` directly to `RegisterState.gprs[1]`
   (still in `x1`).
5. `ldr x1, [sp], #16` -- recover user `x0` from the kernel stack into `x1`.
6. `str x1, [x0, #0]` -- save user `x0` to `RegisterState.gprs[0]`.
7. `b __el0_trampoline_N` -- branch to the per-source trampoline.

After the trampoline (which saves user `x19` and sets the source identifier),
the common handler `__el0_exception_common` saves the remaining context:

- `x2`-`x30` (skipping `x19`, already saved by the trampoline) to
  `RegisterState.gprs[2..31]`.
- `SP_EL0` via `mrs x9, sp_el0` to `RegisterState.sp` (offset 248).
- `ELR_EL1` via `mrs x9, elr_el1` to `RegisterState.pc` (offset 256).
- `SPSR_EL1` via `mrs x9, spsr_el1` to `RegisterState.pstate` (offset 264).
- `TPIDR_EL0` via `mrs x9, tpidr_el0` to `RegisterState.tpidr` (offset 272).
- `q0`-`q31` to `RegisterState.fp_regs[0..32]` (offsets 288-800).
- `FPCR` and `FPSR` to offsets 800 and 808.

After saving, the assembly reads `ESR_EL1` and `FAR_EL1` into `x1` and `x2`
(passed as function parameters, NOT saved to `RegisterState`), resets `SP` to
the kernel stack top from `PerCoreData.kernel_stack_top` (offset 16), and calls
into the Rust handler:

```text
el0_exception_handler(source: u64, esr: u64, far: u64) -> !
```

This handler is divergent -- it never returns. It calls `__restore_observer` or
`__enter_idle` to exit the kernel.

### 9.2 EL0 Exception Exit (Restore)

The `__restore_observer` function restores an Observer's context and returns to
EL0 via `eret`. It receives three parameters:

- `x0` = pointer to `RegisterState`
- `x1` = `TTBR0_EL1` value (page table root for the target Observer)
- `x2` = `clock_access` (0 or 1, controls `CNTKCTL_EL1` bit 1)

Restore sequence (D85):

1. **Conditional TTBR0 switch.** Compare current `TTBR0_EL1` with `x1`. If
   different: `msr ttbr0_el1, x1; isb; tlbi vmalle1is; dsb ish; isb`. If same:
   skip (saves approximately 40-80 cycles).

2. **CNTKCTL_EL1 update.** Branchless bit-field insert: `bfi x9, x2, #1, #1`
   copies the `clock_access` bit into `CNTKCTL_EL1[1]` (EL0 virtual counter
   access). No barrier needed -- `eret` is a context synchronization event.

3. **System register restore.** From `RegisterState`:
   - `SP_EL0` from offset 248
   - `ELR_EL1` from offset 256
   - `SPSR_EL1` from offset 264
   - `TPIDR_EL0` from offset 272

4. **FP/SIMD restore.** `FPCR` and `FPSR` from offsets 800 and 808, then
   `q0`-`q31` from offsets 288-800.

5. **GPR restore.** `x2`-`x30` from `RegisterState`. `x1` loaded second-to-last.
   `x0` loaded last (it is the base pointer and must remain valid until the
   final load).

6. **`eret`.** The processor loads `SPSR_EL1` into `PSTATE` and `ELR_EL1` into
   `PC`, returning to EL0. `eret` is a context synchronization event (ARM
   Architecture Reference Manual D1.13.4) -- all preceding system register
   writes take effect before EL0 execution begins.

---

## 10. Fast Path Conditions

The IPC fast path (D50) enables direct context switch from sender to receiver
when all of the following conditions are met:

1. **Operation is Call or ReplyRecv.** Send, Receive, and Yield are not
   eligible. Call and ReplyRecv are eligible because the sender voluntarily
   blocks (Send is fire-and-forget; the sender continues).

2. **A receiver is waiting on the target Field.** A blocked Observer must be in
   the Field's wait queue.

3. **Receiver is on the same core.** Cross-core direct switch is not supported
   (would require IPI).

4. **Scheduler approves the switch.** The scheduler's `should_switch_to`
   callback must return true.

5. **Zero user capabilities in the message.** The `x6` register must contain the
   cap-absent sentinel (`0xFFFF_FFFF_FFFF_FFFF`). Capability transfer requires
   table manipulation that is incompatible with the fast path.

When all conditions are met, the kernel returns
`DispatchResult::ResumeFastPath`. The restore path then loads `x4`-`x30` (and
all system registers and FP/SIMD state) from the receiver's `RegisterState`, but
skips loading `x0`-`x3`. The data words pass through in physical registers from
sender to receiver with zero memory operations on the data-word path, saving
approximately 40-60 cycles per crossing.

The save path always writes `x0`-`x3` to the outgoing Observer's `RegisterState`
(D74: RegisterState is always correct). The optimization is restore-side only.

From the ABI perspective, fast path versus slow path is invisible to userspace.
The register contents on return are identical either way. The distinction
affects only performance.

---

## 11. PerCoreData Layout

The kernel stores a pointer to a `PerCoreData` structure in `TPIDR_EL1`. The
assembly exception entry code reads this to locate the current Observer's
`RegisterState`.

| Byte offset | Size (bytes) | Field                | Description                                                                     |
| ----------- | ------------ | -------------------- | ------------------------------------------------------------------------------- |
| 0           | 8            | `register_state_ptr` | Pointer to current Observer's `RegisterState`. Updated on every context switch. |
| 8           | 8            | `core_state_ptr`     | Type-erased pointer to `CoreState<S>`. Used by the Rust handler.                |
| 16          | 8            | `kernel_stack_top`   | Top of the kernel stack for this core. Set once at boot.                        |

Total: 24 bytes. `#[repr(C)]` with compile-time offset and size assertions.

---

## 12. Assembly Examples

### 12.1 Typed Syscall: SpaceSplit

```asm
    mov     x4, #11            // op_code: SpaceSplit
    mov     x5, #3             // target_handle: root Space at slot 3
    mov     x0, #4096          // args[0] = size in bytes
    mov     x1, #0             // args[1] (unused)
    mov     x2, #0             // args[2] (unused)
    mov     x3, #0             // args[3] (unused)
    svc     #0                 // typed syscall
    tbnz    x0, #63, .error    // branch if negative (error)
    // success: x0 = new Space capability slot index
```

### 12.2 IPC: Send + Receive

```asm
    // Send
    movz    x0, #0xBEEF        // data[0]
    movz    x1, #0xCAFE        // data[1]
    movz    x2, #0xDEAD        // data[2]
    movz    x3, #0xF00D        // data[3]
    mov     x4, #0x42          // label
    mov     x5, x19            // Field handle (previously obtained)
    movn    x6, #0             // user cap = CAP_ABSENT (0xFFFF_FFFF_FFFF_FFFF)
    mov     x7, #0             // reply_info = 0
    svc     #1                 // Send
    b.cs    .send_error        // branch if carry set (error)

    // Receive
    mov     x5, x19            // Field handle
    svc     #2                 // Receive (blocks until message available)
    b.cs    .recv_error        // branch if carry set (error)
    // success: x0-x3 = data, x4 = label, x5 = badge
```

### 12.3 IPC: Yield

```asm
    svc     #5                  // Yield (always succeeds)
    // execution continues here after the kernel returns
```

### 12.4 Typed Syscall from Rust

```rust
pub fn typed_syscall(op_code: u16, target: u64, args: [u64; 4]) -> i64 {
    let result: u64;
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x0") args[0],
            in("x1") args[1],
            in("x2") args[2],
            in("x3") args[3],
            in("x4") op_code as u64,
            in("x5") target,
            lateout("x0") result,
            lateout("x1") _,
            lateout("x2") _,
            lateout("x3") _,
            lateout("x6") _,
            lateout("x7") _,
        );
    }
    result as i64
}
```

### 12.5 IPC Send from Rust

```rust
pub fn send(handle: u64, label: u64, data: [u64; 4]) -> bool {
    let nzcv: u64;
    unsafe {
        core::arch::asm!(
            "svc #1",
            "mrs {nzcv}, NZCV",
            nzcv = out(reg) nzcv,
            in("x0") data[0],
            in("x1") data[1],
            in("x2") data[2],
            in("x3") data[3],
            in("x4") label,
            in("x5") handle,
            in("x6") u64::MAX,  // CAP_ABSENT
            in("x7") 0u64,
        );
    }
    // Carry is bit 29 of NZCV. Clear = success.
    (nzcv & (1 << 29)) == 0
}
```

---

## 13. Derivation Index

| Derivation | Topic                                                                 |
| ---------- | --------------------------------------------------------------------- |
| D47        | Trap mechanism, register convention, two-level numbering              |
| D48        | Operation enumeration (5 IPC + 20 typed = 25)                         |
| D49        | Error signaling, cap-present sentinel, SVC/operation-code assignments |
| D50        | IPC fast-path conditions (Call/ReplyRecv direct switch)               |
| D74        | Register save/restore flow (direct-to-RegisterState)                  |
| D76        | Pull model for reads, push model for result writes                    |
| D83        | PerCoreData at TPIDR_EL1                                              |
| D84        | EL0 exception entry mechanics (7-instruction bootstrap)               |
| D85        | Context switch restore sequence                                       |
| D103       | WriteRegisters/ReadRegisters inline format                            |
