# 060 — Pulsar message content layout

Date: 2026-04-24

## Starting point

D44 settled Pulsar delivery semantics including actual fire time and overrun
count. D28 settled the fixed-size message format. The specific data word
assignments were not stated.

## The layout

```text
badge     → minter-assigned Pulsar ID (from creation record; D17)
label     → LABEL_TIMER_FIRE (kernel constant; value TBD in ABI enumeration)
data[0]   → actual fire time: CNTVCT_EL0 read at timer interrupt entry
data[1]   → overrun count: 0 for normal fire; N for N missed periods
data[2]   → 0 (reserved)
data[3]   → 0 (reserved)
cap       → empty (u64::MAX sentinel; ack-to-re-arm rejected by D44)
reply_cap → absent (kernel deposit, not Call())
```

### What forces each field

- **badge:** D17 kernel-injected from Pulsar creation record
- **label:** D28 kernel-set dispatch discriminant (parallels D12 fault labels)
- **data[0]:** D44 explicitly includes actual fire time — the one datum the
  Observer cannot reconstruct without a second syscall
- **data[1]:** D44 includes overrun count. Always present (0 for normal fires) —
  D28 fixed format rules out conditional content
- **data[2..3]:** No structurally motivated content. Scheduled deadline rejected
  (Observer already has it from creation). Reserved zero.
- **cap:** D44 rejected ack-to-re-arm (2x syscall cost, kernel manages re-arm
  per A5). Empty cap = 0-cap message, satisfying D50 fast-path eligibility.
- **reply_cap:** kernel deposit, D16

### Clock domain

data[0] is raw CNTVCT_EL0 ticks, not nanoseconds. Cheaper at interrupt time,
directly comparable to Observer CNTVCT_EL0 reads. CNTFRQ_EL0 is known under A2
for conversion when needed.

### Prior art departure

No surveyed system (Zircon, QNX, seL4 MCS, POSIX, L4, Plan 9) includes a firing
timestamp in the timer notification. D44 deliberately departs on the grounds
that actual fire time has genuine information content the Observer cannot obtain
otherwise.

## Status

**Settled.** Badge + LABEL_TIMER_FIRE + fire_time + overrun_count + 2 reserved

- empty cap + no reply cap.

Does NOT settle: LABEL_TIMER_FIRE numeric value (ABI enumeration), data[2]
disposition (reserved zero vs scheduled deadline — medium confidence), one-shot
field-full behavior (implied by D44 but not explicitly stated for one-shots).
