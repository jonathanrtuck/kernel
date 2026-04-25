# D78 — IPC message ownership

**Question:** Who holds the message at each point in the send / receive / call /
reply_recv paths? Where does it live when "in transit"?

**Rests on:** D13 (queued fields), D16 (reply via send-once), D28 (message
format), D50 (fast-path conditions), D74 (register pass-through).

**Status:** settled.

---

## Context

The IPC operations move a Message through several ownership stages: sender
registers, kernel structs, field queues, receiver registers. Before this
derivation, the ownership transfers were implicit — `send()` consumed the
Message by value but dropped it on the `WokeReceiver` path without delivering
it. The dispatch layer had no way to deliver the message to a woken receiver's
registers because the message data was lost.

D74 established that x0–x3 pass through in physical registers on the fast path,
and D76 established the write helpers (`write_message_to_registers` for slow
path, `write_metadata_to_registers` for fast path). This derivation settles
which path constructs a Message struct, which does not, and how ownership is
tracked through the return types.

## Decisions

### Message ownership through each path

#### Send (slow path only — never D50-eligible)

```text
Sender registers → Message construction (dispatch) → send(field, message)
  → WokeReceiver(observer, message): message returned to dispatch for delivery
  → Enqueued: message ownership transferred into queue
```

`send()` takes `Message` by value. On the `WokeReceiver` path, the message is
returned in the enum variant so the dispatch layer can deliver it to the
receiver's registers via `write_message_to_registers`. On the `Enqueued` path,
the message moves into the queue — ownership complete.

#### Receive

```text
Queue → dequeue → Received(message): message returned to dispatch
  → dispatch writes to receiver's registers via write_message_to_registers
Blocked: receiver added to waiters list, no message
```

No change from prior design. The Message comes out of the queue and the dispatch
layer writes it to registers.

#### Call — fast path (D50 conditions met)

```text
Sender registers → NO Message construction for data words
  → call(field, message_without_cap, badge) → DirectSwitch(observer)
  → dispatch writes only x4–x7 via write_metadata_to_registers
  → x0–x3 pass through in physical registers (D74)
  → DispatchResult::ResumeFastPath
```

D50 condition 4 guarantees no user cap. D74 guarantees x0–x3 pass through. The
kernel does NOT need to construct a Message struct for data words on this path.
`DirectSwitch` carries only the observer pointer. The dispatch layer reads label
from x4, badge from the cap entry, writes x4–x7 metadata to the receiver's
RegisterState, and returns `ResumeFastPath` so the restore assembly skips
loading x0–x3.

Note: the current `call()` implementation still takes `Message` by value even on
the fast path, and the message is dropped. This is acceptable because the
dispatch layer can avoid constructing a full Message when it detects fast-path
conditions before calling `call()`. The Message param is needed for the
slow-path fallback (waiter not present → enqueue).

#### Call — slow path with cap (waiter present)

```text
Sender registers → Message construction (dispatch) → call(field, message, badge)
  → WokeReceiverSlowPath(observer, message): message returned for delivery
  → dispatch writes to receiver's registers via write_message_to_registers
  → cap transfer through table installation
```

New variant: `CallOutcome::WokeReceiverSlowPath(observer, message)`. When the
message carries a user cap (D50 condition 4 fails) but a waiter IS present, the
waiter is still popped (message bypasses the queue) but delivery requires the
slow path for cap transfer. The message is returned to the dispatch layer.

This is distinct from the previous behavior where user-cap messages with a
waiter would enqueue and leave the waiter stranded. The new behavior is correct:
the waiter should be woken regardless of cap presence. The cap just means the
fast-path register pass-through cannot be used.

#### Call — slow path without waiter

```text
Sender registers → Message construction → call(field, message, badge)
  → Enqueued: message ownership into queue
  → caller blocks on reply field
```

No change. Message goes into the queue.

#### ReplyRecv

```text
Reply phase:
  → waiter on reply_field: ReplyDelivery{client, message} returned
  → no waiter: message enqueued or dropped (full queue)

Receive phase:
  → same as Receive above
```

`reply_recv` now returns `ReplyRecvOutcome` containing both
`reply_delivery: Option<ReplyDelivery>` and `receive_outcome: ReceiveOutcome`.
When the reply field has a waiting client, the reply message is returned via
`ReplyDelivery` so dispatch can deliver it to the client's registers. Previously
the waiter pointer was discarded.

### Interface changes

1. **`SendOutcome::WokeReceiver`** — now carries `(NonNull<Observer>, Message)`
   instead of just `NonNull<Observer>`.

2. **`CallOutcome`** — new variant
   `WokeReceiverSlowPath(NonNull<Observer>, Message)` for waiter-present +
   user-cap case.

3. **`reply_recv`** — returns `ReplyRecvOutcome` (new struct) instead of
   `ReceiveOutcome`. Contains `reply_delivery: Option<ReplyDelivery>` and
   `receive_outcome: ReceiveOutcome`.

4. **New types:** `ReplyRecvOutcome`, `ReplyDelivery`.

### Behavioral change: call with user cap and waiter

Previously, `call()` with a user cap skipped the waiter check entirely and
always enqueued. Now, `call()` checks for a waiter even when a user cap is
present, and returns `WokeReceiverSlowPath` if a waiter is found. The waiter is
popped and the message bypasses the queue.

Rationale: leaving the waiter stranded when a message-with-cap arrives creates
unnecessary latency. The receiver should be woken regardless — the cap just
means delivery goes through the slow path (cap table installation) instead of
the fast path (register pass-through).

### What the dispatch layer does with each outcome

| Outcome                                           | Dispatch action                                                                                                         |
| ------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `SendOutcome::Enqueued`                           | Clear sender's carry flag, continue sender                                                                              |
| `SendOutcome::WokeReceiver(obs, msg)`             | Write msg to receiver registers via `write_message_to_registers`, clear receiver's carry flag, schedule receiver        |
| `ReceiveOutcome::Received(msg)`                   | Write msg to receiver registers via `write_message_to_registers`, clear carry flag, resume receiver                     |
| `ReceiveOutcome::Blocked`                         | Transition receiver to Blocked state, schedule_next                                                                     |
| `CallOutcome::DirectSwitch(obs)`                  | Write metadata to receiver via `write_metadata_to_registers`, block caller on reply field, return `ResumeFastPath(obs)` |
| `CallOutcome::WokeReceiverSlowPath(obs, msg)`     | Write msg to receiver via `write_message_to_registers`, install cap, block caller, schedule receiver                    |
| `CallOutcome::Enqueued`                           | Block caller on reply field, schedule_next                                                                              |
| `ReplyRecvOutcome { delivery: Some(..), .. }`     | Write reply msg to client registers, wake client                                                                        |
| `ReplyRecvOutcome { delivery: None, .. }`         | Reply was enqueued or dropped                                                                                           |
| `ReplyRecvOutcome { .., receive: Received(msg) }` | Write msg to server registers, resume server                                                                            |
| `ReplyRecvOutcome { .., receive: Blocked }`       | Server blocks on recv_field                                                                                             |

### Fast-path Message struct avoidance

On the Call fast path (D50 all conditions met), the dispatch layer can avoid
constructing a full `Message` struct. It reads only:

- label from x4 (IpcRegisters)
- badge from the cap entry (already resolved)
- user_cap_slot = u64::MAX (sentinel, no cap)
- reply_cap_slot from the reply cap installation

These are passed directly to `write_metadata_to_registers`. x0–x3 stay in
physical registers. No 128-byte Message struct is constructed or copied.

## Rejected alternatives

**Deliver inside `send()`/`call()`.** Would require the communication module to
know about RegisterState and frame/ helpers. Violates the module boundary:
communication handles queue mechanics, frame/ handles register access, dispatch
ties them together.

**Always enqueue, let receive deliver.** Forces all messages through the queue
even when a receiver is waiting. Destroys the D13 direct-switch optimization
(~400 cycle target becomes ~600–800).

**Carry message in DirectSwitch.** Unnecessary — D50 condition 4 guarantees no
cap, D74 guarantees x0–x3 pass through. The only data the dispatch layer needs
is metadata (label, badge, cap slots), which it already has from register reads
and cap resolution.
