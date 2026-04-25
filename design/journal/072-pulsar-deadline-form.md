# Journal 072 — Pulsar deadline form: relative duration

Settles G09: the `create_pulsar` deadline parameter is a relative duration
("fire in N nanoseconds from now"), not an absolute deadline. The kernel
absorbs the conversion to an absolute comparator value internally.

## Context

D44 explicitly deferred "duration vs. absolute deadline API" as an open
question (G09). D62 settled the creation shape (single-call, armed-at-creation)
but left the deadline parameter form open. G09's exploration
(`.brain/explorations/G09-pulsar-duration-vs-deadline/`) evaluated three
viable options: absolute-only (Zircon), relative-only (L4, Plan 9), and both
via flag bit (POSIX/QNX). All three fit within the D49 ABI budget; all three
produce identical kernel-internal behavior (the kernel always works in absolute
space after the syscall entry).

The key dependency G09 identified was M11 (clock access authority) — whether
Observers have direct CNTVCT_EL0 access affects the ergonomic cost of each
option. D66 (clock access mechanism) settled the per-Observer mechanism but
left the authority model and default policy open.

## D66 establishes the decisive pattern

D66 settled interrupt routing as kernel-automatic: the kernel tracks
Observer→core mapping and updates GICD_IROUTER on migration and receive-cap
transfer. The argument that closed the routing-exposure option was:

> "The API would let userspace say what the kernel already knows."

The kernel knows the receive-cap-holder → core mapping; a routing API would
only restate information the kernel derives from existing relationships. D66
rejected the API as redundant.

The absolute-only timer design has the same structure. The kernel knows the
current counter value (CNTPCT_EL0). When the caller wants "fire in 10ms," the
kernel can compute `now + 10_000_000 ns` internally — one counter read and one
addition, both negligible. An absolute-only API forces the caller to obtain
`now` (via direct counter access or `clock_read` syscall), add the duration,
and pass the result — recomputing what the kernel already has. For Observers
without clock access (D66/M11), this requires a `clock_read` round-trip solely
to tell the kernel something it knows natively.

This is the same anti-pattern D66 rejected: the caller providing information
the kernel already has, at cost to the caller and no benefit to the kernel.

## Relative-only is the D66-consistent resolution

**Relative duration** accepts the caller's natural expression ("fire in N
nanoseconds") and has the kernel absorb the trivial conversion to an absolute
comparator. This follows D66's routing precedent and A5's absorption principle:

1. **Common case served optimally.** "Sleep for N ms," "retry in 5s," "timeout
   in 100ms" — the dominant timer patterns — are one syscall for all Observers,
   regardless of clock access authority. No `clock_read` prerequisite. M11's
   authority distinction does not affect ergonomics for common timer use.

2. **Repeating Pulsars unaffected.** D44 settles that the kernel converts the
   initial arm to an absolute deadline internally and re-arms with
   `next = scheduled + period`. The API form is a presentation layer over
   absolute internals. Drift compensation is fully kernel-managed regardless of
   parameter form.

3. **One-shot precision loop served adequately.** D44's "manual control"
   escape hatch (one-shot Pulsars in a loop for adaptive timing) can maintain
   drift-free operation: the Pulsar message includes the actual fire time in raw
   CNTVCT_EL0 ticks (D63). The Observer computes
   `next_duration = desired_time - now`, requiring one counter read (direct
   access) or one `clock_read` syscall. This is the same cost as
   absolute-only's common case for Observers without clock access — the cost
   is merely shifted to the minority precision use case rather than the majority
   common case.

4. **ABI simplicity.** The `duration` parameter has one interpretation. No flag
   bit, no mode, no dual-path dispatch in the kernel. The kernel reads the
   counter, adds the converted duration, programs CVAL. One code path.

## Forward-compatibility

Relative-only is forward-compatible with adding absolute mode later. A future
derivation could add a flag bit (bit 63 of the duration field, reducing range
to ~292 years — practically unlimited) or a second operation code. Either
approach is additive: existing callers using relative durations are unbroken.
The reverse — shipping absolute-only now and adding relative later — is equally
additive, but that direction penalizes the majority use case now to serve the
minority use case first.

D66 used this same forward-compatibility argument for priority: flat absorption
now, exposure additive later. The principle applies identically here: start
with the option that serves the common case, defer the minority option until a
concrete workload demonstrates the need.

## The parameter is named `duration`, not `deadline`

The settled parameter name is `duration` (in nanoseconds). The kernel converts
to counter ticks internally using CNTFRQ_EL0. All spec references to the
`create_pulsar` parameter change from `deadline` to `duration`.

The kernel's internal representation remains absolute (CVAL comparator value).
D44's `next = scheduled + period` arithmetic and D42's EDF admission test are
unchanged — they operate on the kernel's internal state, not the API parameter.

## Nanoseconds as the API unit

A5 implies the API accepts nanoseconds (human-meaningful) rather than raw
counter ticks (hardware unit). The kernel knows CNTFRQ_EL0 and performs the
conversion. This was identified as a separate downstream question (units) in
G09's derive phase, but follows directly from A5 and is settled here: the
`duration` parameter is in nanoseconds.

## Interaction with D66's unsettled authority choices

D66 notes that the clock access authority mechanism and default policy "should
be settled alongside G09." With G09 now settled as relative-only, the
interaction simplifies:

- If clock access is **granted by default**: Observers use direct counter reads
  for precision one-shot loops. The relative API's cost for precision callers is
  near-zero (~1 cycle counter read + subtraction to compute duration).
- If clock access is **restricted by default**: Observers without access use
  `clock_read` for precision one-shot loops (~100–200 cycles). The relative API
  still serves the common case with one syscall.

Either M11 resolution works. The authority model no longer needs to compensate
for the timer API's parameter form — the relative API is ergonomically neutral
regardless of M11's outcome. The D66 note about settling alongside G09 is
satisfied; the remaining authority choices can be settled on their own merits.

## Rejected alternatives

**Absolute-only (Zircon).** Forces callers without clock access to make two
syscalls for the dominant "sleep for N ms" pattern. The kernel already knows
`now`; requiring the caller to provide it is the redundant-API pattern D66
rejected for interrupt routing. Zircon's absolute-only design works because
Zircon unconditionally exposes the monotonic clock via VDSO — there is no
per-Observer clock access gating. This kernel has per-Observer clock access
control (D66), making the ergonomic penalty real.

**Both via flag bit (POSIX/QNX).** Maximum flexibility, but adds a mode to the
API (bit 63 of the time field) and a branch in the kernel. The benefit is
one-syscall absolute arms for precision callers — but the precision one-shot
loop already requires a clock read for the computation
`next_duration = desired - now` regardless of which mode is used to express the
result. The flag bit's benefit reduces to saving the subtraction step (trivial).
Not foreclosed — can be added later if a defined workload demonstrates that the
`clock_read` cost in precision one-shot loops is a bottleneck.

## Prior art

Relative-only: L4 (mantissa/exponent microseconds), Plan 9 (`sleep(ms)`,
`alarm(ms)`), seL4 MCS (budget/period as durations). Absolute-only: Zircon
(`zx_timer_set` with `zx_deadline_after` helper). Both: POSIX/Linux
(`TIMER_ABSTIME` flag), QNX. The majority of surveyed systems accept relative
durations natively; Zircon is the only one requiring absolute.

## Summary

| Aspect | Resolution |
|--------|-----------|
| Parameter form | Relative duration in nanoseconds |
| Parameter name | `duration` (replaces `deadline`) |
| Kernel conversion | Counter read + frequency conversion on every `create_pulsar` — negligible |
| Common case cost | One syscall, all Observers |
| Precision one-shot cost | One clock read (direct or syscall) + one `create_pulsar` |
| Forward path | Flag bit or second operation additive; not foreclosed |

- **Rests on:** D44 (Pulsar semantics — deferred G09 here; kernel-managed
  re-arm uses absolute internals regardless of API form), D66 (clock access
  mechanism — established the "kernel already knows" anti-pattern for
  redundant APIs; per-Observer CNTKCTL_EL1 means some Observers lack direct
  counter access), D49 (ABI encoding — duration fits in x2 under resolved
  register mapping; no flag bit needed), D62 (creation API shape — single-call,
  armed-at-creation; duration is the fourth parameter), D63 (message layout —
  fire time in raw ticks enables drift-free one-shot loops with relative API),
  A5 (absorb complexity — kernel absorbs relative-to-absolute conversion and
  nanosecond-to-tick frequency conversion), A3 (generic — relative serves all
  workloads; absolute mode not foreclosed for future hard-RT needs), A2 (ARM64
  — TVAL/CVAL symmetric; nanosecond API with kernel frequency conversion),
  `.brain/explorations/G09-pulsar-duration-vs-deadline/`.
- **Status:** settled. Closes G09. Revisit if a defined workload demonstrates
  that the `clock_read` cost in precision one-shot loops is a correctness or
  performance bottleneck not addressable by granting clock access authority —
  in that case, add the flag-bit absolute mode (additive, non-breaking).
- **Journal:** `journal/072-pulsar-deadline-form.md`.
