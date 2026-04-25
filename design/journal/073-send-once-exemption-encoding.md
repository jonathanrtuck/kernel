# 073 — Send-once exemption encoding

Date: 2026-04-24

## Starting point

D17 settled badge-closure notifications and explicitly accepted tension T1:
"Send-once caps consumed by use must NOT trigger badge-closure (redundant — the
reply already arrived). Send-once caps closed WITHOUT use SHOULD trigger
badge-closure (informative — the reply will never come). This requires the
kernel to distinguish consumed-by-use from closed-without-use. Deferred to
send-once encoding details."

D51 settled the send-once flag as a boolean on the cap entry. D64 settled the
badge-closure message format. D65 settled the reply badge as caller-supplied.
The remaining open question is the encoding mechanism: how does the kernel
implement the distinction, and what happens to the reply Field's tracking?

## Exploration

### The two removal paths

A send-once cap can be removed from the table by exactly two paths:

1. **Consume-on-delivery.** The kernel removes the cap after a successful Send
   through it. This is the used path — the server replied.
2. **D11 close.** Userspace-triggered close (explicit drop), cascade-triggered
   close (D33 Observer destroy), or any other cap-close path. This is the unused
   path — the reply will never come.

These are already distinct code paths. The consume-on-delivery path is
kernel-internal (post-delivery cleanup); D11 close is the general-purpose cap
removal path where badge-closure checking lives.

### Option analysis

Four options were evaluated (full analysis in
`.brain/explorations/G10-send-once-exemption-encoding/`):

**Option A — Structural exemption.** The consume-on-delivery path simply does
not call D11's close logic. Badge-closure is in D11 close only. The exemption
falls out of code structure with no extra data and no extra branch on the used
path.

**Option B — Explicit `consumed` flag on cap entry.** Under D53's arena-lock
model, the race between concurrent close and consume-on-delivery is serialized
by the field/observer arena lock. The flag would only matter if both paths could
run simultaneously on the same cap — they cannot. Option B collapses to Option A
under the arena-lock model.

**Option C — Per-Field exemption policy.** A flag at Field creation controls
whether consumed-by-use triggers badge-closure. Adds flexibility for
authorization-token use cases where the issuer wants audit trail on cap use. No
settled design pattern requires this. Adds a conditional branch on the
consume-on-delivery hot path and a reason code to the badge-closure message
format. Speculative generalization — not foreclosed but not motivated.

**Option D — Direct cancellation.** The kernel directly unblocks the stuck
caller with an error instead of using badge-closure. If implemented as a message
to the reply Field (D13 compliance), this reduces to Option A with a different
message label. If implemented as a direct unblock bypassing the Field queue, it
violates D13. The D13-compliant form has no structural advantage over Option A.

### Why Option A

The existing design graph constrains the choice:

- D17 settles badge-closure as the mechanism. Option D's D13-compliant form is
  structurally equivalent.
- D51 settles send-once as a boolean flag. No additional data needed.
- D13 commits to Field-based delivery. No separate primitive.
- D53's arena-lock serialization eliminates the race that would motivate Option
  B.

Option A introduces no new state, no new branches on the hot path, and no new
decisions. The exemption is a consequence of code structure: the
consume-on-delivery operation is not D11 close, so D11's badge-closure check is
never reached.

The invariant ("consume-on-delivery does not call D11 close") is behavioral, not
compile-time enforced. This is the primary cost — a future refactor that
consolidates cap removal must preserve the separation. Mitigated by Rust's
ownership semantics (consume = move, close = drop are naturally distinct
operations) and testability (a test that verifies badge-closure does NOT fire
after a successful reply delivery).

### Reply Field tracking: always-on

For badge-closure to fire when a reply cap is dropped without use, the reply
Field must have opt-in tracking enabled (D17). The question is whether this is
the caller's responsibility or the kernel's default.

**Always-on is correct.** The reply Field is a kernel-managed, per-Observer,
pre-allocated structural object (D16). Its purpose is reply routing. A reply
Field without tracking means the caller can be permanently blocked with no
signal — violating A4 (purely reactive). Making tracking opt-in for the reply
Field creates a footgun where the most important use case for badge-closure (RPC
cancellation detection) silently fails if the creator forgets.

This is a specialization of D17 for the reply Field, not a change to D17's
general rule. General Fields remain opt-in. The reply Field defaults to
tracking-on because its structural purpose (reply routing) requires it for
correctness.

The cost is trivial: one bit per reply Field (the tracking-enabled flag is
already in the Field struct for general tracked Fields).

### Prior art

- **seL4 classic:** Structural separation — reply cap consumed by
  `seL4_Reply`/`seL4_ReplyRecv` through a different code path than
  `seL4_CNode_Delete`. No notification fires on either path, but the structural
  technique is the same.
- **Mach:** `MACH_NOTIFY_SEND_ONCE` fires on both used and abandoned send-once
  rights. Mach does NOT structurally separate the paths — it uses the
  notification destination to encode the distinction. Our approach is cleaner:
  the separation is in the code, not the data.
- **EROS/KeyKOS:** Resume key dropping blocks the caller permanently with no
  notification. Our design improves on this with badge-closure on the reply
  Field.
- **Zircon:** `ZX_CHANNEL_PEER_CLOSED` fires regardless of whether a reply was
  sent. The receiver checks for pending messages to distinguish. Our design
  converges on the same insight: the presence of the notification IS the signal
  (no reason code needed in the message body).

## What this settles

1. **Exemption mechanism:** structural code-path separation. Consume-on-delivery
   is a separate kernel operation from D11 close. Badge-closure checking lives
   entirely in D11 close. The consume-on-delivery path clears the slot and
   decrements the refcount without entering D11's badge-closure logic.

2. **Reply Field tracking:** always-on. The kernel creates the reply Field with
   badge-closure tracking enabled. This is a D16 reply Field specialization, not
   a change to D17's general opt-in rule.

3. **Notification semantics:** badge-closure notification on the reply Field
   means "reply will never come." Its presence is self-discriminating — no
   reason code needed (D64's all-zero data words are correct for this case).

## What this does NOT settle

- Badge value assigned by the kernel to reply send-once caps (downstream of D65:
  caller-supplied reply_badge).
- Reply Field creation timing (pre-allocated at Observer creation vs. lazy on
  first Call). D16 defers this; the tracking-always-on policy applies whenever
  the reply Field is created.
- Whether a caller can voluntarily cancel a pending Call() (symmetric to server
  dropping the reply cap). Not currently in scope; not foreclosed.
- Per-Field exemption policy for non-reply use cases (Option C). Not foreclosed
  — additive if a concrete authorization-audit workload motivates it.

## Status

Settled. Closes G10 from D17's "send-once exemption encoding" open item.
Decision moves to spec.md as D73.
