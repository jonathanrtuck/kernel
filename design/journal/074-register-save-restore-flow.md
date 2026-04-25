# 074 — Register save/restore flow: direct-to-RegisterState on EL0, TrapFrame on EL1h

2026-04-25. Starting from the gap identified in D43's downstream: "register save
area layout within structural backing" is settled (register_state.rs, 816
bytes), but the flow connecting TrapFrame (exception entry) to RegisterState
(persistent per-Observer storage) is unexplored. D47's x0–x3 pass-through
optimization, D50's fast-path conditions, and D69's DAIF masking all place
constraints on this flow.

Parent decisions settled: D1 (per-core hot path), D6 (single execution unit),
D43 (Observer schema — register save pointer in metadata), D47 (IPC-optimized
register convention — x0–x3 pass-through), D49 (error signaling — SPSR carry
flag), D50 (six fast-path conditions), D66 (CNTKCTL_EL1 on context switch), D69
(DAIF.I masking during fast path), A1 (unsafe in frame/), A2 (ARM64 register
set), A4 (purely reactive — single exception invocation).

---

## The question

How does the kernel convert between TrapFrame (the transient stack snapshot
created by exception entry assembly) and RegisterState (the persistent
per-Observer saved context in structural backing)?

TrapFrame and RegisterState are both 816 bytes but have different layouts.
TrapFrame includes ESR_EL1 and FAR_EL1 (exception-specific, not restored) but
not SP_EL0 or TPIDR_EL0. RegisterState includes SP_EL0, TPIDR_EL0, PC (from
ELR_EL1), and PSTATE (from SPSR_EL1) but not ESR or FAR. The GPR block (x0–x30)
and FP/SIMD block (q0–q31, FPCR, FPSR) are identical in both.

---

## Options evaluated

### Option A: TrapFrame as universal intermediate

Assembly always saves full state to a stack-allocated TrapFrame (the current
design). On context switch, copy from TrapFrame to outgoing RegisterState, then
load from incoming RegisterState.

Pro: one entry path, assembly unchanged. Con: double-write (~816 bytes to stack,
then ~700 bytes copied to RegisterState) on every context switch — ~100–150
extra cycles of unnecessary memory traffic on the hot path. Field remapping
required (not a memcpy). x0–x3 saved to TrapFrame even when the fast path
doesn't need them saved there.

**Rejected:** if direct-to-RegisterState is the correct long-term design (and it
is — the RegisterState IS the canonical representation per D35/D43), building a
TrapFrame intermediate that will be torn out is tech debt.

### Option B: Direct save to RegisterState on EL0 path

SVC/fault/IRQ from EL0: assembly saves registers directly into the current
Observer's RegisterState. EL1h exceptions: save to stack TrapFrame (unchanged).
TrapFrame continues to exist for the kernel-interrupting-kernel case.

Pro: zero-copy save side, RegisterState always correct, x0–x3 pass-through on
restore side is a natural skip. Con: two entry paths, assembly needs
RegisterState pointer at exception entry.

### Option C: Hybrid with fast-path specialization

Three paths: dedicated fast-path assembly (saves only x4–x30, leaves x0–x3 in
registers), slow-path (full save to RegisterState), EL1h (stack TrapFrame).

Pro: absolute minimum cycle count on IPC fast path. Con: x0–x3 unsaved in
outgoing RegisterState after fast-path switch — requires deferred-save machinery
(dirty bit), breaks D39 read-registers correctness for suspended Observers,
triples verification surface.

**Rejected:** the delta between B and C is the save-side x0–x3 skip — ~8–20
cycles (2–5% of ~400-cycle fast-path budget). B captures the bulk of D47's
optimization on the restore side (not loading incoming x0–x3). The save-side
skip adds real complexity (dirty bit, deferred save, read-registers correctness
edge case with D39 suspend) for a small gain. Verus readiness (journal 023)
favors fewer invariants. If profiling later shows those cycles matter, C can be
added as a surgical enhancement.

---

## Decision: Option B — direct save to RegisterState

The EL0 exception path saves registers directly into the current Observer's
RegisterState. The EL1h exception path saves to the stack TrapFrame (unchanged).

### Save path (outgoing Observer)

EL0 exceptions (SVC, fault, IRQ from userspace):

1. Assembly obtains the current Observer's RegisterState pointer via per-core
   state (see TPIDR_EL1 below).
2. Assembly saves all GPRs (x0–x30), SP_EL0 (mrs), PC (from ELR_EL1), PSTATE
   (from SPSR_EL1), TPIDR_EL0 (mrs), FP/SIMD (q0–q31, FPCR, FPSR) directly to
   the RegisterState.
3. Assembly reads ESR_EL1 (for SVC immediate / fault classification) and FAR_EL1
   (for data/instruction aborts) into scratch registers or a small on-stack
   struct — these are used for dispatch, not stored in RegisterState.
4. Assembly calls into Rust (core_manager dispatch).

EL1h exceptions (timer IRQ during kernel code):

1. Assembly saves to stack TrapFrame (existing behavior).
2. Calls Rust exception handler.
3. Restores from TrapFrame. Eret.

### Restore path (incoming Observer)

When the kernel selects a new Observer to resume (or returns to the same one):

1. Load RegisterState pointer from the incoming Observer's metadata.
2. Load all GPRs, SP_EL0 (msr), TPIDR_EL0 (msr), FP/SIMD from RegisterState.
3. Load ELR_EL1 (from RegisterState's PC field) and SPSR_EL1 (from PSTATE
   field).
4. Modify SPSR if needed (D49 carry flag for IPC error signaling).
5. Write CNTKCTL_EL1.EL0VCTEN from incoming Observer's clock_access flag (D66).
6. Switch TTBR0_EL1 if the incoming Observer has a different page table (D5).
7. Eret.

### x0–x3 handling

Save side: x0–x3 are saved unconditionally to the outgoing RegisterState on all
EL0 paths. This keeps RegisterState always correct — D39 read-registers on a
suspended or blocked Observer returns accurate values. The cost is 2 STP
instructions (~4–8 cycles) that are "wasted" when the fast path passes x0–x3
through. This is ~1–2% of the fast-path budget.

Restore side: on the IPC fast path (D50 conditions met), x0–x3 are NOT loaded
from the incoming RegisterState. They stay in physical registers, carrying data
words from sender to receiver. On the slow path and non-IPC paths, x0–x3 are
loaded from RegisterState normally. This is where D47's primary optimization
lives.

### TPIDR_EL1 as per-core state pointer

The EL0 exception entry assembly needs the current Observer's RegisterState
pointer before it can save anything. TPIDR_EL1 is the mechanism:

- TPIDR_EL1 is a per-core register, readable/writable only at EL1.
- Userspace (EL0) cannot read or write it — no leakage concern.
- The kernel sets TPIDR_EL1 to point to a per-core state struct during boot-time
  per-core initialization. The struct contains (at minimum) the current
  Observer's RegisterState pointer.
- Exception entry assembly reads TPIDR_EL1 with a single mrs instruction to
  obtain the save target.
- On context switch, the kernel updates the struct's RegisterState pointer to
  point to the new current Observer's RegisterState.

TPIDR_EL1 is the standard convention for per-core kernel data across Linux,
FreeBSD, seL4, and Zircon on ARM64.

### SP_EL0 and TPIDR_EL0

Neither is captured by the current TrapFrame (exception.S line 47 notes: "SP_EL0
only needed when handling exceptions from EL0"). Both must be saved on the EL0
path and restored on context switch. RegisterState already has fields for both
(sp, tpidr). The save-side assembly adds two mrs instructions; the restore side
adds two msr instructions. ~4 cycles each direction.

### Lazy FP/SIMD

Not settled by this derivation. The design accommodates lazy FP/SIMD save
(disable FP access via CPACR_EL1, trap on first use) as a future optimization.
The FP/SIMD save block in the EL0 entry path can be conditionally skipped when a
"FP clean" flag indicates the Observer hasn't used FP since last restore.
Orthogonal to the EL0/EL1h path split.

---

## What this settles

1. **EL0 exceptions save directly to RegisterState, not to TrapFrame.** The EL0
   path in exception entry assembly writes registers into the current Observer's
   RegisterState in structural backing. No intermediate TrapFrame, no copy.

2. **EL1h exceptions continue to use stack TrapFrame.** Kernel-interrupting-
   kernel exceptions have no Observer context to save into. The existing
   TrapFrame-based path is unchanged.

3. **TPIDR_EL1 holds per-core state pointer.** The assembly accesses the current
   Observer's RegisterState pointer through a per-core struct pointed to by
   TPIDR_EL1.

4. **x0–x3 saved unconditionally, restored conditionally.** Save side always
   writes x0–x3 (RegisterState always correct). Restore side skips x0–x3 on the
   IPC fast path (D47 optimization). Deferred-save machinery (Option C) is not
   needed.

5. **ESR_EL1 and FAR_EL1 are transient.** Captured into scratch registers or a
   small on-stack struct for dispatch, not stored in RegisterState.

## What this does NOT settle

- Lazy FP/SIMD save (orthogonal, future optimization).
- Per-core state struct layout beyond "contains RegisterState pointer"
  (implementation detail).
- Fast-path assembly as a separate routine vs. a branch within exception.S
  (implementation detail — either approach is compatible with this design).
- Exact assembly instruction sequence (implementation).

---

- **Rests on:** D43 (Observer schema — register save pointer in metadata;
  structural backing location; ~4-cycle pointer chase on hot path), D47
  (IPC-optimized registers — x0–x3 pass-through; kernel restricted to x4–x15;
  invariant on exception entry code), D49 (SPSR carry-flag modification on
  restore), D50 (fast-path conditions — Call/ReplyRecv scope; same-core;
  scheduler callback; defines when x0–x3 pass-through applies), D69 (DAIF.I
  masking — save/restore inside non-preemptible window), D66 (CNTKCTL_EL1 on
  context switch — additional restore-side step from Observer metadata), D6
  (single execution unit — one RegisterState per Observer, no nesting), D35
  (write-registers writes directly to RegisterState — establishes RegisterState
  as the primary representation), D39 (read-registers must return accurate state
  — rejects save-side x0–x3 skip), A1 (Rust unsafe — all save/restore in
  frame/), A2 (ARM64 — register set, TPIDR_EL1, SP_EL0, CNTKCTL_EL1), A4 (purely
  reactive — full save→work→restore in single exception invocation), D1
  (per-core hot path — context switch is per-core, no shared state),
  `design/landscape.md` §5.4 (minimal save + ESR dispatch is the microkernel
  consensus; full save is the monolithic-kernel approach),
  `design/research/execution-unit.md` §5 (register state belongs to execution
  unit, not address space), `design/research/ipc-fastpath-conditions.md`
  (seL4/L4/Fiasco.OC all use direct save to thread save area, not intermediate
  stack frames).
- **Status:** settled. Revisit if D47 is revised (register convention change
  alters the pass-through optimization), if D43 is revised (changes
  RegisterState location or pointer layout), if profiling demonstrates save-side
  x0–x3 cost justifies Option C's deferred-save machinery, or if a second
  architecture (beyond ARM64) requires a fundamentally different split.
