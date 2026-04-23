# Syscall ABI: Trap Mechanism, Register Convention, and Operation Numbering

How do real kernels bridge userspace and kernel at the instruction level? This
document covers the ARM64 trap mechanism, register layout, and operation
identification — the wire protocol beneath the syscall interface, not the
interface itself.

---

## 1. The Question

Three separable sub-questions:

1. **Trap mechanism** — what instruction crosses the privilege boundary, and how
   does the hardware encode context for the handler?
2. **Register convention** — which registers carry arguments, return values,
   error codes, and the operation identifier?
3. **Operation numbering** — how does the kernel identify which operation to
   dispatch, and what numbering or encoding scheme do real systems use?

---

## 2. ARM64 Trap Mechanism: SVC Instruction

### Hardware behavior

On AArch64, `SVC #<imm16>` (Supervisor Call) is the instruction that causes a
synchronous exception, raising execution from EL0 to EL1. The hardware:

- Saves PC, PSTATE to ELR_EL1, SPSR_EL1
- Sets the exception vector base (VBAR_EL1) + offset for the SVC handler
- Records the 16-bit immediate in `ESR_EL1.ISS` (bits [15:0])
- Does **not** touch general-purpose registers (x0–x30, SP, PC remain unchanged
  except as modified by the handler)

The immediate can range from 0 to 65535. The handler can read it from
`ESR_EL1.ISS`, but this requires an MRS instruction with a data dependency
before any register work — slower than simply reading a register that was loaded
before the SVC.

**Source:** ARM Architecture Reference Manual, DDI 0487, Section D1.10.2 "SVC
exception".

### Practical use of the immediate

Most kernels use `svc #0` uniformly and carry the operation identifier in a
general-purpose register. Exceptions and rationale:

| Kernel      | SVC immediate | Why                                                     |
| ----------- | ------------- | ------------------------------------------------------- |
| Linux       | `svc #0`      | Operation in x8; immediate always ignored               |
| seL4        | `svc #0`      | Operation in x7; immediate always ignored               |
| XNU (macOS) | `svc #0x80`   | Convention only; value is not decoded; operation in x16 |
| Zircon      | Not in ABI    | vDSO mediates all kernel entry; raw SVC not exposed     |

XNU's `svc #0x80` appears to originate from the ARM32 legacy where the SWI
immediate was meaningful, preserved as a convention with no functional effect in
AArch64.

A non-zero immediate _could_ encode the syscall number (up to 65535 operations),
which would save one register load. No surveyed production kernel uses this
approach, likely because reading ESR_EL1 adds a pipeline hazard and the
immediate encoding offers no advantage over a register.

---

## 3. Register Conventions

### 3.1 Linux ARM64

Defined by the ARM64 Linux ABI, stable since the kernel's ARM64 introduction
(~3.7, 2012).

| Register | Role              | Notes                                          |
| -------- | ----------------- | ---------------------------------------------- |
| x0–x5    | Arguments 1–6     | Per AAPCS64 but only first 6 used for syscalls |
| x8       | Syscall number    | Distinguishes this from function call conv     |
| x0       | Return value      | Negative → errno (−1 to −4095 range)           |
| x1       | 2nd return (rare) | Only for a few calls (e.g., pipe2 uses x1)     |

x6 and x7 are caller-saved but not used by the kernel for syscall arguments,
preserving them for library wrappers to use freely.

**Error convention:** Return value in x0 in range [−4095, −1] is interpreted as
`-errno` by the C library. The kernel returns the actual −errno value directly;
glibc negates it, sets `errno`, and returns −1 to the caller. This means the
kernel's error domain is 12 bits of errno codes.

### 3.2 XNU (macOS) ARM64

| Register | Role           | Notes                                      |
| -------- | -------------- | ------------------------------------------ |
| x0–x7    | Arguments 1–8  | Up to 8 arguments                          |
| x16      | Syscall number | Positive = BSD/POSIX, negative = Mach trap |
| x0       | Return value   | Carry flag in CPSR indicates error         |

The sign-encoding in x16 bifurcates the two subsystems within XNU:

- **Positive numbers** → BSD layer syscall table (POSIX-compatible)
- **Negative numbers** → Mach trap table (IPC, task management, VM)

The CPSR carry flag as error indicator differs from Linux's negative-errno
approach. The C library checks the carry flag after SVC and sets errno if set.

**Source:** TheAppleWiki "Kernel Syscalls"; HackTricks ARM64 assembly
documentation.

### 3.3 seL4 ARM64

The seL4 ARM64 ABI is designed around IPC as the dominant operation. The
register layout reflects this: IPC data occupies x0–x5, with x7 reserved for the
(small, enumerated) syscall number.

| Register | Role                 | Notes                                                      |
| -------- | -------------------- | ---------------------------------------------------------- |
| x0       | Dest cap / badge out | Input: capability index; output: badge received            |
| x1       | Message info         | Encodes: length, caps transferred, label (see §4.3 below)  |
| x2–x5    | MR0–MR3              | Inline message data (4 message registers, 64 bits each)    |
| x6       | Reply cap            | MCS kernel only                                            |
| x7       | Syscall number       | Enum with 8 values (Send=1, NBSend=2, Recv=3, ... Yield=8) |

Output registers after return:

| Register | Role                                     |
| -------- | ---------------------------------------- |
| x0       | Badge received (for Recv/ReplyRecv)      |
| x1       | Message info returned (with error flags) |
| x2–x5    | MR0–MR3 received                         |

**Instruction:** `svc #0` (immediate ignored).

**Source:** seL4 GitHub,
`libsel4/sel4_arch_include/aarch64/sel4/sel4_arch/syscalls.h`. seL4 Reference
Manual v14.0.0.

The choice of x7 (not x8 as Linux uses) leaves x0–x6 as a contiguous IPC payload
region. With 8 distinct syscall values, the number fits in 4 bits; x7's full
64-bit width is unused.

---

## 4. Operation Numbering Schemes

### 4.1 Flat sequential table (Linux, Zircon)

Each operation has a unique integer. The syscall dispatch is a table lookup.

**Linux:** Numbers allocated sequentially, never reused, never removed
(compatibility guarantee). As of kernel 6.x: ~450 entries on ARM64. The table is
architecture-specific — ARM64's `syscall_64.tbl` differs from x86_64's but
maintains semantic equivalence where possible.

Gaps exist: some numbers are reserved or architecture-specific. ARM64-specific
syscalls (e.g., `syslog`) occupy different numbers than on x86_64.

**Zircon:** Numbers are assigned sequentially and generated from `.fidl`
interface definition files, compiled into a `syscalls.inc` header consumed by
both kernel and vDSO. As of current Fuchsia: ~170 entries. The target was ~100
at launch; the number grew.

However, Zircon differs crucially: the SVC instruction is **not in the ABI**.
Only calling through libzircon.so (the vDSO) is valid. This means the raw
syscall number→register mapping is an internal implementation detail, not a
stability contract. The stable ABI is the C function signatures of the vDSO.

**Source:** Fuchsia documentation "Zircon vDSO";
`zircon/kernel/lib/userabi/vdso/`.

### 4.2 Capability-typed invocation (seL4, Coyotos)

The syscall number identifies the _communication primitive_ (send, receive,
call, reply), not the _operation_. The actual operation is identified by the
**invocation label**, encoded in the high bits of the message info word (x1 in
seL4's ARM64 ABI).

**seL4 split:**

- **8 raw syscalls** (enum): Send, NBSend, Recv, NBRecv, Call, Reply, ReplyRecv,
  Yield — identified by x7
- **~60 capability methods** (invocation labels): CNode.Copy, TCB.Configure,
  Untyped.Retype, IRQHandler.Ack, etc. — identified by the label field of the
  message info word, carried in x1

The message info word (seL4_MessageInfo_t) packs:

- `label` [63:12] — invocation label (identifies the method)
- `capsUnwrapped` [11:9] — number of capabilities unwrapped
- `extraCaps` [8:7] — number of extra capability slots
- `length` [6:0] — number of message registers used

So: `svc #0` with x7=`seL4_SysCall` (6) and
x1=`seL4_MessageInfo{label=TCBConfigure}` invokes TCB.Configure. There is no
separate syscall for every operation type.

**Coyotos:** 3 syscalls (InvokeCap, CopyCap, Yield). All kernel operations other
than Yield and capability copy go through InvokeCap. The capability type at
invocation determines what happens.

**Source:** seL4 Reference Manual v14.0.0, Chapter 4 "Kernel Services and
Objects"; `libsel4/include/sel4/types.h` (seL4_MessageInfo_t definition).

### 4.3 Sign-encoded dispatch (XNU)

The syscall number's sign encodes which subsystem handles the call:

- **Positive** → BSD layer (numbers ≥ 0)
- **Negative** → Mach layer (numbers < 0)

Within each subsystem, numbers are sequential. There is no encoding of operation
type within the number itself — each operation has a unique number in its table.

**Source:** XNU source, `bsd/kern/syscalls.master` (BSD table);
`osfmk/mach/mach_traps.h` (Mach trap table).

### 4.4 Two-tier kernel/user privilege (Genode/base-hw)

Genode's base-hw kernel maintains a split:

- **Public syscalls** (all threads): ~12 operations (IPC, yield, signals)
- **Core-private syscalls** (root component only): ~15 operations (thread/PD
  management)

Both sets use a flat number space but the kernel checks privilege before
dispatching to core-private numbers. Privilege is determined by the thread's
TCB, not by a separate instruction.

**Source:** Genode Foundations documentation, "Execution on bare hardware
(base-hw)".

---

## 5. Error Return Conventions

Error encoding is orthogonal to argument passing but shapes the ABI:

| Kernel | Error mechanism                                                                            |
| ------ | ------------------------------------------------------------------------------------------ |
| Linux  | Return value in x0 ∈ [−4095, −1] → `−errno`; glibc inverts                                 |
| XNU    | Carry flag in CPSR; x0 contains errno if carry set                                         |
| seL4   | `seL4_MessageInfo.label` = 0 on success, nonzero on error; x0 returns badge; no errno      |
| Zircon | Return type is `zx_status_t` (signed 32-bit); ZX_OK=0, errors are negative named constants |
| Genode | Return value convention per syscall; no unified error domain                               |

seL4's approach has no errno mapping at all — errors are either kernel panics
(on policy violations) or a return in the info word label field.

---

## 6. SVC Immediate: Alternative Uses Surveyed

While no production kernel uses the SVC immediate for operation numbering on
ARM64, several approaches have been discussed or implemented in research:

- **Immediate = class, register = method:** Use the 16-bit immediate to identify
  a coarse operation class (IPC=1, memory=2, etc.), with a register for the
  specific method. This would speed dispatch by avoiding a full table lookup.
  Not deployed in any surveyed kernel.
- **All operations in immediate:** With 16 bits, 65536 distinct operations are
  possible. A kernel with ≤65535 operations could encode everything in SVC,
  eliminating one register load. The cost is ESR_EL1 read latency. Not deployed.
- **Immediate as security check:** Use the immediate to encode a "call tag" that
  the kernel verifies — a defense against syscall confusion attacks. Not
  deployed; typically addressed by seccomp-style filtering.

The ARM Architecture Reference Manual notes that the immediate is "for use by
the OS" without specifying its purpose, explicitly leaving the encoding to the
OS designer.

---

## 7. Message Registers vs. IPC Buffer

Two approaches to passing data beyond the register count:

**Register-only (seL4, L4 fast path):** The kernel defines a fixed number of
"message registers" mapped to physical registers (x2–x5 in seL4 ARM64: 4
registers × 8 bytes = 32 bytes of inline payload). Additional data requires a
separate shared-memory IPC buffer (seL4_IPCBuffer), mapped at a well-known
address in the thread's address space. The kernel copies the IPC buffer on
slow-path IPC.

**UTCB-based (L4/Fiasco.OC, L4Ka::Pistachio):** The User Thread Control Block
(UTCB) is a kernel-mapped region in each thread's address space. All message
registers are virtual — the "fast path" maps virtual registers to physical
registers for small messages; larger messages spill to the UTCB. The L4 X.2 spec
defines 64 virtual registers (MR0–MR63) with the first ~8 mapped to hardware
registers.

**POSIX-style (Linux, Zircon):** No "message registers" — data is always a
userspace pointer. The kernel copies data from user address space for small
payloads (write, ioctl) or maps pages (mmap, VMO). No convention for inline
payload beyond 8 arguments.

---

## 8. Benchmark Data

IPC round-trip measurements are the primary ABI performance metric, since IPC
dominates microkernel workloads:

| System          | IPC round-trip | Year | Notes                            |
| --------------- | -------------- | ---- | -------------------------------- |
| Mach            | ~100 µs        | 1990 | Async buffered, memory copy      |
| L4/x86          | ~5 µs          | 1993 | Synchronous, register-based      |
| L4Ka::Pistachio | ~0.5 µs        | 2003 | UTCB fast path                   |
| seL4/ARM64      | ~0.2 µs        | 2013 | Register IPC fast path           |
| seL4/ARM64 MCS  | ~0.25 µs       | 2020 | Slightly higher due to reply cap |

(Source: Elphinstone & Heiser, "From L3 to seL4: What Have We Learnt in 20 Years
of L4 Microkernels?", SOSP 2013.)

Linux `getpid` (trivial syscall) on ARM64: ~80 ns via vDSO (user-level),
~200–300 ns for full kernel entry — provided for comparison.

---

## 9. Summary of Tradeoffs

### SVC immediate vs. register for syscall number

| Approach               | Tradeoffs                                                                                                                               |
| ---------------------- | --------------------------------------------------------------------------------------------------------------------------------------- |
| Register (x7, x8, x16) | Fast to decode (register already in flight); wastes one register; requires standardizing which register                                 |
| SVC immediate          | Saves a register; requires reading ESR_EL1 (adds latency, pipeline stall); limits to 65536 operations                                   |
| vDSO abstraction       | Stable ABI above instruction level; allows changing raw mechanism; adds indirection; cannot be used in interrupt handlers or early boot |

### Operation identification

| Approach                                      | Tradeoffs                                                                                                                |
| --------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| Flat sequential table (Linux, Zircon)         | Simple dispatch; numbers accumulate; gaps for removed syscalls; architecture-specific numbering                          |
| Capability-typed with invocation label (seL4) | Small raw syscall count; dispatch involves decoding message info; uniform interface for kernel and user-defined services |
| Sign-encoded (XNU)                            | Two subsystems in one number space; simple but non-extensible                                                            |
| Privilege-split table (Genode)                | Clean security boundary; privilege check adds path                                                                       |

### Error encoding

| Approach                | Tradeoffs                                                                                    |
| ----------------------- | -------------------------------------------------------------------------------------------- |
| Negative return (Linux) | Simple; poisons one return value range; requires glibc wrapper to invert                     |
| Carry flag (XNU)        | Doesn't consume return register range; requires checking CPSR; unusual                       |
| Status type (Zircon)    | Named constants; explicit; return value separate from error; requires 2 outputs or wide type |
| Info word label (seL4)  | Orthogonal to data; no errno; maps naturally to capability model                             |

---

## References

### Primary Sources

- ARM Architecture Reference Manual (DDI 0487), Section D1.10.2 — SVC exception
  behavior, ESR_EL1.ISS encoding
- seL4 GitHub: `libsel4/sel4_arch_include/aarch64/sel4/sel4_arch/syscalls.h` —
  ARM64 register layout
  https://github.com/seL4/seL4/blob/master/libsel4/sel4_arch_include/aarch64/sel4/sel4_arch/syscalls.h
- seL4 Reference Manual v14.0.0 — Chapter 4 (Kernel Services), Appendix A (ABI)
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- Linux kernel ARM64 syscall table: `arch/arm64/include/asm/unistd.h`,
  `include/uapi/asm-generic/unistd.h`
  https://github.com/torvalds/linux/blob/master/arch/arm64/include/asm/unistd.h
- Linux `syscall(2)` man page — tabulates per-architecture register conventions
  https://man7.org/linux/man-pages/man2/syscall.2.html
- XNU `bsd/kern/syscalls.master` — BSD syscall table with numbers
  https://github.com/apple/darwin-xnu/blob/main/bsd/kern/syscalls.master
- XNU `osfmk/mach/mach_traps.h` — Mach trap table
  https://github.com/apple/darwin-xnu/blob/main/osfmk/mach/mach_traps.h
- Fuchsia vDSO documentation
  https://fuchsia.dev/fuchsia-src/concepts/kernel/vdso
- Zircon ARM64 vDSO assembly:
  `zircon/kernel/lib/userabi/vdso/zx_futex_wake_handle_close_thread_exit-arm64.S`
  https://fuchsia.googlesource.com/fuchsia/+/refs/heads/main/zircon/kernel/lib/userabi/vdso/
- TheAppleWiki "Kernel Syscalls" — x16 register convention, sign encoding
  https://theapplewiki.com/wiki/Kernel_Syscalls
- ARM64 System calls reference (duetorun.com)
  https://duetorun.com/blog/20230604/a64-svc/
- AAPCS64 Procedure Call Standard (ARM-software/abi-aa)
  https://github.com/ARM-software/abi-aa/blob/main/aapcs64/aapcs64.rst
- Genode Foundations — base-hw kernel syscall interface
  https://genode.org/documentation/genode-foundations/19.05/under_the_hood/Execution_on_bare_hardware_(base-hw).html

### Papers

- Elphinstone, K. and Heiser, G. "From L3 to seL4: What Have We Learnt in 20
  Years of L4 Microkernels?" SOSP 2013.
  https://sigops.org/s/conferences/sosp/2013/papers/p133-elphinstone.pdf (IPC
  benchmark data in §5)
- Liedtke, J. "On µ-Kernel Construction." SOSP 1995. Established register-based
  IPC as the defining performance constraint for microkernel viability.
  https://dl.acm.org/doi/10.1145/224056.224075

### Gaps and limitations

- Zircon ARM64 raw syscall register assignment: the ARM64 `syscall.S` source
  returns 404 at time of writing (file may have moved); the convention is
  inferred from vDSO assembly fragments and Fuchsia documentation. The raw
  mapping is not a stability contract.
- L4Ka::Pistachio ARM ABI: not found in a single authoritative reference. The
  UTCB-based virtual register model means the register↔MR mapping is
  architecture-specific and not standardized across the L4 family.
- seL4 on AArch32 uses a different register mapping (see the aarch32
  `syscalls.h`); this document covers AArch64 only.
