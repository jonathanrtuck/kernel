# Badge Semantics: Representation, Assignment, and Lifecycle Visibility

## The Question

When a many-to-one IPC endpoint exists (one receiver, many senders), the
receiver needs a way to identify which logical source a message arrived from
without requiring per-source endpoint objects. The "badge" pattern—attaching a
discriminator value to a capability rather than to the message payload—is one
answer. Three sub-questions arise:

1. **Representation/encoding** — How many bits? Where does the badge live in the
   capability table entry and in the delivered message?
2. **Assignment** — Who controls the value and when: kernel-minted,
   creator-specified, or hybrid?
3. **Lifecycle visibility** — When a badged capability is destroyed (client
   disconnects, process dies), does the receiver learn which badge is gone?

These sub-questions are related but separable; different systems answer them
independently.

---

## How Existing Systems Answer Each Question

### seL4

**Representation.** Every endpoint and notification capability entry in the
CNode carries one badge field. On 32-bit platforms, 28 bits are usable; the
kernel silently ignores the high 4 bits (reserved for internal type/metadata
encoding). On 64-bit platforms the full machine word is available. The badge
value zero is special: it means "unbadged." The kernel delivers the badge to the
receiver in the `badges` array of the IPC buffer, indexed by the slot position
of the received capability in the message. The badge travels alongside message
words and capsUnwrapped metadata, not inside the message payload.

**Assignment.** Creator-specified via `seL4_CNode_Mint`. The minter supplies the
badge value when deriving a new capability slot from an existing endpoint. After
minting, the badge is immutable: a badged capability cannot be rebadged, and a
derived child capability cannot carry a different badge. Zero-badged
capabilities cannot be minted from a badged capability—the derivation tree
terminates.

**Lifecycle visibility.** None. seL4 has no notification mechanism when a badged
capability is destroyed. `seL4_CNode_CancelBadgedSends` (a CNode invocation, not
an endpoint invocation) cancels all queued sends on an endpoint that match a
specific badge value, but it is a cleanup tool for reuse, not a delivery
mechanism to the receiver. The receiver never learns that a specific badge has
been retired. A thread blocked in Receive will not be signaled when a badge
disappears from the system.

A related operation, `CancelBadgedSends`, was deliberately placed as a CNode
invocation rather than an endpoint invocation. The rationale (seL4 forum, 2019):
endpoint invocations are interpreted as IPC to the endpoint's receiver, so a
kernel management operation on the endpoint object itself must use CNode
addressing to bypass the normal receive path.

Notification objects (a separate seL4 type) use badges differently: the badge
acts as a bit-mask, and `seL4_Signal` ORs the badge into the notification word.
This allows N notification capabilities to map to N bits of a single word,
providing a simple select()-style multiplexer. Notification badges have no
lifecycle visibility either.

---

### L4 / Fiasco.OC IPC Gates

**Representation.** An IPC gate carries a "label" field of machine-word size.
The two least-significant bits are forced to the send/write permission bits of
the capability used to invoke the gate; the remaining bits are available as a
user-controlled discriminator. When forwarded IPC arrives at the bound thread,
the sender's original label is replaced by the gate's label. The receiver always
sees the gate label, never the sender's own descriptor.

**Assignment.** Creator-specified at gate creation or binding time
(`l4_rcv_ep_bind_thread`, `l4_rcv_ep_bind_snd_destination`). The bound thread
sets the label.

**Lifecycle visibility.** None automatic. When a gate is deleted, the L4 manual
notes that IPC already in flight retains the old label. The kernel does not
signal the bound thread. The manual documents `l4_thread_modify_sender_start` as
a mechanism to retroactively relabel pending IPC (e.g., before gate deletion),
but this is a manual administrative operation, not a notification.

---

### Coyotos (Shapiro, 2007)

**Representation.** Coyotos separates two discriminator fields:

- **Protected payload** (32 bits): embedded in an Entry capability (the Coyotos
  term for a sendable reference to an endpoint). Neither readable nor modifiable
  by the capability's invoker. Delivered to the receiver as an additional output
  of invocation.
- **Endpoint identifier** (60 bits): stored in the endpoint object itself.
  Receiver-controlled; provided to the receiver during message receive.
  Meaningful only to the recipient.

This two-field design separates the _capability-level_ discriminator (what the
sender's delegation authority says) from the _endpoint-level_ discriminator
(what the receiver tracks internally).

**Assignment.** Protected payload: set by the capability creator (whoever mints
the Entry cap). Endpoint ID: set by the receiver who owns the endpoint object.

**Lifecycle visibility.** None explicit. Capability invalidation in Coyotos uses
allocation-count revocation: when an object is reallocated, its allocation count
increments, and any capability referencing the old count becomes invalid on next
invocation. There is no proactive notification; the sender discovers invalidity
only when invoking a stale capability.

---

### Mach / XNU

**Representation.** Mach does not have an in-capability badge. Port rights are
identified by _port names_: 32-bit integers local to each task's namespace
(analogous to file descriptors). A task holds a name for a port right; the same
port may have different names in different tasks. The sender's identity is not
automatically embedded in the IPC buffer. For server-identification, Mach
provides an opt-in _trailer_: the message trailer can carry the sender's audit
token, security token, or context value, depending on the trailer type requested
by the receiver.

**Assignment.** Kernel-minted. Port names are assigned by the kernel when a task
acquires a port right (`mach_port_allocate`, right transfer on message receive).
The user has no control over the numeric value.

**Lifecycle visibility.** Explicit opt-in via `mach_port_request_notification`.
A task registers a send-once right as the destination for one of several
notification types:

- `MACH_NOTIFY_DEAD_NAME`: when a port is destroyed, all send rights to it turn
  into dead names, and this notification fires. The receiver of the notification
  gets the local port name that has become dead.
- `MACH_NOTIFY_PORT_DELETED`: fires if the right is deallocated before the port
  dies.
- `MACH_NOTIFY_SEND_ONCE`: fires when a send-once right is used or destroyed.

Dead-name notification is per-right: a server tracking N clients must register N
separate notification requests. On notification delivery, the user reference
count of the dead name is incremented (preventing name reuse until the dead name
is explicitly cleaned up). This creates a two-step cleanup: receive the
notification, then call `mach_port_mod_refs` to decrement the dead-name ref and
free the name.

---

### QNX Neutrino

**Representation.** QNX uses connection IDs (scoid, server-connection-id)
assigned by the kernel when a client connects to a channel. The scoid is a
per-channel integer that serves as the client identifier. When the server
receives a message or pulse, the scoid embedded in the `_msg_info` or `_pulse`
structure identifies which client sent it.

**Assignment.** Kernel-minted. The kernel assigns scoid on `ConnectAttach`. The
server has no control over the numeric value; it learns the scoid by inspecting
received message metadata.

**Lifecycle visibility.** Explicit opt-in per channel at creation time:

- `_NTO_CHF_DISCONNECT`: server requests notification when a client disconnects.
  The kernel delivers a pulse with `_PULSE_CODE_DISCONNECT` and the
  disconnecting scoid to the server's channel. If the client process dies, the
  kernel detaches all its connections and fires the pulse for each. The server
  must then call `ConnectDetach(scoid)` to clean up.
- `_NTO_CHF_COID_DISCONNECT`: client requests notification when the server goes
  away. Kernel delivers `_PULSE_CODE_COIDDEATH` to the client.

These flags are set once at channel creation and apply to all connections to
that channel. This is the most automatic lifecycle mechanism surveyed: once
opted in, no per-connection registration is needed.

---

### Zircon (Fuchsia)

**Representation.** Zircon does not have a badge field on capabilities. Channels
are bidirectional, two-endpoint objects; each handle to a channel endpoint has a
unique koid (kernel object ID: a 64-bit monotonically increasing integer, never
reused). Handle koids are accessible via
`zx_object_get_info(ZX_INFO_HANDLE_BASIC)`.

**Assignment.** Kernel-minted. koids are assigned at object creation time and
are immutable for the lifetime of the object.

**Lifecycle visibility.** Automatic, no opt-in required. When one end of a
channel is closed (handle dropped or process killed), the kernel sets
`ZX_CHANNEL_PEER_CLOSED` on the other endpoint. Any thread waiting on the
channel with `zx_object_wait_one` or a port (`zx_port_wait`) will be woken. This
covers both graceful close and process death with no additional registration.

---

## Measured Data

- **seL4 Mint overhead**: Minting a badge is a CNode invocation; no published
  per-mint latency separate from general capability manipulation benchmarks. The
  badge field itself adds zero runtime overhead to IPC—it is read from the CTE
  and written to the IPC buffer as part of the normal message transfer path.
- **seL4 CancelBadgedSends**: O(n) in the length of the endpoint send queue; not
  on any fast path. No published benchmark.
- **QNX pulse delivery**: pulse is fixed-size (4 bytes data + 1 byte code +
  priority); documented as non-blocking send. Exact latency not independently
  published; QNX marketing materials cite sub-microsecond pulse delivery on
  embedded hardware.
- **Mach dead-name notification**: delivered as a normal Mach message to the
  registered send-once right; subject to normal message delivery latency. No
  separate benchmark.

---

## Tradeoffs

| Axis                              | Options observed                                                                                                                                     | Consequences                                                                                                                                                                                                             |
| --------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Badge size                        | 28 bits (seL4/32), full word (seL4/64, L4), 32 bits (Coyotos payload), 60 bits (Coyotos endpoint ID), 64 bits (Zircon koid)                          | Larger badges allow more clients without value reuse; smaller badges save CTE space                                                                                                                                      |
| Badge placement                   | In capability entry (seL4, L4, Coyotos) vs. kernel namespace integer (Mach, QNX, Zircon)                                                             | In-cap placement ties badge to delegation chain; namespace placement makes badge kernel-controlled                                                                                                                       |
| Assignment authority              | Creator-specified (seL4, L4, Coyotos) vs. kernel-minted (Mach, QNX, Zircon)                                                                          | Creator-specified enables semantic encoding; kernel-minted prevents forgery but removes expressiveness                                                                                                                   |
| Two-field discriminator (Coyotos) | Protected payload (creator-set) + endpoint ID (receiver-set)                                                                                         | Allows receiver to add its own tagging without trusting the sender's badge value                                                                                                                                         |
| Lifecycle notification            | None (seL4, L4, Coyotos) vs. opt-in per-right (Mach) vs. opt-in per-channel (QNX) vs. automatic (Zircon)                                             | None forces polling or in-band signals; per-right opt-in is precise but heavyweight for many clients; per-channel opt-in trades precision for simplicity; automatic is lowest effort but may produce unsolicited wakeups |
| Cleanup cost                      | seL4: CancelBadgedSends clears queue; Mach: dead-name refcount must be manually decremented; QNX: ConnectDetach must be called                       | Each notification design imposes a corresponding cleanup obligation                                                                                                                                                      |
| Reuse of badge values             | seL4: explicit cancel required before badge reuse is safe; L4: no kernel mechanism; others: kernel controls namespace, no user-visible reuse problem | Creator-specified badges can be reused if prior badged caps are fully revoked; kernel-minted IDs are typically not reused (Zircon guarantees no reuse for lifetime of system)                                            |

---

## References

- seL4 Reference Manual v14.0.0: §4 (CNode operations, Mint, CancelBadgedSends),
  §5 (IPC, badge delivery).
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf
- seL4/seL4 GitHub, `manual/parts/ipc.tex`.
  https://github.com/seL4/seL4/blob/master/manual/parts/ipc.tex
- seL4 forum: "Why is CancelBadgedSends not an invocation on an Endpoint?"
  (2019).
  https://sel4.discourse.group/t/why-is-cancelbadgedsends-not-an-invocation-on-an-endpoint/86
- L4Re IPC Gate API.
  https://l4re.org/doc/group__l4__kernel__object__gate__api.html
- Shapiro, J. S. et al., "Coyotos Microkernel Specification."
  https://hydra-www.ietfng.org/capbib/cache/shapiro:coyotosspec.html
- GNU Mach Reference Manual: Port Destruction, Request Notifications.
  https://www.gnu.org/software/hurd/gnumach-doc/Port-Destruction.html
- `mach_port_request_notification` man page (Darwin).
  https://web.mit.edu/darwin/src/modules/xnu/osfmk/man/MP_request_notification.html
- QNX Neutrino System Architecture, "Pulses."
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.sys_arch/topic/ipc_Pulses.html
- QNX `ChannelCreate()` reference (flags `_NTO_CHF_DISCONNECT`,
  `_NTO_CHF_COID_DISCONNECT`).
  https://www.qnx.com/developers/docs/8.0/com.qnx.doc.neutrino.lib_ref/topic/c/channelcreate.html
- Zircon Kernel Concepts.
  https://fuchsia.dev/fuchsia-src/concepts/kernel/concepts
