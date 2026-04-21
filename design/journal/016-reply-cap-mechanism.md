# 016 — Reply via pre-allocated reply field with send-once cap

**Date:** 2026-04-18 **Starting point:** D15 settled unidirectional,
many-to-many fields and listed reply-cap mechanism as the first downstream
question: "One-shot (kernel mints during call, auto-revoked after reply) vs.
persistent (client creates reply field, transfers send cap explicitly)."

---

## The question

How does RPC reply routing work over unidirectional fields? D15 accepted
"reply-cap transfer per RPC" as a cost. The question is: who creates the reply
object, what kind of object is it, what is its lifecycle, and how does it
interact with the fast path?

---

## D14's impact on the design space

The archive (journal/011) unified IPC reply and fault resume from the sender's
perspective: both were "send to a reply cap." The unification argument was
load-bearing for choosing Option A (client's reply field) — it made the sender's
interface identical for both cases.

D14 has since settled fault resume as `resume(observer_handle)`, a typed kernel
syscall (D7). The pager receives the Observer handle via capability transfer in
the fault message, then calls resume(). IPC reply is send-to-field; fault resume
is a typed kernel op. They are different mechanism families per D7. The
unification no longer holds.

Consequence: the reply-cap mechanism only needs to serve IPC RPC. Arguments
based on fault-resume compatibility are no longer load-bearing. The archive's
rejection of one-shot reply caps ("doesn't compose from existing mechanisms")
was strongest when the composability enabled a unified sender interface — with
D14, that motivation is gone.

---

## Options considered

### Option 1: Persistent reply field (archive's choice)

Client creates a regular field, holds receive cap, transfers a send cap in every
RPC request. Server sends reply to the transferred cap. Uses only existing
mechanisms.

- No new kernel types (five types unchanged)
- Standard D8 cap table entries, standard D11 revocation
- Costs: per-client reply field, per-RPC cap transfer, client manages lifecycle,
  reply path goes through field queue, server can retain send cap after reply
  (capability leak vector)

### Option 2: One-shot kernel-minted reply cap (seL4 MCS style)

Client issues Call(). Kernel mints a Reply capability — a lightweight object
naming the blocked caller directly (not a field). Server sends reply via the
Reply cap; cap is consumed.

- Reply path bypasses field queue (names caller directly)
- Minimal cap table pressure (1 Reply per Observer, reused)
- Costs: new kernel type (Reply, sixth type), seL4's known reply-cap-stealing
  vulnerability must be addressed (MCS fixed with explicit Reply objects)

### Option 3: Pre-allocated reply field with send-once cap

Each Observer has a pre-allocated reply field (a regular field). D8's rights
mask gains a send-once right: use-limited attenuation that applies to any field
cap — consumed after one send. On Call(), the kernel creates a send-once cap to
the caller's reply field, includes it in the request message, and blocks the
caller on the reply field. Server sends reply to the send-once cap; cap is
consumed.

- No new kernel types (reply field is a regular field)
- Send-once right is a general-purpose attenuation, not reply-specific
- Kernel can optimize the reply fast path internally (bypass queue for known
  reply-field patterns — implementation detail, not exposed in object model)

---

## Why Option 3

Two observations converge:

**1. The reply field IS a field.** A dedicated Reply type (Option 2) names the
blocked caller directly, bypassing the field queue. But the fast-path bypass is
an optimization, not a categorical difference. A field with exactly one waiter
and a send-once cap is structurally isomorphic to a Reply cap — the kernel knows
the caller is waiting (it set up the Call()) and can optimize the reply path
behind the field interface. The optimization is real; the new type is not
structurally required.

**2. Send-once is a general-purpose capability concept.** Use-limited
attenuation is not reply-specific. Send-once rights have independent use cases:
one-shot event notifications (fire once, cap dies), single-use authorization
tokens, edge-triggered interrupt delivery, acknowledgment/receipt patterns. Mach
has send-once rights as a fundamental part of its port model. EROS resume keys
are effectively send-once. Adding send-once to D8's rights mask extends the
attenuation hierarchy D4 provides — it is a general mechanism that happens to
serve reply, not a reply-specific mechanism.

**Structural parallel with D14.** Both IPC reply (D16) and fault resume (D14)
follow the same message-level pattern: the receiver gets a caller-specific
response capability in the message. The mechanism families differ per D7 (IPC
reply is send-to-field; fault resume is resume(observer_handle)), but the
message shape is parallel. A pager Observer that handles both faults and user
RPCs sees a consistent pattern: every incoming message carries a response
capability naming exactly one caller.

---

## Rejected alternatives

| Alternative                    | Rejected because                                                                                                                                        |
| ------------------------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| One-shot Reply type (Option 2) | New kernel type not structurally required — a field with send-once achieves the same function; fast-path bypass is an optimization behind the interface |
| Persistent send cap (Option 1) | Server retains reply path after RPC (capability leak); no use-limiting; archive chose this but without send-once, which addresses the retention concern |
| Badge-based reply              | Foreclosed by D4 — reply-by-badge is ambient addressing through identity, not capability designation                                                    |
| Synchronous MsgSend/MsgReply   | Foreclosed by D13 — synchronous-only contradicts queued model; caller can block on reply field voluntarily to achieve the same effect                   |

---

## Archive convergence

The archive (journal/011) chose Option A: client's reply field with a send cap
transferred in the request. Option 3 is a refinement of the same approach: same
object model (reply field is a regular field), but with send-once attenuation on
the transferred cap. The archive did not have send-once; the client transferred
a regular send cap, which the server could retain after reply. Send-once
addresses this: the cap is consumed on use, preventing post-reply retention.

The archive rejected one-shot reply caps (Option B) because they "don't compose
from existing mechanisms." Option 3 preserves this value — send-once is a right
on a standard capability to a standard field, not a new mechanism. The one-shot
behavior composes from the existing capability + field model with one additional
right.

---

## The decision

**Pre-allocated reply field with send-once cap (Option 3).**

Each Observer has a pre-allocated reply field — a regular field (D15). The D8
rights mask gains a **send-once** right: a use-limited attenuation where the
capability is consumed after one send operation. On Call(), the kernel creates a
send-once cap to the caller's reply field, includes it in the request message,
and blocks the caller on its reply field. The server sends the reply to the
send-once cap; the cap is consumed. The reply field persists (pre-allocated,
reused across RPCs); the cap is ephemeral.

The kernel is free to optimize the reply fast path behind the field interface
(bypassing the queue structure when the kernel knows the caller is the sole
waiter). This is an implementation optimization, not an object-model commitment.

Send-once is a general-purpose right, not reply-specific. It extends D4's
attenuation hierarchy with use-limiting and has independent applications:
one-shot notifications, single-use authorization, edge-triggered interrupt
delivery.

---

## Axioms not load-bearing here

**A1 (Rust)** is not load-bearing. Rust's type system can express send-once
semantics (affine types align naturally), but the mechanism choice doesn't
depend on the implementation language.

**A2 (ARM64)** is not load-bearing. The register file provides the fast-path
message container but does not distinguish reply mechanisms.

**A3 (generic)** provides background motivation (RPC is the dominant IPC
pattern, all workloads need it) but does not choose between options.

---

## What remains open

- **Call() and ReplyRecv() syscall details.** Part of the specific syscall
  surface question. Call() semantics are defined here (send + block on reply
  field); ReplyRecv() (atomic reply + wait for next request) is a natural
  companion. Exact signatures and flags are deferred.
- **Reply field per Observer policy.** Pre-allocated at Observer creation, or
  created on first Call()? Pre-allocation is simpler (no hot-path allocation);
  lazy creation avoids waste for Observers that never do RPC.
- **Send-once right encoding.** How send-once is represented in D8's rights mask
  (a right bit, or a modifier on the send right). Encoding detail, deferred with
  entry layout.
- **Shared reply field with badge disambiguation.** An Observer could use one
  reply field for all servers, with badges distinguishing which server's reply
  arrived. Requires badge semantics (unsettled). Alternative: one reply field
  per Observer (simpler, no disambiguation needed).
- **Message format interaction.** The send-once cap must fit in the message's
  capability slots. The Call() syscall must encode "include my reply cap in slot
  N." Deferred with message format.

---

## Status

**Settled.**
