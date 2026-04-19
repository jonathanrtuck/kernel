# 019 — Multi-endpoint wait: badge fan-in is sufficient

**Date:** 2026-04-18. **Starting point:** D13 revisit trigger #3: "the
multi-endpoint wait problem has no clean solution." D18 resolved trigger #1
(coalescing); trigger #2 (priority inversion/deadlock from bounded queues)
remains as a D2 downstream. This exploration addresses trigger #3 directly.

---

## The question

How does an Observer wait on multiple endpoints simultaneously? What mechanism —
if any — does the kernel provide?

---

## The key finding: badge fan-in covers the common patterns

D15 (unidirectional, many-to-many endpoints) and D17 (minter-assigned badges)
together solve many-sources-to-one-endpoint wait. A server can consolidate
structurally different traffic onto a single endpoint:

- **Client requests:** clients hold badged send caps (D17), server distinguishes
  by badge.
- **Fault messages (D12):** kernel deposits with per-Observer fault badges.
- **Timer/interrupt signals:** signaling services hold badged send caps.
- **RPC replies (D16):** send-once caps targeting the same endpoint, badge-
  distinguished.

The receiver does one receive() call and gets any pending message with its
badge. No multi-endpoint wait is needed — the multiplexing is per-source on a
single object.

This is not a new mechanism. It is a consequence of D15 + D17 that was not
recognized when D13 first listed multi-endpoint wait as an open question. At
that point, neither D15 nor D17 were settled.

---

## The residual problem

Badge fan-in does not help when an Observer must receive from _structurally
distinct endpoints_ — endpoints that cannot share an object because they have:

- Different queue capacities (high-capacity service endpoint vs. small fault
  endpoint)
- Different tracking configurations (D17 opt-in per-badge tracking on one, not
  the other)
- Different capability distribution (one endpoint shared in a D15 many:many
  worker pool, the other private)
- Different owners (one endpoint created by the Observer, the other created by a
  parent and passed in)

---

## Options considered

Four mechanisms were evaluated against the settled constraint set:

**Option A: No kernel primitive.** Badge fan-in for consolidatable sources.
Thread-per-source (additional Observers) for structurally distinct endpoints.
Zero interface cost. Residual cases pay one Time (D6) per additional endpoint.

**Option B: Port set (new kernel object type).** A capability-designated
aggregator of receive rights. receive(port_set_handle) blocks until any member
has a message. Most general. Highest interface cost: sixth kernel object type,
~4 new syscalls, cross-object lifecycle interactions with D11.

**Option C: Multi-receive syscall (stateless).** receive(handles[], count)
resolves N handles, checks all queues, blocks on all. No persistent kernel
state. One new syscall, no new object type. Per-call handle resolution cost.
Variable-length argument convention needed.

**Option D: Endpoint binding (N=2).** One Observer field (bound_endpoint).
receive(primary) also checks the bound endpoint. Covers the canonical case
(pager: fault + service). O(1) fast-path cleanup. One new syscall. Does not
generalize past N=2.

All four mechanisms share a structural cost: O(N) somewhere in the receive path.
For the port set and multi-receive, O(N) appears in queue checking or handle
resolution. For binding, N=2 makes it O(1). For no-primitive, the cost is zero
(no multi-wait in the kernel).

---

## Why no kernel primitive

Two observations converge:

**1. Badge fan-in's coverage is broader than the original framing assumed.**
When D13 listed multi-endpoint wait as an open question, D15 and D17 were
unsettled. With both now settled, badge fan-in handles the patterns that
motivated the question: servers handling mixed traffic, pagers receiving faults
alongside service requests, event loops aggregating timers and IPC. The
canonical motivating case (pager: fault endpoint + service endpoint) dissolves —
the pager consolidates both onto one endpoint with badge-distinguished messages.

**2. The residual cases are narrow and thread-per-source is proportionate.** The
cases where endpoints genuinely cannot be consolidated (different capacity,
different tracking, worker-pool membership) are structurally uncommon. A server
in a shared worker pool that also has a private management endpoint is the
strongest example — and even there, the management traffic is low-frequency
enough that a dedicated Observer (one additional Time) is a proportionate cost.

Adding a kernel mechanism for these residual cases would be premature: the
mechanism's interface cost (new object type, new syscalls, new lifecycle
interactions) is disproportionate to the frequency of the need. Applying "push
complexity to the leaves" fractally: multi-endpoint coordination is leaf-node
complexity that userspace (via additional Observers sharing a Space, D10) can
handle with existing primitives.

A5 does not demand a kernel mechanism here because the complexity being pushed
to userspace (thread creation + shared-memory synchronization) is not essential
in the A5 sense — it is reducible to existing kernel primitives (Observer
creation, Space sharing via D10, IPC between co-located Observers). The kernel
provides the building blocks; userspace composes them.

---

## Forward note: multi-receive is not foreclosed

The stateless multi-receive syscall (Option C) can be added at any time without
architectural disruption. It introduces no new kernel object type, no new
lifecycle interactions, and no changes to the endpoint object. The only
implementation consideration: the Observer's internal wait state should
accommodate blocking on N endpoints (a waiters-list entry per endpoint, O(N)
cleanup on wakeup). If the initial implementation assumes single-endpoint
blocking, adding multi-receive later requires reworking that data structure.

**Design recommendation:** when implementing the Observer wait state, use a
structure that supports a list of waited endpoints (even if the initial syscall
surface only uses N=1). This preserves the option to add multi-receive without
rework.

Call() (D16) is unaffected by this deferral. If multi-receive is added later, a
caller that wants "RPC + keep serving" decomposes Call() into send() (with
send-once reply cap to its service endpoint) + multi_receive(). Call() itself
stays the simple synchronous shortcut.

---

## Effect on D13 status

D13 revisit trigger #3 ("multi-endpoint wait problem has no clean solution") is
resolved. The solution is: badge fan-in via D15+D17 covers the common patterns;
thread-per-source handles residual cases; multi-receive is deferrable and not
foreclosed.

D13's remaining trigger is #2: bounded queue capacity creates unsolvable
priority inversion or deadlock patterns. This is a downstream concern of
priority/scheduling interaction (D2).

---

## Archive convergence

The archive (restart-1) does not contain a dedicated multi-endpoint wait
exploration. The archive's IPC model (journal/011) mentions port sets in passing
but does not derive a decision. No convergence or divergence to check.
