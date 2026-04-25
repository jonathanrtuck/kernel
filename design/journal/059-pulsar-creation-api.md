# 059 — Pulsar creation API: single-call, armed-at-creation

Date: 2026-04-24

## Starting point

D44 settled Pulsar semantics. D48 settled the syscall table. D52 settled the
rights mask. The creation API shape was noted as open in the autonomous plan
(M07).

## Exploration

Three settled decisions jointly encode the answer:

1. **D44:** "The Pulsar is armed on creation with a delivery field cap, badge,
   deadline, and period."
2. **D48:**
   `create_pulsar(space_cap, field_cap, badge, deadline, period) → cap`. No
   `arm_pulsar` operation.
3. **D52:** Pulsar rights = destroy + clone (2 bits). "No modify or rearm
   right."

### Why D35's composable pattern does not apply

D35 derived Observer's two-step (create inert → configure → start) because of a
structural gap: an Observer without code Space caps has no valid PC. Pulsars
have no equivalent gap — all parameters (field, badge, deadline, period) are
knowable at creation. "Inert Pulsar" has no structural content.

D35's test: each post-creation operation must have independent utility.
`arm_pulsar` would exist solely to serve creation — no independent use for fault
resolution, debugging, or delegation. D35 explicitly rejected kernel surface for
single-use operations.

### Parameters

| Parameter | Type                      | Constraint               | Rationale                                   |
| --------- | ------------------------- | ------------------------ | ------------------------------------------- |
| space_cap | Space cap                 | consumed (D32)           | Type conversion to structural backing       |
| field_cap | Field cap with send right | referenced, not consumed | Kernel-as-sender destination                |
| badge     | u64 (D58)                 | immutable (D17)          | Distinguishes this Pulsar in delivery Field |
| deadline  | timestamp                 | form depends on G09      | When the Pulsar first fires                 |
| period    | u64 (0 = one-shot)        | D44, D42                 | Re-arm interval; 0 for one-shot             |

### Lifecycle

- Armed immediately on creation
- Repeating re-arm: kernel-managed, `next = scheduled + period`
- Cancel = destroy(pulsar_cap), returns structural backing as Space cap
- Modify: not possible (D52). Change = destroy + create.
- One-shot loop: manual control escape hatch for adaptive timing

### D53 carve-out

`create_pulsar` acquires the Pulsar arena and briefly increments the delivery
Field's refcount. D53 says creation acquires only the target type's arena. The
Field refcount increment is a cross-arena write that is safe (not held
simultaneously with anything else) but should be documented as an exception.

## Status

**Settled.** Single-call, armed-at-creation. No arm, modify, or disarm
operation. Cancel = destroy. Deadline parameter form depends on G09.
