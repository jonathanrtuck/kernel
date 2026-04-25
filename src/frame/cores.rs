//! Core manager unsafe operations — per-core state access and register reads.
//!
//! The safe `core_manager.rs` module delegates hardware-dependent operations
//! here: reading TPIDR_EL1 for per-core state, reading Observer saved register
//! contexts for syscall dispatch, and test helpers for constructing Observer
//! contexts in unit tests.
//!
//! D1:  per-core state access via TPIDR_EL1.
//! D47: IPC register layout (x0–x7) in saved register context.
//! D74: EL0 exception entry saves directly to RegisterState (not TrapFrame).

#[cfg(test)]
extern crate alloc;

#[cfg(target_os = "none")]
use crate::core_manager::CoreState;
#[cfg(any(target_os = "none", test))]
use crate::observer::Observer;
#[cfg(any(target_os = "none", test))]
use crate::syscall::{IpcRegisters, TypedRegisters};
#[cfg(target_os = "none")]
use crate::time_manager::Scheduler;
#[cfg(any(target_os = "none", test))]
use core::ptr::NonNull;

/// Read the current core's state from TPIDR_EL1.
///
/// Each core stores a pointer to its `CoreState<S>` in TPIDR_EL1 at boot.
/// This function reads that register and returns a shared reference.
///
/// D1: core-local, no cross-core sharing. The returned reference is valid
/// for the duration of the exception handler (A4: non-reentrant).
///
/// # Safety (structural invariant)
///
/// TPIDR_EL1 must contain a valid pointer to a `CoreState<S>` set during
/// boot. Each core writes its own value once via `set_tpidr_el1` during
/// initialization; the value is stable afterward but the register is
/// NOT immutable (it is per-core writable state — do NOT use
/// `sysreg_read_const!`/`nomem`).
#[cfg(target_os = "none")]
pub fn read_core_state<S: Scheduler>() -> &'static CoreState<S> {
    // SAFETY: TPIDR_EL1 was initialized at boot to point to a valid
    // CoreState<S> for this core. Per-core writable state — uses
    // sysreg_read! (no nomem) so LLVM cannot reorder memory accesses
    // past this read. A4 non-reentrancy guarantees no aliasing on a
    // single core.
    unsafe {
        let ptr = crate::frame::arch::tpidr_el1() as *const CoreState<S>;

        &*ptr
    }
}

/// Mutable access to the current core's state from TPIDR_EL1.
///
/// Same as `read_core_state` but returns `&'static mut`. Safe because A4
/// guarantees the kernel is non-reentrant on a single core — only one
/// exception handler runs at a time, so there can be no aliasing.
#[cfg(target_os = "none")]
pub fn read_core_state_mut<S: Scheduler>() -> &'static mut CoreState<S> {
    // SAFETY: Same invariant as read_core_state (per-core writable
    // state, no nomem). Mutable access is safe because A4 guarantees
    // non-reentrancy — the caller is the only exception handler
    // running on this core.
    unsafe {
        let ptr = crate::frame::arch::tpidr_el1() as *mut CoreState<S>;

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
    use crate::observer::{
        DEFAULT_RESPONSIVENESS, DEFAULT_THROUGHPUT, Observer, PrimaryState, RegisterStateHandle,
        WaitState,
    };
    use core::sync::atomic::AtomicU64;

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

        let mut observer = Observer {
            register_state: RegisterStateHandle::new(rs_ptr),
            page_table_root: 0,
            cap_table: NonNull::dangling(),
            state: PrimaryState::Runnable,
            suspended: false,
            compute_aggregate: 0,
            responsiveness: DEFAULT_RESPONSIVENESS,
            throughput: DEFAULT_THROUGHPUT,
            clock_access: false,
            wait_state: WaitState::None,
            refcount: 1,
            generation: AtomicU64::new(0),
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

        let mut observer = Observer {
            register_state: RegisterStateHandle::new(rs_ptr),
            page_table_root: 0,
            cap_table: NonNull::dangling(),
            state: PrimaryState::Runnable,
            suspended: false,
            compute_aggregate: 0,
            responsiveness: DEFAULT_RESPONSIVENESS,
            throughput: DEFAULT_THROUGHPUT,
            clock_access: false,
            wait_state: WaitState::None,
            refcount: 1,
            generation: AtomicU64::new(0),
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
}
