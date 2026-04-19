# Capability Clonability: Uniform vs. Per-Type Restriction

## 1. The Question

Can every capability in a system be copied to create additional independent
holders, or are some capability types restricted to a single holder at any given
time?

Two poles define the design axis:

**Uniform clonability:** Any capability can be copied (subject to rights
attenuation). The system does not distinguish types that allow multiple holders
from types that require exactly one. Authority over an object can always be
shared by minting additional copies.

**Per-type restriction:** Some capability types are enforced as non-clonable by
the kernel — a structural property of the type, not a policy applied by the
holder. Multiple holders of that type for the same object cannot coexist.

Between these poles lies a middle position: per-capability flags (a rights bit)
that allow the capability creator to choose clonability at issue time, rather
than deriving it from the type.

The design choice interacts with lifecycle semantics, revocation complexity, and
the rights model.

---

## 2. Survey of Existing Systems

### 2.1 Mach / XNU — Receive Rights as the Canonical Non-Clonable Type

Mach is the oldest and most studied example of a per-type clonability
distinction in a microkernel. Mach ports have three right types:

- **Receive right:** Dequeue messages from the port. **At most one holder in the
  entire system.** The kernel enforces this: you cannot copy a receive right;
  you can only _move_ it to another task or _retain_ it. Precisely one task at a
  time is the "owner" of the port in the sense of being able to receive from it.
- **Send right:** Enqueue messages. Many holders. Freely copyable; multiple
  tasks can independently hold send rights to the same port.
- **Send-once right:** Enqueue exactly one message. Consumed on first use.
  Non-renewable; a new send-once right must be created if a second message is
  needed.

Source: Mach Interface Generator (MIG) documentation; Carnegie Mellon Mach
technical reports; Apple Kernel Programming Guide (Mach Overview).

**Lifecycle implication.** Destroying the receive right destroys the port
entirely. All send-right holders receive a `DEAD_NAME` notification. Because
receive rights are unique, "port lifetime = receive-right holder lifetime" is a
clean invariant: the kernel can implement port death as a simple event from a
single point of authority.

**Authority meaning.** The receive right is the "owner" capability — the entity
that controls the message queue and decides what service is provided. The send
rights are "access" capabilities — entities that can invoke the service but not
control it or destroy it. This owner/accessor split is expressed structurally
through type-level non-clonability.

---

### 2.2 seL4 — Non-Clonable "Authority Source" Capabilities

seL4's standard capability operations (`seL4_CNode_Copy`, `seL4_CNode_Mint`) are
available for almost all capability types. However, certain capability types
that represent global singleton authorities are explicitly restricted:

**IRQControl.** The single capability granting authority to create IRQ handler
capabilities for specific interrupt lines. The kernel does not permit copying it
(`seL4_CNode_Copy` returns an error for IRQControl caps).

**ASIDControl.** Similarly, the single capability governing ASID pool creation.
Non-copyable by the same constraint.

**SchedControl (MCS kernel).** Authority over scheduling context creation. Also
restricted.

Gerwin Klein (seL4 core team) explained the design rationale in a forum
discussion (seL4 Discourse, 2020):

> _"Without a strong use case, we chose the most restrictive and
> easiest-to-implement policy."_

The CDT implementation was also a factor: for a capability with "global effect,"
determining whether a copy is a sibling or descendant in the CDT requires
encoding additional information per-copy. No natural use case for splitting
IRQControl authority (e.g., "authority over a subset of IRQ lines") existed at
the time; implementing subset-splitting would require work comparable to seL4's
`IOPort` range restriction mechanism.

**The authority-source / access-instance split.** IRQControl (non-clonable)
creates IRQHandler capabilities. IRQHandler capabilities (one per IRQ line,
derived from IRQControl) **can** be copied, moved, and delegated normally. The
pattern: the capability that mints derived capabilities is non-clonable; the
derived instances are freely clonable.

Source: seL4 Discourse thread "Why can seL4_IRQControl not be copied?" (2020);
seL4 Reference Manual v14.0.0.

---

### 2.3 Zircon — Per-Handle Rights Bit (ZX_RIGHT_DUPLICATE)

Zircon does not use type-level non-clonability. Instead, a rights bit
(`ZX_RIGHT_DUPLICATE`) on each handle controls whether the holder may duplicate
that handle. This is a per-handle policy, not a per-type structural rule.

**Default rights by object type.** The kernel assigns default rights at handle
creation time. Different object types have different defaults:

- **Channel endpoints:** Do **not** include `ZX_RIGHT_DUPLICATE` by default.
  There is never more than one handle to a given channel endpoint; the holder is
  the sole owner.
- **VMOs, Events, Ports, Sockets:** Include `ZX_RIGHT_DUPLICATE` by default.
  Multiple processes can hold independent handles.
- **Interrupts:** Do not include `ZX_RIGHT_DUPLICATE` by default.

**Channel endpoint ownership.** The Fuchsia channel documentation states
directly: "Unlike many other kernel object types, channels are not duplicatable.
Thus, there is only ever one handle associated with a channel endpoint, and the
process holding that handle is considered the owner. Only the owner can read or
write messages or send the channel endpoint to another process."

This creates a Mach-like owner/accessor split for channels: the channel endpoint
is exclusively owned; only the owner can transfer it (move it to another process
via `zx_channel_write`).

**Lifecycle implication.** "When the last handle to an endpoint is closed the
unread messages in that endpoint's queue are destroyed." With non-duplicatable
channel endpoints, "last handle" = "sole handle" — the lifecycle event is
unambiguous. Message ordering across transfers is well-defined: messages before
the transfer event belong to the previous owner, messages after to the new
owner.

**Rights removal at transfer.** `ZX_RIGHT_DUPLICATE` can be stripped from a
handle before transfer via `zx_channel_write_etc`. A sender can grant a
non-duplicatable handle even for object types that are duplicatable by default.
This allows dynamically enforcing unique ownership at any transfer point.

Source: Fuchsia kernel documentation —
[Channel reference](https://fuchsia.dev/fuchsia-src/reference/kernel_objects/channel);
[Handles](https://fuchsia.dev/fuchsia-src/concepts/kernel/handles);
`zx_handle_duplicate` syscall documentation.

---

### 2.4 EROS / KeyKOS — Uniform Clonability

EROS and KeyKOS use uniform clonability: any key (capability) can be copied
freely into any c-list slot. There is no type-level non-clonable restriction.

The tradeoff is that lifecycle tracking requires an explicit mechanism — EROS
maintains capability link chains (doubly-linked lists of all holders per object)
to support revocation. Revocation traverses the entire holder list (O(holders)).

KeyKOS had exactly 16 capability slots per domain. The copy freedom was
practical given the small, bounded namespace: copying a key was cheap and
well-understood in that context.

Source: Shapiro et al., "EROS: A Fast Capability System" (SOSP 1999).

---

### 2.5 Coyotos — Opaque / Non-Delegatable Capabilities

Coyotos (Shapiro, Johns Hopkins, ~2003–2007) introduced "opaque" capabilities
that could not be delegated. This is distinct from non-copyability: an opaque
capability can exist in multiple places (the holder minted multiple copies), but
those copies cannot be passed to other principals. The restriction is on
outbound delegation, not on in-place copying.

Source: Jonathan Shapiro, Coyotos design documentation; Miller et al.,
"Capability Myths Demolished" (2003).

---

### 2.6 CHERI — Sealed Capabilities and Linear Capabilities

CHERI (Cambridge) hardware capabilities can be **sealed**: a sealed capability
cannot be loaded from, stored to, or used for any operation until it is
explicitly unsealed. Sealed capabilities serve as opaque object references
(e.g., vtable pointers, unforgeable domain-crossing tokens). Sealing does not
prevent copying in memory, but copying a sealed capability does not grant access
— the tag bit tracks validity, and sealed caps cannot be used directly.

The **Capstone** system (USENIX Security 2023, Yu et al.) extends CHERI with
**linear capabilities**: alias-free capabilities where holding one guarantees
exclusive access to the designated memory region. Linear capabilities are
consumed (invalidated) by operations that would otherwise create overlapping
aliases. The hardware tracks this through the validity tag — any move of a
linear capability clears the source.

Source: Watson et al., "CHERI: A Hybrid Capability-System Architecture" (IEEE
Micro 2015); Yu et al., "Capstone: A Capability-based Foundation for Trustless
Secure Enclaves" (USENIX Security 2023).

---

### 2.7 RedLeaf — Linear Types as Language-Level Non-Clonability

RedLeaf (OSDI 2020, Narayanan et al.) implements capability isolation at the
language level using Rust's ownership system. Cross-domain calls pass `RRef<T>`
(remote reference) objects with move semantics: the sender loses access at the
call site, enforced at compile time by the Rust borrow checker.

The kernel does not enforce capability uniqueness in a table — the enforcement
is the type system. `RRef<T>` is a linear type: it cannot be copied; it can only
be moved or dropped. "Rust's ownership discipline ensures that there is always
only one remote reference to the object inside the domain."

This achieves zero-copy cross-domain communication: the receiving domain gets
exclusive access without any runtime kernel check, because the language
guarantees the sender no longer holds the reference.

**Tradeoff vs. kernel-enforced uniqueness.** Language-level enforcement is only
valid within the trusted language boundary. Once unsafe code is involved, or if
the kernel is verifying cross-domain authority for untrusted components,
kernel-level enforcement is required. RedLeaf's model works because all domain
code is trusted-correct Rust.

Source: Narayanan et al., "RedLeaf: Isolation and Communication in a Safe
Operating System" (OSDI 2020).

---

### 2.8 seL4 Grant Right and IPC Delegation

seL4's access rights model includes a `Grant` right distinct from copy
permission. A capability with the `Grant` right can be passed to another entity
during an IPC call (via the capability-transfer slots in a message).

A capability **without** the `Grant` right cannot be passed in an IPC message.
This is a form of delegation restriction but is not the same as non-clonability:
the capability can still be copied within the same CSpace by privileged
operations; it just cannot be handed to arbitrary recipients via normal IPC.

The `GrantReply` right (MCS kernel) is a finer-grained version: it permits
passing reply capabilities but not arbitrary capabilities.

Source: seL4 Reference Manual v14.0.0, §2.

---

## 3. Taxonomy of Mechanisms

Four distinct mechanisms implement non-clonability or delegation restriction:

| Mechanism                  | Enforcement site         | Granularity                 | Example systems                       |
| -------------------------- | ------------------------ | --------------------------- | ------------------------------------- |
| Type-level kernel rule     | Kernel, per type         | All instances of that type  | Mach receive rights, seL4 IRQControl  |
| Rights-bit absence         | Kernel, per-handle       | Chosen at issue or transfer | Zircon (ZX_RIGHT_DUPLICATE)           |
| Delegation restriction     | Kernel, per rights field | IPC transfer only           | seL4 Grant right, Coyotos opaque caps |
| Language-level linear type | Compiler                 | Per object, trust boundary  | RedLeaf RRef<T>, Rust ownership       |

---

## 4. The Authority-Source / Access-Instance Split

A recurring pattern across systems that use non-clonable capabilities:

- One capability type represents **authority over the object itself** (create,
  configure, destroy, or exclusively consume from it). This type is
  non-clonable: exactly one entity holds "ownership" at any time.
- Another capability type represents **access to the object's functionality**
  (send a message, invoke a service). This type is freely clonable: many
  entities can hold it simultaneously.

| System | Non-clonable (authority)                 | Clonable (access)               |
| ------ | ---------------------------------------- | ------------------------------- |
| Mach   | Receive right                            | Send right                      |
| seL4   | IRQControl, ASIDControl                  | IRQHandler, ASID pool cap       |
| Zircon | Channel endpoint (no ZX_RIGHT_DUPLICATE) | VMO handle (ZX_RIGHT_DUPLICATE) |

This split means the "who can destroy or reconfigure this object?" question has
a unique answer at the kernel level, while the "who can use this object?"
question may have many answers.

---

## 5. Lifecycle Implications

**Non-clonable (unique holder):**

- Revocation is trivial: when the unique holder drops the capability, the
  object's lifecycle event fires immediately. No CDT traversal, no link chain,
  no generation-number check required.
- Object lifetime = capability holder lifetime. Clean invariant.
- Lifecycle delegation requires **transferring** the capability (move
  semantics), not sharing it. Only one entity at a time can "own" the lifecycle.
- Mach port death: when the receive right is destroyed, the kernel sends
  `DEAD_NAME` notifications to send-right holders. This notification cost is
  O(send-right holders), but the trigger event is unambiguous.

**Clonable (multiple holders):**

- Revocation requires tracking all copies: CDT traversal (seL4), link chains
  (EROS), generation numbers (Coyotos), or object destroy (Zircon).
- Multiple entities can independently hold lifecycle authority. A policy
  question arises: who can destroy the object? This is typically resolved by
  holding the most-privileged capability (e.g., the original mint call)
  separately.
- Enables shared authority patterns: multiple managers can independently observe
  and interact with the object.

---

## 6. Rights Model Interaction

Non-clonable capabilities change the rights model in a specific way:

**In a uniform-clonability system** with rights attenuation, the rights field
controls what operations each holder can perform on the object. Copies with
different rights masks can coexist. The "maximum rights" holder is whoever holds
the cap minted with the most rights.

**In a non-clonable system**, there is no "multiple holders with different
rights" for that type. The single holder implicitly holds all applicable rights
for that role (because they are the sole holder of the non-clonable type). To
grant someone else access, they must either:

- Receive the entire capability via transfer (former holder loses access), or
- Be given a different, clonable capability type (e.g., a "send right" or
  "access instance") with reduced authority.

This means the rights model for non-clonable caps is effectively binary: you
hold it (full authority of that role) or you don't.

---

## 7. Measured Data

No published benchmarks isolate the performance difference between non-clonable
and clonable capability types at the kernel level. The lifecycle and revocation
cost differences are the primary data:

| Mechanism                   | Revocation cost                              | Source                |
| --------------------------- | -------------------------------------------- | --------------------- |
| Unique holder drop          | O(1) — no traversal needed                   | Mach port model       |
| CDT traversal (seL4)        | O(derived caps), non-preemptible in baseline | seL4 Reference Manual |
| Link chain (EROS)           | O(holders)                                   | SOSP 1999             |
| Generation number (Coyotos) | O(1) at revoke time, O(1) per use            | Coyotos spec          |
| Object destroy (Zircon)     | O(1) destroy + O(h) dead-handle delivery     | Zircon docs           |

For Mach port death notification (O(send-right holders)): no published
benchmark. Qualitatively, large services with many clients can have hundreds to
thousands of send-right holders; port death delivers a notification to each.

---

## 8. Tradeoffs

**Per-type non-clonability:**

- Simplifies lifecycle: unique holder = clear lifecycle authority.
- Complicates delegation: to share "ownership," the type must be redesigned to
  separate owner/accessor roles (as Mach does with receive/send rights).
- Makes revocation cheap for the non-clonable type.
- Inflexible: if the use case requires multiple co-equal managers, the design
  must evolve the type system.

**Rights-bit non-clonability (Zircon):**

- Flexible: clonability is a per-handle property, not a per-type rule.
- More complex: the kernel must check the rights bit on every duplicate attempt.
- Enables dynamic enforcement: a sender can strip `ZX_RIGHT_DUPLICATE` before
  transferring, creating unique ownership for any object type.
- Harder to reason about: the same object type may have instances with and
  without clonability, depending on how they were issued.

**Uniform clonability:**

- Simpler capability model: one set of operations for all types.
- Lifecycle tracking required: must implement CDT, link chains, or generation
  numbers to support revocation.
- Multiple co-equal managers are naturally expressible.
- Revocation is more expensive.

**Language-level non-clonability (linear types):**

- Zero runtime overhead in the kernel.
- Only valid within the language trust boundary; does not protect against unsafe
  code or external components.
- The capability system is implicit (the type system is the capability system).

---

## 9. References

- Apple. "Mach Overview." _Darwin Kernel Programming Guide._
  https://developer.apple.com/library/archive/documentation/Darwin/Conceptual/KernelProgramming/Mach/Mach.html

- Mach Interface Generator (MIG) documentation. Carnegie Mellon.

- seL4 Discourse. "Why can seL4_IRQControl not be copied?"
  https://sel4.discourse.group/t/why-can-sel4-irqcontrol-not-be-copied/80

- seL4 Reference Manual Version 14.0.0.
  https://sel4.systems/Info/Docs/seL4-manual-latest.pdf

- Fuchsia documentation. "Channel."
  https://fuchsia.dev/fuchsia-src/reference/kernel_objects/channel

- Fuchsia documentation. "Zircon Handles."
  https://fuchsia.dev/fuchsia-src/concepts/kernel/handles

- Fuchsia documentation. "zx_handle_duplicate."
  https://fuchsia.dev/reference/syscalls/handle_duplicate

- Jonathan Shapiro, Jonathan Smith, David Farber. "EROS: A Fast Capability
  System." _SOSP 1999._
  https://courses.cs.washington.edu/courses/cse551/19wi/readings/eros-sosp99.pdf

- Shapiro, Coyotos design documentation. coyotos.org (archived).

- Robert N.M. Watson et al. "CHERI: A Hybrid Capability-System Architecture for
  Scalable Software Compartmentalization." _IEEE Micro_, 2015.
  https://cseweb.ucsd.edu/~dstefan/cse291-spring21/papers/watson:cheri.pdf

- Jason Yu et al. "Capstone: A Capability-based Foundation for Trustless Secure
  Enclaves." _USENIX Security 2023._
  https://www.usenix.org/system/files/usenixsecurity23-yu-jason.pdf

- Vikram Narayanan et al. "RedLeaf: Isolation and Communication in a Safe
  Operating System." _OSDI 2020._
  https://www.usenix.org/system/files/osdi20-narayanan_vikram.pdf
