//! SMP control-plane test: Pulsar timer delivery to waiting Observer.
//!
//! Creates a Pulsar that fires periodically into a Field. A child
//! Observer blocks on Receive for that Field. When the timer fires,
//! the kernel delivers the message to the Field, unblocking the child.
//! Verifies that Pulsar → Field → waiter delivery works correctly
//! under preemptive scheduling with multiple Observers.
//!
//! Exercises: Pulsar fire in handle_timer, message delivery to Field
//! with blocked waiter, Observer unblock from timer context,
//! concurrent timer + IPC paths.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const FIRE_COUNT: u64 = 10;
const PERIOD_NS: u64 = 500_000;
const DONE_MARKER: u64 = 0xBEEF;
const TIMER_BADGE: u64 = 0x42;

// Child entry point:
//   x0 = packed: timer_field[31:0] | sync_field[63:32]
//
// Receives FIRE_COUNT messages from timer_field (Pulsar deliveries),
// then sends the count on sync_field.
global_asm!(
    ".global _smp_pulsar_child",
    "_smp_pulsar_child:",
    "and x19, x0, #0xFFFFFFFF",
    "lsr x20, x0, #32",
    // Receive FIRE_COUNT timer messages.
    "mov x21, #10",
    "mov x22, #0",
    "1:",
    "cbz x21, 2f",
    "mov x5, x19",
    "svc #2",
    "add x22, x22, #1",
    "sub x21, x21, #1",
    "b 1b",
    // Send count + DONE on sync_field.
    "2:",
    "mov x0, x22",
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
        fn _smp_pulsar_child();
    }

    _smp_pulsar_child as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let timer_field = alloc_field(64);
    let sync_field = alloc_field(4);
    let child = create_child(handler_field);
    let child_timer = share_field(child.handle, timer_field);
    let child_sync = share_field(child.handle, sync_field);
    let packed = child_timer | (child_sync << 32);

    start_child(&child, child_entry(), packed);

    // Create a Pulsar targeting timer_field. The Pulsar fires repeatedly
    // at PERIOD_NS intervals. The child receives each fire message.
    let pulsar_space = space_split(ROOT_SPACE_HANDLE, 4096);

    assert_or_fail!(pulsar_space.is_ok());

    let r = create_pulsar(
        pulsar_space.value(),
        timer_field,
        TIMER_BADGE,
        PERIOD_NS,
        PERIOD_NS,
    );

    assert_or_fail!(r.is_ok());

    // Wait for child to receive all timer fires and report.
    let report = receive(sync_field);

    assert_eq_or_fail!(report.data[0], FIRE_COUNT);
    assert_eq_or_fail!(report.data[1], DONE_MARKER);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
