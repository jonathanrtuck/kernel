# Memory Object Model — 2026-04-16

Ninth exploration. Derived the nature of the capability-designated memory
resource: what kernel object type do Observers hold capabilities to in order to
use memory?

## Starting point

D5 settled MMU-backed virtual memory with per-Observer address spaces and listed
"memory object model" as one level down. D4 settled capability-based authority.
D8 settled kernel-managed flat capability tables with typed-memory backing. The
open question (spec.md): "what is the capability-designated memory resource?
seL4-style typed frames, Zircon-style VMOs, or something new shaped by the Space
vocabulary?"

## Prior work

The archive (restart-1, journal 009) derived a tentative answer: byte-addressed
Memory objects with kernel-internal page backing. That derivation rested on a
prior decision (page size is hidden) that is not settled in the current chain.
The archive answer was treated as a data point, not imported.

The landscape (§2.1) identifies four families: VM objects (Mach/Zircon), typed
capabilities from untypeds (seL4/Barrelfish), flexpages/dataspaces (L4/Genode),
persistent pages/nodes (EROS/KeyKOS). Two-step create/map is dominant (§2.3).

## Derivation

### What necessarily follows from settled decisions

Working through every axiom and derivation:

1. **The memory resource is a kernel object type designated by capabilities**
   (D4). This is not a choice.

2. **Operations on it are typed kernel syscalls** (D7). Create, destroy, bind to
   address space — all kernel operations, not IPC.

3. **Sharing is through capability transfer** (D4 + D6). Two Observers share
   memory by holding capabilities to the same resource.

4. **Physical backing flows through the Space manager** (D3). No bypassing the
   single allocation interface.

5. **The resource exists independently of any address space binding.** D4 says
   the capability designates the resource. If the resource only existed as a
   mapping, the capability would designate a (Space, address range) pair, not
   the resource itself. Two-step follows from capability semantics. This is also
   the dominant pattern across surveyed systems (landscape §2.3).

### The D8 precedent

D8 settled: kernel-managed structure (not userspace-managed), flat (not
tree-structured), physical memory charged to the Observer's budget. The
reasoning was: D7 eliminates the dispatch role that motivated trees, and A5
creates tension with management-complexity pushed to userspace.

This precedent is load-bearing. The same A5 argument that rejected CNode trees
(userspace managing capability table structure is interface complexity) applies
to physical memory management. If managing capability table structure was too
much interface complexity for userspace, managing physical memory allocation and
page table construction is at least as much.

### A5 vs. seL4's untyped model

seL4 pushes memory management entirely to userspace: userspace receives untyped
capabilities, retypes them into kernel objects, constructs page table
hierarchies, and tracks the CDT for revocation. This is motivated by formal
verification (no kernel allocation = no kernel memory leaks by construction).

A5 says the kernel absorbs complexity behind a simple interface. The seL4
approach is the opposite — it exposes all memory management as userspace
responsibility. D8 already used this reasoning to reject CNode trees; the same
argument applies with equal force to the entire untyped-memory model.

The accounting concern (seL4's explicit userspace-visible accounting prevents
resource exhaustion bugs) is real but does not require userspace management.
D8's pattern (kernel manages internally, charges physical memory to Observer's
budget) provides accounting without management burden.

### D5's CHERI note

D5 says "the memory interface should be shaped around objects and permissions,
not page-table-specific concepts, to avoid foreclosing CHERI as a future
complementary enforcement layer."

This pushes against page-granularity interfaces — interfaces that expose page
size, alignment requirements, or page table levels. It pushes toward opaque
memory objects that the kernel manages internally.

### Foreclosed options

- **Ambient memory access** — foreclosed by D4.
- **IPC-mediated memory operations** — foreclosed by D7 (does not foreclose
  fault delegation via IPC, which is a separate question).
- **Persistent memory objects (EROS/KeyKOS)** — foreclosed by A4 (no background
  checkpointing).
- **Tree-structured memory objects (EROS nodes)** — D8 rejected tree-structured
  capability tables on D7 + A5 grounds; same reasoning applies.
- **L4 flexpages** — D7 tension (flexpages couple memory with IPC; D7 separates
  them).

### The three options

After foreclosures, three options remained:

**A. Page-granularity objects.** Each capability designates one hardware page.
Minimal abstraction, transparent accounting. Costs: capability proliferation
(256 capabilities for 1MB at 4K pages), forced page size exposure, CHERI
forward-compatibility weakened (D5 note violated).

**B. Variable-size kernel-managed memory objects.** One capability per logical
allocation. Kernel manages physical backing internally. Satisfies D5 CHERI note.
Costs: kernel absorbs allocation/fragmentation complexity, physical accounting
less visible to userspace.

**C. Variable-size objects with explicit budget.** Same as B, plus a first-class
budget object for accounting and delegation.

### Evaluation

**Option A discounted.** Capability proliferation is mechanical overhead with no
benefit under D8 (flat table means more entries = more memory for the table
itself). The forced page size exposure directly contradicts D5's CHERI note.

**B vs. C dissolved.** The Space vocabulary already describes the budget
concept: "a claim to a portion of the system's bounded memory resource." A
Observer's Spaces are its memory budget. Subdividing Space is budget delegation.
The total conserves because Space is bounded. Option C's explicit budget was
already Space. B vs. C is not a real fork — it's the same answer.

**Accounting transparency is a non-issue.** The "less transparent physical
accounting" listed as a cost of Option B assumes userspace needs to know its
physical page count. It doesn't. An Observer needs to know whether an allocation
succeeded (error return) and how much Space it has (queryable). Which physical
pages back an object, how much tail waste exists, whether huge pages were used —
these are kernel-internal concerns. The kernel enforcing limits internally is A5
working as intended.

### Vocabulary correction

During evaluation, a vocabulary over-specification was identified and corrected.
The Space definition said "not fungible once allocated (a specific claim binds
to specific addresses)" — implying physical address binding, which would
re-couple virtual and physical memory (defeating D5's MMU decoupling) and
prevent the kernel from managing physical layout freely.

Corrected to: "not fungible once allocated (a specific claim has object identity
— it is not interchangeable with a different claim of the same size). Which
physical pages back a claim is a kernel-internal concern."

Non-fungibility means object identity (this allocation is not interchangeable
with another), not physical address binding.

### Non-load-bearing axioms

- **A1** is not load-bearing here. A1 answers "what language"; this entry
  answers "what memory resource shape." Rust's ownership model is compatible
  with the answer but did not discriminate between options.
- **A2** is not load-bearing here. A2 provides the MMU hardware that D5 builds
  on, but D5 (which IS load-bearing) already absorbed A2's contribution. A2 does
  not independently constrain the memory resource shape.
- **A3** creates mild tension with any opinionated model (embedded vs. server
  workloads) but did not discriminate between the surviving options — all three
  were general enough. Not load-bearing in the final decision.
- **A4** forecloses persistent objects (EROS) and background compaction, but
  among the surviving options (A, B, C) it did not discriminate. Not
  load-bearing in the final decision.

## Status

**Settled:**

- The capability-designated memory resource is a variable-size, kernel-managed
  memory object.
- The kernel allocates physical pages behind memory objects and maps them into
  address spaces internally (A5 + D8 precedent).
- Memory objects exist independently of address space binding (two-step: create,
  then bind). Follows from D4 capability semantics.
- Sharing is through capability transfer — multiple Observers hold capabilities
  to the same object.
- Physical backing is drawn from the Observer's Space; which physical pages back
  an object is a kernel-internal concern.
- Space (from the vocabulary) serves as the accounting mechanism: an Observer's
  Space claims are its memory budget.

**Not settled (one level down):**

- Page size exposure (byte-addressed vs. page-addressed interface). Tightly
  coupled — the memory object's interface granularity depends on this.
- Specific operations on memory objects (create, bind, COW/clone, resize,
  subdivide).
- Object-rights (read, write, execute is likely but not derived here).
- Fault delegation (who handles page faults when an Observer accesses unmapped
  virtual memory).
- Precise Space-to-memory-object relationship (is creating an object a Space
  subdivision? How is Space consumed and returned?).
- Observer-Space binding model (how Observers attach to address spaces).
