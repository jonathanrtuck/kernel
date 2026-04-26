# D87 — IPC fast-path register pass-through (deferred)

**Question:** How do x0-x3 (IPC data words) pass from sender to receiver on the
fast path at the assembly level?

**Rests on:** D50 (fast-path conditions), D74 (register save/restore flow), D79
(scheduling decision matrix).

**Status:** settled (mechanics understood), implementation deferred.

---

## Settles

### "x0-x3 stay in physical registers" is conceptual, not literal

D74 settles that x0-x3 are saved **unconditionally** to the sender's
RegisterState on all EL0 paths. This keeps RegisterState always correct — D39
read-registers on a suspended or blocked Observer returns accurate values. The
cost is 2 STP instructions (~4-8 cycles), ~1-2% of the fast-path budget.

Once Rust dispatch runs, the AAPCS64 calling convention freely uses x0-x7 as
argument/scratch registers. By the time the kernel returns a DispatchResult,
physical x0-x3 contain whatever Rust left in them — not the sender's data words.

The "pass-through" description is accurate at the _effect_ level (the receiver
gets the sender's data words without them being explicitly written to the
receiver's RegisterState) but not at the _mechanism_ level.

### Actual mechanism

1. **Entry:** EL0 assembly saves sender's x0-x3 to sender's RegisterState
   (unconditionally, D74).
2. **Dispatch:** Rust identifies fast-path conditions (D50). Calls
   `write_metadata_to_registers` — writes only x4-x7 to receiver's
   RegisterState. x0-x3 in receiver's RegisterState are NOT touched.
3. **Restore:** Assembly reloads x0-x3 from the **sender's** RegisterState (not
   the receiver's). PerCoreData.register_state_ptr still points to the sender
   until the context switch updates it. The assembly reads x0-x3 before
   switching PerCoreData to the receiver.

The optimization eliminates:

- 4 stores on the write side (not writing x0-x3 to receiver's RegisterState)
- Message struct construction for x0-x3 on the DirectSwitch path

It does NOT eliminate the x0-x3 load on the restore side — those 4 values must
still be loaded from memory (from the sender's save area instead of the
receiver's).

### Total savings: ~8-16 cycles out of ~400

- Write side: skip 4 stores = ~4-8 cycles
- Skip Message construction for x0-x3 = ~4-8 cycles
- Restore side: same number of loads either way = 0 savings

Compare to: TLB invalidation ~30-100 cycles, FP/SIMD save/restore ~200 cycles
round-trip. The optimization is 2-4% of the fast-path budget.

### Implementation plan gap

Task 2.6 says "if fast_path == 1: skip loading x0-x3" without specifying where
x0-x3 come from. The resolution: PerCoreData.register_state_ptr still points to
the sender's RegisterState. The assembly must:

1. Read x0-x3 from PerCoreData → sender's RegisterState.
2. Update PerCoreData.register_state_ptr to receiver's RegisterState.
3. Load x4-x30, system registers, FP/SIMD from receiver's RegisterState.
4. eret.

If the Rust wrapper updates PerCoreData before calling the restore assembly, the
sender's RegisterState pointer is lost. The ordering is critical.

### D50 fast-path conditions (confirmed)

All must hold:

1. Operation is Call (SVC #3) or ReplyRecv (SVC #4)
2. Same core (structural constraint from D1)
3. Target field has a waiting receiver (`field.pop_waiter()` returns Some)
4. No user cap in message (x6 == u64::MAX)
5. Scheduler approves (`should_switch_to` returns true)
6. Field routing resolved

Signaled by `CallOutcome::DirectSwitch(observer_ptr)` from communication module,
then `DispatchResult::ResumeFastPath(observer_ptr)` from core manager.

## Decision: defer implementation

The fast path is deferred from Phase C to a future optimization pass. Reasons:

1. **Marginal savings** (~8-16 cycles / 2-4%) relative to implementation
   complexity (dual load sources, careful PerCoreData ordering).
2. **Already sequenced last** in the implementation plan (Task 2.6, end of
   dependency chain).
3. **No interface changes needed** to add it later — re-introduce the
   `DirectSwitch` variant and conditional skip in restore assembly.
4. **Simplifies initial bring-up** — the context switch restore path always
   loads all registers from one RegisterState, no conditional branching.

For Phase C: `CallOutcome::DirectSwitch` treated as `WokeReceiver` on the slow
path. `write_message_to_registers` used for all receives.
`DispatchResult::Resume` used for all Observer returns (no `ResumeFastPath`
variant in the restore assembly).

## Rejected alternatives

**D74 Option C — true register pass-through (never save x0-x3 on entry).**
Rejected by D74. Saves ~8-20 additional cycles but requires deferred-save
machinery and breaks D39 read-registers correctness.

**Implement now.** The optimization adds complexity to the restore path and
interacts with the PerCoreData update sequence. Not worth the risk for initial
bring-up. The first priority is a correct EL0 round-trip.

## Reference check

Matches implementation plan Task 2.6 structurally. Corrects the gap in the
plan's restore-side specification. Defers implementation per project stance (no
deadline, correctness first).
