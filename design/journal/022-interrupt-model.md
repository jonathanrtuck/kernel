# Journal 022 — Interrupt model: delegation through endpoints

## Question

What is the kernel's interrupt model for device interrupts? Specifically:
ownership (kernel-handled vs. delegated to userspace), delivery mechanism,
acknowledgment protocol, and the kernel object model that mediates it.

Decomposed in Phase 1 into four sub-questions: (1) ownership, (2) routing, (3)
delivery mechanism, (4) masking/flow control. The derivation revealed that
(2)–(4) were already substantially committed by D13, D17, and D18. The primary
open question was (1) ownership, plus the object model and authority.

## Scope

Three categories of hardware interrupt exist on ARM64 (A2):

- **SGIs (0–15):** Software Generated Interrupts = IPIs. Kernel-internal for
  cross-core coordination (O2). Not subject to delegation.
- **PPIs (16–31):** Private Peripheral Interrupts. Per-core. The preemption
  timer is kernel-internal (D2). Other PPIs (per-core watchdog, performance
  monitors) may be delegated.
- **SPIs (32–1019) and LPIs (8192+):** Shared/routable device interrupts.

This derivation concerns device interrupts (SPIs, LPIs, delegatable PPIs). The
preemption timer and IPIs are kernel-internal by necessity and excluded.

Landscape §5.1 confirms: "What stays in-kernel universally: masking/unmasking,
EOI, preemption timer. No microkernel delegates the preemption timer."

---

## Delegation: three convergent paths paralleling D12

The kernel delegates device interrupt handling to userspace driver Observers.
Three independent paths converge, exactly paralleling D12 (fault delegation):

**Path 1 — A4 (purely reactive).** No kernel thread means no background
interrupt processing, no deferred work queues, no bottom halves. The kernel's
interrupt path is: exception entry → identify interrupt → minimal kernel work →
signal driver → return. Landscape §5.6: "Microkernels dissolve the problem: the
kernel's path is mask-signal-EOI-return; the scheduler IS the deferred
processing mechanism. The entire 'bottom half' is the userspace driver thread."

**Path 2 — A3 (generic).** Different devices need different handling strategies.
No single hardcoded interrupt policy fits all workloads. A3 says policy belongs
in userspace where each driver implements its own.

**Path 3 — A5 (net).** The dispatch interface (mask, signal, EOI) is smaller
than a policy-configuration interface would be. The kernel absorbs the GIC
programming complexity behind a simple capability + endpoint interface. Driver
complexity (device-specific handling) lives in userspace leaf nodes. Applying
"push complexity to the leaves": the GIC programming is kernel-internal
complexity behind a simple interface; device-specific handling is leaf-node
complexity in userspace drivers.

The parallel with D12 is point-for-point:

- D12 path 1 (A4 → no background paging) ‖ path 1 (A4 → no background interrupt
  processing)
- D12 path 2 (A3 → no single paging policy) ‖ path 2 (A3 → no single interrupt
  policy)
- D12 path 3 (A5 → dispatch < policy interface) ‖ path 3 (A5 → dispatch < policy
  interface)

### Foreclosed alternatives

- **Kernel-internal device interrupt handling.** Foreclosed by A3 + A4 (paths 1
  and 2). The kernel cannot embed device-specific handling policy (A3) and
  cannot do background processing (A4).
- **ISR-in-userspace at interrupt priority (QNX InterruptAttach).** Foreclosed
  by D1 + D13. Running user code on the interrupt exception path violates D1's
  minimal per-core hot path. D13 commits to information delivery through
  endpoints, not through special execution contexts.
- **Integer IRQ IDs (QNX/Minix 3).** Foreclosed by D4 — ambient authority, no
  designation = authority.
- **File descriptor model (Redox irq:N).** Foreclosed by D4 — namespace-
  dependent, not capability-mediated.
- **Kernel-level interrupt coalescing.** Foreclosed by D18 + A3. Coalescing is
  reducible to shared memory + signaling (D9/D10 + capacity-1 endpoints). Not
  all workloads need it (A3).

---

## Already committed by prior derivations

Several sub-questions turned out to be already settled:

- **Delivery through queued endpoints.** D13 commits: "All information delivery
  — IPC, faults, interrupts, system signals — uses the same mechanism." The
  kernel enqueues an interrupt message to the driver Observer's endpoint.
- **Identification through badges.** D17 + D19: "Timer/interrupt signals:
  signaling services hold badged send caps." Multiple interrupts fan into one
  driver endpoint, distinguished by per-interrupt badges.
- **Overflow uses mask-on-delivery.** D18 explicitly: "interrupt masking: mask
  on delivery, unmask on driver acknowledgment. If the endpoint signal fails,
  the interrupt stays masked — the interrupt controller holds the pending
  state."

---

## No separate IRQ object type: interrupts are endpoint traffic

D13 commits to all information delivery through endpoints. Faults, IPC, and
interrupts all arrive as messages on endpoints. The driver receives from one
endpoint and dispatches by badge. This is already settled.

Phase 5 evaluation explored whether interrupts need their own kernel object type
(IRQ objects, IRQControl factories, interrupt bindings) and progressively
eliminated each candidate. The evaluation proceeded through three stages:

### Stage 1: IRQControl factory + interrupt binding (rejected)

The initial derivation introduced a two-level model (seL4 precedent): an
IRQControl root capability creates interrupt binding objects. Rejected because
Space doesn't need a SpaceControl factory — D4 says holding a cap to a resource
IS the authority over it. A factory separates authority-to-create from the
created object, an indirection D4 doesn't require.

### Stage 2: IRQ objects parallel to Space (rejected)

Replaced the factory with a Space-parallel model: IRQ objects as claims on the
bounded IRQ namespace, splittable and transferable like Space. Eliminated
IRQControl but retained a separate IRQ kernel object type with bind, ack, split,
and destroy operations.

Rejected because the IRQ object's relationship to the endpoint mirrors the
relationship between a sender and a channel — structurally the same as an
Observer with a send cap. The kernel deposits messages on behalf of hardware,
but the delivery mechanism is the same endpoint mechanism used by everything
else (D13). The IRQ object was duplicating structure the endpoint already
provides: the endpoint IS the delivery point, and authority over its receive
side IS authority over what it delivers.

### Stage 3: Endpoints only (settled)

The interrupt namespace maps onto the endpoint namespace. The kernel maintains
an internal IRQ→endpoint routing table. Authority over a set of hardware
interrupts = holding the receive cap on the endpoint where those interrupts
arrive.

The model:

1. At boot, the kernel discovers device interrupts from the device tree / GIC
   configuration and routes them to a root interrupt endpoint.
2. The initial Observer receives a receive cap to this endpoint (same mechanism
   as receiving initial Space — the boot distribution protocol, not yet settled,
   applies uniformly).
3. To delegate interrupts to a driver, the holder splits the endpoint by IRQ
   range: a new endpoint is created that receives the specified subset. The
   original endpoint loses those IRQs. The new endpoint cap is transferred to
   the driver.
4. The driver receives interrupt messages on its endpoint. Each message carries
   a badge identifying the IRQ and a send-once ack cap (D16).
5. After handling the interrupt, the driver uses the send-once ack cap. This
   tells the kernel to unmask the interrupt. The cap is consumed (D16
   semantics). If the driver crashes and the cap is closed without use, the
   interrupt stays masked (D18 mask-on-delivery safety).
6. If the driver wants interrupts and IPC on one endpoint, it either uses the
   interrupt endpoint as its IPC endpoint (distributing send caps to clients) or
   combines multiple endpoints into one (a new endpoint that receives all
   sources).

No new kernel object type. No IRQ-specific operations on the driver's critical
path — the driver just calls receive() and uses send-once caps, exactly like
handling RPCs. From the driver's perspective, an interrupt is structurally
identical to an incoming RPC request with a reply cap.

### Why no IRQ object type is needed

**D13 already commits interrupt delivery to endpoints.** The endpoint receives
the message. The endpoint receive cap is the authority. Adding an IRQ object in
front of the endpoint is an indirection that D13 already eliminated by
committing to "all information delivery is one mechanism."

**D16 already provides the ack mechanism.** Send-once caps are a general-purpose
one-shot authorization (D16: "edge-triggered interrupt delivery" listed as an
independent application). The interrupt message carries an ack cap; the driver
uses it to unmask. No IRQ-specific ack operation needed.

**D7's split classification falls out naturally.** Delivery = the kernel
deposits a message (IPC-family, kernel-as-sender). Ack = the driver uses a
send-once cap (IPC-family, driver-as-sender). There is no typed kernel operation
specific to interrupts — both sides are IPC. This is simpler than the previous
models where ack was a typed kernel op on an IRQ object.

**Every identified downside traces to a parent decision:**

| Concern                             | Actually lives in                                        |
| ----------------------------------- | -------------------------------------------------------- |
| Send-once cap performance           | D16 (every RPC already uses send-once)                   |
| Crash recovery                      | General lifecycle (Space, Time, endpoints all have this) |
| Split/combine endpoint operations   | D13/D15 (endpoint model evolution)                       |
| Endpoints carrying hardware sources | D13 ("all information delivery uses the same mechanism") |

No risk is introduced by D22. Every concern is inherited from a parent decision
that is independently settled.

---

## Endpoint split and combine

Two operations on endpoints emerge from the interrupt model but are potentially
general:

**Split by IRQ range.** Create a new endpoint; move the specified IRQ routes to
it. The original endpoint loses those routes. The holder of the original
endpoint's receive cap authorizes the split (they hold authority over those
IRQs). For kernel-controlled sources (IRQs), the kernel redirects its internal
routing. Whether split generalizes to badge-range-based partitioning for IPC
sources is a downstream question — the kernel could maintain internal routing
rules, checking badge range on enqueue.

**Combine.** Take N endpoints, return a new endpoint that receives all their
messages. The original endpoints are consumed. For IRQs, the kernel merges
routing. For IPC, existing send caps to the originals... either become dead
(D11) and clients re-connect, or the kernel transparently forwards. The details
are downstream of the endpoint model.

Both operations are cold-path (setup/reconfiguration, not per-message). Both are
potentially useful independent of interrupts: split for structured load
distribution, combine as an alternative to multi-wait (D19).

---

## Hot path analysis

The interrupt hot path (every interrupt) is:

1. Exception entry on the targeted core (O3)
2. Kernel reads ICC_IAR1_EL1 to identify the interrupt (per-core GIC CPU
   interface register)
3. Kernel looks up the IRQ→endpoint routing (kernel-internal mapping)
4. Kernel masks the interrupt in the GIC (or auto-masked by IAR read)
5. Kernel creates a send-once ack cap, enqueues message to the endpoint with
   badge + ack cap
6. Kernel writes ICC_EOIR1_EL1 for EOI
7. Kernel runs scheduler — if the driver Observer is waiting on the endpoint on
   this core, direct process switch (D13 fast path, ~400 cycles ARM64)

Steps 2, 4, 6 touch only per-core GIC CPU interface registers — no shared state
(D1 hot-path requirement satisfied). Step 5 includes send-once cap creation —
the same operation D16's Call() performs on every RPC.

The ack path:

1. Driver uses the send-once ack cap (IPC send)
2. Kernel receives the ack, unmasks the interrupt in the GIC
3. Send-once cap is consumed

Both paths are per-core with no shared mutable state on the hot path.

---

## Tensions

**T1 — Interrupt-to-driver latency.** Every device interrupt traverses the
endpoint mechanism before reaching userspace. Inherently longer than monolithic
ISR dispatch. D13's direct-switch fast path mitigates for the common case
(driver waiting on its endpoint, same core). Landscape §5.5: "Most microkernels
keep the in-kernel path so short that nesting provides negligible benefit"
(Blackham et al.: non-preemptible seL4 achieves 10k–100k cycle worst-case).
Accepted cost of the microkernel model.

**T2 — GIC complexity in the kernel.** GICv3 programming (Distributor,
Redistributors, CPU Interface, potentially ITS for LPIs) is substantial. A5
places it kernel-side behind the simple endpoint interface. Philosophy "use what
the hardware provides" confirms: the kernel programs the GIC, doesn't
reimplement interrupt routing in software.

**T3 — Routing is shared mutable state.** GICD_IROUTER per SPI is cross-core.
Cold-path per D1, but changing routing requires touching the shared Distributor.
Cross-core coordination (O2) may be needed.

**T4 — Endpoint destroy ↔ GIC state coupling.** Destroying an endpoint that
carries IRQ routes must mask those interrupts. The kernel must track which
endpoints have IRQ routes and clean up GIC state on destroy.

**T5 — Send-once cap per interrupt.** Cap-table slot allocation/deallocation on
every interrupt. Same cost as D16's Call() on every RPC — not a new cost, but
present on the interrupt hot path. Send-once performance optimization benefits
both RPC and interrupts.

---

## Non-load-bearing axioms

**A1 (Rust)** is not load-bearing. The work is done by A3, A4, A5, D1, D4, D7,
D12, D13, D16, D17, D18.

**A2 (ARM64)** is load-bearing but only for the hardware mechanism (GIC). The
delegation decision derives from A3 + A4 + A5; A2 provides the specific
hardware. If A2 changed to a different architecture, delegation and the endpoint
model would remain; only the kernel-internal GIC code would change.

---

## Archive convergence

The archive (restart-1) established the unification principle independently:
journal/002 ("all information delivery is one mechanism — faults, interrupts,
IPC are just messages with different metadata"). D22's endpoint-only model is
the strongest form of this commitment: interrupts are not just "like" messages
through endpoints — they ARE messages through endpoints, with no separate object
type. The archive arrived at the principle; D22 follows it to its conclusion.

---

## Derivation trail

D22 went through three revisions during Phase 5 evaluation, each eliminating a
proposed object type:

1. **IRQControl factory + interrupt binding** — rejected: factory pattern
   inconsistent with D4 designation = authority.
2. **IRQ objects parallel to Space** — rejected: the IRQ object duplicated
   structure the endpoint already provides; the object's relationship to the
   endpoint is sender-to-channel, not resource-to-framework.
3. **Endpoints only** — settled: D13 commits to endpoint-based delivery, D16
   provides ack via send-once, no new type needed.

Each revision applied the design's own principles more thoroughly. The
progression demonstrates philosophy "find the abstraction that absorbs the edge
cases": the endpoint already handles IPC, faults, and (with send-once) the
interrupt ack pattern. Adding an IRQ object was reaching for a new type when the
existing abstraction already covered the use case.

---

## What remains open

- **Endpoint split semantics.** The split-by-IRQ-range operation creates a new
  endpoint and moves IRQ routes. Exact semantics: does the original endpoint
  retain a reference for automatic return on destroy (crash recovery)? Does
  split generalize to badge-range partitioning for IPC sources?
- **Endpoint combine semantics.** Take N endpoints, return one. What happens to
  existing send caps on the originals? Transparent forwarding, dead handles, or
  explicit migration?
- **Boot distribution of IRQ authority.** The initial Observer receives the root
  interrupt endpoint. The mechanism is the same as Space distribution — one
  unsettled question, one answer for both.
- **Interrupt priority exposure.** GICv3 8-bit hardware priority. Deferred.
- **IRQ routing policy.** Which core receives a given SPI. Deferred.
- **Userspace timers.** Preemption timer is kernel-internal (D2). Userspace
  timer callbacks connect to D2 scheduling model.
- **GICv4 forward-compatibility.** Direct virtual interrupt injection. Out of
  scope for the base model.
- **D18 revisit trigger check.** D18's status says "revisit if the interrupt
  model derivation reveals that error-on-full combined with interrupt masking
  creates unsolvable delivery gaps." D22 confirms mask-on-delivery with
  send-once ack: the GIC holds pending state, the driver unmasks via ack cap. No
  unsolvable delivery gap. D18 trigger does not fire.
