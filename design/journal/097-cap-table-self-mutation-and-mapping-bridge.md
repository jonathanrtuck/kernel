# D97 — Cap table self-mutation and mapping bridge

**Question:** How do the six cap-table-mutating typed operations (Clone, Close,
Mint, ObserverInstallCap, ObserverWriteRegisters, ObserverReadRegisters,
ObserverChangeHandler) dispatch through the kernel, and how does the D24
cap-mapping invariant connect cap table mutations to page table mutations?

**Rests on:** D4 (designation = authority — closing is user's choice), D8
(kernel-managed cap table), D11 (close + ABA with slot tags), D17 (badge —
immutable, minter-assigned), D21 (fault handler at slot 0), D24 (cap-mapping
invariant — no separate map/unmap), D26 (kernel-assigned VA bases), D33 (destroy
cascade uses close), D35 (composable Observer operations), D38 (Time is linear —
no clone), D39 (Observer rights including read/write-registers, change-handler),
D51 (send-once preserved through clone/mint), D91 (L2/L3 mapping orchestration).

**Status:** settled.

---

## Settles

### Clone (#17a): duplicate entry in caller's own cap table

The caller presents a handle to the source cap. The kernel reads the source
Entry, checks the CLONE right (D52), checks that the object type is not Time
(D38: linear — clone structurally forbidden), allocates a free slot via
`Table::allocate_slot`, and writes a new Entry via `Table::install_at` with the
same object reference, rights, badge, generation, and send-once flag.

The infrastructure exists: `Table::allocate_slot` finds a free slot from the
intrusive freelist, `Table::install_at` writes the entry at a given index. The
dispatch handler is pure wiring — read source, validate, allocate, install.

Send-once (D51) is preserved: the boolean flag is copied from the source entry.
A send-once cap cloned produces another send-once cap. Each can be used once
independently — the use-limited property is per-entry, not per-object.

Badge (D17) is immutable and copied verbatim. The clone inherits the minter's
original badge assignment. Stored generation (D67) is copied from the source —
both entries share the same revocation horizon.

### Close (#17b): free slot in caller's cap table

The caller presents a handle. The kernel calls `Table::close(index)`, which
bumps the slot tag (D11 ABA defense), marks the slot empty, threads it into the
freelist, and returns a `CloseResult` carrying the object type and arena id.

The dispatch path must then handle the D24 cap-mapping invariant: if the closed
cap is a Space cap held by an Observer, closing may trigger unmapping. The
kernel checks whether the Observer still holds any other cap to the same Space
(cap table scan, O(capacity), cold path — D91 established this as acceptable at
~1 us for 1024 slots). If no caps remain, the kernel calls
`frame::mapping::unmap_space_from_observer()` with the Observer's TTBR0 root,
Space VA base, Space L3 table PA, page count, and ASID. This issues the L2
descriptor clear and TLB invalidation (`TLBI VAE1IS` per page, `DSB ISH`,
`ISB`).

`Table::close()` already implements the slot-level mechanics. The mapping bridge
is the new work: reading the closed entry's object type, and when it is a Space,
performing the scan-and-unmap sequence before returning.

Badge tracking (D17 opt-in lifecycle) is deferred — the internal per-badge map
data structure does not exist yet. `CloseResult::ClosedWithBadgeClosure` is
structurally defined but the tracking map that would trigger it is not built.

### Mint (#17c): create attenuated cap with optional badge

The caller presents a source handle and a requested rights mask plus optional
badge value. The kernel reads the source Entry, checks the MINT right (D52),
allocates a slot via `Table::allocate_slot`, and constructs a new Entry with:

- `rights = source.rights.attenuate(requested_rights)` — intersection, can only
  remove rights, never add (D4 attenuation hierarchy).
- `badge = caller_provided_badge` if specified, otherwise `source.badge`. The
  minter chooses the badge (D17); mint is the mechanism for badge assignment.
- `send_once = source.send_once` — preserved through mint (D51).
- `stored_generation = source.stored_generation` — same revocation horizon.
- `object`, `slot_tag` — same object reference, fresh slot tag from the
  allocated slot.

The new entry is installed via `Table::install_at`. The slot index is returned
in x0.

Pure wiring: the Entry struct and `Rights::attenuate` already exist. No new data
structures.

### ObserverInstallCap (#18): cap install triggers page table mapping

D35 defines `observer_install_cap(observer_cap, source_cap) -> slot` as a
general-purpose operation. The caller holds an Observer cap with INSTALL_CAP
right and a source cap to install. The kernel:

1. Resolves the Observer cap (INSTALL_CAP right check, D52).
2. Resolves the source cap in the caller's table (no rights check — any held cap
   can be installed into another Observer's table).
3. Reads the source Entry.
4. Allocates a slot in the target Observer's cap table via `allocate_slot`.
5. Constructs a new Entry (same object, rights, badge, generation, send-once).
6. Installs via `install_at` in the target table.

**The D24 mapping bridge**: if the source cap is a Space cap, the kernel must
also establish the page table mapping in the target Observer's address space.
After the cap table install succeeds, the kernel calls
`frame::mapping::map_space_in_observer(observer_ttbr0, space_va_base, space_l3_table_pa, space_manager)`.
This handles L2 table allocation from the root pool if needed (D92: L2 tables
from kernel root pool).

On success, the Space is mapped — the Observer can access the memory. On failure
(OutOfMemory from L2 allocation), the cap installation is rolled back: the
kernel calls `Table::free_slot` on the just-allocated slot, restoring the table
to its pre-operation state, and returns an error to the caller.

Reverse direction: Close of a Space cap from an Observer triggers
`unmap_space_from_observer()` + TLB invalidation (the Close path above).

No separate map/unmap syscalls exist. D24 is structural: the mapping follows the
cap.

### ObserverWriteRegisters and ObserverReadRegisters (#21): batch register state

D35 specifies composable setup operations. WriteRegisters and ReadRegisters
operate on the full RegisterState as a unit — not per-register.

The RegisterState struct (816 bytes: 31 GPRs + SP + PC + PSTATE + TPIDR + 32
FP/SIMD + FPCR + FPSR) already exists in `frame/arch/aarch64/register_state.rs`.
Compile-time layout assertions guarantee the struct matches assembly offsets.

**WriteRegisters:** The caller provides a RegisterState worth of data in a
memory region designated by a Space cap. The kernel copies from the caller's
memory into the target Observer's saved RegisterState (pointed to by
`Observer::register_state`, a `RegisterStateHandle` into structural backing).
Requires WRITE_REGISTERS right on the Observer cap. The target Observer must be
in a stopped state (Inert or Faulted — D39 state machine).

**ReadRegisters:** The kernel copies from the target Observer's saved
RegisterState into a memory region in the caller's address space. Requires
READ_REGISTERS right. The target Observer must be stopped.

Both operations require `frame/` helpers for bulk register state copy — the
RegisterState lives in structural backing accessed through the
RegisterStateHandle, and the copy crosses the safe/unsafe boundary (pointer
dereference into structural backing pages).

D35 establishes these as composable setup operations: create Observer (inert),
write registers (set PC, SP, initial GPRs), install caps (Space, Time, handler),
resume. The same operations serve fault resolution (write PC to redirect after
fault) and debugging (read registers of suspended Observer).

### ObserverChangeHandler (#22): replace fault handler Field

Arguments: target Observer cap (with CHANGE_HANDLER right, D39) + new handler
Field cap handle (in caller's table). The kernel resolves both caps, then
replaces the Entry at SLOT_FAULT_HANDLER (slot 0, D21) in the target Observer's
cap table with the new Field Entry.

The old handler cap is NOT auto-closed. D4: designation = authority. Closing is
the user's responsibility. If the caller wants to close the old handler, it must
explicitly call Close on it (if it holds a reference). The kernel's role is slot
overwrite, not lifecycle management.

Implementation: read the new handler Entry from the caller's table, construct a
replacement Entry preserving the Field's object reference and badge, call
`Table::install_at(SLOT_FAULT_HANDLER, new_entry)` on the target Observer's
table. The install_at method handles count bookkeeping (the slot transitions
from occupied to occupied — count unchanged).

D12 establishes the fault handler as the root of the Observer's supervision
relationship. D39 separates CHANGE_HANDLER from INSTALL_CAP because the handler
slot is structurally special — it determines who receives fault notifications, a
fundamentally different authority from routine cap provisioning.

---

## Rejected alternatives

**Per-register read/write (by index):** D35 specifies composable batch
operations. Per-register operations add syscall overhead for multi-register
setup — a typical Observer start requires setting PC, SP, and potentially
several GPRs (5-10 registers), meaning 5-10 syscalls instead of one. The
RegisterState struct is the natural unit. Batch is the common case; the
per-register case (modify one register during fault resolution) is served by
reading the full state, modifying one field, and writing it back.

**Auto-close old handler on ChangeHandler:** D4 says designation = authority.
The kernel does not close on behalf of the user. The old handler may still be
needed (the caller may hold it for other purposes, or another Observer may share
it). Explicit Close is required for any lifecycle transition.

**Separate map/unmap syscalls:** D24 settles this definitively — mapping follows
the cap. No separate operations. The mapping bridge is internal kernel
machinery, not a user-visible interface.

**Clone produces fresh badge:** D17 says badge is minter-assigned and immutable.
Clone copies the badge. To assign a different badge, use Mint.

---

## Does NOT settle

- Badge tracking map data structure (D17 opt-in lifecycle — internal map
  deferred)
- Cap table scan optimization for duplicate Space cap detection (fast-path for
  single-cap-per-Space common case)
- WriteRegisters/ReadRegisters memory region designation (which cap points to
  the buffer, how the kernel locates it in the caller's address space)
- Register state validation on WriteRegisters (which PSTATE bits the kernel
  masks, whether PC must be aligned)
