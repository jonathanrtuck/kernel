# 037 — Time donation on IPC

2026-04-21. Starting from the explicit open question in spec.md: "Time donation
on IPC. seL4 MCS donates scheduling context during IPC for priority inversion
prevention." All parent decisions settled: D29 (Time is capability-held), D30
(multi-Time), D31 (abstract capacity), D36 (normalized compute units), D13
(queued fields), D16 (reply via send-once cap), D28 (fixed-size message format).

---

## Four options considered

### Option A: Explicit cap transfer in the user cap slot

The caller includes a Time cap in the D28 user cap slot during Call(). Standard
move semantics. The server receives the Time cap (D30 multi-Time), returns it in
the reply message's cap slot.

### Option B: Kernel-internal donation on Call()

The kernel automatically loans the caller's Time for the duration of the Call(),
tracking the donation internally. The Time cap is removed from the caller's
table, the kernel holds it, and the server's aggregate is boosted. On reply, the
kernel returns the Time.

### Option C: No kernel-level donation

Servers have their own Time (D31 pager chain). Priority inversion handled
through D2 scheduling hints or userspace convention.

### Option D: Kernel-injected dedicated Time field in the message

Parallel to badge and reply cap. The kernel takes the caller's Time cap and
places it in a dedicated message field, separate from the user cap slot.

---

## Key findings from derivation

### Time transfer must be a move, not a copy

D30's cached aggregate sums the quantities of all held Time caps:
`total += cap.amount` on acquisition, `total -= cap.amount` on loss. This
assumes each cap references a distinct Time object.

If two caps reference the same Time object (clone), the aggregate double-counts
— the system believes more compute capacity exists than is real. This violates
the vocabulary constraint "the kernel cannot over-allocate."

This means: (1) Time donation via IPC is necessarily a move. The caller
relinquishes the Time cap. (2) Time clonability is constrained by D30's
aggregate model — clone creates double-counting. This finding constrains the
open "Time clonability" question but does not settle it.

### Crash safety is not a kernel concern

If the server crashes while holding a donated Time cap, D33's cascade closes the
cap. D32's asymmetry means the destroyed Time returns to the kernel pool — the
caller permanently loses that scheduling capacity.

But this is not worse than the base case. Under D16 Call(), the caller is
blocked on its reply field. If the server dies, no one sends the reply — the
caller is stuck regardless. The Time loss is a symptom of the server crash, not
a separate catastrophe. The caller's fault handler / supervisor chain handles
both.

Partial donation further mitigates: under D30, the caller can hold multiple Time
caps and donate only one. If the server crashes, the caller loses the donated
cap but retains the others. On unblock (via badge-closure notification or
supervisor intervention), the caller still has scheduling capacity and can
request more from its pager.

Kernel-managed return (Options B and D) solves a problem that doesn't exist in
practice. The operational cost of crash safety is borne by the supervision
architecture, not the donation mechanism.

### The user cap slot is the right place for Time

D28 provides one user cap slot per message. The reply cap is kernel-injected
(D16). The badge is kernel-injected (D17). The single user-controlled
authority-per-request slot remains.

Time donation is the most natural use of this slot during Call(). Every Call()
implicitly asks the server to spend compute on the caller's behalf — donating
Time makes this explicit. Only some Call()s delegate a specific resource (Space,
Field). The common case during an established client-server relationship
(long-lived shared Spaces per D26) is data-words-only — the cap slot is free.

Cases that need both Time donation and a payload cap in the same Call() follow
the same decomposition pattern D26 established for data: the message is the
signal, the pre-established relationship (shared Spaces, pre-granted
capabilities) is the payload. Grant the authority via Send() first, then Call()
with Time and data words. This is not a workaround — it is the structural
pattern of the system.

Adding a second cap slot was considered and rejected: the same conflict
re-appears at N+1 caps. The number of cap slots is a quantitative parameter, not
a structural solution. One slot for the atomic authority-per-request is
structurally right.

### Donation transfers capacity, not priority

D36 settles Time as normalized compute units. D2 places scheduling hints
(priority, CPU/IO classification) on the Observer. Donating a Time cap gives the
server more compute quantum but does not change where the server sits in the
scheduling order.

seL4 MCS donation transfers both budget and priority (the SchedContext carries
both). Here, the D36/D2 split means donation solves the capacity half of
priority inversion (the server has enough compute to reply) but not the priority
half (a medium-priority Observer can preempt the server).

Priority-level inheritance is a D2 scheduling-hint question — orthogonal to Time
donation, and deferred to the D2 exploration of minimum abstract scheduling
properties.

### Queued model interaction

D13 settles queued fields. When the server is not already waiting, the message
(and donated Time cap) enters the queue. Between deposit and pickup, the Time
cap is "in transit" — neither in the caller's aggregate (caller is blocked) nor
in the server's (hasn't received it). Compute capacity is temporarily
unscheduled.

D24 establishes the precedent: Space caps in transit during IPC are unmapped for
both parties. Time caps in transit are unscheduled for both parties. The pattern
is the same.

D13's direct-switch fast path eliminates transit time for the common case
(receiver already waiting). Donation goes directly from caller to server —
equivalent to seL4 MCS. The novel transit state exists only when the message is
actually queued.

---

## Option B rejected: cap-graph tension

D29 was motivated by three convergent paths, including journal 023's cap-graph
completeness principle. Kernel-internal donation creates a Time reference
outside the capability graph for the duration of the Call/reply round-trip —
exactly the kind of kernel-internal reference D29 was designed to eliminate.

The reference is temporary, but it spans multiple syscall invocations (the full
round-trip). This is structurally different from register state held during a
single syscall. Partially contradicts the reasoning that produced D29.

## Option C rejected: foregoes D30's motivating scenario

D30 was settled specifically on the server multi-client Time-holding scenario.
Not adopting donation weakens D30's primary justification. While D30 has
independent support, the design would carry a settled decision whose strongest
argument is undermined.

Additionally, without donation, priority inversion requires either (a)
kernel-internal priority inheritance on D2 scheduling hints — not obviously
simpler than donation — or (b) userspace protocol — A5 tension for scheduling
policy. Neither is clearly better.

## Option D rejected: unnecessary D28 revision

Option D (kernel-injected Time field) solves a crash-safety problem that doesn't
exist (see above). Its sole advantage over Option A — keeping the user cap slot
free — requires revising D28 (a settled decision) to add a new dedicated field.
Every queued message slot grows by ~8 bytes regardless of whether donation is
used. The cost exceeds the benefit.

---

## Decision: Option A

Time donation on IPC is explicit capability transfer via the user cap slot.

**Mechanism:** On Call(), the caller may include a Time cap in D28's user cap
slot. Standard move semantics: the Time cap transfers from the caller's table to
the message, then to the server's table on Receive(). The server holds the
donated Time alongside its own Time caps (D30 multi-Time additive). The server
returns the Time cap in the reply message's cap slot.

**Opt-in:** Donation is optional. A Call() without a Time cap in the user slot
works identically to today's model. The caller chooses whether to donate, and
which of its Time caps to include.

**No kernel enforcement of return.** The kernel does not track or enforce Time
return. If the server doesn't return the Time cap in the reply, it keeps it.
This is a userspace protocol concern — same as: the server could also not reply
at all.

**Scope:** This transfers scheduling capacity (D36 compute units). It does not
transfer scheduling priority (D2 hints). Priority-level inheritance during IPC
is orthogonal and deferred to the D2 exploration.

---

## Archive convergence

The archive (restart-1) had Time donation as a settled concept: claims.toml
"events-carry-resources": "Events can carry resource handles (Time, Space,
routing capabilities). A sender that donates Time cannot run (effectively
blocked)." The archive converges on donation-via-IPC. The current derivation
arrives at the same conclusion through a different path: D30's settling scenario
(server multi-client) plus D28's single-cap-slot analysis, rather than the
archive's first-principles resource-transfer framing.

---

## Costs

- **D28 cap slot consumed by Time donation.** When the caller donates Time, it
  cannot also send a payload cap in the same message. Mitigated by the D26
  decomposition pattern (pre-establish shared Spaces, use data-words-only
  Call()s).

- **Server protocol obligation.** The server must return the Time cap in the
  reply. Not kernel-enforced. A library can standardize the
  receive-Time-return-Time pattern.

- **Partial priority inversion solution.** Donation addresses the capacity
  dimension only. The priority dimension requires a separate D2 mechanism.

---

## What this does NOT settle

- **Priority-level inheritance during IPC.** D2 scheduling-hint question.
  Whether and how the kernel boosts the server's scheduling hints to the
  caller's level during a Call(). Orthogonal to Time donation.

- **Time clonability.** D30's aggregate double-counts clones. Move-only
  semantics for donation are consistent with non-clonable Time. The uniformity
  argument (D23) is now in tension with aggregate correctness (D30). Requires
  its own derivation.

- **Server-side protocol.** Convention for receiving and returning Time caps.
  Userspace library concern.

- **Send() with Time.** Whether non-blocking Send() can carry Time caps in the
  user cap slot. Naturally follows from standard cap transfer — not
  donation-specific, not kernel-specific. The Observer voluntarily transfers a
  resource and continues on its remaining Time caps.

- **Designated donation cap.** Whether the caller designates which Time cap to
  donate explicitly (by handle) or whether the kernel selects one. Under Option
  A, the caller provides a cap handle in the user cap slot — explicit selection
  by the caller.
