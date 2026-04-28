//! SMP control-plane test: destroy object while another Observer uses it.
//!
//! Parent creates a Field, shares it with a child. The child enters a
//! tight Send loop on the shared Field. The parent then destroys the
//! Field. The child's next Send should fail cleanly (carry set) because
//! the Field's generation was revoked and the arena slot freed.
//!
//! Exercises: Destroy typed operation under scheduling contention,
//! cap revocation propagation, Field arena free while active caps exist,
//! graceful error detection on stale caps.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const DONE_MARKER: u64 = 0xBEEF;

// Child entry point:
//   x0 = packed: target_field[31:0] | sync_field[63:32]
//
// Tight Send loop on target_field. Checks carry after each Send.
// When carry is set (Field destroyed), sends the iteration count on
// sync_field and halts.
global_asm!(
    ".global _smp_destroy_child",
    "_smp_destroy_child:",
    "and x19, x0, #0xFFFFFFFF",
    "lsr x20, x0, #32",
    "mov x21, #0",
    // Send loop — up to 10000 iterations.
    "mov x23, #10000",
    "1:",
    "cbz x23, 2f",
    // Send iteration count on target_field.
    "mov x0, x21",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    // Check carry — set means error (field destroyed or queue full).
    "mrs x22, NZCV",
    "tbnz x22, #29, 2f",
    "add x21, x21, #1",
    "sub x23, x23, #1",
    "b 1b",
    // Done — send iteration count on sync_field.
    "2:",
    "mov x0, x21",
    "mov x1, #0xBEEF",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "3:",
    "svc #5",
    "b 3b",
);

fn child_entry() -> u64 {
    unsafe extern "C" {
        fn _smp_destroy_child();
    }

    _smp_destroy_child as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    // Large capacity so the child can send many messages before queue full.
    let target_field = alloc_field(128);
    let sync_field = alloc_field(4);
    let child = create_child(handler_field);
    let child_target = share_field(child.handle, target_field);
    let child_sync = share_field(child.handle, sync_field);
    let packed = child_target | (child_sync << 32);

    start_child(&child, child_entry(), packed);

    // Let child start its send loop.
    for _ in 0..5 {
        yield_cpu();
    }

    // Destroy the target Field while the child is sending to it.
    let d = destroy(target_field);

    assert_or_fail!(d.is_ok());

    // Wait for child to detect the error and report.
    let report = receive(sync_field);

    assert_eq_or_fail!(report.data[1], DONE_MARKER);
    // The child should have completed at least one send before destroy.
    assert_or_fail!(report.data[0] > 0);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
