# Badge Assignment — 2026-04-12

Twelfth exploration. Settled the remaining badge question from journal 010: who
assigns badges, what purpose they serve, and where they live. The answer falls
out of a sharper definition of what badges are for.

## Starting point

Journal 010 defined badges as an integer on a capability, set at clone time,
attached to every message the kernel delivers through that cap. Unforgeable by
the sender. Open questions at the end of that journal:

- Minter-assigned vs. kernel-auto. (Leaning minter-assigned.)
- Which scenarios actually need badges.

Journal 011 flagged the same question from the fault path: "the control Endpoint
capability needs a badge for the handler to distinguish which Context faulted."

## Scenario walk: when are badges needed?

Two conditions must both hold for a badge to earn its keep:

1. The receiver's behavior depends on **which sender** sent the message.
2. Multiple senders share the **same Endpoint**.

If either fails, badges are dead weight. Rules out:

- **1:1 Endpoints** — receiver already knows the sender.
- **Stateless services** — behavior doesn't vary by sender.
- **Bulk data transfer** — runs over shared memory; trust established at share
  time.
- **Reply targeting** — handled by Option A (reply cap in message), not badges.

The scenarios where badges are load-bearing:

- **Fault handler serving multiple children.** One handler, N children pointing
  their `fault_handler` at the same Endpoint. The handler needs to know which
  child faulted to look up per-child state.
- **Server keying per-client state.** One service Endpoint, many clients. The
  server's per-client table is keyed by badge.
- **Authorization by role.** Admin caps carry one badge, user caps carry
  another; server dispatches on badge. (Cheaper than minting separate Endpoints
  per role.)

## Distinguish vs. identify

The load-bearing scenarios expose a subtlety: badges need to **identify**, not
merely distinguish.

- **Distinguish** — "Alice ≠ Bob." Any unique value per clone suffices.
- **Identify** — "this is Alice, the one with account #42." The badge value must
  correspond to state the receiver already has.

A server keying per-client state needs the badge to map to a row in its table. A
fault handler needs the badge to map to a specific child. Both require the
_receiver_ to control the badge's meaning — randomly-generated distinguishers
would force the receiver to maintain a badge → identity side table, which is
exactly what the badge was supposed to be.

Identification requires receiver-chosen values. Which forces:

## Minter-assigned

The minter — whoever calls `clone(handle, badge: N)` — chooses the value.

The kernel enforces **mechanism** only: badges exist on caps, are immutable
after clone, unforgeable by senders, attached to every message.

The **policy** of what badges mean and who assigns them is userspace. The useful
pattern is: the eventual receiver holds the template cap and mints per-client
copies, because they own the key space. But the kernel doesn't enforce that — a
middleman could mint, with the implication that the receiver now trusts the
middleman's badge choices.

Kernel-auto was rejected because it only supports distinguishing. If the kernel
picks opaque IDs, the receiver has to maintain a separate table mapping kernel
IDs to meaningful identities — defeating the purpose.

## Badges live on the referrer

A cap is `(object_ref, rights, badge)`. The badge is a field on the **referrer**
(the cap), not the **referent** (the Endpoint).

Every clone creates a new cap with its own badge. Multiple caps to the same
Endpoint carry different badges. That's what makes badges useful: a single
Endpoint carries distinguishable senders because each sender's cap is stamped
differently.

If badges lived on the Endpoint, every sender would attach the same value —
equivalent to "the Endpoint's ID," which the receiver already knows from holding
a cap to it.

This tightens the language from journal 010 ("identifies the capability, not the
Context"), which was imprecise because every clone creates a new capability.
Cleaner statement: **badge is a per-cap field, set by the minter at clone time,
attached to every message sent through that cap.**

## Trust model consequence

Because the kernel enforces unforgeability but not badge semantics, a receiver
trusts that badges mean what the minting chain said they mean. If a manager
mints a cap with badge = "child_7" and installs it as a child's `fault_handler`,
the handler believes faults stamped with that badge came from child_7. If the
manager lies or is compromised, the handler gets a false story about who
faulted.

This is the kernel's correct posture: unforgeability at the IPC layer, semantics
delegated to userspace. But it should be noted that **badges are only as
truthful as the minting chain** — useful for structural prevention of sender
impersonation, not for verifying claims the minter itself might fabricate.

## Consequence for the fault path

IPC messages read the badge from the sender's cap. The fault path has no sender
cap — the kernel synthesizes fault messages using its `fault_handler` direct ref
on the Context.

For identification to work on fault messages, the Context model must store a
badge alongside the fault handler reference. Whoever installs the handler (the
manager wiring `fault_handler = endpoint_X`) also chooses a badge. The
`fault_handler` field becomes `(endpoint_ref, badge)`, or a sibling field
`fault_badge` is added.

Either shape works; pick whichever reads better when the Context model is next
revisited. The decision is: **the minter-assigned pattern extends to the fault
path by having the handler-installer supply both the endpoint ref and the badge
at wiring time.**

## Open sub-questions

- **Badge value shape.** Size (32-bit? 64-bit?), null/default value, collision
  behavior if a minter reuses a value. Likely 64-bit, null = 0, collision is the
  minter's problem (it's their key space). Deferrable.
- **Fault path field shape.** `fault_handler: (endpoint, badge)` vs. two fields.
  Cosmetic; resolve during next Context model pass.
- **Rebadging.** Can a minter clone a cap they already hold, assigning a new
  badge different from the one on their own cap? Yes — otherwise badges couldn't
  be hierarchically redefined by middlemen. The mechanism is "clone with badge
  N" regardless of the source cap's badge.

## Status

**Settled:**

- Badges are minter-assigned. The kernel enforces unforgeability and attachment;
  the minter chooses the value.
- Badges serve identification (key into receiver state), not merely
  distinguishing.
- Badges live on the referrer (the cap), not the referent (the Endpoint). One
  cap, one badge; many caps to one Endpoint, many badges.
- Trust model: badges are as truthful as the minting chain.
- Fault path: the Context model stores a badge alongside `fault_handler`, set by
  whoever installs the handler.

**Open:** badge value shape, exact Context model field shape, rebadging rules
(if any beyond "clone with badge N").
