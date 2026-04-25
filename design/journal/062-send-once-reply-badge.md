# 062 — Send-once reply badge: caller-supplied

Date: 2026-04-24

## Starting point

D16 settled send-once capabilities and the pre-allocated reply field. D17
settled badge semantics. The badge value on kernel-created send-once caps
(specifically the reply cap created during Call) was deferred.

## Exploration

### The answer

Call() takes a `reply_badge` parameter. The kernel embeds the caller-provided
value into the send-once cap entry verbatim. When the server replies, the
message arrives at the caller's reply field carrying that badge.

### Forcing chain

**D16 + journal 019:** One reply field per Observer, shared across all RPCs.
Journal 019 explicitly states: "RPC replies (D16): send-once caps targeting
the same field, badge-distinguished." Badge IS the discriminator.

**D17 receiver-controls-badge:** "The receiver's internal state structure
determines what values are useful." The calling Observer IS the receiver of its
own reply field. The caller holds the call-site context and should control the
badge at Call() time.

**D17 rejects kernel-auto-assigned:** "Opaque values... every receiver with
per-source state needs a translation layer." The caller has per-RPC state;
a kernel-assigned counter would require a counter-to-callsite map.

### Foreclosed alternatives

- **Fixed sentinel (e.g., zero):** Forecloses concurrent RPC disambiguation.
  Inconsistent with journal 019's settled multi-RPC pattern.
- **Kernel-auto-assigned:** Explicitly rejected by D17's reasoning, applied
  directly to this case.

### No correlation with request badge

Request badges identify callers to the server (server namespace). Reply-cap
badges identify RPCs to the caller (caller namespace). Independent.

### Downstream impact

Call()'s syscall encoding (D49) needs a `reply_badge` register parameter. This
is a D49-level follow-on.

## Status

**Settled.** Reply badge is caller-supplied at Call() time. Forced by D16
(single reply field, badge-discriminated) + D17 (receiver-controls-badge).
