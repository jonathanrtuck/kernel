# D85 — Context switch restore sequence

**Question:** What is the correct ARM64 instruction sequence to restore an
Observer's RegisterState and return to EL0 via eret?

**Rests on:** D74 (register save/restore flow), D66 (clock access via
CNTKCTL_EL1), D83 (PerCoreData), ARM64 architecture (D1.13, D8.14).

**Status:** settled.

---

## Settles

### TTBR0_EL1 switch sequence (corrected)

The implementation plan specified `dsb ish; isb; tlbi vmalle1is; dsb ish; isb`.
The leading `dsb ish` is unnecessary — it ensures prior stores are visible, but
the restore path hasn't written page table entries. The correct minimal sequence
after writing TTBR0:

```asm
msr   ttbr0_el1, xN      // Write new page table root
isb                        // Context-sync so TLBI sees the new TTBR0
tlbi  vmalle1is            // Invalidate all EL0/EL1 TLB entries, inner-shareable
dsb   ish                  // Wait for TLBI to complete on all cores
isb                        // Sync instruction stream for new translations
```

Role of each instruction:

- **isb (first):** ARM ARM requires a context synchronization event between an
  `msr` to a translation register and a subsequent TLBI that depends on it.
  Without this, the TLBI might execute against the old TTBR0 value.
- **tlbi vmalle1is:** Conservative full invalidation. Correct by construction —
  no stale entry survives. ASID-tagged TLB entries are a future leaf-node
  optimization (no interface changes needed).
- **dsb ish:** TLBI completion is asynchronous. The DSB waits until all cores in
  the inner-shareable domain have completed the invalidation.
- **isb (second):** Ensures the pipeline refetches instructions with the new
  translations active.

### TTBR0 comparison skip

Same-space context switches (two Observers sharing a Space) can skip the entire
TTBR0/TLB sequence by comparing the current and target values:

```asm
mrs   x9, ttbr0_el1
cmp   x9, xN
b.eq  .skip_ttbr_switch
// ... msr + isb + tlbi + dsb + isb ...
.skip_ttbr_switch:
```

Comparing the full 64-bit TTBR0 value is safe: ASID bits are zero (no ASIDs
yet), and the physical address of the page table root is the meaningful content.
When ASIDs are introduced, same-space Observers share both PA and ASID, so the
comparison remains valid.

Savings: ~40-80 cycles (the TLB invalidation) vs ~5 cycles (comparison +
branch).

### CNTKCTL_EL1: branchless clock access control

D66 settles that `clock_access: bool` on Observer controls EL0VCTEN (bit 1) of
CNTKCTL_EL1. The branchless pattern:

```asm
mrs   x9, cntkctl_el1
bfi   x9, x2, #1, #1       // Bit-field insert: CNTKCTL[1] = clock_access[0]
msr   cntkctl_el1, x9
```

Three instructions, no branch. The `bfi` copies bit 0 of x2 (the clock_access
argument, 0 or 1) into bit 1 of x9.

No barrier needed after the `msr` — `eret` is a context synchronization event
(ARM ARM D1.13.4) and ensures the CNTKCTL change takes effect before EL0
execution begins.

CNTKCTL_EL1 is per-core. Changing it on one core has no effect on other cores.
It controls only EL0 access to the counter — the timer interrupt continues to
fire regardless of this setting.

**Unverified:** Apple HVF treatment of `msr cntkctl_el1`. If HVF traps this
instruction, there will be a hypervisor exit cost. Must be tested on the
hypervisor runner.

### Register restore ordering before eret

The restore path receives the RegisterState pointer as a function argument (x0).
Ordering:

1. **TTBR0 switch** (if needed) — uses scratch registers
2. **CNTKCTL_EL1** — uses scratch register
3. **System registers from RegisterState** — SP_EL0, ELR_EL1, SPSR_EL1,
   TPIDR_EL0, using x9 as scratch, x0 as base
4. **FP/SIMD** — FPCR, FPSR via x9 scratch, then q0-q31 via `ldp` (does not
   consume GPRs)
5. **GPRs** — x2-x30 from RegisterState, x1 second-to-last, x0 last (base
   pointer consumed)
6. **eret**

Hard constraints:

- **x0 (base pointer) loaded last.** All other loads reference `[x0, #offset]`.
- **System registers before GPR restore.** They need a scratch register for
  `msr`.
- **SPSR_EL1 and ELR_EL1 may be loaded at any point** — no ISB needed between
  `msr spsr_el1/elr_el1` and `eret`, because eret is itself a context
  synchronization event.

No constraint between SP_EL0, TPIDR_EL0, SPSR_EL1, and ELR_EL1 writes — they are
independent system register writes with no inter-dependencies.

### Idle path: WFI at EL1

When `DispatchResult::Idle` is returned (no runnable Observer):

```asm
__idle_loop:
    msr   daifclr, #2        // Enable IRQs (clear PSTATE.I)
    wfi                       // Wait for interrupt
    b     __idle_loop         // Re-enter (IRQ handler runs before we reach here)
```

No EL0 context setup needed — WFI executes at EL1. When an IRQ fires:

1. CPU takes exception through EL1h vector (source 5, current EL SP_EL1).
2. Existing TrapFrame save/restore handles the nested exception.
3. `eret` returns to the instruction after `wfi` in the idle loop.
4. The idle loop calls back into Rust (`schedule_next()`) to check for runnable
   work. If found, exits to the restore path. If not, re-enters WFI.

TTBR0_EL1 can be left pointing to the last Observer's page table — EL1 does not
use TTBR0 for its own accesses, and speculative TTBR0 walks hit a valid table.

### eret is a context synchronization event

ARM ARM D1.13.4: "An exception return from ELx is a Context synchronization
event." This means:

- All system register writes take effect before EL0 execution begins.
- The instruction stream at EL0 is fetched fresh.
- Speculative operations are resolved or discarded.
- No ISB needed between the last `msr` and `eret`.

On Apple Silicon (CSV3): no speculation barrier needed before eret. CSV3
guarantees cross-context speculation cannot disclose data.

Porting note for non-CSV3 hardware: branch predictor invalidation should be
added before eret (`IC IALLU; ISB` or implementation-specific sequence).

## Rejected alternatives

**Leading `dsb ish` before TLBI in the TTBR0 switch.** Unnecessary — the restore
path hasn't written page table entries. The `isb` between `msr ttbr0` and `tlbi`
is sufficient.

**Branching CNTKCTL pattern.** `tbz/orr/bic` with conditional branch. Works but
has a branch mispredict penalty (~10 cycles on Apple Silicon). The `bfi` pattern
is branchless and one instruction shorter.

**Separate context_switch.S file.** Considered for merge-conflict avoidance with
entry-path work, but the restore path is a single function
(`__restore_observer`) that naturally lives in exception.S alongside the entry
code. If Wave 2 agents conflict, the file can be split then.

## Reference check

Corrects implementation plan Task 2.4: TTBR0 switch sequence drops the leading
`dsb ish`. All other details match. The TTBR0 comparison skip is an addition
(optimization) not in the plan.
