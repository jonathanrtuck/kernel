# Implementation Plan: Layers 1-3

From "domain logic passes tests" to "EL0 exceptions dispatch through domain
logic and return to userspace." Each task names files to modify, derivations it
implements, and dependencies.

All 417+ existing tests must continue to pass. `scripts/verify` must be green
after each task. All new `unsafe` goes in `src/frame/`.

---

## LAYER 1: Wire the dispatch path

After Layer 1, `dispatch_ipc` and `dispatch_typed` are complete implementations
testable on the host via `cargo test`.

### Task 1.1: Define `KernelState` global struct (D75)

**Files:** new `src/kernel_state.rs`, modify `src/lib.rs`

Define the bundle for all kernel-wide shared cold-path state (D75):

```rust
pub struct KernelState {
    pub fields: Lock<Arena<Field>>,
    pub observers: Lock<Arena<Observer>>,
    pub spaces: Lock<Arena<Space>>,
    pub times: Lock<Arena<Time>>,
    pub pulsars: Lock<Arena<Pulsar>>,
    pub space_manager: Lock<SpaceManager>,
}
```

Each Lock uses the D53 ordering: `LockOrder::Field`, `LockOrder::Observer`,
`LockOrder::Pulsar`, `LockOrder::Space`, `LockOrder::Time`.

Provide `KernelState::new()` with empty arenas and placeholder SpaceManager.
Layer 1 tests construct it locally. The global `static` placement is Task 3.5.

Add `pub mod kernel_state;` to `src/lib.rs`.

**Dependencies:** None.

### Task 1.2: Add `cap_table_capacity` to Observer

**Files:** modify `src/observer.rs`

Add `pub cap_table_capacity: u32` to Observer. This is the spec open question
"Observer cap_table_capacity." Set at Observer creation, updated on D8 table
growth.

Update `Observer::test_default()` to include the field. Update `observer_layout`
size assertion (88 -> 96 bytes with alignment).

**Dependencies:** None.

### Task 1.3: Add IRQ routing table to KernelState

**Files:** modify `src/kernel_state.rs`

```rust
pub struct IrqRoute {
    pub field_id: ObjectId,
    pub badge: Badge,
    pub generation: u64,
}

pub struct IrqRoutingTable {
    pub routes: [Option<IrqRoute>; MAX_IRQS],
}
```

Add `pub irq_routes: Lock<IrqRoutingTable>` to KernelState. The lock order is
unordered (like Space/Time) since it doesn't participate in the
Field-Observer-Pulsar chain.

**Dependencies:** Task 1.1. **DECIDED:** Max IRQ count = 1024. Direct-indexed by
INTID — no translation layer. 16 KB static array covers the full GICv3 SPI
range. The hardware already bounds the space.

### Task 1.4: Add per-core Pulsar deadline data to CoreState

**Files:** modify `src/core_manager.rs`

```rust
pub struct DeadlineEntry {
    pub deadline_ticks: u64,
    pub pulsar_id: ObjectId,
    pub field_id: ObjectId,
    pub badge: Badge,
}

const MAX_DEADLINES_PER_CORE: usize = 32;
```

Add `pub deadlines: [Option<DeadlineEntry>; MAX_DEADLINES_PER_CORE]` and
`pub deadline_count: usize` to `CoreState<S>`. Per-core (D1), no lock needed.

**Dependencies:** None. **DECIDED:** 32 deadlines per core, hard cap. Reject at
CreatePulsar if core is full. Per-core (not global) — Pulsars fire into Fields,
so core assignment is decoupled from Observer placement. Assignment policy
(currently: creating Observer's core) is a leaf node, iterable later.

### Task 1.5: Implement full `dispatch_ipc`

**Files:** modify `src/core_manager.rs`

The full flow:

1. Return `Idle` if `self.current` is None.
2. For `Yield`: call `yield_cpu()`, return `schedule_next()`.
3. For Send/Receive/Call/ReplyRecv: a. Read IPC registers via
   `frame::cores::read_ipc_registers(current_ptr)`. b. Extract target handle
   from `ipc_regs.handle_or_badge` (x5). c. Construct a `Table` view from
   Observer's `cap_table` + `cap_table_capacity`. d. `table.resolve(handle)` —
   check rights (SEND for Send/Call, RECEIVE for Receive/ReplyRecv). e. Check
   generation against target Field's live generation (arena access via
   KernelState). f. Construct `Message` from IPC registers (data words, label,
   badge from cap entry). g. Dispatch to
   `communication::send`/`receive`/`call`/`reply_recv`. h. Handle outcome: for
   `WokeReceiver`/`DirectSwitch`, consult `scheduler.should_switch_to()` (D50).
   If approved and `is_fast_path_eligible()`, direct-switch. Otherwise enqueue
   woken Observer and `schedule_next()`. i. For errors: write error code to
   RegisterState (D49: carry flag + x0).

**Dependencies:** Tasks 1.1, 1.2. **DECIDED:** Handle ABI encoding: lower 32
bits = index, upper 32 bits = slot_tag. Index extraction is a single `and` (mask
low 32). Conventional: more-frequently-accessed field in low bits.

### Task 1.6: Implement full `dispatch_typed`

**Files:** modify `src/core_manager.rs`

The full flow:

1. Return `Idle` if `self.current` is None.
2. Read typed registers via `frame::cores::read_typed_registers(current_ptr)`.
3. For operations requiring a target cap: resolve handle, check type via
   `TypedOperation::target_type()`, check rights.
4. Dispatch to the 20 type-specific operations. Each acquires the appropriate
   arena lock from KernelState, looks up the object by ObjectId, calls the
   domain method, writes result to RegisterState x0.

Operations grouped by arena:

- **Observer (7):** Resume, InstallCap, WriteRegisters, ReadRegisters, Suspend,
  ChangeHandler, SetScheduling → `Lock<Arena<Observer>>`
- **Generic (4):** Destroy, Clone, Close, Mint → target type's arena
- **Space (2):** Split, Merge → `Lock<Arena<Space>>` + `Lock<SpaceManager>`
- **Field (2):** CreateField, FieldSplit → `Lock<Arena<Field>>`
- **Time (1):** TimeSplit → `Lock<Arena<Time>>`
- **Pulsar (2):** CreatePulsar, ClockRead → `Lock<Arena<Pulsar>>`
- **Observer creation (1):** CreateObserver → `Lock<Arena<Observer>>` +
  `Lock<SpaceManager>`
- **Resource (1):** ResourceRequest → `Lock<SpaceManager>`

Wire each operation to the existing domain method. Where the domain method is
incomplete, return `SyscallError::InvalidState` and document what remains.

**Dependencies:** Tasks 1.1, 1.2.

### Task 1.7: Wire `handle_irq` with IRQ-to-Field routing

**Files:** modify `src/core_manager.rs`

1. Look up IRQ number in KernelState's IRQ routing table.
2. If route exists + generation matches: construct Message with route's badge
   and kernel-defined IRQ label. Enqueue into target Field (Field arena lock).
   If full (D18), add to pending list.
3. If no route: log and ignore.
4. Return `schedule_next()`.

**Dependencies:** Tasks 1.1, 1.3.

### Task 1.8: Wire `handle_timer` with Pulsar deadline checking

**Files:** modify `src/core_manager.rs`

1. Read current timer counter via frame helper.
2. Iterate `self.deadlines` for entries where `deadline_ticks <= current_ticks`.
3. For each expired: construct `Message::timer_fire()`, enqueue into target
   Field (Field arena lock from KernelState). Call `pulsar.rearm()` if
   repeating.
4. Remove expired one-shot deadlines.
5. Existing: `scheduler.on_preempt()` + `schedule_next()`.

**Dependencies:** Tasks 1.1, 1.4. **DECIDED:** Option A — `current_ticks: u64`
as parameter from the exception handler. Single consistent snapshot for all
deadline comparisons (no per-entry drift). Keeps `handle_timer` pure and
testable. frame/ reads the counter, safe code operates on the value.

### Task 1.9: Error reporting helpers (D49)

**Files:** modify `src/frame/cores.rs`

Add frame helpers for writing syscall results to RegisterState:

- `write_ipc_error(observer_ptr, error: SyscallError)`: set carry flag in
  SPSR_EL1 pstate field (bit 29), write error code to gprs[0].
- `write_typed_result(observer_ptr, value: u64)`: write value to gprs[0].
  Negative values are errors per D49.
- `clear_ipc_carry(observer_ptr)`: clear carry flag for successful IPC.

**Dependencies:** None.

---

## LAYER 2: Context switch and exception path

After Layer 2, the kernel can take an SVC from EL0, dispatch through the core
manager, and eret back to a (possibly different) Observer.

### Task 2.1: Define per-core state struct for TPIDR_EL1

**Files:** modify `src/frame/cores.rs`, modify `src/core_manager.rs`

Define a `#[repr(C)]` struct for assembly access:

```rust
#[repr(C)]
pub struct PerCoreData {
    /// Offset 0: assembly reads this for RegisterState save target.
    pub register_state_ptr: *mut RegisterState,
    /// Offset 8: Rust handler reads this to reach CoreState.
    pub core_state_ptr: *mut u8,  // erased generic
}
```

Update `read_core_state`/`read_core_state_mut` to go through PerCoreData.

**Dependencies:** None. **DECIDED:** Option B — separate `#[repr(C)]`
PerCoreData struct. TPIDR_EL1 → PerCoreData (tiny, known layout, assembly reads
offset 0) → pointer to CoreState<S> (Rust side, one dereference). Keeps
assembly's ABI contract decoupled from the generic CoreState layout. The extra
pointer chase is negligible — happens once per exception entry after register
save completes.

### Task 2.2: Rewrite EL0 exception entry assembly (D74)

**Files:** modify `src/frame/arch/aarch64/exception.S`

Split vector table into two paths:

**EL0 entries (sources 8-11):** New `VECTOR_ENTRY_EL0` macro:

1. Read TPIDR_EL1 → PerCoreData pointer.
2. Load `register_state_ptr` from offset 0.
3. Save x0-x30 to RegisterState gprs (offset 0-247).
4. Save SP_EL0 via `mrs` (offset 248).
5. Save ELR_EL1 → pc (offset 256).
6. Save SPSR_EL1 → pstate (offset 264).
7. Save TPIDR_EL0 → tpidr (offset 272).
8. Save FP/SIMD q0-q31, FPCR, FPSR (offset 280+).
9. Read ESR_EL1 and FAR_EL1 into scratch registers.
10. Switch to kernel stack (SP_EL1).
11. Branch to Rust `el0_exception_handler(source, esr, far)`.

**EL1h entries (sources 4-7):** Keep existing `VECTOR_ENTRY` +
`__exception_common`.

**Critical:** Add compile-time offset assertions for every RegisterState field
to match the assembly offsets. First two saves (x0, x1) must happen before any
register is clobbered.

**Dependencies:** Task 2.1.

### Task 2.3: Implement EL0 exception handler in Rust

**Files:** modify `src/frame/arch/aarch64/exception.rs`

New `el0_exception_handler(source: u64, esr: u64, far: u64)`:

1. Read per-core state via `current_core_mut()`.
2. Decode exception:
   - Source 8 (Sync): EC from ESR[31:26].
     - EC 0x15 (SVC): ISS = ESR[15:0].
       - SVC #0: `dispatch_typed(TypedOperation::from_code(x4))`.
       - SVC #1-5: `dispatch_ipc(IpcOperation::from_svc(imm))`.
       - Other: fault.
     - EC 0x20/0x24 (abort from EL0): fault delivery.
     - Other: fault delivery.
   - Source 9 (IRQ): GIC acknowledge → handle_timer or handle_irq.
   - Source 10 (FIQ): handle or ignore.
   - Source 11 (SError): fatal.
3. `DispatchResult` → restore path (Task 2.4).

**Dependencies:** Tasks 2.1, 2.2, 1.5, 1.6.

### Task 2.4: Implement restore path (context switch)

**Files:** new assembly in `exception.S` or `context_switch.S`, Rust wrapper

Assembly
`__restore_observer(register_state_ptr, page_table_root, clock_access, fast_path)`:

1. If page_table_root != current TTBR0_EL1: switch TTBR0, TLB invalidate
   (`dsb ish; isb; tlbi vmalle1is; dsb ish; isb`).
2. Set CNTKCTL_EL1.EL0VCTEN from clock_access (D66).
3. Load SPSR_EL1 from pstate field.
4. Load ELR_EL1 from pc field.
5. Load SP_EL0, TPIDR_EL0.
6. Load FP/SIMD: FPCR, FPSR, q0-q31.
7. If fast_path == 0: load x0-x3. If 1: skip (D47 pass-through).
8. Load x4-x30.
9. `eret`.

Rust wrapper:

- Extract page_table_root, clock_access, register_state from Observer.
- Update PerCoreData.register_state_ptr and CoreState.current.
- Call `__restore_observer`.

For `DispatchResult::Idle`: enter WFI with interrupts enabled.

**Dependencies:** Tasks 2.1, 2.2. **DECIDED:** Full `tlbi vmalle1is` on every
TTBR0 switch for bring-up. Correct by construction — no stale entry survives.
ASID-tagged TLB entries are a future leaf-node optimization (allocator sits
behind the context switch path, no interface changes needed).

### Task 2.5: Connect EL0 IRQ handler to core manager

**Files:** modify `src/frame/arch/aarch64/exception.rs`

In `el0_exception_handler`, source 9 (IRQ):

1. `gic::acknowledge()` → INTID.
2. VTIMER → `core.handle_timer()`.
3. Device IRQ → `core.handle_irq(intid)`.
4. `gic::end_of_interrupt(intid)`.
5. DispatchResult → restore (Task 2.4).

**Dependencies:** Tasks 2.3, 1.7, 1.8.

### Task 2.6: IPC fast-path x0-x3 pass-through

**Files:** modify Task 2.4's restore path, modify `src/core_manager.rs`

Per D74: on the IPC fast path (D50 conditions met), x0-x3 are NOT loaded from
the incoming RegisterState. They stay in physical registers carrying data words
from sender to receiver. The kernel sets x4 (label), x5 (badge), x6 (user cap),
x7 (reply cap) in the receiver's RegisterState before calling restore with
`fast_path = 1`.

`dispatch_ipc` returns metadata alongside `DispatchResult` indicating whether
the fast path applies.

**Dependencies:** Tasks 2.4, 1.5.

---

## LAYER 3: Per-Observer page tables

After Layer 3, each Observer has its own virtual address space.

### Task 3.1: Define page table builder interface

**Files:** new `src/frame/arch/aarch64/page_table.rs`, modify aarch64 `mod.rs`

Interface for per-Observer page tables (16 KiB granule, 36-bit VA):

```rust
pub fn create_page_table(sm: &mut SpaceManager) -> Result<u64, AllocError>;
pub fn map_pages(l2_root_pa: u64, pa_base: usize, va_base: usize,
    page_count: usize, attrs: u64, sm: &mut SpaceManager) -> Result<(), AllocError>;
pub fn unmap_pages(l2_root_pa: u64, va_base: usize, page_count: usize,
    sm: &mut SpaceManager);
pub fn destroy_page_table(l2_root_pa: u64, sm: &mut SpaceManager);
```

Add EL0 permission constants: `AP_RW_EL0 = 0b01 << 6`, `AP_RO_EL0 = 0b11 << 6`.

**Dependencies:** None. **DECIDED:** TTBR0/TTBR1 split. Clear TCR_EL1.EPD1 to
enable TTBR1 walks (T1SZ/granule/cacheability already configured). Set E0PD1
(bit 56) for speculative access mitigation. Kernel mapped via TTBR1 in upper VA
range — requires linker script update and boot trampoline for low-PA →
upper-half-VA transition. User page tables (TTBR0) contain zero kernel entries.
Context switch only writes TTBR0; TTBR1 is fixed post-boot.

### Task 3.2: Implement page table creation/mapping

**Files:** implement `page_table.rs`

- `create_page_table`: allocate L2 root from SpaceManager, zero it.
- `map_pages`: compute L2/L3 indices, allocate L3 pages as needed, write
  descriptors.
- `unmap_pages`: clear entries, free empty L3 pages.
- `destroy_page_table`: walk and free all L2/L3 pages.

Page table pages accessed through identity map (kernel VA == PA).

**Dependencies:** Task 3.1.

### Task 3.3: Map/unmap on Space cap install/remove

**Files:** modify `src/core_manager.rs`

When Space cap installed into Observer (ObserverInstallCap or initial setup):

1. Space → physical base + size from arena.
2. VA assignment from SpaceManager.
3. `page_table::map_pages()` with Observer's page_table_root.

When Space cap removed (Close, cascade): `page_table::unmap_pages()`.

**Dependencies:** Tasks 3.2, 1.6.

### Task 3.4: Boot sequence — initialize KernelState and per-core data

**Files:** modify `src/main.rs`, modify `src/frame/cores.rs`

Update `kernel_main`:

1. After DTB + MMU init: initialize global KernelState with arenas and
   SpaceManager from DTB-discovered RAM.
2. BSP: allocate PerCoreData + CoreState, write to TPIDR_EL1.
3. Secondaries: each initializes own PerCoreData + CoreState.

Use `static MaybeUninit<KernelState>` with boot-time init, safe accessor
`fn kernel_state() -> &'static KernelState`.

**Dependencies:** Tasks 1.1, 2.1. **DECIDED:** KernelState static lives in
`frame/`. The `MaybeUninit` write and `assume_init_ref` are genuinely unsafe —
they belong in the trusted boundary. frame/ owns the static, boots it, exports
`fn kernel_state() -> &'static KernelState`. `kernel_state.rs` defines the type
and safe methods. Framekernel discipline preserved — no `#[allow(unsafe_code)]`
outside frame/.

---

## Dependency graph

```text
1.1 KernelState ─────┬──> 1.3 IRQ routing ──> 1.7 handle_irq
                     |
                     ├──> 1.5 dispatch_ipc ──────> 2.3 EL0 handler ──> 2.5 EL0 IRQ
                     |         ^                         ^
1.2 cap_table_cap ───┘         |               2.2 EL0 assembly <── 2.1 PerCoreData
                               |                         |
                     ├──> 1.6 dispatch_typed             v
                     |                          2.4 restore <── 2.6 fast-path
                     ├──> 1.8 handle_timer
                     |         ^
1.4 deadline data ───┘                          3.1 page table iface
                                                        |
1.9 error helpers                               3.2 page table impl
                                                        |
                                                3.3 map/unmap on cap
                                                3.4 boot sequence (1.1 + 2.1)
```

## Implementation order

1. Tasks 1.1, 1.2, 1.4, 1.9 (independent data structures + helpers)
2. Task 1.3 (depends on 1.1)
3. Tasks 1.5, 1.6 (big dispatch implementations)
4. Tasks 1.7, 1.8 (complete Layer 1)
5. Task 2.1 (PerCoreData)
6. Task 2.2 (assembly rewrite — highest risk)
7. Task 2.3 (Rust EL0 handler)
8. Tasks 2.4, 2.6 (restore + fast-path)
9. Task 2.5 (EL0 IRQ)
10. Tasks 3.1, 3.2 (page table builder)
11. Tasks 3.3, 3.4 (connect + boot)

---

## Decisions — all settled (2026-04-25)

1. **Max IRQ count** (Task 1.3): **1024**, direct-indexed by INTID.
2. **Max deadlines per core** (Task 1.4): **32**, hard cap, per-core with fixed
   assignment at creation. Assignment policy is a leaf node.
3. **Handle ABI encoding** (Task 1.5): **index in low 32, slot_tag in high 32.**
4. **Timer counter access** (Task 1.8): **Parameter from caller.** Single
   consistent snapshot, pure function.
5. **PerCoreData placement** (Task 2.1): **Separate `#[repr(C)]` struct.** One
   pointer chase on Rust side, assembly ABI decoupled from generic CoreState.
6. **TLB strategy** (Task 2.4): **Full invalidation** for bring-up. ASID is a
   future leaf node.
7. **TTBR0 vs TTBR0/TTBR1** (Task 3.1): **TTBR0/TTBR1 split.** Clear EPD1, set
   E0PD1, upper-half kernel VA, linker script update.
8. **KernelState static** (Task 3.4): **In `frame/`.** Framekernel discipline
   intact, safe accessor exported.
