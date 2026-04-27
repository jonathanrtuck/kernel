//! Multi-Observer harness for benchmark and stress test binaries.
//!
//! Provides child Observer creation, shared Field installation, and
//! barrier synchronization. All timing is done by the root Observer
//! (children lack clock_access per D66).

use crate::{
    ROOT_SPACE_HANDLE, clone_cap, close, create_field, create_observer, fail, install_reply_field,
    observer_install_cap, observer_resume, observer_write_registers, receive, send, space_info,
    space_split,
};
use core::arch::global_asm;

// ── Constants ──────────────────────────────────────────────────

/// Structural backing Space for CreateObserver: RegisterState (816 B)
/// + L1 table (16384 B) + cap table entries. 4 pages = 65536 B gives
/// ~48 KiB for cap table (~1500 entries at 32 B/entry).
const OBSERVER_SPACE_SIZE: u64 = 65536;

/// Stack Space per child Observer (1 page).
const STACK_SIZE: u64 = 16384;

/// Space consumed by CreateField.
const FIELD_SPACE_SIZE: u64 = 16384;

// ── Field allocation ───────────────────────────────────────────

/// Allocate a Field from the root Space. Returns the Field handle.
///
/// Splits `FIELD_SPACE_SIZE` from root Space, converts to a Field
/// with the given queue capacity. Calls `fail()` on any error.
pub fn alloc_field(capacity: u64) -> u64 {
    let space = space_split(ROOT_SPACE_HANDLE, FIELD_SPACE_SIZE);

    if !space.is_ok() {
        fail();
    }

    let field_handle = space.value();
    let result = create_field(field_handle, capacity);

    if !result.is_ok() {
        fail();
    }

    field_handle
}

/// Allocate a reply Field and install it at SLOT_REPLY_FIELD (slot 1).
///
/// Must be called once before the caller uses Call (SVC #3). The reply
/// Field is how the kernel routes replies back to the caller.
pub fn setup_reply_field() {
    let reply_field = alloc_field(4);

    if !install_reply_field(reply_field) {
        fail();
    }
}

// ── Child Observer creation ────────────────────────────────────

/// Result of creating a child Observer.
pub struct Child {
    pub handle: u64,
    pub stack_top: u64,
}

/// Create a child Observer with its own stack, in Inert state.
///
/// Steps:
/// 1. SpaceSplit for structural backing → CreateObserver
/// 2. SpaceSplit for stack → ObserverInstallCap (D26 auto-maps)
/// 3. Query stack VA via BRK #0x49
///
/// Returns the Observer handle and stack_top. Registers are NOT set —
/// call `start_child` after installing any additional caps.
pub fn create_child(handler_field: u64) -> Child {
    let struct_space = space_split(ROOT_SPACE_HANDLE, OBSERVER_SPACE_SIZE);

    if !struct_space.is_ok() {
        fail();
    }

    let observer_handle = struct_space.value();
    let result = create_observer(observer_handle, handler_field, 0);

    if !result.is_ok() {
        fail();
    }

    let stack_space = space_split(ROOT_SPACE_HANDLE, STACK_SIZE);

    if !stack_space.is_ok() {
        fail();
    }

    let stack_handle = stack_space.value();
    let (stack_va, stack_size) = space_info(stack_handle);

    if stack_va == u64::MAX {
        fail();
    }

    let install = observer_install_cap(observer_handle, stack_handle);

    if !install.is_ok() {
        fail();
    }

    let _ = close(stack_handle);

    Child {
        handle: observer_handle,
        stack_top: stack_va + stack_size,
    }
}

/// Set registers and resume a child Observer.
///
/// PC = `entry`, SP = `stack_top` (from `create_child`),
/// x0 = `arg` (typically the child's IPC Field handle).
pub fn start_child(child: &Child, entry: u64, arg: u64) {
    let wr = observer_write_registers(child.handle, entry, child.stack_top, arg, 0);

    if !wr.is_ok() {
        fail();
    }

    let r = observer_resume(child.handle);

    if !r.is_ok() {
        fail();
    }
}

/// Install a Field cap from the parent's table into a child Observer.
///
/// Clones the Field cap (InstallCap consumes the source from the
/// parent's table), installs the clone. Returns the encoded handle
/// in the *child's* cap table.
pub fn share_field(child: u64, field: u64) -> u64 {
    let cloned = clone_cap(field);

    if !cloned.is_ok() {
        fail();
    }

    let cloned_handle = cloned.value();
    let installed = observer_install_cap(child, cloned_handle);

    if !installed.is_ok() {
        fail();
    }

    installed.value()
}

/// Create child, install shared Field, set x0 to child's Field handle,
/// and resume. Returns the Observer handle.
pub fn spawn_echo_server(handler_field: u64, ipc_field: u64) -> u64 {
    let child = create_child(handler_field);
    let child_field = share_field(child.handle, ipc_field);

    start_child(&child, echo_server_entry(), child_field);

    child.handle
}

// ── Barrier synchronization ────────────────────────────────────

/// N-way barrier using a shared Field.
///
/// Each participant Sends a "ready" message, then Receives N-1
/// messages. When all have sent and received, the barrier is released.
///
/// The Field must have queue capacity >= N (all participants Send
/// before any Receives, so up to N messages may be in-flight).
pub struct Barrier {
    field: u64,
    count: u64,
}

impl Barrier {
    pub fn new(field: u64, participant_count: u64) -> Self {
        Self {
            field,
            count: participant_count,
        }
    }

    /// Block until all participants have reached the barrier.
    pub fn wait(&self) {
        send(self.field, 0, [0; 4]);

        for _ in 1..self.count {
            receive(self.field);
        }
    }
}

// ── Echo server (child entry point) ────────────────────────────

// Stackless receive → reply_receive loop in assembly.
//
// x0 = Field handle (from ObserverWriteRegisters).
// x19 = callee-saved, preserved by kernel across context switches.
//
// Protocol:
//   1. Receive first message (SVC #2)
//   2. ReplyRecv loop: reply with received data, receive next (SVC #4)
//
// Register flow per iteration:
//   After SVC #2/#4 return: x0-x3 = data, x4 = label, x7 = reply_cap
//   For SVC #4 entry: x0-x3 = reply data (echo), x5 = reply_cap, x7 = field
global_asm!(
    ".global _echo_server",
    "_echo_server:",
    "mov x19, x0",
    "mov x5, x19",
    "svc #2",
    "1:",
    "mov x5, x7",
    "movn x6, #0",
    "mov x7, x19",
    "svc #4",
    "b 1b",
);

/// Address of the stackless echo server entry point.
pub fn echo_server_entry() -> u64 {
    unsafe extern "C" {
        fn _echo_server();
    }

    _echo_server as *const () as u64
}
