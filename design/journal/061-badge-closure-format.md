# 061 — Badge-closure message format

Date: 2026-04-24

## Starting point

D17 settled badge-closure notifications as a concept. D28 settled the message
format envelope. The specific content of badge-closure messages was deferred.

## The format

```text
badge     → B (the closed badge value; D17 + D28 kernel-injected)
label     → LABEL_CLOSURE (reserved constant; D4 distinguishability)
data[0]   → 0
data[1]   → 0
data[2]   → 0
data[3]   → 0
cap       → absent (no capability needed for cleanup)
reply_cap → absent (kernel deposit, not Call())
```

### What forces each field

- **badge = B:** D17 (badge identifies which client disconnected) + D28 (badge
  is kernel-injected identification field)
- **label = LABEL_CLOSURE:** D4 (must distinguish kernel-synthesized from
  user-sent messages with matching badge). Parallels D12 fault-type labels.
- **data = all zero:** D29 analyzed badge-closure and concluded "1 word at
  most." Zero words is sufficient: the badge identifies the client, the label
  identifies the event type, the reaction (free per-badge state) is the same
  regardless of closure reason. Reason codes fail the A5 test — badge assignment
  discipline (D17 minter-assigned) lets servers self-encode capability types in
  badge ranges.
- **cap = absent:** Unlike faults (Observer handle for resume) or interrupts
  (ack cap for unmask), closure requires no capability to act on.
- **reply_cap = absent:** kernel deposit, D16

### Closest prior art

QNX disconnect pulses: opt-in at channel creation, notification carries only the
client identifier (scoid), a code distinguishes the notification type. Better
than Mach (per-right registration, dead-name refcount cleanup). Unlike Zircon
(1:1 peer-closed doesn't work for many-to-many Fields).

## Status

**Settled.** Badge B + LABEL_CLOSURE + zero data + no caps.

Does NOT settle: LABEL_CLOSURE numeric value (ABI enumeration), routing
interaction (does closure notification for a badge in a routed range follow
routing or bypass it?), T1 kernel detection mechanism (how the kernel internally
distinguishes consumed-by-use from closed-without-use — D16/D17 deferred).
