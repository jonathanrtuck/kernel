# Journal 070 — Badge condition form

Settles D45's deferred question: "badge condition form (range, bitmask,
predicate)." Also closes D44's deferred note: "badge-filtered receive (noted as
independently interesting, deferred)."

**Decision:** Range conditions (`low <= badge <= high`) for routing rules. No
receive-time badge filtering.

---

## Two mechanisms, not one

The question conflates two distinct mechanisms that must be separated before
analysis:

**Routing condition (D45):** A static predicate embedded in a routing rule,
installed at split time, evaluated by the kernel on every send to a split Field.
Part of D54's sorted-array routing table.

**Receive-time filter:** A hypothetical predicate supplied with a Receive
syscall, letting a receiver skip non-matching messages. Deferred by D44. Not in
the design.

These have different cost profiles, different interactions with the design
graph, and different answers.

---

## Part A: Routing condition form

### What the routing condition must do

From D45 and D54: map badge values to {route to child Field, fall through to
self}. Evaluated once per sent message on a split Field, before enqueuing.

From D1 and D50: total routing evaluation budget ~10–20 cycles (out of ~400).
Each condition check is a sub-cost within an O(log N) binary search.

From D17: badges are 64-bit minter-assigned integers. The condition must work
with any u64 value.

From D45: conditions must be non-overlapping. The routing table is a sorted
array of non-overlapping conditions.

From A3: the condition must work for any workload's badge allocation strategy.

### Options evaluated

**Range: `low <= badge <= high`.** Two CMP instructions (~2–3 cycles). Natural
total order on `low` enables binary search (O(log N)). At 20 splits: ~10 cycles.
At 100 splits: ~14 cycles. Well inside the routing budget. Disjointness
verification is a simple inequality check at split time.

Expressiveness: sequential client IDs, IRQ number ranges, category partitions,
exact match (`low == high`). Cannot express non-contiguous badge sets as a
single entry — but under D17, the minter controls badge values and can structure
allocation to produce contiguous ranges.

**Bitmask: `badge & mask == expected`.** One AND + one CMP (~2 cycles per
check). More expressive for bit-structured badge spaces (e.g., route all badges
with bits [7:4] == 3). But bitmask conditions have no natural total order —
binary search does not apply. Forces O(N) linear scan. At N=20: ~40 cycles for
condition checks alone. At N=100: ~200 cycles — half the 400-cycle fast-path
budget. Disjointness verification requires a satisfiability check (more complex
than range inequality). D54's sorted-array structure must be redesigned.

**Predicate (arbitrary user-supplied).** Foreclosed by A5 (computational model
in kernel interface — interpreter/verifier in kernel) and D1 (unpredictable cost
on hot path). Use cases served by multiple routing rules or userspace dispatch.

**Exact match: `badge == expected`.** Dominated by range. Range expresses
everything exact match does at the same cost and same O(log N) lookup.

### Three independent convergences on range

**Path 1 — D54 binary search compatibility.** The routing table is a sorted
array with binary search. This structure requires conditions with a natural
total order. Range satisfies this; bitmask does not. Switching to bitmask
requires O(N) linear scan, which at moderate split counts consumes an
unacceptable fraction of the fast-path budget. Range is the only condition form
compatible with D54's binary search.

**Path 2 — Expressive sufficiency.** Common badge allocation patterns —
sequential IDs, IRQ ranges, category-per-range partitions — are naturally
range-expressible. Bit-structured badge spaces (category in high nibble,
instance in low nibble) are also range-expressible: category K occupies
`[K << shift, K << shift + (2^width - 1)]`. The cases where bitmask adds unique
expressiveness (odd/even, arbitrary bit selections) require non-contiguous
allocation that is unusual in practice and can use per-source Fields or
userspace dispatch.

**Path 3 — Incumbent.** Every journal entry, the spec text for D45, and D54's
routing entry layout use "badge range" language. The mechanism is called
"badge-range routing." The terminological consistency reflects a design
intuition stable across multiple derivation sessions. Overturning the incumbent
requires positive evidence that range fails a structural requirement, not merely
that bitmask is theoretically more expressive.

### What range cannot do

Non-contiguous badge sets without structural regularity cannot be expressed as a
single routing entry. A server routing badges {1, 3, 5, 7} needs four entries.
Under D17, the minter controls badge values — the correct response (A5, A3) is
to choose a contiguous allocation, not to add kernel complexity.

### Sub-question: range representation

Closed range `[low, high]` vs. half-open `[low, high)` is an implementation
detail not settled here. Half-open is more natural for partitioning
(`[0, 1000) + [1000, 2000)`); closed is more natural for exact match and
documentation. No structural constraint from settled decisions; deferred to
implementation.

Catch-all entry `[0, u64::MAX]`: expressible as a normal range entry, no special
kernel status needed. Unmatched messages fall through to the source's queue (D45
fallback-on-destroy model).

---

## Part B: Receive-time filter

### Why it is not needed

D45 routing already serves the primary use case: routing a subset of messages to
a dedicated Field. The design graph provides the mechanisms that receive-time
filtering would add, without the complications.

### Structural tensions with receive-time filtering

**D13 (queued fields):** Skipping messages in the queue changes O(1) front-
dequeue to O(queue_len) scan per receive. The FIFO queue model expects front
dequeue.

**D15 (senders oblivious):** Skipped messages occupy queue slots. A client whose
messages are being skipped can fill the queue, blocking all other senders.

**D18 (error-to-sender overflow):** When the queue fills with skipped messages,
legitimate senders get errors even though the receiver is running.

**D50 (fast-path):** Each arriving message must be checked against the
receiver's filter condition before deciding to wake the receiver — additional
~5–10 cycles per arrival.

### Prior art

No surveyed kernel has badge-range filtering on a Receive call for queue-based
IPC. seL4 Receive accepts any message; receiver inspects badge in userspace.
L4's `from` parameter is exact-match on sender thread ID (not badge-based). Mach
and QNX have no badge filtering on receive.

### The one remaining use case

A receiver wanting in-queue priority ordering — drain badge-0 administrative
messages before badge-1+ bulk requests — without allocating separate Fields per
priority level. But this is precisely the "skipping messages in queue" problem:
it causes the D13/D18 tensions above. The structurally correct approach is D45
routing to separate Fields, composed with multi-receive (D19 deferred but not
foreclosed).

### Conclusion

D44's "badge-filtered receive" deferral is closed. The use case is served by D45
routing without the queue-semantic, overflow, and fast-path complications.

---

## Prior art summary

| Kernel     | Routing mechanism           | Receive-time filter                        |
| ---------- | --------------------------- | ------------------------------------------ |
| seL4       | None (receiver accepts any) | None (notification bitmask for async only) |
| L4         | Sender-ID exact match       | `from` parameter (thread ID, not badge)    |
| Mach       | Port topology               | None                                       |
| QNX        | Channel/connection model    | None                                       |
| Zircon     | Channel + port aggregation  | None                                       |
| Barrelfish | Channel topology            | None                                       |

No surveyed kernel uses badge-range conditions for queue-based IPC routing. The
mechanism is novel. seL4's notification bitmask (badge-OR for async event
coalescing) is the closest analog, but operates on a different mechanism
(coalescing, not queue routing).

---

## What this settles

- D45's open "badge condition form": **range** (`low <= badge <= high`)
- D44's deferred "badge-filtered receive": **closed, not needed**

## What this does NOT settle

- Range representation: closed `[low, high]` vs. half-open `[low, high)`
  (implementation detail)
- Exact routing entry layout (D54 open item)
- Catch-all entry semantics (expressible as normal range, no special status)

## Exploration source

`.brain/explorations/G07-badge-condition-form/`
