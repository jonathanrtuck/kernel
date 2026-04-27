//! Core manager unsafe operations — per-core state access, register reads, and
//! result writes.
//!
//! The safe `core_manager.rs` module delegates hardware-dependent operations
//! here: reading TPIDR_EL1 for per-core state, reading Observer saved register
//! contexts for syscall dispatch, writing syscall results back to
//! RegisterState, and test helpers for constructing Observer contexts.
//!
//! D1:  per-core state access via TPIDR_EL1.
//! D47: IPC register layout (x0–x7) in saved register context.
//! D49: error signaling — carry flag for IPC, negative x0 for typed ops.
//! D74: EL0 exception entry saves directly to RegisterState (not TrapFrame).
//! D76: pull model for reads, push model for writes. Safe dispatch reads
//!      registers lazily and writes results via these helpers before returning
//!      DispatchResult.
//! D83: PerCoreData — assembly-visible #[repr(C)] struct stored in TPIDR_EL1.
//!      TPIDR_EL1 → PerCoreData → CoreState<S>. One pointer chase from
//!      assembly-visible struct to generic Rust struct.

#[cfg(test)]
extern crate alloc;

#[cfg(target_os = "none")]
use crate::core_manager::CoreState;
#[cfg(any(target_os = "none", test))]
use crate::frame::arch::register_state::RegisterState;
#[cfg(any(target_os = "none", test))]
use crate::observer::Observer;
#[cfg(any(target_os = "none", test))]
use crate::syscall::{IpcRegisters, SyscallError, TypedRegisters};
#[cfg(target_os = "none")]
use crate::time_manager::Scheduler;
#[cfg(any(target_os = "none", test))]
use core::ptr::NonNull;

// ── Per-core data (D83) ───────────────────────────────────────────

/// Assembly-visible per-core data stored in TPIDR_EL1 (D83).
///
/// `#[repr(C)]` with compile-time offset assertions so assembly code can
/// load fields at known offsets without depending on Rust's layout algorithm.
///
/// TPIDR_EL1 → PerCoreData (tiny, known layout) → CoreState<S> (generic
/// Rust struct, one pointer chase). This decouples assembly's ABI contract
/// from the generic CoreState layout.
///
/// D74: assembly reads `register_state_ptr` at offset 0 to find the save
/// target for EL0 exception entry. The Rust exception handler reads
/// `core_state_ptr` at offset 8 to reach the full CoreState.
///
/// Gated on `target_os = "none"` or `test` because it references
/// `RegisterState` from `frame::arch`, which is only available in those
/// configurations.
#[cfg(any(target_os = "none", test))]
#[repr(C)]
pub struct PerCoreData {
    /// Offset 0: pointer to the current Observer's RegisterState.
    /// Assembly reads this for the EL0 register save target (D74).
    /// Updated on every context switch.
    pub register_state_ptr: *mut RegisterState,

    /// Offset 8: type-erased pointer to CoreState<S>.
    /// Rust handler reads this to reach the full per-core state.
    /// The generic parameter is erased because assembly and this
    /// `#[repr(C)]` struct cannot name the concrete scheduler type.
    pub core_state_ptr: *mut u8,

    /// Offset 16: top of the kernel stack for this core.
    /// EL0 exception entry resets SP to this value before calling the
    /// Rust handler (noreturn pattern — the handler calls
    /// `__restore_observer` or `__enter_idle` instead of returning).
    /// Set once during boot, stable afterward.
    pub kernel_stack_top: *mut u8,
}

/// Byte offset of `register_state_ptr` within `PerCoreData`.
/// Assembly code uses this constant to load the save target.
#[cfg(any(target_os = "none", test))]
pub const PER_CORE_DATA_REGISTER_STATE_OFFSET: usize = 0;

/// Byte offset of `core_state_ptr` within `PerCoreData`.
/// Rust exception handler uses this to reach CoreState.
#[cfg(any(target_os = "none", test))]
pub const PER_CORE_DATA_CORE_STATE_OFFSET: usize = 8;

/// Byte offset of `kernel_stack_top` within `PerCoreData`.
/// EL0 exception entry assembly reads this to reset SP.
#[cfg(any(target_os = "none", test))]
pub const PER_CORE_DATA_KERNEL_STACK_TOP_OFFSET: usize = 16;

// Compile-time layout assertions — these MUST match the assembly offsets.
#[cfg(any(target_os = "none", test))]
const _: () = {
    assert!(core::mem::size_of::<PerCoreData>() == 24);
    assert!(core::mem::align_of::<PerCoreData>() == 8);
};

// Field offset assertions using core::mem::offset_of! (stable in Edition 2024).
// This guarantees the offsets match what assembly uses.
#[cfg(any(target_os = "none", test))]
const _: () = {
    assert!(
        core::mem::offset_of!(PerCoreData, register_state_ptr)
            == PER_CORE_DATA_REGISTER_STATE_OFFSET
    );
    assert!(core::mem::offset_of!(PerCoreData, core_state_ptr) == PER_CORE_DATA_CORE_STATE_OFFSET);
    assert!(
        core::mem::offset_of!(PerCoreData, kernel_stack_top)
            == PER_CORE_DATA_KERNEL_STACK_TOP_OFFSET
    );
};

/// Read the current core's PerCoreData from TPIDR_EL1 (D83).
///
/// Each core stores a pointer to its `PerCoreData` in TPIDR_EL1 at boot.
/// This function reads that register and returns a shared reference.
///
/// D83: TPIDR_EL1 → PerCoreData (assembly-visible) → CoreState<S> (Rust).
///
/// # Safety (structural invariant)
///
/// TPIDR_EL1 must contain a valid pointer to a `PerCoreData` set during
/// boot. Each core writes its own value once via `set_tpidr_el1` during
/// initialization; the value is stable afterward but the register is
/// NOT immutable (it is per-core writable state — do NOT use
/// `sysreg_read_const!`/`nomem`).
#[cfg(target_os = "none")]
pub fn read_per_core_data() -> &'static PerCoreData {
    // SAFETY: TPIDR_EL1 was initialized at boot to point to a valid
    // PerCoreData for this core. Per-core writable state — uses
    // sysreg_read! (no nomem) so LLVM cannot reorder memory accesses
    // past this read. A4 non-reentrancy guarantees no aliasing on a
    // single core.
    unsafe {
        let ptr = crate::frame::arch::tpidr_el1() as *const PerCoreData;

        &*ptr
    }
}

/// Read the current core's state from TPIDR_EL1 via PerCoreData (D83).
///
/// D83: TPIDR_EL1 → PerCoreData → core_state_ptr → CoreState<S>.
/// One pointer chase from assembly-visible struct to generic Rust struct.
///
/// D1: core-local, no cross-core sharing. The returned reference is valid
/// for the duration of the exception handler (A4: non-reentrant).
#[cfg(target_os = "none")]
pub fn read_core_state<S: Scheduler>() -> &'static CoreState<S> {
    // SAFETY: TPIDR_EL1 points to a valid PerCoreData, whose
    // core_state_ptr was initialized at boot to point to a valid
    // CoreState<S>. Per-core writable state — no nomem. A4
    // non-reentrancy guarantees no aliasing on a single core.
    unsafe {
        let per_core = read_per_core_data();
        let ptr = per_core.core_state_ptr as *const CoreState<S>;

        &*ptr
    }
}

/// Mutable access to the current core's state via PerCoreData (D83).
///
/// Same as `read_core_state` but returns `&'static mut`. Safe because A4
/// guarantees the kernel is non-reentrant on a single core — only one
/// exception handler runs at a time, so there can be no aliasing.
#[cfg(target_os = "none")]
pub fn read_core_state_mut<S: Scheduler>() -> &'static mut CoreState<S> {
    // SAFETY: Same invariant as read_core_state. Mutable access is
    // safe because A4 guarantees non-reentrancy — the caller is the
    // only exception handler running on this core.
    unsafe {
        let per_core_ptr = crate::frame::arch::tpidr_el1() as *mut PerCoreData;
        let per_core = &*per_core_ptr;
        let ptr = per_core.core_state_ptr as *mut CoreState<S>;

        &mut *ptr
    }
}

/// Read IPC registers from an Observer's saved register state (D47).
///
/// D47: x0–x3 = data words, x4 = label, x5 = target handle,
/// x6 = user cap handle (u64::MAX = absent), x7 = reply info.
///
/// The register state was saved by the EL0 exception entry code directly
/// into RegisterState before calling into the core manager (D74).
#[cfg(any(target_os = "none", test))]
pub fn read_ipc_registers(observer_ptr: NonNull<Observer>) -> IpcRegisters {
    // SAFETY: observer_ptr was obtained from CoreState::current, which
    // points to a live Observer in the arena. The Observer's
    // register_state.0 points to a valid RegisterState in structural
    // backing. The EL0 exception entry code saved the full register
    // context directly into RegisterState before calling dispatch (D74).
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &*(observer.register_state.as_ptr().as_ptr()
            as *const crate::frame::arch::register_state::RegisterState);

        IpcRegisters {
            data: [rs.gprs[0], rs.gprs[1], rs.gprs[2], rs.gprs[3]],
            label: rs.gprs[4],
            handle_or_badge: rs.gprs[5],
            user_cap: rs.gprs[6],
            reply_info: rs.gprs[7],
        }
    }
}

/// Read typed operation registers from an Observer's saved register state (D47, D49).
///
/// D49: SVC #0, x4 = operation code, x5 = target cap handle.
/// x0–x3 carry operation-specific arguments.
#[cfg(any(target_os = "none", test))]
pub fn read_typed_registers(observer_ptr: NonNull<Observer>) -> TypedRegisters {
    // SAFETY: same invariant as read_ipc_registers.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &*(observer.register_state.as_ptr().as_ptr()
            as *const crate::frame::arch::register_state::RegisterState);

        TypedRegisters {
            op_code: rs.gprs[4] as u16,
            target_handle: rs.gprs[5],
            args: [rs.gprs[0], rs.gprs[1], rs.gprs[2], rs.gprs[3]],
        }
    }
}

// ── Write helpers (D76, D49) ───────────────────────────────────────
//
// Safe dispatch writes syscall results to RegisterState via these
// helpers before returning DispatchResult. The helpers are the frame/
// side of the D76 contract: dispatch knows WHAT to write, frame/ knows
// HOW (RegisterState layout, SPSR bit positions).

/// ARM64 SPSR carry flag position (NZCV: bits 31:28, C = bit 29).
const SPSR_CARRY_BIT: u64 = 1 << 29;

/// Write an IPC error to an Observer's saved register state (D49, D76).
///
/// Sets the carry flag in SPSR_EL1 (pstate field) and writes the error
/// code to x0 (gprs[0]). On eret, userspace sees carry set = error.
#[cfg(any(target_os = "none", test))]
pub fn write_ipc_error(observer_ptr: NonNull<Observer>, error: SyscallError) {
    // SAFETY: observer_ptr points to a live Observer in the arena.
    // RegisterState was saved by EL0 exception entry (D74) and is
    // valid for mutation until the Observer is resumed.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &mut *(observer.register_state.as_ptr().as_ptr()
            as *mut crate::frame::arch::register_state::RegisterState);

        rs.pstate |= SPSR_CARRY_BIT;
        rs.gprs[0] = error as u64;
    }
}

/// Clear the IPC carry flag for a successful IPC return (D49, D76).
///
/// Clears the carry flag in SPSR_EL1 (pstate field). On eret,
/// userspace sees carry clear = success, registers carry message data.
#[cfg(any(target_os = "none", test))]
pub fn clear_ipc_carry(observer_ptr: NonNull<Observer>) {
    // SAFETY: same invariant as write_ipc_error.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &mut *(observer.register_state.as_ptr().as_ptr()
            as *mut crate::frame::arch::register_state::RegisterState);

        rs.pstate &= !SPSR_CARRY_BIT;
    }
}

/// Write a typed operation result to an Observer's saved register state (D49, D76).
///
/// Writes `value` to x0 (gprs[0]). D49: non-negative values are success
/// (slot indices, timestamps, zero-for-void). Negative values (bit 63 set)
/// are error codes.
#[cfg(any(target_os = "none", test))]
pub fn write_typed_result(observer_ptr: NonNull<Observer>, value: u64) {
    // SAFETY: same invariant as write_ipc_error.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &mut *(observer.register_state.as_ptr().as_ptr()
            as *mut crate::frame::arch::register_state::RegisterState);

        rs.gprs[0] = value;
    }
}

/// Write ReadRegisters result values to x1–x3 of the caller (D103).
///
/// x0 is already written by `write_typed_result` (PC). This writes
/// the remaining three inline register values: SP, x0, PSTATE.
#[cfg(any(target_os = "none", test))]
pub fn write_read_registers_result(observer_ptr: NonNull<Observer>, sp: u64, x0: u64, pstate: u64) {
    // SAFETY: same invariant as write_typed_result.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &mut *(observer.register_state.as_ptr().as_ptr()
            as *mut crate::frame::arch::register_state::RegisterState);

        rs.gprs[1] = sp;
        rs.gprs[2] = x0;
        rs.gprs[3] = pstate;
    }
}

/// Write IPC receive registers to a receiver's saved register state (D76).
///
/// Slow-path receive: writes all x0–x7. Used when the receiver is not
/// on the fast path (different exception entry, or D50 conditions not met).
///
/// D47 register mapping: x0–x3 = data words, x4 = label, x5 = badge,
/// x6 = user cap slot (u64::MAX if absent), x7 = reply cap handle
/// (u64::MAX if absent).
///
/// Takes pre-computed receive-side values — the dispatch layer handles
/// cap installation (Message.user_cap → slot index) before calling.
#[cfg(any(target_os = "none", test))]
pub fn write_message_to_registers(
    observer_ptr: NonNull<Observer>,
    data: &[u64; 4],
    label: u64,
    badge: u64,
    user_cap_slot: u64,
    reply_cap_slot: u64,
) {
    // SAFETY: same invariant as write_ipc_error.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &mut *(observer.register_state.as_ptr().as_ptr()
            as *mut crate::frame::arch::register_state::RegisterState);

        rs.gprs[0] = data[0];
        rs.gprs[1] = data[1];
        rs.gprs[2] = data[2];
        rs.gprs[3] = data[3];
        rs.gprs[4] = label;
        rs.gprs[5] = badge;
        rs.gprs[6] = user_cap_slot;
        rs.gprs[7] = reply_cap_slot;
    }
}

/// Write IPC metadata registers for fast-path receive (D50, D74, D76).
///
/// Writes only x4–x7 (label, badge, user cap, reply cap). x0–x3 pass
/// through in physical registers carrying data words from sender to
/// receiver — the restore path skips loading them (ResumeFastPath).
#[cfg(any(target_os = "none", test))]
pub fn write_metadata_to_registers(
    observer_ptr: NonNull<Observer>,
    label: u64,
    badge: u64,
    user_cap_slot: u64,
    reply_cap_slot: u64,
) {
    // SAFETY: same invariant as write_ipc_error.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &mut *(observer.register_state.as_ptr().as_ptr()
            as *mut crate::frame::arch::register_state::RegisterState);

        rs.gprs[4] = label;
        rs.gprs[5] = badge;
        rs.gprs[6] = user_cap_slot;
        rs.gprs[7] = reply_cap_slot;
    }
}

// ── Observer access helpers for dispatch ──────────────────────────
//
// Safe dispatch (core_manager.rs) cannot dereference NonNull<Observer>
// directly — that would violate the framekernel boundary. These helpers
// provide the unsafe access, with the same safety invariant as
// read_ipc_registers: the pointer is the current core's Observer
// (or a recently-resolved Observer from an arena), and A4 non-reentrancy
// guarantees no aliasing on a single core.

/// Read an Observer's cap table pointer and capacity (D8, D77).
///
/// Returns the raw (entries pointer, capacity) pair needed for
/// `resolve_cap_entry` on the hot path. The cap table is part of
/// the Observer's structural backing (D43) — always valid while
/// the Observer is alive.
#[cfg(any(target_os = "none", test))]
pub fn observer_cap_table(
    observer_ptr: NonNull<Observer>,
) -> (core::ptr::NonNull<crate::capability::Entry>, u32) {
    // SAFETY: observer_ptr points to a live Observer in the arena.
    // A4 non-reentrancy guarantees no aliasing on a single core.
    // The cap_table and cap_table_capacity fields are always valid
    // while the Observer is alive (D8, D43).
    unsafe {
        let observer = observer_ptr.as_ref();

        (observer.cap_table, observer.cap_table_capacity)
    }
}

/// Prepare an Observer's wait_state for a blocking Receive (D18, D13).
///
/// Sets observer.wait_state = WaitState::Single(WaitEntry{...}) with the
/// given observer pointer and field pointer, and returns a mutable reference
/// to the WaitEntry inside. The WaitEntry must persist if the Observer
/// blocks — it's linked into the Field's waiters list.
///
/// The returned reference is valid for the duration of the dispatch
/// (A4 non-reentrancy). The caller must NOT drop or move the Observer
/// while the WaitEntry reference is live.
#[cfg(any(target_os = "none", test))]
pub fn observer_prepare_wait(
    observer_ptr: NonNull<Observer>,
    field_ptr: NonNull<crate::field::Field>,
) -> &'static mut crate::observer::WaitEntry {
    use crate::observer::{WaitEntry, WaitState};

    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy
    // guarantees exclusive access on this core. We set the wait_state
    // field and then return a mutable reference into it. The 'static
    // lifetime is bounded by the Observer's arena lifetime (same
    // invariant as read_ipc_registers).
    unsafe {
        let observer = &mut *observer_ptr.as_ptr();

        observer.wait_state = WaitState::Single(WaitEntry {
            observer: observer_ptr,
            field: field_ptr,
            prev: None,
            next: None,
        });

        match &mut observer.wait_state {
            WaitState::Single(entry) => &mut *(entry as *mut WaitEntry),
            _ => core::hint::unreachable_unchecked(),
        }
    }
}

/// Call reply_recv with two distinct &mut Field references (D16).
///
/// Safe dispatch cannot obtain two &mut Field references from the arena
/// simultaneously (Arena::get_mut borrows the whole arena). This helper
/// takes NonNull pointers to two distinct Field slots and converts them to
/// mutable references for the reply_recv call.
///
/// The caller guarantees that reply_field_ptr and recv_field_ptr point
/// to different arena slots (different ObjectIds, checked before calling).
#[cfg(any(target_os = "none", test))]
pub fn call_reply_recv(
    reply_field_ptr: NonNull<crate::field::Field>,
    recv_field_ptr: NonNull<crate::field::Field>,
    reply_message: crate::field::Message,
    receiver: &mut crate::observer::WaitEntry,
) -> crate::communication::ReplyRecvOutcome {
    // SAFETY: The caller has verified that reply_field_ptr and recv_field_ptr
    // point to different arena slots (different ObjectIds). Both pointers
    // were obtained from Arena::get_mut within the same lock acquisition,
    // so they point to valid, live Field objects. The arena lock is held
    // for the duration of this call, preventing deallocation. No aliasing
    // occurs because the two pointers target different slots.
    unsafe {
        let reply_field = &mut *reply_field_ptr.as_ptr();
        let recv_field = &mut *recv_field_ptr.as_ptr();

        crate::communication::reply_recv(reply_field, recv_field, reply_message, receiver)
    }
}

/// Clear an Observer's wait_state back to None after a non-blocking receive.
///
/// Called when Receive returns Received (not Blocked) — the WaitEntry
/// was never linked into any list, so we just clean up the state.
#[cfg(any(target_os = "none", test))]
pub fn observer_clear_wait(observer_ptr: NonNull<Observer>) {
    use crate::observer::WaitState;

    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        let observer = &mut *observer_ptr.as_ptr();

        observer.wait_state = WaitState::None;
    }
}

/// Transition an Observer from Runnable to Blocked (D39).
///
/// Called after the Observer's WaitEntry has been linked into a Field's
/// waiters list. The wait_state is already set up by observer_prepare_wait.
///
/// Returns Ok(()) on success, Err if the transition is invalid.
#[cfg(any(target_os = "none", test))]
pub fn observer_set_blocked(
    observer_ptr: NonNull<Observer>,
) -> Result<(), crate::observer::ObserverError> {
    use crate::observer::PrimaryState;

    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        let observer = &mut *observer_ptr.as_ptr();

        match observer.state {
            PrimaryState::Runnable => {
                observer.state = PrimaryState::Blocked;
                Ok(())
            }
            _ => Err(crate::observer::ObserverError::InvalidTransition),
        }
    }
}

/// Transition an Observer from Blocked to Runnable (D39).
///
/// Returns Ok(true) if the Observer should be enqueued (not suspended),
/// Ok(false) if suspended, Err if the transition is invalid.
#[cfg(any(target_os = "none", test))]
pub fn observer_unblock(
    observer_ptr: NonNull<Observer>,
) -> Result<bool, crate::observer::ObserverError> {
    use crate::observer::PrimaryState;

    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        let observer = &mut *observer_ptr.as_ptr();

        match observer.state {
            PrimaryState::Blocked => {
                observer.state = PrimaryState::Runnable;
                observer.wait_state = crate::observer::WaitState::None;
                Ok(!observer.suspended)
            }
            _ => Err(crate::observer::ObserverError::InvalidTransition),
        }
    }
}

// ── Cap transfer helpers (D96) ────────────────────────────────────
//
// Unsafe pointer wrappers for D96 IPC cap transfer. These dereference
// the Observer pointer (the only unsafe part) and delegate to Table
// methods via Observer::with_cap_table for freelist operations.

/// Extract a capability from an Observer's cap table (D96 move semantics).
/// Delegates to Table::extract_cap. Returns None if out of bounds or empty.
#[cfg(any(target_os = "none", test))]
pub fn observer_extract_cap(
    observer_ptr: NonNull<Observer>,
    index: u32,
) -> Option<crate::capability::TransferredCap> {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe { (*observer_ptr.as_ptr()).with_cap_table(|table| table.extract_cap(index)) }
}

/// Install a transferred capability into an Observer's cap table (D96).
/// Delegates to Table::install_transferred_cap. Returns the encoded handle
/// or Err(TableFull).
#[cfg(any(target_os = "none", test))]
pub fn observer_install_transferred_cap(
    observer_ptr: NonNull<Observer>,
    transferred: &crate::capability::TransferredCap,
) -> Result<u64, crate::capability::CapError> {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        (*observer_ptr.as_ptr()).with_cap_table(|table| table.install_transferred_cap(transferred))
    }
}

/// Read a specific cap table entry from an Observer (D96, D43).
/// Delegates to Table::read_entry. Returns None if empty or out of bounds.
#[cfg(any(target_os = "none", test))]
pub fn observer_read_cap_entry(
    observer_ptr: NonNull<Observer>,
    index: u32,
) -> Option<(crate::capability::ObjectType, crate::arena::ObjectId, u64)> {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe { (*observer_ptr.as_ptr()).with_cap_table(|table| table.read_entry(index)) }
}

/// Read the full cap table Entry at a slot index (D100).
///
/// Used by dispatch_fault to read the handler cap at slot 0 with all
/// fields (rights, badge, generation) for validate_handler_cap.
#[cfg(any(target_os = "none", test))]
pub fn observer_read_full_cap_entry(
    observer_ptr: NonNull<Observer>,
    index: u32,
) -> Option<crate::capability::Entry> {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe { (*observer_ptr.as_ptr()).with_cap_table(|table| table.read_full_entry(index)) }
}

/// Get a NonNull<Observer> from the arena by ObjectId (D97).
///
/// Used by ObserverInstallCap and ObserverChangeHandler to obtain a
/// pointer to the target Observer for cap table mutation. The pointer
/// is valid while the arena lock is not held (the Observer's arena
/// slot persists until free() is called).
#[cfg(any(target_os = "none", test))]
pub fn observer_ptr_from_arena(
    kernel_state: &crate::kernel_state::KernelState,
    object_id: crate::arena::ObjectId,
) -> Option<NonNull<Observer>> {
    let mut observers = kernel_state.observers.acquire();
    let observer = observers.get_mut(object_id)?;

    Some(NonNull::from(&mut *observer))
}

/// Run one batch of cascade steps on an Observer's cap table (D98).
///
/// Closes up to `batch_size` cap slots starting from the cascade's
/// current cursor. Returns true if the cascade level is complete
/// (cursor reached cap_capacity), false if more work remains.
#[cfg(any(target_os = "none", test))]
pub fn observer_cascade_step(
    observer_ptr: NonNull<Observer>,
    cascade: &mut crate::capability::CascadeContinuation,
    batch_size: u32,
) -> bool {
    let level = match cascade.current_mut() {
        Some(l) => l,
        None => return true,
    };
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    let cap_capacity = unsafe { (*observer_ptr.as_ptr()).cap_table_capacity };
    let end = (level.slot_cursor + batch_size).min(cap_capacity);

    for slot in level.slot_cursor..end {
        observer_close_cap(observer_ptr, slot);
    }

    level.slot_cursor = end;
    end >= cap_capacity
}

// ── Cap table self-mutation helpers (D97) ─────────────────────────
//
// Unsafe pointer wrappers for D97 cap-table-mutating typed operations.
// Clone, Close, Mint operate on the caller's own table. InstallCap and
// ChangeHandler operate on a target Observer's table.

/// Close a capability slot in an Observer's cap table (D97, D11).
/// Delegates to Table::close. Returns the CloseResult indicating what
/// was closed (or AlreadyEmpty if the slot was free).
#[cfg(any(target_os = "none", test))]
pub fn observer_close_cap(
    observer_ptr: NonNull<Observer>,
    index: u32,
) -> crate::capability::CloseResult {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe { (*observer_ptr.as_ptr()).with_cap_table(|table| table.close(index)) }
}

/// Check whether an Observer's cap table holds any cap to a specific
/// (type, id) pair, excluding one slot index.
#[cfg(any(target_os = "none", test))]
pub fn observer_has_cap_to_object(
    observer_ptr: NonNull<Observer>,
    target_type: crate::capability::ObjectType,
    target_id: crate::arena::ObjectId,
    exclude_index: u32,
) -> bool {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        (*observer_ptr.as_ptr())
            .with_cap_table(|table| table.has_cap_to_object(target_type, target_id, exclude_index))
    }
}

/// D97: write a cap entry at a specific slot in an Observer's cap table.
/// Used by ObserverChangeHandler to overwrite SLOT_FAULT_HANDLER.
/// Returns true if the write succeeded (index in bounds).
#[cfg(any(target_os = "none", test))]
pub fn observer_write_cap_at(
    observer_ptr: NonNull<Observer>,
    index: u32,
    new_entry: crate::capability::Entry,
) -> bool {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        let observer = &*observer_ptr.as_ptr();

        crate::frame::capabilities::write_entry(
            observer.cap_table,
            observer.cap_table_capacity,
            index,
            new_entry,
        )
    }
}

/// Read the faulting Observer's ObjectId and generation (D100).
///
/// Used by dispatch_fault to construct the TransferredCap for the
/// fault message without resolving the self-cap at slot 2.
#[cfg(any(target_os = "none", test))]
pub fn observer_fault_info(observer_ptr: NonNull<Observer>) -> (crate::arena::ObjectId, u64) {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        let observer = observer_ptr.as_ref();

        (
            observer.object_id,
            observer
                .generation
                .load(core::sync::atomic::Ordering::Acquire),
        )
    }
}

/// Transition an Observer from Runnable to Faulted (D39, D100).
///
/// Returns Ok(()) on success, Err if the transition is invalid.
#[cfg(any(target_os = "none", test))]
pub fn observer_set_faulted(
    observer_ptr: NonNull<Observer>,
) -> Result<(), crate::observer::ObserverError> {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe { (*observer_ptr.as_ptr()).fault() }
}

/// Read the faulting Observer's saved PC (D100 diagnostic output).
///
/// Returns the ELR_EL1 value saved in RegisterState — the instruction
/// address at which the fault occurred.
#[cfg(any(target_os = "none", test))]
pub fn observer_read_pc(observer_ptr: NonNull<Observer>) -> u64 {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &*(observer.register_state.as_ptr().as_ptr() as *const RegisterState);

        rs.pc
    }
}

/// D98: check whether an Observer's cap table has at least one free slot.
/// Used by Destroy's upfront table-full check before marking the target dead.
#[cfg(any(target_os = "none", test))]
pub fn observer_has_free_slot(observer_ptr: NonNull<Observer>) -> bool {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe { (*observer_ptr.as_ptr()).cap_table_free_head.is_some() }
}

/// Write inline registers to a target Observer (D103).
///
/// Sets PC (ELR_EL1), SP (SP_EL0), x0 (gprs[0]), and PSTATE (SPSR_EL1)
/// in the target's RegisterState. The target must be in a stopped state
/// (Inert or Faulted). Returns false if the target is not stopped.
///
/// SECURITY: `pstate` MUST be pre-masked to NZCV only (0xF000_0000)
/// by the caller. This function does not re-mask.
#[cfg(any(target_os = "none", test))]
pub fn observer_write_registers(
    target_ptr: NonNull<Observer>,
    pc: u64,
    sp: u64,
    x0: u64,
    pstate: u64,
) -> bool {
    // SAFETY: target_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        let observer = &*target_ptr.as_ptr();

        if !observer.state.is_stopped() {
            return false;
        }

        let rs = &mut *(observer.register_state.as_ptr().as_ptr()
            as *mut crate::frame::arch::register_state::RegisterState);

        rs.pc = pc;
        rs.sp = sp;
        rs.gprs[0] = x0;
        rs.pstate = pstate;

        true
    }
}

/// Read inline registers from a target Observer (D103).
///
/// Returns (PC, SP, x0, PSTATE) from the target's RegisterState.
/// The target must be in a stopped state (Inert or Faulted).
/// Returns None if the target is not stopped.
#[cfg(any(target_os = "none", test))]
pub fn observer_read_registers(target_ptr: NonNull<Observer>) -> Option<(u64, u64, u64, u64)> {
    // SAFETY: target_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        let observer = &*target_ptr.as_ptr();

        if !observer.state.is_stopped() {
            return None;
        }

        let rs = &*(observer.register_state.as_ptr().as_ptr()
            as *const crate::frame::arch::register_state::RegisterState);

        Some((rs.pc, rs.sp, rs.gprs[0], rs.pstate))
    }
}

/// Enable direct EL0 timer counter access on an Observer (D66).
///
/// Sets `clock_access = true` so the next context restore writes
/// CNTKCTL_EL1.EL0VCTEN=1, allowing the Observer to read CNTVCT_EL0
/// directly without trapping.
#[cfg(any(target_os = "none", test))]
pub fn observer_set_clock_access(observer_ptr: NonNull<Observer>) {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        (*observer_ptr.as_ptr()).clock_access = true;
    }
}

// ── Observer restore helpers for EL0 exception exit ─────────────

/// Extract the restore parameters for an Observer (D74, D76).
///
/// Returns `(register_state_ptr, page_table_root, clock_access)` — the
/// three values the assembly `__restore_observer` entry point needs to
/// resume an Observer in EL0.
///
/// `clock_access` is encoded as `1u64` (enable CNTVCT_EL0 access) or
/// `0u64` (disable). Assembly writes CNTKCTL_EL1.EL0VCTEN accordingly.
#[cfg(any(target_os = "none", test))]
pub fn observer_restore_info(observer_ptr: NonNull<Observer>) -> (*mut RegisterState, u64, u64) {
    // SAFETY: observer_ptr was obtained from CoreState::current or from
    // DispatchResult, which points to a live Observer in the arena. The
    // Observer's register_state.as_ptr() points to a valid RegisterState
    // in structural backing. A4 non-reentrancy guarantees no aliasing
    // on a single core.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs_ptr = observer.register_state.as_ptr().as_ptr() as *mut RegisterState;
        let pt_root = observer.page_table_root;
        let clock_access = if observer.clock_access { 1u64 } else { 0u64 };

        (rs_ptr, pt_root, clock_access)
    }
}

/// Read an Observer's page table root and ASID (D88, D91).
///
/// Returns `(page_table_root, asid)` for use with map/unmap operations.
#[cfg(any(target_os = "none", test))]
pub fn observer_page_table_info(observer_ptr: NonNull<Observer>) -> (u64, u16) {
    // SAFETY: observer_ptr points to a live Observer. A4 non-reentrancy.
    unsafe {
        let observer = observer_ptr.as_ref();

        (observer.page_table_root, observer.asid)
    }
}

/// Update PerCoreData.register_state_ptr for the next EL0 exception
/// entry (D74, D83).
///
/// Called before `__restore_observer` so that the NEXT EL0 exception
/// entry saves registers into the correct Observer's save area. The
/// assembly reads `register_state_ptr` at PerCoreData offset 0.
///
/// Accepts the already-resolved `*mut RegisterState` — callers already
/// have this from `observer_restore_info`, avoiding a redundant Observer
/// dereference on every exception exit.
#[cfg(target_os = "none")]
pub fn update_register_state_ptr(rs_ptr: *mut RegisterState) {
    // SAFETY: TPIDR_EL1 points to a valid PerCoreData set during boot
    // (D83). Per-core writable state — no nomem. A4 non-reentrancy
    // guarantees no aliasing on a single core. rs_ptr was obtained from
    // observer_restore_info, which validates the Observer pointer.
    unsafe {
        let per_core_ptr = crate::frame::arch::tpidr_el1() as *mut PerCoreData;

        (*per_core_ptr).register_state_ptr = rs_ptr;
    }
}

/// Allocate a unique ASID from the kernel's AsidAllocator (D101).
///
/// The TLB flush on wrap is issued outside the lock's critical section.
/// This is safe because the returned ASID is freshly allocated — no
/// existing TTBR0 encodes it, so no stale TLB entries can exist for it.
/// The flush only needs to complete before the ASID enters TTBR0, which
/// happens after this function returns.
pub fn allocate_asid(ks: &crate::kernel_state::KernelState) -> u16 {
    let (asid, wrapped) = {
        let mut alloc = ks.asid_allocator.acquire();
        let result = alloc.allocate();

        (result.asid, result.wrapped)
    };

    #[cfg(target_os = "none")]
    if wrapped {
        crate::frame::arch::mmu::tlb_flush_all_user();
    }

    #[cfg(not(target_os = "none"))]
    let _ = wrapped;

    asid
}

/// Read the hardware counter frequency (D72, CNTFRQ_EL0).
#[cfg(any(target_os = "none", test))]
pub fn read_counter_freq() -> u64 {
    crate::frame::arch::cntfrq_el0()
}

/// Read the current hardware counter value (CNTVCT_EL0).
#[cfg(any(target_os = "none", test))]
pub fn read_counter_ticks() -> u64 {
    crate::frame::arch::cntvct_el0()
}

/// Allocate a RegisterState for a new Observer (D95, D32).
///
/// RegisterState lives in the consumed Space's structural backing (D95).
/// Test builds use the heap allocator; bare-metal builds allocate zeroed
/// pages from the SpaceManager root pool (identity-mapped PA = VA).
#[cfg(any(target_os = "none", test))]
pub fn allocate_register_state() -> Option<NonNull<u8>> {
    #[cfg(test)]
    {
        Some(alloc_test_register_state())
    }
    #[cfg(not(test))]
    {
        use crate::frame::arch::register_state::RegisterState;

        let total_bytes = core::mem::size_of::<RegisterState>();
        let page_count = total_bytes.div_ceil(crate::frame::arch::mmu::page_size());
        let ks = crate::frame::kernel_state();
        let pa = crate::frame::boot::alloc_zeroed_pages(ks, page_count).ok()?;

        NonNull::new(crate::frame::phys_to_virt(pa) as *mut u8)
    }
}

/// Allocate a test RegisterState and return a handle to it (test-only).
///
/// Returns a NonNull<u8> suitable for use as Observer::register_state.
/// The allocation is leaked — acceptable for test code only.
#[cfg(test)]
pub fn alloc_test_register_state() -> NonNull<u8> {
    use crate::frame::arch::register_state::RegisterState;

    let layout = alloc::alloc::Layout::new::<RegisterState>();
    // SAFETY: layout is non-zero-sized (RegisterState is 816 bytes).
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };

    assert!(!ptr.is_null(), "test register state allocation failed");

    // SAFETY: ptr is non-null (asserted above).
    unsafe { NonNull::new_unchecked(ptr) }
}

/// Write IPC register values into a test RegisterState (test-only).
///
/// Sets x0–x7 in the RegisterState pointed to by the handle. Used to
/// set up Observer saved state for dispatch_ipc tests.
#[cfg(test)]
pub fn write_test_ipc_registers(register_state: NonNull<u8>, regs: &IpcRegisters) {
    use crate::frame::arch::register_state::RegisterState;

    // SAFETY: register_state was allocated by alloc_test_register_state
    // and points to a valid, zero-initialized RegisterState.
    unsafe {
        let rs = &mut *(register_state.as_ptr() as *mut RegisterState);

        rs.gprs[0] = regs.data[0];
        rs.gprs[1] = regs.data[1];
        rs.gprs[2] = regs.data[2];
        rs.gprs[3] = regs.data[3];
        rs.gprs[4] = regs.label;
        rs.gprs[5] = regs.handle_or_badge;
        rs.gprs[6] = regs.user_cap;
        rs.gprs[7] = regs.reply_info;
    }
}

/// Write IPC register values through an Observer pointer (test-only).
///
/// Same as `write_test_ipc_registers` but takes `NonNull<Observer>` and
/// resolves the register state internally. Avoids callers needing unsafe
/// to extract the register state handle from an Observer.
#[cfg(test)]
pub fn write_test_ipc_registers_via_observer(observer_ptr: NonNull<Observer>, regs: &IpcRegisters) {
    // SAFETY: observer_ptr points to a valid Observer allocated on the test stack.
    let rs = unsafe { observer_ptr.as_ref().register_state.as_ptr() };

    write_test_ipc_registers(rs, regs);
}

/// Read the IPC error state from an Observer's saved registers (test-only).
///
/// Returns `(carry_set, x0)` — the carry flag indicates IPC error (D49),
/// x0 carries the error code when carry is set.
#[cfg(test)]
pub fn read_ipc_carry_and_x0(observer_ptr: NonNull<Observer>) -> (bool, u64) {
    // SAFETY: observer_ptr points to a valid Observer. Same invariant as
    // read_ipc_registers.
    unsafe {
        let observer = observer_ptr.as_ref();
        let rs = &*(observer.register_state.as_ptr().as_ptr()
            as *const crate::frame::arch::register_state::RegisterState);
        let carry_set = (rs.pstate & (1u64 << 29)) != 0;

        (carry_set, rs.gprs[0])
    }
}

/// Write typed operation registers through an Observer pointer (test-only).
///
/// Same as `write_test_typed_registers` but takes `NonNull<Observer>` and
/// resolves the register state internally. Avoids callers needing unsafe
/// to extract the register state handle from an Observer.
#[cfg(test)]
pub fn write_test_typed_registers_via_observer(
    observer_ptr: NonNull<Observer>,
    regs: &TypedRegisters,
) {
    // SAFETY: observer_ptr points to a valid Observer allocated on the test stack.
    let rs = unsafe { observer_ptr.as_ref().register_state.as_ptr() };
    write_test_typed_registers(rs, regs);
}

/// Write typed operation register values into a test RegisterState (test-only).
#[cfg(test)]
pub fn write_test_typed_registers(register_state: NonNull<u8>, regs: &TypedRegisters) {
    use crate::frame::arch::register_state::RegisterState;

    // SAFETY: same as write_test_ipc_registers.
    unsafe {
        let rs = &mut *(register_state.as_ptr() as *mut RegisterState);

        rs.gprs[0] = regs.args[0];
        rs.gprs[1] = regs.args[1];
        rs.gprs[2] = regs.args[2];
        rs.gprs[3] = regs.args[3];
        rs.gprs[4] = regs.op_code as u64;
        rs.gprs[5] = regs.target_handle;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::observer::{Observer, RegisterStateHandle};

    #[test]
    fn test_ipc_register_roundtrip() {
        let rs_ptr = alloc_test_register_state();
        let written = IpcRegisters {
            data: [0x1111, 0x2222, 0x3333, 0x4444],
            label: 0xABCD,
            handle_or_badge: 0x5555,
            user_cap: u64::MAX,
            reply_info: 0x7777,
        };

        write_test_ipc_registers(rs_ptr, &written);

        let mut observer = {
            let mut o = crate::observer::Observer::test_default();

            o.register_state = RegisterStateHandle::new(rs_ptr);
            o.compute_aggregate = 0;

            o
        };
        let obs_ptr = NonNull::from(&mut observer);
        let read = read_ipc_registers(obs_ptr);

        assert_eq!(read.data, written.data, "data words must roundtrip");
        assert_eq!(read.label, written.label, "label must roundtrip");
        assert_eq!(
            read.handle_or_badge, written.handle_or_badge,
            "handle must roundtrip"
        );
        assert_eq!(read.user_cap, written.user_cap, "user_cap must roundtrip");
        assert_eq!(
            read.reply_info, written.reply_info,
            "reply_info must roundtrip"
        );
    }

    #[test]
    fn test_typed_register_roundtrip() {
        let rs_ptr = alloc_test_register_state();
        let written = TypedRegisters {
            op_code: 7,
            target_handle: 0xBEEF,
            args: [0xA, 0xB, 0xC, 0xD],
        };

        write_test_typed_registers(rs_ptr, &written);

        let mut observer = {
            let mut o = crate::observer::Observer::test_default();

            o.register_state = RegisterStateHandle::new(rs_ptr);
            o.compute_aggregate = 0;

            o
        };
        let obs_ptr = NonNull::from(&mut observer);
        let read = read_typed_registers(obs_ptr);

        assert_eq!(read.op_code, written.op_code, "op_code must roundtrip");
        assert_eq!(
            read.target_handle, written.target_handle,
            "target_handle must roundtrip"
        );
        assert_eq!(read.args, written.args, "args must roundtrip");
    }

    fn make_test_observer(rs_ptr: NonNull<u8>) -> Observer {
        let mut o = crate::observer::Observer::test_default();

        o.register_state = RegisterStateHandle::new(rs_ptr);
        o.compute_aggregate = 0;

        o
    }

    // ── D76 write helper tests ──────────────────────────────────────

    #[test]
    fn test_d76_write_ipc_error_sets_carry_and_x0() {
        let rs_ptr = alloc_test_register_state();
        let mut observer = make_test_observer(rs_ptr);
        let obs_ptr = NonNull::from(&mut observer);

        write_ipc_error(obs_ptr, SyscallError::InvalidCap);

        let rs = unsafe {
            &*(rs_ptr.as_ptr() as *const crate::frame::arch::register_state::RegisterState)
        };

        assert_ne!(
            rs.pstate & SPSR_CARRY_BIT,
            0,
            "D49: carry must be set for IPC error"
        );
        assert_eq!(
            rs.gprs[0],
            SyscallError::InvalidCap as u64,
            "D49: x0 must contain error code"
        );
    }

    #[test]
    fn test_d76_clear_ipc_carry() {
        let rs_ptr = alloc_test_register_state();
        let mut observer = make_test_observer(rs_ptr);
        let obs_ptr = NonNull::from(&mut observer);

        write_ipc_error(obs_ptr, SyscallError::QueueFull);

        let rs = unsafe {
            &*(rs_ptr.as_ptr() as *const crate::frame::arch::register_state::RegisterState)
        };

        assert_ne!(rs.pstate & SPSR_CARRY_BIT, 0, "precondition: carry set");

        clear_ipc_carry(obs_ptr);

        let rs = unsafe {
            &*(rs_ptr.as_ptr() as *const crate::frame::arch::register_state::RegisterState)
        };

        assert_eq!(
            rs.pstate & SPSR_CARRY_BIT,
            0,
            "D49: carry must be cleared for IPC success"
        );
    }

    #[test]
    fn test_d76_write_typed_result() {
        let rs_ptr = alloc_test_register_state();
        let mut observer = make_test_observer(rs_ptr);
        let obs_ptr = NonNull::from(&mut observer);

        write_typed_result(obs_ptr, 42);

        let rs = unsafe {
            &*(rs_ptr.as_ptr() as *const crate::frame::arch::register_state::RegisterState)
        };

        assert_eq!(rs.gprs[0], 42, "D49: x0 must contain the return value");
    }

    #[test]
    fn test_d76_write_typed_result_negative_is_error() {
        let rs_ptr = alloc_test_register_state();
        let mut observer = make_test_observer(rs_ptr);
        let obs_ptr = NonNull::from(&mut observer);
        let error_value = (-1i64) as u64;

        write_typed_result(obs_ptr, error_value);

        let rs = unsafe {
            &*(rs_ptr.as_ptr() as *const crate::frame::arch::register_state::RegisterState)
        };

        assert_eq!(
            rs.gprs[0] as i64, -1,
            "D49: negative x0 signals error for typed ops"
        );
    }

    #[test]
    fn test_d76_write_message_to_registers_all_fields() {
        let rs_ptr = alloc_test_register_state();
        let mut observer = make_test_observer(rs_ptr);
        let obs_ptr = NonNull::from(&mut observer);
        let data = [0x1111, 0x2222, 0x3333, 0x4444];

        write_message_to_registers(obs_ptr, &data, 0xABCD, 0x5555, 7, u64::MAX);

        let rs = unsafe {
            &*(rs_ptr.as_ptr() as *const crate::frame::arch::register_state::RegisterState)
        };

        assert_eq!(rs.gprs[0], 0x1111, "x0 = data[0]");
        assert_eq!(rs.gprs[1], 0x2222, "x1 = data[1]");
        assert_eq!(rs.gprs[2], 0x3333, "x2 = data[2]");
        assert_eq!(rs.gprs[3], 0x4444, "x3 = data[3]");
        assert_eq!(rs.gprs[4], 0xABCD, "x4 = label");
        assert_eq!(rs.gprs[5], 0x5555, "x5 = badge");
        assert_eq!(rs.gprs[6], 7, "x6 = user cap slot");
        assert_eq!(rs.gprs[7], u64::MAX, "x7 = no reply cap (sentinel)");
    }

    #[test]
    fn test_d76_write_metadata_only_x4_through_x7() {
        let rs_ptr = alloc_test_register_state();
        let mut observer = make_test_observer(rs_ptr);
        let obs_ptr = NonNull::from(&mut observer);
        let sentinel_data: [u64; 4] = [0xDEAD; 4];

        write_message_to_registers(obs_ptr, &sentinel_data, 0, 0, 0, 0);
        write_metadata_to_registers(obs_ptr, 0x1ABE, 0xBAD6E, 3, 5);

        let rs = unsafe {
            &*(rs_ptr.as_ptr() as *const crate::frame::arch::register_state::RegisterState)
        };

        assert_eq!(rs.gprs[0], 0xDEAD, "x0 must be untouched by metadata write");
        assert_eq!(rs.gprs[1], 0xDEAD, "x1 must be untouched by metadata write");
        assert_eq!(rs.gprs[2], 0xDEAD, "x2 must be untouched by metadata write");
        assert_eq!(rs.gprs[3], 0xDEAD, "x3 must be untouched by metadata write");
        assert_eq!(rs.gprs[4], 0x1ABE, "x4 = label");
        assert_eq!(rs.gprs[5], 0xBAD6E, "x5 = badge");
        assert_eq!(rs.gprs[6], 3, "x6 = user cap slot");
        assert_eq!(rs.gprs[7], 5, "x7 = reply cap slot");
    }

    #[test]
    fn test_d76_carry_flag_preserves_other_pstate_bits() {
        let rs_ptr = alloc_test_register_state();

        unsafe {
            let rs =
                &mut *(rs_ptr.as_ptr() as *mut crate::frame::arch::register_state::RegisterState);

            rs.pstate = 0x9000_0000;
        }

        let mut observer = make_test_observer(rs_ptr);
        let obs_ptr = NonNull::from(&mut observer);

        write_ipc_error(obs_ptr, SyscallError::NoRight);

        let rs = unsafe {
            &*(rs_ptr.as_ptr() as *const crate::frame::arch::register_state::RegisterState)
        };

        assert_ne!(rs.pstate & SPSR_CARRY_BIT, 0, "carry must be set");
        assert_eq!(
            rs.pstate & !SPSR_CARRY_BIT,
            0x9000_0000,
            "other NZCV bits must be preserved"
        );

        clear_ipc_carry(obs_ptr);

        let rs = unsafe {
            &*(rs_ptr.as_ptr() as *const crate::frame::arch::register_state::RegisterState)
        };

        assert_eq!(rs.pstate & SPSR_CARRY_BIT, 0, "carry must be cleared");
        assert_eq!(
            rs.pstate, 0x9000_0000,
            "original pstate bits must be restored"
        );
    }

    // ── D83 PerCoreData layout tests ───────────────────────────────

    #[test]
    fn test_d83_per_core_data_size() {
        assert_eq!(
            core::mem::size_of::<PerCoreData>(),
            24,
            "D83: PerCoreData must be exactly 24 bytes (three pointers)"
        );
    }

    #[test]
    fn test_d83_per_core_data_alignment() {
        assert_eq!(
            core::mem::align_of::<PerCoreData>(),
            8,
            "D83: PerCoreData must be 8-byte aligned (pointer alignment)"
        );
    }

    #[test]
    fn test_d83_register_state_ptr_at_offset_zero() {
        // The register_state_ptr field must be at offset 0 so assembly
        // can load it with a simple ldr from the TPIDR_EL1 value.
        assert_eq!(
            PER_CORE_DATA_REGISTER_STATE_OFFSET, 0,
            "D83: register_state_ptr must be at offset 0 for assembly access"
        );

        // Verify the actual struct layout matches.
        let base = core::ptr::null::<PerCoreData>();
        let offset = unsafe { core::ptr::addr_of!((*base).register_state_ptr) as usize };

        assert_eq!(offset, 0, "D83: register_state_ptr actual offset must be 0");
    }

    #[test]
    fn test_d83_core_state_ptr_at_offset_eight() {
        assert_eq!(
            PER_CORE_DATA_CORE_STATE_OFFSET, 8,
            "D83: core_state_ptr must be at offset 8"
        );

        let base = core::ptr::null::<PerCoreData>();
        let offset = unsafe { core::ptr::addr_of!((*base).core_state_ptr) as usize };

        assert_eq!(offset, 8, "D83: core_state_ptr actual offset must be 8");
    }

    #[test]
    fn test_d83_per_core_data_field_access_roundtrip() {
        // Verify that writing and reading through PerCoreData fields works
        // correctly — the repr(C) layout must not reorder or pad fields.
        let rs_ptr = alloc_test_register_state();
        let mut per_core = PerCoreData {
            register_state_ptr: rs_ptr.as_ptr()
                as *mut crate::frame::arch::register_state::RegisterState,
            core_state_ptr: core::ptr::null_mut(),
            kernel_stack_top: core::ptr::null_mut(),
        };

        // Write a sentinel via the register_state_ptr.
        unsafe {
            (*per_core.register_state_ptr).gprs[0] = 0xDEAD_BEEF;
        }

        // Read it back through the raw pointer.
        let read_back = unsafe { (*per_core.register_state_ptr).gprs[0] };

        assert_eq!(
            read_back, 0xDEAD_BEEF,
            "D83: register_state_ptr must provide valid access to RegisterState"
        );

        // Set and verify core_state_ptr.
        let sentinel: u64 = 0x1234_5678;

        per_core.core_state_ptr = &sentinel as *const u64 as *mut u8;

        let recovered = unsafe { *(per_core.core_state_ptr as *const u64) };

        assert_eq!(
            recovered, 0x1234_5678,
            "D83: core_state_ptr must provide valid type-erased access"
        );
    }

    #[test]
    fn test_d83_per_core_data_raw_byte_access() {
        // Verify that raw byte access at known offsets yields the correct
        // field values — this is what assembly will do.
        let rs_ptr = alloc_test_register_state();
        let sentinel_core_state: u64 = 0xCAFE_BABE;
        let per_core = PerCoreData {
            register_state_ptr: rs_ptr.as_ptr()
                as *mut crate::frame::arch::register_state::RegisterState,
            core_state_ptr: &sentinel_core_state as *const u64 as *mut u8,
            kernel_stack_top: core::ptr::null_mut(),
        };
        let base = &per_core as *const PerCoreData as *const u8;
        // Read register_state_ptr at offset 0 as a raw u64.
        let rs_from_offset = unsafe { *(base.add(0) as *const u64) };

        assert_eq!(
            rs_from_offset,
            rs_ptr.as_ptr() as u64,
            "D83: raw byte offset 0 must yield register_state_ptr value"
        );

        // Read core_state_ptr at offset 8 as a raw u64.
        let cs_from_offset = unsafe { *(base.add(8) as *const u64) };

        assert_eq!(
            cs_from_offset, &sentinel_core_state as *const u64 as u64,
            "D83: raw byte offset 8 must yield core_state_ptr value"
        );

        // Read kernel_stack_top at offset 16 as a raw u64.
        let kst_from_offset = unsafe { *(base.add(16) as *const u64) };

        assert_eq!(
            kst_from_offset, 0,
            "D83: raw byte offset 16 must yield kernel_stack_top value (null)"
        );
    }

    #[test]
    fn test_d83_kernel_stack_top_at_offset_sixteen() {
        assert_eq!(
            PER_CORE_DATA_KERNEL_STACK_TOP_OFFSET, 16,
            "D83: kernel_stack_top must be at offset 16"
        );
        assert_eq!(
            core::mem::offset_of!(PerCoreData, kernel_stack_top),
            16,
            "D83: kernel_stack_top actual offset must be 16"
        );
    }

    #[test]
    fn test_d83_kernel_stack_top_roundtrip() {
        // Verify kernel_stack_top can be written and read back correctly
        // through both field access and raw byte access.
        let rs_ptr = alloc_test_register_state();
        let stack_sentinel: u64 = 0xFFFF_0000_DEAD_CAFE;
        let mut per_core = PerCoreData {
            register_state_ptr: rs_ptr.as_ptr()
                as *mut crate::frame::arch::register_state::RegisterState,
            core_state_ptr: core::ptr::null_mut(),
            kernel_stack_top: stack_sentinel as *mut u8,
        };

        // Field access.
        assert_eq!(
            per_core.kernel_stack_top as u64, stack_sentinel,
            "D83: kernel_stack_top field access must roundtrip"
        );

        // Raw byte access at offset 16 (what assembly does).
        let base = &per_core as *const PerCoreData as *const u8;
        let raw = unsafe { *(base.add(16) as *const u64) };

        assert_eq!(
            raw, stack_sentinel,
            "D83: kernel_stack_top raw byte offset 16 must match"
        );
    }
}
