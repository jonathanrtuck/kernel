# 048 — Syscall enumeration: 5 IPC + 20 typed = 25 operations

Date: 2026-04-23

## Starting point

The "specific syscall surface" open question in spec.md. D7 settled two
mechanism families (IPC + typed kernel operations). D47 settled the ABI
framework (SVC immediate, register convention, two-level numbering). Individual
derivations named operations piecemeal (D14, D28, D35, D39, D41, D44, D45). No
derivation collected the full set or verified completeness.

## Exploration

### Method

Collected every named operation from spec.md by scanning each derivation for
operations characterized as syscalls, typed kernel operations, or rights (D7:
each right = typed kernel operation). Then checked against the research's
irreducible set (§8: IPC, object creation, resource management, lifecycle
control, capability operations) for gaps.

### IPC operations (Family 1 — nonzero SVC immediates)

D13 establishes queued fields. D16 establishes Call/ReplyRecv. D47 lists
illustrative IPC operations. The question: which are genuinely distinct?

**Send.** D13 line 624: "Sender deposits and continues (non-blocking)." D18
settles queue-full as error-to-sender. Send never blocks. It deposits a message
into a Field's queue and returns immediately (success or error-on-full).

**Receive.** D13: blocking wait on a Field. Returns when a message is available.
The only blocking IPC operation from the caller's perspective.

**Call.** D16: send + block on reply field. Kernel creates a send-once cap to
the caller's reply field, includes it in the request message. Compound
operation. Cannot be decomposed into Send + Receive without losing atomicity
(the send-once cap creation is kernel-internal).

**ReplyRecv.** D16: send reply via send-once cap + receive next on same field.
Server loop optimization — saves one kernel entry per RPC round-trip. Without
it, the server does Send(reply) + Receive(field) = two SVC transitions. With it,
one.

**NBSend — rejected (redundant).** In seL4's synchronous model, Send blocks
until the receiver is ready; NBSend tries without blocking. In this kernel's
queued model, Send is inherently non-blocking (deposit-and-continue). NBSend
would be identical to Send. No surveyed queued-IPC system has a separate
non-blocking send variant because the base send is already non-blocking.

**Reply — rejected (redundant).** The server holds a send-once cap to the
caller's reply field. Sending a reply is Send to that cap. The kernel enforces
the send-once property via the rights mask, not via a distinct syscall. Send
doesn't need to know whether the target is send-once or regular. A dedicated
Reply operation would be a strict alias for Send.

This is a structural consequence of D16's design: send-once is a right, not a
mechanism. seL4 needs separate Reply because its reply cap has special handling;
in this kernel, reply caps are regular Field caps with a right bit.

**Yield — included.** Every surveyed microkernel has yield (seL4, Coyotos, L4,
Genode, Zircon). Without it, an Observer that finishes useful work must either:
(a) Receive on an empty Field — semantic abuse of IPC as a scheduling hint, (b)
spin until preempted — wastes cycles and power, or (c) create a zero-delay
Pulsar — heavyweight for a scheduling hint. None are satisfying.

A3 (generic) requires supporting compute-bound workloads that reach natural
pause points between timer ticks. The A5 counter-argument (scheduling is
kernel-internal) applies to scheduling _policy_, not to the hint that work is
complete. Yield is an input to the scheduler, not a policy bypass — the kernel
still chooses what runs next. Parallel: observer_set_scheduling() provides
scheduling hints (R, T, P values) without violating A5; Yield is the same class
of input.

Yield is the only IPC-level operation that doesn't touch a Field. It's a
scheduling hint, not communication. But it's in Family 1 (nonzero SVC immediate)
because it's a hot-path operation — fast-path entry, minimal kernel work
(enqueue current Observer, pick next), fast-path exit. It doesn't carry a
message or target a capability.

**NBRecv — deferred (not foreclosed).** Non-blocking receive (check for messages
without blocking) would enable polling and event-loop patterns. Use cases: queue
drain, mixed compute+IPC, multiplexing Fields without thread-per-source.

Deferred following the D19 pattern: multi-receive was "explicitly not
foreclosed" and the Observer wait-state internals were designed to accommodate
it. NBRecv follows the same template — it's a flag on Receive (one bit of x7) or
a separate SVC number. Nothing in the current design forecloses it. If
polling/event-loop patterns prove painful in practice, add it.

The strongest argument for eventual inclusion: queue drain without blocking is
impossible without NBRecv. An Observer processing a burst of messages has no way
to know when the queue is empty except by blocking on an empty Receive. The
Pulsar-timeout workaround (create a one-shot Pulsar, Receive, destroy Pulsar) is
heavyweight for what should be a lightweight check.

### Typed kernel operations (Family 2 — SVC #0, operation code in x4)

These are more constrained — most are mechanically forced by settled
derivations.

**Observer operations (D39).** Nine rights = nine operations. Fully settled.
observer_resume, observer_install_cap, observer_write_registers,
observer_read_registers, observer_suspend, observer_change_handler,
observer_set_scheduling.

D28's "inspect(observer_handle)" and D39's "observer_read_registers(cap)" are
the same operation. D28 named it tentatively when establishing that full
Observer state is accessible via a typed kernel operation; D39 formalized the
name when deriving the complete rights set. read_registers is the authoritative
name.

**Generic cap operations.** Four operations that apply across object types:

destroy(cap) — D11, D33. Authoritative destruction. Observer destroy cascades
(D33). Space destroy returns pages. Time destroy returns capacity to kernel
pool. Pulsar destroy releases backing. One operation, type-specific kernel
behavior. D33 makes it preemptible (bounded steps with saved continuation for
Observer destroy).

clone(cap, reduced_rights) → new_cap — D23, D39. Creates a duplicate cap with
equal or reduced rights. Per-type: Observer, Space, Field include clone in their
rights sets. Time excludes clone (D38 — linear, conservation invariant). The
kernel checks the target type's rights set.

close(slot) — D11. Relinquish a capability without replacement. Decrements
refcount. If last reference and no destroyer exists, object becomes unreachable
(for resource-reclamation Spaces this means the pages return to the containing
Space's owner; for Fields this means the queue and waiters are released). close
operates on the cap-table slot, not the object — it's a cap-table operation.

mint(cap, badge, reduced_rights) → new_cap — D17. Creates a badged copy of a
Field send cap. The badge is minter-assigned (receiver-controlled values for
sender identification). Mint is a right in D8's rights mask — the caller must
hold a cap with the mint right. The new cap has the specified badge and
(optionally) reduced rights.

**Space operations (D41, D32).** Partially settled:

space_split(cap, size) → new_cap — D41. Extract a portion into a new Space.
Conservation: target shrinks, new Space gets extracted pages.

space_merge(target_cap, source_cap) — D41. Absorb source into target. Source
ceases to exist. VA range extends. Motivated by D40's demand-paging gap.

time_merge was considered. Time is fungible — two Time caps of 100 units each
are functionally identical to one of 200 (D30 additive aggregate). Merge would
be cap-table hygiene (fewer slots) not functional necessity. Space merge has a
functional need (D40 demand paging); Time merge does not. Not included, not
foreclosed.

**Field operations (D45, D32).** Partially settled:

create_field(space_cap) → field_cap — D32. Type conversion: Space consumed,
Field created.

field_split(cap, badge_range, dest_field) — D45. Install badge-range routing.
Senders oblivious. Fallback-on-destroy for crash recovery.

**Time operations (D38):**

time_split(cap, amount) → new_cap — D38. Authority delegation for linear Time
caps. Creates new Time object with a portion of the original's quantity.
Conservation: quantities sum to original.

**Pulsar operations (D44):**

create_pulsar(space_cap, field_cap, badge, deadline, period) → cap — D44, D32.
Type conversion. Armed on creation — no separate arm syscall. Delivery
configured at creation time.

clock_read() → timestamp — D44. For Observers without direct counter access
(CNTKCTL_EL1.EL0VCTEN not set). The only typed operation that takes no
capability argument — it reads the Observer's own clock.

D44: "cancel = destroy via D11." No separate cancel or re-arm operation.
Adaptive timing uses one-shot Pulsars: create → fire → destroy → create.

**Observer creation (D35):**

create_observer(space_cap, handler_field_cap, badge) → observer_cap — D35, D32.
Type conversion. Observer starts inert; configured via install_cap,
write_registers, then resumed.

**Resource acquisition (D31):**

resource_request(type) — D31, D7. Observer explicitly requests more Space or
Time. Kernel converts to a fault message and routes to the fault handler. The
Observer blocks until the handler resolves (install_cap + resume) or denies.

### Completeness check against research §8 irreducible set

| Irreducible category       | Coverage                                                              |
| -------------------------- | --------------------------------------------------------------------- |
| IPC send+receive           | Send, Receive, Call, ReplyRecv                                        |
| Thread/context create      | create_observer                                                       |
| Thread/context yield       | Yield                                                                 |
| Memory map/grant           | space_split, space_merge (+ cap transfer via IPC)                     |
| Capability/resource create | create_field, create_observer, create_pulsar, time_split, space_split |
| Capability/resource revoke | destroy, close                                                        |
| Interrupt delivery         | Via Fields (D22) — no separate operation                              |
| Scheduling control         | observer_set_scheduling                                               |

All irreducible categories are covered.

### Pending downstream

The enumeration is complete for what is currently settled. Three open questions
will add operations when derived:

- Space rights mask — may add inspect-space, resize, or other operations
- Field rights mask — may add inspect-field, configuration operations
- Pulsar rights mask — may add inspect-pulsar or modification operations

These additions will be typed operations (SVC #0); the IPC set is complete.

## Status

Settled. The kernel's syscall surface is:

**IPC (5):** Send, Receive, Call, ReplyRecv, Yield.

**Typed (20):** 7 Observer + 4 generic cap + 2 Space + 2 Field + 1 Time + 2
Pulsar + 1 Observer creation + 1 resource request.

**Total: 25 operations.** NBRecv deferred (not foreclosed). Pending additions
from Space/Field/Pulsar rights masks (typed operations only).
