# 031 — Multi-Time: an Observer holds one or more Time capabilities

**Question:** Can an Observer hold multiple Time capabilities, or is it
structurally limited to exactly one?

**Answer:** An Observer holds one or more Time capabilities in its D8 flat
capability table (regular entries, not a reserved slot). Each Time cap
represents a portion of scheduling allocation on a specific logical core. The
kernel maintains a cached per-Observer scheduling aggregate, updated on Time cap
acquisition/loss (cold-path). The per-core scheduler reads the cached aggregate
(O(1), hot-path).

This parallels D27 (flat Space cardinality) in structure: multiple independent
caps with no kernel-tracked hierarchy. The parallel is not mechanical — Time is
fungible within a core while Space is not — but the benefits of multi-cap
holding (multi-source delegation, partial transfer, cross-core reservation)
justify the same structural choice.

---

## Prior work

**Journal 006** (D6) rejected "Multi-Time Observers" in the alternatives table.
The rejection cited the vocabulary: "Explicitly forbidden ('not as a single
Observer with multiple Times')." However, the vocabulary's context was the SMT
paragraph (spec.md lines 94-97): "SMT-concurrent workloads... are expressed as
multiple Observers sharing a Space, each with its own Time on its own logical
core — not as a single Observer with multiple Times." This was about preventing
multiple independent execution streams within one Observer — not about additive
resource claims on a single execution stream.

**Journal 030** (D29) flagged "exactly one Time" as an unexamined vocabulary
assumption. Noted the Space parallel and deferred cardinality.

**Journal 028** (D27) settled flat Space cardinality on five convergent grounds:
D8 (flat table), D6 (grouping is policy), D4 (no implicit authority), D11 (no
cascade), A3 (no tree assumption).

No surveyed system in research/time-as-kernel-object.md gives a thread multiple
independent time allocations. All use 1:1 binding (seL4 MCS, KeyKOS, Zircon,
QNX, L4, Mach, Plan 9, Barrelfish, Composite, EROS).

---

## Derivation

### Fungibility breaks the D27 mechanical parallel

The vocabulary defines Space as "not fungible once allocated" (object identity)
and Time as "fungible within a logical core." For Space, multiple caps designate
fundamentally different objects at different VA bases — independently useful.
For Time, multiple caps on the same core are claims on the same fungible
resource — collectively useful. Two 10% Time caps are semantically equivalent to
one 20% cap.

This means D27's five arguments do not transfer mechanically. D27 depends on
Spaces being independently useful (D26 per-Space VA bases, D4 independent
designation). Time caps are not independently useful — they are additive.

The question is therefore genuinely open: the D27 parallel is suggestive but not
forcing.

### The server scenario settles it

A server receiving Call() from clients A and B can receive Time caps from each
client (via IPC cap transfer or kernel-internal donation). Under multi-Time, the
server holds both Time caps simultaneously. On reply to client A, the server
returns A's Time cap. On reply to B, it returns B's Time cap. The bookkeeping is
automatic — each Time cap is a distinct handle.

Under single-Time, the server can hold only one Time cap. Receiving B's Time
while holding A's requires either:

1. **Kernel-internal donation** (invisible to the cap graph) — breaks the
   cap-graph completeness principle (journal 023).
2. **Explicit merge** — the server merges A's and B's Time, then must split and
   return the correct amount to each client. This requires the server to track
   how much Time came from each source — protocol complexity pushed to userspace
   (A5 tension).
3. **Replace-on-receive** — the kernel swaps A's Time for B's, stashing A's
   somewhere. Requires new kernel mechanism.

All three alternatives create complexity to enforce a restriction (single-Time)
whose only benefit — simplicity — is already addressed by the cached aggregate.
Multi-Time absorbs multi-source delegation automatically.

### Costs are minimal

**Hot-path cost: zero.** The kernel maintains a cached scheduling aggregate (one
field on the Observer struct), updated on Time cap add/remove (cold-path). The
scheduler reads this cached value — identical cost to a reserved-slot lookup.

**Cap-table cost: marginal.** Time caps consume regular entries (1-3 per
Observer typically). Cap tables grow dynamically (D8: table-full fault triggers
growth).

**Aggregate update cost: O(1) per mutation.** On Time cap add:
`total += cap.amount`. On remove: `total -= cap.amount`. The kernel already
knows which cap is being mutated — no scanning required. Same pattern as D26's
page table update on Space cap acquisition/loss.

**D29 revision: wording only.** "Reserved slot" → "regular cap-table entries."
Time remains capability-held, in the cap table, with D11 lifecycle management.
The fault handler (D21) retains its reserved slot. Only the slot placement
changes.

### D6 rejection is superseded for this interpretation

D6's rejected alternative "Multi-Time Observers" addressed multiple execution
streams (the SMT paragraph). Multi-Time as additive resource claims on a single
execution stream is a different concern. The SMT paragraph's core commitment —
"multiple concurrent workloads are multiple Observers, not one Observer with
multiple execution points" — is preserved under multi-Time. The Observer still
has one register state, one PC, one execution stream. It simply holds claims to
more scheduling allocation.

---

## Archive convergence

The archive (restart-1) explicitly considered multi-Time:

- claims.toml "event-resource-slots" (line 1035): "If an Object holds multiple
  Time fragments, it combines them before sending." Acknowledged that an Object
  could hold multiple Time fragments.
- claims.toml "dynamic-resource-bindings" (line 102): "Resource bindings (memory
  objects and time objects to Objects) are dynamic — they can be added and
  removed at any time."
- spec.md line 245: "Time subdivides into Time." (Subdivision = split.)

The archive converges: Time fragments are holdable and combinable. The archive's
IPC constraint (one Time per event) is consistent with D28 (one user cap slot
per message).

---

## Costs

- **Vocabulary revision.** "Exactly one Time" → "one or more Times." The SMT
  paragraph must be revised to preserve its concern (no multi-execution-stream)
  while allowing additive resource accumulation.

- **D29 revision.** "Reserved slot" → "regular cap-table entries." The fault
  handler retains its reserved slot (D21).

- **D6 note.** The rejected-alternative "Multi-Time Observers" must be annotated
  to clarify that the rejection addressed execution streams, and D30 settles
  multi-Time as additive resource claims on a single execution stream.

- **Cached aggregate.** One additional field per Observer struct. Cold-path O(1)
  bookkeeping on Time cap mutations.

- **Novel position.** No surveyed system provides multi-time-object per
  execution unit. The landscape is 100% single-binding. This is a novel
  position, justified by the server multi-client scenario and the low cost.

---

## What this does NOT settle

- **Time parameters.** What a Time object carries (budget, fraction,
  claim-to-participate). Interacts with how the aggregate works.

- **Time clonability.** D23 uniformity suggests clonable. Not re-examined here.

- **Time creation authority.** Per-core Time manager. Unchanged by cardinality.

- **Time donation mechanism.** Multi-Time makes donation via explicit cap
  transfer more natural (the client's Time cap appears in the server's table),
  but kernel-internal donation on Call() remains an option. Deferred.

- **Cross-core Time holding.** An Observer can now hold Time on multiple cores.
  At any instant, it runs on one core and uses that core's Time aggregate. Time
  on other cores is reservation for migration. The scheduler only consults the
  current core's aggregate. Interaction with D2 migration story needs future
  derivation.

---

## Rejected alternatives

| Alternative                       | Reason                                                                                                                           |
| --------------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| Single-Time, no split (A1)        | Forecloses granular delegation and multi-source holding. Creates protocol complexity for server multi-client scenario.           |
| Single-Time with split/merge (A2) | Granular delegation via split, but cannot hold multiple sources simultaneously. Server must merge/unmerge — protocol complexity. |

---

## Axioms not load-bearing here

**A1 (Rust):** Not load-bearing. Rust handles either model.

**A2 (ARM64):** Not load-bearing. Timer hardware is indifferent to Time
cardinality.

**A4 (reactive):** Not load-bearing. Neither model requires background
management.

**A3 (generic):** Mildly load-bearing — multi-Time is more generic (no workload
assumption about Time source count). But A3 was not the discriminating argument;
the server scenario was.

**A5 (leaf node):** Load-bearing as the deciding factor. Single-Time pushes
multi-source coordination complexity to userspace (merge protocol). Multi-Time
absorbs it in the kernel (cached aggregate, automatic on cap mutations). A5
favors the kernel absorbing this mechanism complexity.
