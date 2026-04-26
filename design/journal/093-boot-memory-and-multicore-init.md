# D93 — Boot memory and multi-core initialization

**Question:** How does the kernel bootstrap its memory allocator and bring up
multiple cores without circular dependencies or races?

**Rests on:** D31 (root Space pool — kernel retains unallocated memory), D46
(core lifecycle — all cores activate before root Observer creation), D70
(per-type slab with page return — arena page source), D75 (KernelState bundle —
arenas + SpaceManager in one struct), D82 (global placement in frame/ —
MaybeUninit static), D83 (PerCoreData layout — assembly-visible indirection
struct), D1 (per-core hot path), D2 (per-core schedulers may run different
algorithms), D59 (Scheduler trait — five methods, concrete implementations), A2
(ARM64 — PSCI CPU_ON for secondary bring-up), A4 (purely reactive — no kernel
thread).

**Status:** settled.

---

## Settles

### Bare-metal slab page source

Arena pages are drawn from the SpaceManager's root pool (D70, D31). The
chicken-and-egg between "arenas need a page allocator" and "the page allocator
is constructed at boot" is resolved by sequencing, not by special-casing:

1. BSP parses DTB (physical memory nodes, kernel image extent, DTB blob
   location).
2. BSP constructs a SpaceManager with a root pool covering all available RAM
   minus the kernel image, DTB, and initial binary (see D94).
3. BSP constructs empty arenas — `Arena::new()` with an empty `SlabStore`.
4. BSP bundles arenas + SpaceManager into a `KernelState` and calls
   `frame::init_kernel_state()`.
5. First object allocation (e.g., root Observer creation) triggers a slab page
   request, which draws from the root pool through the now-initialized
   SpaceManager.

No circular dependency: the SpaceManager exists before any arena allocation
occurs. Arenas reference the SpaceManager through the `KernelState` global, not
through a constructor parameter — the indirection through
`frame::kernel_state()` means arenas need only be structurally valid (empty) at
construction time. The page source becomes available by the time the first
allocation fires.

This matches the existing code structure:

- `KernelState::new()` accepts empty arenas and a SpaceManager (D82).
- `frame::init_kernel_state()` moves the bundle into the `MaybeUninit` global
  (D82).
- The bare-metal `SlabStore` is currently a stub that panics — wiring it to the
  root pool is the next implementation step.

### Physical memory partitioning

DTB memory nodes describe available physical RAM. The kernel must subtract
reserved regions before constructing the root pool:

| Region         | Source                                                 | Size                    |
| -------------- | ------------------------------------------------------ | ----------------------- |
| Kernel image   | Linker symbols (`__kernel_start`, `__kernel_end`)      | Determined at link time |
| DTB blob       | x0 register (DTB base), DTB header (`totalsize` field) | Determined at boot      |
| Initial binary | DTB module node (see D94) — physical address and size  | Described by bootloader |

The remainder becomes the root pool's `total_bytes` and `free_bytes`. The
`next_physical_base` cursor is initialized past the last reserved region (or to
`page_size` if all reserved regions are below the first free page).

Reserved-memory DTB nodes (`/reserved-memory`) are a hardware-port concern. The
initial hypervisor environment does not generate reserved-memory nodes. When a
hardware port is attempted, the DTB parser will subtract these regions from the
pool. Noted, not blocking.

### Secondary core handoff

D46 settles that all cores activate before root Observer creation. The full boot
sequence is:

1. **BSP completes system init:** parses DTB, constructs SpaceManager,
   constructs KernelState, calls `frame::init_kernel_state()`. The global is now
   live. BSP creates the root Observer and related boot objects (root Space,
   root Time, initial Fields).
2. **BSP assigns root Observer to itself:** BSP's `CoreState` gets the root
   Observer as `current`. BSP resumes it — root Observer begins executing.
3. **BSP issues PSCI CPU_ON for each secondary:** passes per-core stack top
   (from `CoreStacks`, existing code in `cpu.rs`) and entry point
   (`__secondary_entry`).
4. **Each secondary initializes:** exception vectors, MMU (secondary init), GIC
   redistributor, TPIDR_EL1. This is the existing `secondary_main` path. The
   addition: each secondary constructs its `PerCoreData` and `CoreState<S>`
   (D83), then enters the scheduling loop.
5. **Secondaries find no runnable Observers:** `scheduler.pick_next()` returns
   `None`. The core enters WFI (D46 idle). It wakes on IPI (O2) when work
   arrives (e.g., a newly created Observer is placed on its run queue via D56).

The ordering guarantee — BSP completes `init_kernel_state()` before any
`PSCI CPU_ON` — prevents races on the global `KernelState`. Secondaries can
safely call `frame::kernel_state()` after their TPIDR_EL1 setup because the
`KERNEL_STATE_INITIALIZED` flag is `true` by that point (BSP set it with
`Release` ordering; secondary cores' first access uses `Acquire`).

### Per-core scheduler instantiation

Each `CoreState<S>` is parameterized by a concrete `Scheduler` type (D2, D59).
The initial implementation uses `RoundRobin` for all cores:

- BSP:
  `CoreState { core_id: 0, current: Some(root_observer), scheduler: RoundRobin::new(), deadlines: [None; MAX], deadline_count: 0 }`
- Each secondary:
  `CoreState { core_id: N, current: None, scheduler: RoundRobin::new(), deadlines: [None; MAX], deadline_count: 0 }`

`RoundRobin::new()` is `const fn` — no allocation, no side effects. The generic
`S` parameter allows swapping algorithms per-core later (D2: big.LITTLE
asymmetric scheduling) without changing the dispatch path. The extension point
is the type parameter, not runtime selection.

---

## Rejected alternatives

- **Arenas pre-allocated with fixed-size backing.** Wastes memory for types that
  are never or rarely used (e.g., Pulsar arena may stay empty for workloads
  without timers). Violates D70's on-demand page draw — slab pages should be
  requested from the root pool only when the first object of that type is
  allocated.

- **Secondary cores idle before KernelState exists.** If secondaries activate
  before `init_kernel_state()` completes, they race on global state access. Even
  with the `KERNEL_STATE_INITIALIZED` check, spinning secondaries while BSP
  constructs KernelState wastes energy and adds synchronization complexity.
  D46's sequencing (BSP completes init, then PSCI CPU_ON) prevents this
  entirely. The existing code in `cpu.rs` already follows this pattern —
  `activate_secondaries()` is called after all BSP init.

- **Per-core scheduler selection at runtime (enum dispatch).** Over-engineering
  for initial development. The generic `S` parameter provides static dispatch
  (zero-cost abstraction), which is both faster (no branch prediction miss on
  `pick_next` hot path) and simpler to reason about. Runtime selection via
  `enum { RoundRobin, FixedPriority, Deadline }` can be added later if
  heterogeneous scheduling is needed on hardware that requires it. RoundRobin is
  the simplest correct initial choice — it is fair and makes no assumptions
  about Observer priorities.

---

## Does NOT settle

- Root pool internal recycling behavior (bump allocator vs. bitmap vs. free list
  — implementation concern behind the SpaceManager interface).
- DTB reserved-memory node handling (hardware-port concern, deferred).
- Core activation ordering — parallel vs. sequential PSCI CPU_ON (D46 noted
  this; both work with the current sequencing guarantee).
- Per-core scheduler algorithm selection policy for heterogeneous hardware (D2
  noted this; RoundRobin for all cores is the initial implementation).
