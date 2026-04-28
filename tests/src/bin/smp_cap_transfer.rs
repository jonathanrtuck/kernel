//! SMP control-plane test: IPC capability transfer via user_cap slot.
//!
//! Parent creates a Field, clones it, and sends the clone to a child
//! Observer via the IPC user_cap slot (D96 move semantics). The child
//! receives the transferred cap and sends data through it back to the
//! parent, proving the cap is valid and usable in the child's table.
//!
//! Exercises: D96 move semantics (cap extracted from sender, installed
//! in receiver), cap table mutation on both Observers under scheduling
//! contention, cross-Observer Field access via IPC-transferred cap.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const TRANSFER_MARKER: u64 = 0xCAFE;
const DONE_MARKER: u64 = 0xBEEF;

// Child entry point:
//   x0 = ipc_field handle (in child's cap table)
//
// Protocol:
//   1. Receive on ipc_field — gets user_cap (transferred Field handle)
//   2. Send TRANSFER_MARKER on the transferred cap
//   3. Send DONE_MARKER on ipc_field
global_asm!(
    ".global _smp_cap_transfer_child",
    "_smp_cap_transfer_child:",
    "mov x19, x0",
    // Step 1: Receive on ipc_field — gets transferred cap in x6.
    "mov x5, x19",
    "svc #2",
    "mov x20, x6",
    // Step 2: Send TRANSFER_MARKER on transferred cap (x20).
    "mov x0, #0xCAFE",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    // Step 3: Send DONE on ipc_field.
    "mov x0, #0xBEEF",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "2:",
    "svc #5",
    "b 2b",
);

fn child_entry() -> u64 {
    unsafe extern "C" {
        fn _smp_cap_transfer_child();
    }

    _smp_cap_transfer_child as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);
    let test_field = alloc_field(8);
    let child = create_child(handler_field);
    let child_ipc = share_field(child.handle, ipc_field);

    start_child(&child, child_entry(), child_ipc);

    // Let the child start and block on Receive(ipc_field).
    for _ in 0..10 {
        yield_cpu();
    }

    // Clone test_field — send_with_cap will move the clone to the child
    // via D96 IPC cap transfer. Parent keeps the original for receiving.
    let cloned = clone_cap(test_field);

    assert_or_fail!(cloned.is_ok());

    let clone_handle = cloned.value();
    // Send message with cap transfer: child gets clone_handle as user_cap.
    let ok = send_with_cap(ipc_field, 0, [0; 4], clone_handle);

    assert_or_fail!(ok);

    // Receive on test_field — child sent TRANSFER_MARKER through the
    // IPC-transferred cap.
    let msg = receive(test_field);

    assert_eq_or_fail!(msg.data[0], TRANSFER_MARKER);

    // Receive DONE on ipc_field — child confirms it completed.
    let done = receive(ipc_field);

    assert_eq_or_fail!(done.data[0], DONE_MARKER);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
