//! Userspace test support library.
//!
//! Syscall wrappers and test protocol for EL0 test binaries running
//! on the kernel under the hypervisor. Each binary is a standalone
//! test — it signals PASS via `brk #0x42` or FAIL via `brk #1`.
//!
//! ABI reference: kernel src/syscall.rs (D47, D48, D49).

#![no_std]

use core::arch::asm;

// ── Test protocol ───────────────────────────────────────────────

#[inline(never)]
pub fn pass() -> ! {
    // SAFETY: BRK #0x42 is the kernel's test-pass signal (D102).
    // The kernel prints "TEST PASSED" and exits via PSCI SYSTEM_OFF.
    unsafe {
        asm!("brk #0x42", options(noreturn));
    }
}

#[inline(never)]
pub fn fail() -> ! {
    // SAFETY: BRK #1 triggers a debug exception the kernel treats as
    // a fault — it prints "FATAL FAULT" and exits via PSCI SYSTEM_OFF.
    unsafe {
        asm!("brk #1", options(noreturn));
    }
}

#[macro_export]
macro_rules! assert_eq_or_fail {
    ($left:expr, $right:expr) => {
        if $left != $right {
            $crate::fail();
        }
    };
}

#[macro_export]
macro_rules! assert_or_fail {
    ($cond:expr) => {
        if !$cond {
            $crate::fail();
        }
    };
}

// ── IPC syscalls (SVC #1–#5) ────────────────────────────────────

pub fn yield_cpu() {
    // SAFETY: SVC #5 = Yield (D48). No register inputs needed beyond
    // the SVC immediate. The kernel saves and restores all GPRs.
    unsafe {
        asm!(
            "svc #5",
            // Yield may clobber x0-x7 (syscall registers) per ABI,
            // though Yield specifically preserves them. Be defensive.
            out("x0") _,
            out("x1") _,
            out("x2") _,
            out("x3") _,
            out("x4") _,
            out("x5") _,
            out("x6") _,
            out("x7") _,
        );
    }
}

/// IPC message as seen at the syscall boundary.
pub struct Message {
    pub data: [u64; 4],
    pub label: u64,
    pub badge: u64,
    pub user_cap: u64,
    pub reply_cap: u64,
}

/// No-cap sentinel (D49): indicates no capability in this slot.
pub const CAP_ABSENT: u64 = u64::MAX;

/// Send a message to a Field (SVC #1).
///
/// Returns true on success (carry clear), false on error (carry set).
pub fn send(handle: u64, label: u64, data: [u64; 4]) -> bool {
    let result: u64;
    // SAFETY: SVC #1 = Send (D48). Register layout per D47:
    //   x0-x3 = data words, x4 = label, x5 = handle,
    //   x6 = user cap (CAP_ABSENT), x7 = reply info (0).
    // On return: carry clear = success, carry set = error (x0 = code).
    // We read NZCV via mrs to check carry.
    unsafe {
        asm!(
            "svc #1",
            "mrs {result}, NZCV",
            result = out(reg) result,
            in("x0") data[0],
            in("x1") data[1],
            in("x2") data[2],
            in("x3") data[3],
            in("x4") label,
            in("x5") handle,
            in("x6") CAP_ABSENT,
            in("x7") 0u64,
        );
    }
    // Carry is bit 29 of NZCV. Clear = success.
    (result & (1 << 29)) == 0
}

/// Receive a message from a Field (SVC #2).
///
/// Blocks until a message is available. Returns the received message.
pub fn receive(handle: u64) -> Message {
    let d0: u64;
    let d1: u64;
    let d2: u64;
    let d3: u64;
    let label: u64;
    let badge: u64;
    let user_cap: u64;
    let reply_cap: u64;
    // SAFETY: SVC #2 = Receive (D48). x5 = handle on entry.
    // On return: x0-x3 = data, x4 = label, x5 = badge,
    // x6 = user cap slot, x7 = reply cap handle.
    unsafe {
        asm!(
            "svc #2",
            in("x5") handle,
            out("x0") d0,
            out("x1") d1,
            out("x2") d2,
            out("x3") d3,
            lateout("x4") label,
            lateout("x5") badge,
            out("x6") user_cap,
            out("x7") reply_cap,
        );
    }
    Message {
        data: [d0, d1, d2, d3],
        label,
        badge,
        user_cap,
        reply_cap,
    }
}

/// Call: send a message and block until reply (SVC #3).
///
/// Sends data to the Field at `handle` and blocks until the server replies.
/// This is the client-side fast path for ping-pong IPC.
pub fn call(
    handle: u64,
    label: u64,
    data: [u64; 4],
    user_cap: u64,
    reply_badge: u64,
) -> Message {
    let d0: u64;
    let d1: u64;
    let d2: u64;
    let d3: u64;
    let rlabel: u64;
    let badge: u64;
    let ruser_cap: u64;
    let reply_cap: u64;
    // SAFETY: SVC #3 = Call (D48). Sends message then blocks on reply.
    // Register layout per D47:
    //   Entry: x0-x3 = data, x4 = label, x5 = handle, x6 = user cap, x7 = reply badge.
    //   Return: x0-x3 = reply data, x4 = reply label, x5 = badge, x6 = user cap, x7 = reply cap.
    // All eight registers are both inputs and outputs. Using `lateout` is correct
    // because the SVC traps into the kernel before any register is modified.
    unsafe {
        asm!(
            "svc #3",
            in("x0") data[0],
            in("x1") data[1],
            in("x2") data[2],
            in("x3") data[3],
            in("x4") label,
            in("x5") handle,
            in("x6") user_cap,
            in("x7") reply_badge,
            lateout("x0") d0,
            lateout("x1") d1,
            lateout("x2") d2,
            lateout("x3") d3,
            lateout("x4") rlabel,
            lateout("x5") badge,
            lateout("x6") ruser_cap,
            lateout("x7") reply_cap,
        );
    }
    Message {
        data: [d0, d1, d2, d3],
        label: rlabel,
        badge,
        user_cap: ruser_cap,
        reply_cap,
    }
}

/// ReplyRecv: reply to previous caller and receive next message (SVC #4).
///
/// Sends a reply on `reply_handle` (a send-once cap from a previous Receive)
/// and simultaneously waits for the next message on `recv_handle`. This is
/// the server-side fast path — the server never blocks between handling one
/// request and waiting for the next.
pub fn reply_receive(
    reply_handle: u64,
    recv_handle: u64,
    label: u64,
    data: [u64; 4],
    user_cap: u64,
) -> Message {
    let d0: u64;
    let d1: u64;
    let d2: u64;
    let d3: u64;
    let rlabel: u64;
    let badge: u64;
    let ruser_cap: u64;
    let reply_cap: u64;
    // SAFETY: SVC #4 = ReplyRecv (D48). Replies then blocks on receive.
    // Register layout per D47:
    //   Entry: x0-x3 = reply data, x4 = reply label, x5 = reply handle (send-once),
    //          x6 = user cap, x7 = receive handle.
    //   Return: x0-x3 = new message data, x4 = label, x5 = badge, x6 = user cap,
    //           x7 = reply cap.
    // Note the asymmetry: x5 entry = reply handle, x7 entry = receive handle.
    // On return: x5 = badge (new message), x7 = reply cap (new message).
    unsafe {
        asm!(
            "svc #4",
            in("x0") data[0],
            in("x1") data[1],
            in("x2") data[2],
            in("x3") data[3],
            in("x4") label,
            in("x5") reply_handle,
            in("x6") user_cap,
            in("x7") recv_handle,
            lateout("x0") d0,
            lateout("x1") d1,
            lateout("x2") d2,
            lateout("x3") d3,
            lateout("x4") rlabel,
            lateout("x5") badge,
            lateout("x6") ruser_cap,
            lateout("x7") reply_cap,
        );
    }
    Message {
        data: [d0, d1, d2, d3],
        label: rlabel,
        badge,
        user_cap: ruser_cap,
        reply_cap,
    }
}

// ── Typed syscalls (SVC #0) ─────────────────────────────────────

/// Result of a typed syscall. Negative = error, non-negative = success.
pub struct TypedResult(pub i64);

impl TypedResult {
    pub fn is_ok(&self) -> bool {
        self.0 >= 0
    }

    pub fn value(&self) -> u64 {
        self.0 as u64
    }
}

/// Execute a typed kernel operation (SVC #0).
///
/// x4 = op_code, x5 = target handle, x0-x3 = operation-specific args.
/// Returns x0 (negative = error, non-negative = success/result).
pub fn typed_syscall(op_code: u16, target: u64, args: [u64; 4]) -> TypedResult {
    let result: u64;
    // SAFETY: SVC #0 = typed operation (D48). Register layout per D47.
    unsafe {
        asm!(
            "svc #0",
            in("x0") args[0],
            in("x1") args[1],
            in("x2") args[2],
            in("x3") args[3],
            in("x4") op_code as u64,
            in("x5") target,
            lateout("x0") result,
            lateout("x1") _,
            lateout("x2") _,
            lateout("x3") _,
            lateout("x6") _,
            lateout("x7") _,
        );
    }
    TypedResult(result as i64)
}

// ── Typed operation helpers ─────────────────────────────────────

pub const OP_SPACE_SPLIT: u16 = 11;
pub const OP_CREATE_FIELD: u16 = 13;
pub const OP_CLOSE: u16 = 9;
pub const OP_CLOCK_READ: u16 = 17;
pub const OP_DESTROY: u16 = 7;
pub const OP_RESOURCE_REQUEST: u16 = 19;

/// Root Space cap lives at slot 3 after boot.
pub const ROOT_SPACE_HANDLE: u64 = 3;

/// Split `size` bytes from the Space at `handle`.
/// Returns the new Space's handle on success.
pub fn space_split(handle: u64, size: u64) -> TypedResult {
    typed_syscall(OP_SPACE_SPLIT, handle, [size, 0, 0, 0])
}

/// Create a Field from the Space at `handle` (consumes the Space).
/// `capacity` is the queue depth hint. Returns 0 on success.
pub fn create_field(handle: u64, capacity: u64) -> TypedResult {
    typed_syscall(OP_CREATE_FIELD, handle, [capacity, 0, 0, 0])
}

/// Close (release) the capability at `handle`.
pub fn close(handle: u64) -> TypedResult {
    typed_syscall(OP_CLOSE, handle, [0, 0, 0, 0])
}

/// Read the system clock (requires clock_access = true).
pub fn clock_read(self_handle: u64) -> TypedResult {
    typed_syscall(OP_CLOCK_READ, self_handle, [0, 0, 0, 0])
}
