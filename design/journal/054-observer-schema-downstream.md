# 054 — Observer schema downstream: budget encoding, default profile, self-reference cap

Date: 2026-04-24

## Starting point

D43 settled the Observer minimum schema but explicitly deferred four downstream
details: register save area layout, budget size/encoding, default scheduling
profile, and self-reference capabilities. The register save layout was found to
be already implemented (`src/arch/aarch64/register_state.rs`, 816 bytes). This
entry settles the remaining three.

## Budget encoding

D42 defined three scheduling values (R, T, P) sharing a fixed per-Observer
budget. The question: what is the budget, and how are the values stored?

### Store two, derive the third

R + T + P = budget is a constraint on a 2D simplex. Storing all three creates a
representational invariant (the sum must equal the budget) that must be enforced
at every write site. Storing two and deriving the third eliminates the invariant
by construction — invalid states are unrepresentable. This is A1 applied to data
representation: ownership of the constraint lives in the type, not in runtime
checks.

Store R (responsiveness) and T (throughput). Derive P (precision) as
`budget - R - T`. The choice of which to derive is arbitrary mathematically; P
is chosen because precision (hard-RT) is the least common non-zero value in
D42's representative profiles, so the common case (adjusting responsiveness vs
throughput) touches stored fields directly.

### Budget = 128

The budget is 128. Each stored value is a `u8`. The derived value is computed
inline:
`fn precision(&self) -> u8 { 128 - self.responsiveness - self.throughput }`.
Validation at `observer_set_scheduling` is a single check: `R + T <= 128`.

Why 128, not 100 or 255:

- **100** (percentages): intuitive, but creates a "percentages pretending to be
  integers" leaky abstraction. With two stored values, the derived third may
  surprise users expecting R+T+P=100 exactly. Also, hot-path scheduler math
  (timeslice weighting) requires division by 100 — actual UDIV, ~5 cycles.
- **255** (fills u8): wastes resolution. No scheduler will distinguish R=127
  from R=128. The excessive precision encourages users to pick pole values (0
  or 255) rather than thinking about the distribution. "178 out of 255" has no
  intuitive meaning.
- **128** (power of 2): scheduler math uses right-shift instead of division. The
  midpoint (64) is intuitively "half my budget." 128 provides ~10-20 meaningful
  levels per dimension (in increments of ~6-12), which matches the resolution
  the scheduler can actually act on. Fine enough for non-trivial profiles,
  coarse enough that users think about the distribution.

### D43 table update

The Observer metadata struct's scheduling profile fields change from:

| Field          | Type    |
| -------------- | ------- |
| Responsiveness | integer |
| Throughput     | integer |
| Precision      | integer |

to:

| Field          | Type | Notes                       |
| -------------- | ---- | --------------------------- |
| Responsiveness | u8   | 0–128; hot path (scheduler) |
| Throughput     | u8   | 0–128; hot path (scheduler) |

Precision is derived: `128 - R - T`. Not stored. This reduces the Observer
metadata struct by one field (~1 byte + alignment, minor).

## Default profile

D42 requires "a kernel-defined middle value (best-effort)." A3 requires serving
all workload types without bias. A5 requires simplicity.

Default: R = 43, T = 43. Derived P = 128 - 43 - 43 = 42.

This is the closest equal distribution on a budget of 128. The slight asymmetry
(P gets 42 instead of 43) is negligible — the scheduler cannot meaningfully
distinguish 42 from 43. The equal split serves A3 (no workload type favored) and
A5 (zero configuration needed for reasonable behavior).

## Self-reference capability

D4 (designation = authority) + D7 (only capabilities designate) + D8 (flat cap
table) rule out any "magic self-handle" — an Observer that wants to act on
itself must hold a capability to itself in its own cap table, like any other
designation.

### Kernel-installed, not optional

The kernel installs the self-cap at Observer creation time, at a third reserved
cap-table slot (alongside fault handler at slot 0 and reply field at slot 1).

Why kernel-installed:

- **D35/D21 pattern.** D35's creation API already installs the fault handler and
  reply field caps at reserved slots. The self-cap follows the same pattern —
  the kernel enforces structural invariants at creation rather than hoping
  userspace remembers.
- **Eliminates a bug class.** Without automatic installation, every Observer
  creator must explicitly install a self-cap for the Observer to modify its own
  scheduling profile, register its own fault handler updates, or perform any
  self-directed operation. Forgetting this creates a silently broken Observer.
  A5 says the kernel should absorb this complexity.
- **Zircon and EROS precedent.** Both provide automatic self-referencing handles
  (Zircon: `zx_process_self()`, EROS: brand/keeper capabilities). seL4 does not
  — but seL4's CSpace sharing model provides an alternative path to
  self-reference that D8's per-Observer table does not offer.

### Rights on the self-cap

The self-cap carries the full rights mask. An Observer can attenuate and
delegate a copy with reduced rights to a supervisor. The self-cap at slot 2 is
the Observer's own unrestricted self-reference — restricting it at the source
would prevent the Observer from performing any self-directed operation that
requires the restricted right, with no recourse.

### Reserved slots

The Observer cap table now has three kernel-reserved slots:

| Slot | Content       | Source |
| ---- | ------------- | ------ |
| 0    | Fault handler | D21    |
| 1    | Reply field   | D43    |
| 2    | Self-cap      | D57    |

User-available slots start at index 3.

## Status

**Settled.** Budget encoding (R and T as u8, budget 128, P derived), default
profile (43/43/42), and self-reference cap (kernel-installed at reserved slot 2
with full rights).
