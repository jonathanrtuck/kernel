# D79 — Scheduling decision matrix

For each (IPC operation x outcome) pair, what state transitions happen on which
Observers, and who runs next?

## Rests on

D2 (per-core schedulers), D13 (queued fields with direct-switch), D16 (reply via
send-once), D39 (Observer state machine), D48 (5 IPC operations), D50 (fast-path
conditions), D59 (Scheduler trait — 5 methods).

## Derivation

The matrix covers all 9 (operation x outcome) pairs from D48 and D78. For each
row:

- **Sender state**: what happens to the calling Observer's PrimaryState.
- **Receiver state**: what happens to the woken/blocked Observer's PrimaryState.
- **Scheduler calls**: which Scheduler trait methods are invoked.
- **DispatchResult**: what the core manager returns to frame/.
- **Register writes**: what D76 write helpers are called before returning.

### Design principles applied

1. **Sender continues unless it voluntarily blocks.** Send is fire-and-forget
   (D13). Call blocks the caller (D16). Receive blocks if queue empty (D13).
   ReplyRecv's receive side may block (D16).

2. **Woken receivers join the run queue via enqueue.** When a message wakes a
   blocked Observer (Blocked -> Runnable), the scheduler's `enqueue` is called
   unless suspension overrides. The scheduler then decides who runs next via
   `pick_next`.

3. **Fast path bypasses the queue.** D50 direct-switch: the scheduler's
   `should_switch_to` callback replaces `enqueue + pick_next` for the receiver.
   If approved, `ResumeFastPath` is returned. If denied, fall back to
   `enqueue + pick_next`.

4. **Yield re-enqueues before picking.** The implementation plan specifies that
   the yielding Observer is re-enqueued into the run queue before `pick_next`.
   This ensures round-robin fairness: the yielder goes to the tail, not lost.

## The 9-row matrix

### Row 1: Send x Enqueued

Message entered the queue. No receiver to wake.

| Aspect          | Value                            |
| --------------- | -------------------------------- |
| Sender state    | Stays Runnable (fire-and-forget) |
| Receiver state  | No receiver involved             |
| Scheduler calls | None                             |
| Register writes | `clear_ipc_carry(sender)`        |
| DispatchResult  | `Resume(sender)`                 |

The sender continues immediately. No scheduling decision needed — the current
Observer keeps running.

### Row 2: Send x WokeReceiver

A waiting receiver was found. Message delivered directly (bypassed queue).

| Aspect          | Value                                                                                               |
| --------------- | --------------------------------------------------------------------------------------------------- |
| Sender state    | Stays Runnable                                                                                      |
| Receiver state  | Blocked -> Runnable via `unblock()`                                                                 |
| Scheduler calls | `enqueue(receiver)` if unblock returns true (not suspended)                                         |
| Register writes | `clear_ipc_carry(sender)`, `write_message_to_registers(receiver, ...)`, `clear_ipc_carry(receiver)` |
| DispatchResult  | `Resume(sender)`                                                                                    |

D50: Send is NOT fast-path eligible (condition 1: only Call and ReplyRecv). The
sender always continues. The woken receiver joins the run queue for later
scheduling. This matches D13: Send is fire-and-forget.

### Row 3: Receive x Received

Message was available in the queue. Dequeued and delivered.

| Aspect          | Value                                                                    |
| --------------- | ------------------------------------------------------------------------ |
| Sender state    | N/A (receiver is the current Observer)                                   |
| Receiver state  | Stays Runnable (message was immediately available)                       |
| Scheduler calls | None                                                                     |
| Register writes | `write_message_to_registers(receiver, ...)`, `clear_ipc_carry(receiver)` |
| DispatchResult  | `Resume(receiver)`                                                       |

No state transition — the Observer was Runnable, found a message, and continues
with the message in its registers.

### Row 4: Receive x Blocked

Queue was empty. Observer linked into waiters list.

| Aspect          | Value                                                                            |
| --------------- | -------------------------------------------------------------------------------- |
| Sender state    | N/A                                                                              |
| Receiver state  | Runnable -> Blocked via `block()`                                                |
| Scheduler calls | `dequeue(receiver)`, then `pick_next()`                                          |
| Register writes | None (the Observer is now blocked; registers written when it's eventually woken) |
| DispatchResult  | `schedule_next()` (Resume next or Idle)                                          |

The receiver leaves the run queue and blocks. The scheduler picks whoever is
next. If nobody is runnable, the core goes idle (D46 WFI).

### Row 5: Call x Enqueued

Message entered the queue (no waiter present). Caller blocks on reply field.

| Aspect          | Value                                                                            |
| --------------- | -------------------------------------------------------------------------------- |
| Sender state    | Runnable -> Blocked (waiting on reply field)                                     |
| Receiver state  | No receiver woken                                                                |
| Scheduler calls | `dequeue(sender)`, then `pick_next()`                                            |
| Register writes | None (caller is blocked; reply message will write its registers when it arrives) |
| DispatchResult  | `schedule_next()`                                                                |

The caller always blocks on Call (D16). Since no receiver was woken, the
scheduler picks the next runnable Observer.

### Row 6: Call x DirectSwitch (D50 fast path)

Waiter present, 0-cap message, D50 conditions met. Scheduler consulted.

| Aspect                     | Value                                                                      |
| -------------------------- | -------------------------------------------------------------------------- |
| Sender state               | Runnable -> Blocked (waiting on reply field)                               |
| Receiver state             | Blocked -> Runnable via `unblock()`                                        |
| Scheduler calls            | `should_switch_to(receiver)`                                               |
|                            | If approved: `dequeue(sender)` (remove from run queue)                     |
|                            | If denied: `dequeue(sender)`, `enqueue(receiver)`, `pick_next()`           |
| Register writes (approved) | `write_metadata_to_registers(receiver, label, badge, sentinel, reply_cap)` |
| Register writes (denied)   | `write_message_to_registers(receiver, ...)`, `clear_ipc_carry(receiver)`   |
| DispatchResult (approved)  | `ResumeFastPath(receiver)`                                                 |
| DispatchResult (denied)    | Result of `schedule_next()`                                                |

This is the IPC hot path (~400 cycles). On approval, x0-x3 pass through in
physical registers (D74). Only x4-x7 metadata is written. The sender is dequeued
and blocked. The receiver is directly switched to without touching the run
queue.

On denial (scheduler says no), fall back to slow path: enqueue the receiver,
dequeue the sender, pick_next.

### Row 7: Call x WokeReceiverSlowPath

Waiter present, but message has a user cap (D50 condition 4 fails).

| Aspect          | Value                                                                         |
| --------------- | ----------------------------------------------------------------------------- |
| Sender state    | Runnable -> Blocked (waiting on reply field)                                  |
| Receiver state  | Blocked -> Runnable via `unblock()`                                           |
| Scheduler calls | `dequeue(sender)`, `enqueue(receiver)` if unblock returns true, `pick_next()` |
| Register writes | `write_message_to_registers(receiver, ...)`, `clear_ipc_carry(receiver)`      |
| DispatchResult  | Result of `schedule_next()`                                                   |

Similar to DirectSwitch but always slow path. The receiver gets the full message
written to registers (including cap slot after cap installation). Scheduler
picks next — could be the receiver or someone else.

### Row 8: ReplyRecv x Received

Reply delivered (if client waiting), then new message immediately available on
recv_field.

| Aspect             | Value                                                                                      |
| ------------------ | ------------------------------------------------------------------------------------------ |
| Server state       | Stays Runnable (received a new message)                                                    |
| Reply client state | If reply_delivery.is_some(): Blocked -> Runnable via `unblock()`                           |
| Scheduler calls    | `enqueue(client)` if client was woken and unblock returns true                             |
| Register writes    | If client woken: `write_message_to_registers(client, reply...)`, `clear_ipc_carry(client)` |
|                    | For server: `write_message_to_registers(server, received...)`, `clear_ipc_carry(server)`   |
| DispatchResult     | `Resume(server)`                                                                           |

The server continues with the new request. If a client was woken by the reply,
it joins the run queue. The server always continues because it has a new message
to process.

### Row 9: ReplyRecv x Blocked

Reply delivered (if client waiting), then recv_field queue was empty. Server
blocks.

| Aspect             | Value                                                                                            |
| ------------------ | ------------------------------------------------------------------------------------------------ |
| Server state       | Runnable -> Blocked (waiting on recv_field)                                                      |
| Reply client state | If reply_delivery.is_some(): Blocked -> Runnable via `unblock()`                                 |
| Scheduler calls    | `dequeue(server)`, `enqueue(client)` if client was woken and unblock returns true, `pick_next()` |
| Register writes    | If client woken: `write_message_to_registers(client, reply...)`, `clear_ipc_carry(client)`       |
|                    | Server: none (blocked; registers written when next message arrives)                              |
| DispatchResult     | `schedule_next()`                                                                                |

The server blocks waiting for the next request. If a client was woken by the
reply, it joins the run queue.

### Row 10: Yield

Voluntary CPU relinquishment.

| Aspect          | Value                                           |
| --------------- | ----------------------------------------------- |
| Sender state    | Stays Runnable (re-enqueued at tail)            |
| Scheduler calls | `enqueue(sender)` (at tail), then `pick_next()` |
| Register writes | None                                            |
| DispatchResult  | Result of `schedule_next()`                     |

The yielding Observer re-enters the run queue at the tail. The scheduler picks
the next — which may be the same Observer if no one else is runnable (Yield is a
hint, not a guarantee of switching).

## should_switch_to: approval vs denial

D50 condition 5. Only consulted for Call x DirectSwitch (Row 6). The scheduler
must answer in <=50 cycles (D59). Round-robin always approves.

**Approval:** direct-switch to receiver. The sender is dequeued (it blocked).
The receiver's x4-x7 are written. `ResumeFastPath(receiver)` returned. x0-x3
pass through in physical registers.

**Denial:** fall back to general path. Sender is dequeued. Receiver is enqueued.
Receiver gets full message written (slow path). `schedule_next()` picks whoever
the scheduler wants.

Denial is the only case where the fast-path-eligible Direct Switch falls back to
slow path. The receiver still gets woken — it just goes through the run queue
instead of being directly switched to.

## Yield semantics: enqueue then pick

The implementation plan specifies: the yielding Observer is re-enqueued before
`schedule_next`. This ensures:

1. Round-robin fairness — the yielder goes to the tail.
2. No lost Observer — if yield did not re-enqueue and pick_next returned the
   same observer (because nobody else is runnable), the yielder would still run.
   But if yield did not re-enqueue and pick_next found someone else, the yielder
   would be lost from the run queue.

Re-enqueueing first is safe because the Observer stays Runnable throughout.

## Status

Settled. Revisit if D50 is revised (fast-path conditions change), if D59 is
revised (Scheduler trait changes), or if D39 is revised (state machine changes).
