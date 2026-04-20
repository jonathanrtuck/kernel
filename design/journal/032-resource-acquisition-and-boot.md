# Resource Acquisition and Boot Architecture — 2026-04-20

Thirty-second exploration. Derived how Observers acquire bounded resources
(Space and Time), the boot architecture (initial capability graph and root fault
handling), and a Time vocabulary revision (abstract scheduling capacity, core
assignment kernel-internal).

## Starting point

Three interlocking open questions from spec.md: "Space acquisition at runtime,"
"Time creation authority," and "Root/bootstrap fault handling." Phase 2
confirmed no journal entry had explored any of them. D22 assumed a "boot
distribution protocol, not yet settled." Multiple entries (004, 012, 020,
030, 031) flagged boot and creation authority as deferred.

The compound question: when the kernel finishes hardware init, what does it
create, and what authority does the first Observer hold?

## Prior work summary

Landscape §1.7 identifies five bootstrapping models: seL4 (root task gets
everything), Zircon (userboot gets handles via channel), EROS (persistence
sidesteps boot), Genode (core holds all, delegates to init), L4Re (sigma0 +
Moe).

Research (authority-models.md §5.6): "capability systems require explicit
initial distribution of authority — there is no 'root' ambient fallback."

Research (memory-resource-capability.md): six-system survey of initial
distribution. Two families: explicit (seL4, Barrelfish — userspace holds all
physical memory as capabilities) and implicit (Zircon, Mach — kernel holds
internal pool, userspace requests allocation).

D9 settled kernel-managed memory (rejecting seL4's untyped model on A5 grounds).
D12 settled fault delegation with "root/bootstrap case" as open. Journal 022
rejected the IRQControl factory pattern with D4 reasoning: "holding a cap to a
resource IS the authority over it."

## Derivation

### Three creation authority models evaluated

**Factory caps:** A capability to the Space manager or per-core Time manager.
Invoking it creates a new object. Rejected: introduces a new capability category
(authority-to-create, not a kernel object), and journal 022's D4 reasoning
applies — a factory separates authority-to-create from the created object.
Additionally, factory caps don't name which Space pays for the creation, making
accounting indirect.

**Split model with omnipotent root Observer:** Root Observer holds all-of-Space
and all-of-Time-per-core. New objects created by splitting. D4-clean (holding a
cap IS the authority). Conservation is visible in the capability graph. Rejected
for security: puts all resources in a userspace Observer — a god object at EL0.
Even if the root Observer redistributes quickly, there is a window during which
a single userspace entity holds all system authority.

**Pager-chain model (chosen):** The kernel retains resource pools internally (as
root Space and root Time objects, subject to the same split invariants). The
root Observer starts with minimal resources. Observers acquire more through the
pager chain: a resource request syscall is routed by the kernel to the
Observer's fault handler (D20/D21), using the same mechanism as page fault
delivery (D12). The handler can grant (from own holdings), deny, or escalate
(its own handler receives the request). The chain terminates at the kernel,
which allocates from its pools.

### Why the pager-chain model wins

**Security (the decisive argument).** The split model puts all resources in a
userspace Observer — EL0, attackable. The pager-chain model puts unallocated
resources in the kernel — EL1, behind the hardware trust boundary. No userspace
entity ever holds all authority. Compromising any Observer (including the root)
yields only that Observer's current holdings.

**Conservation is structural, not lost.** The kernel's internal pools ARE Space
and Time objects with the same split invariants. Total physical memory =
kernel's root Space + all granted Spaces. Total scheduling capacity per core =
kernel's root Time + all granted Times. The kernel can't over-allocate — split
constrains it. Conservation is enforced by the same mechanism regardless of who
holds the root.

**D9/A5 consistency.** D9 settled kernel-managed memory. A5 says the kernel
absorbs complexity. The pager-chain model follows through: the kernel IS the
resource manager, retains the pool, handles allocation. The split model
partially undoes D9 by making the root Observer the de facto allocator.

**D12 reuse (no new mechanism).** Resource acquisition uses the same mechanism
as fault handling. The kernel routes resource request messages to the Observer's
fault handler, identically to page fault messages. The pager handles both fault
types uniformly. No new concept.

**Both models involve kernel policy.** The split model's policy is "give
everything to one Observer and trust it completely" — decided at boot. The
pager-chain model's policy is "allocate on request if available" — trivially
simple. Neither is policy-free. The pager-chain model's policy is simpler and
pushes real policy (quotas, rate-limiting, denial reasons) into userspace
pagers, which is where D12 says policy belongs.

### Resource request mechanism

An Observer that needs more Space or Time invokes a resource request syscall.
The kernel packages the request as a fault message and delivers it to the
Observer's fault handler (D20/D21) using the same mechanism as page fault
delivery (D12). The Observer does not know who its handler is — the kernel
mediates, identically to hardware page faults.

This is D12's pattern applied one level up:

- Page fault: hardware exception → kernel routes to pager → pager resolves →
  kernel resumes.
- Resource request: Observer syscall → kernel routes to pager → pager resolves →
  kernel resumes.

The pager receives both as messages on its fault handler endpoint. The message
carries a fault type distinguishing page faults from resource requests.

D8 already describes this exact pattern for cap table growth: "When the table is
full and a new capability must be stored, the kernel faults the Observer; the
fault handler provides more memory, then retries." The resource request
mechanism generalizes D8's existing pattern to all resource acquisition.

### Structural object creation from Space

Endpoints and Observers are structural objects backed by Space. Creating them
requires presenting a Space cap — the kernel allocates from that Space and
returns a cap to the new object. The Space shrinks by the allocation cost.
Conservation holds: physical bytes changed purpose, not quantity.

- `create_endpoint(space_cap, queue_size) → endpoint_cap`
- `create_observer(space_cap, config) → observer_cap`

The Space cap IS the creation authority (D4: holding a cap IS the authority). A
"create" right in the Space rights mask (D8) controls which Space caps authorize
creation vs. read/write-only access.

Destruction reverses creation: destroying an Endpoint or Observer returns the
physical backing to the Space it came from.

### Root fault handling

D12 requires every Observer to have a fault handler (D20/D21). The root Observer
is the kernel's hand-picked Observer. Its fault handler = the kernel itself.

The kernel-as-root-pager handles two fault types for its direct children:

1. **Page faults on initial memory:** Cannot occur. Initial Spaces are fully
   physically backed at boot (D26 + D24 — holding a Space cap = having the
   mapping). Any fault on initial memory is a programming error → kernel
   terminates the Observer.
2. **Resource requests:** The kernel allocates from its internal pool (root
   Space or per-core root Time) and grants. If the pool is exhausted → deny.

The kernel makes trivially simple policy: have-it-or-deny. No quotas, no
rate-limiting, no eviction. Real policy lives in userspace pagers layered
through the handler chain.

### Boot architecture

At boot, the kernel:

1. Discovers physical memory and core topology from device tree / firmware.
2. Initializes hardware: MMU, GIC, generic timer, BSP core.
3. Retains all physical memory as its internal root Space, all scheduling
   capacity as per-core root Time objects.
4. Creates the root Observer with minimal resources:
   - Initial Space(s) — code + data + stack, fully physically backed.
   - Initial Time — enough scheduling capacity to run on BSP core.
   - Interrupt endpoint (D22) — receive cap for device interrupt delivery.
   - Fault handler = kernel itself (reserved cap-table slot per D21).
5. Resumes the root Observer. Goes dormant (A4 — purely reactive).

Post-boot, the root Observer builds the system by requesting resources through
the pager chain (its handler is the kernel) and creating child Observers. No
special boot protocol — the normal resource acquisition mechanism applies
immediately.

### Time vocabulary revision

During the exploration, a parallel emerged between Space and Time that the
current vocabulary doesn't capture:

|                      | Space                         | Time                                  |
| -------------------- | ----------------------------- | ------------------------------------- |
| Observer sees        | "I have N bytes"              | "I have X% scheduling capacity"       |
| Observer doesn't see | Physical addresses, VA layout | Which core, which scheduler           |
| Kernel manages       | PA, VA, NUMA placement        | Core assignment, migration, algorithm |

D26 settled that Observers don't see virtual addresses — the kernel manages VA
assignment internally. D9 settled that Observers don't see physical addresses.
By the same A5 reasoning, Observers should not see core identity. The kernel
manages core assignment based on abstract scheduling hints (D2: priority, CPU/IO
classification, deadline).

The vocabulary's "a fraction of a specific logical core's scheduling allocation"
exposes a hardware detail that A5 says the kernel should absorb. Time should be
abstract scheduling capacity. Core assignment, migration between cores, and
algorithm selection are kernel-internal concerns.

This dissolves the "Time migration across cores" open question entirely —
migration is the kernel's internal scheduling decision, not a capability
operation. The Observer's Time cap doesn't change when the kernel moves it to
another core.

D29's derivation arguments (D4 designation, D21 cap-table precedent, journal 023
cap-graph completeness) do not rest on per-core-ness and hold unchanged under
abstract Time. D30's multi-Time argument (server multi-client scenario) also
holds unchanged — multiple Time caps are additive scheduling capacity, the
kernel aggregates them regardless of core.

## Archive convergence

**Strong convergence on resource acquisition and boot.** Archive journal 013
("Object Creation and Context Handles") derived the same model independently:

- "The naive version of subdivision puts all resources in the root Context at
  boot — a god object in userspace (EL0), which is a poor security posture."
- "Instead: the kernel retains the physical resource pools and creates the root
  Context with only what it needs."
- "When a Context needs more resources, it faults... The chain terminates at the
  root Context, whose fault handler is the kernel itself."
- "Wormhole creation consumes Space... spend Space to get a Wormhole."

Archive claims.toml: "Boot is not a special protocol. The kernel creates the
root Object with minimal resources and responds to its requests through the
normal supervision interface... Contrast with seL4, which dumps all capabilities
via BootInfo at boot. That requires the root task to be briefly omnipotent — a
single point [of failure]... retains unallocated resources and grants on
request."

The archive arrived through "supervision trees"; the current derivation arrived
through D12 + D4 + A5. Independent paths, same architecture, same security
argument.

**Divergence on Time abstraction.** The archive had per-Context core affinity
and assignment bookkeeping. The Time vocabulary revision (abstract scheduling
capacity, core assignment kernel-internal) is novel — the archive did not take
this step.

## What remains open

- **Resource request fault message format.** What information does the request
  message carry? Requested resource type, size, Observer handle. Parallels page
  fault message format (D28 downstream).
- **Space "create" right.** Whether Endpoint/Observer creation from a Space cap
  requires a specific right in the Space rights mask (D8). Likely yes for D4
  cleanness — distinguishes "memory access" from "creation authority."
- **Pager unavailability in the chain.** The resource acquisition model commits
  to fault handler chains (handler → handler's handler → ... → kernel). The
  pager unavailability protocol (double-fault-kill vs. chains with propagation)
  is still open but now load-bearing: chains must work for resource escalation.
- **Secondary core bring-up.** How does the root Observer activate secondary
  cores? A typed kernel syscall is likely. The activated core's Time pool
  becomes available for allocation through the pager chain.
- **Time parameters.** What does a Time cap carry? Budget/period, fraction, or
  abstract claim-to-participate. Deferred (unchanged from D29/D30).
- **Time clonability.** D23 uniformity suggests clonable. Unchanged.
- **Observer creation API shape.** D31 settles that creation presents a Space
  cap (create_observer(space_cap, config)). The specific config parameters
  (initial PC/SP, fault handler endpoint + badge, initial Time cap, initial
  capabilities) are downstream.
- **Kernel-internal memory accounting.** The kernel's own structures (page
  tables, Observer structs during boot) are allocated from the kernel's root
  Space. The root Space's accounting is kernel-internal. Formal verification of
  the kernel would verify split invariants on the root Space.
