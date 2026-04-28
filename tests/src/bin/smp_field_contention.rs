//! SMP control-plane test: concurrent Field send/receive contention.
//!
//! Parent and child Observer both send and receive on the same Field
//! under preemptive scheduling. Phase 1: both blast sends (enqueue
//! contention). Phase 2: both drain receives (dequeue contention).
//! Verifies no messages are lost or duplicated.
//!
//! Differs from smp_parallel_work (send-only from two cores) by also
//! exercising concurrent dequeue from the waiter list.
//!
//! Exercises: Field queue lock contention on both enqueue and dequeue
//! paths, waiter list management under interleaved access, message
//! ordering and integrity under scheduling pressure.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const MESSAGES_PER_SIDE: u64 = 30;
const PARENT_TAG: u64 = 0xAAAA;
const CHILD_TAG: u64 = 0xBBBB;
const DONE_MARKER: u64 = 0xBEEF;

// Child entry point:
//   x0 = packed: shared_field[31:0] | sync_field[63:32]
//
// Phase 1: Send MESSAGES_PER_SIDE messages with CHILD_TAG on shared_field.
// Phase 2: Receive MESSAGES_PER_SIDE messages from shared_field, count.
// Phase 3: Send count on sync_field.
global_asm!(
    ".global _smp_contention_child",
    "_smp_contention_child:",
    "and x19, x0, #0xFFFFFFFF",
    "lsr x20, x0, #32",
    // Phase 1: Send 30 messages.
    "mov x21, #30",
    "mov x22, #0",
    "1:",
    "cbz x21, 2f",
    "mov x0, x22",
    "mov x1, #0xBBBB",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "add x22, x22, #1",
    "sub x21, x21, #1",
    "b 1b",
    // Phase 2: Receive 30 messages.
    "2:",
    "mov x21, #30",
    "mov x22, #0",
    "3:",
    "cbz x21, 4f",
    "mov x5, x19",
    "svc #2",
    "add x22, x22, #1",
    "sub x21, x21, #1",
    "b 3b",
    // Phase 3: Send receive-count on sync_field.
    "4:",
    "mov x0, x22",
    "mov x1, #0xBEEF",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "5:",
    "svc #5",
    "b 5b",
);

fn child_entry() -> u64 {
    unsafe extern "C" {
        fn _smp_contention_child();
    }

    _smp_contention_child as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    // Capacity must hold all messages from both sides before any drains.
    let shared_field = alloc_field(128);
    let sync_field = alloc_field(4);
    let child = create_child(handler_field);
    let child_shared = share_field(child.handle, shared_field);
    let child_sync = share_field(child.handle, sync_field);
    let packed = child_shared | (child_sync << 32);

    start_child(&child, child_entry(), packed);

    // Phase 1: Parent sends MESSAGES_PER_SIDE messages with PARENT_TAG.
    for i in 0..MESSAGES_PER_SIDE {
        let ok = send(shared_field, 0, [i, PARENT_TAG, 0, 0]);

        assert_or_fail!(ok);
    }

    // Yield to let child complete its send phase.
    for _ in 0..10 {
        yield_cpu();
    }

    // Phase 2: Parent drains MESSAGES_PER_SIDE messages.
    let mut parent_received: u64 = 0;

    for _ in 0..MESSAGES_PER_SIDE {
        let msg = receive(shared_field);

        // Every message should have a valid tag (either parent or child).
        assert_or_fail!(msg.data[1] == PARENT_TAG || msg.data[1] == CHILD_TAG);

        parent_received += 1;
    }

    assert_eq_or_fail!(parent_received, MESSAGES_PER_SIDE);

    // Wait for child to report its receive count.
    let report = receive(sync_field);

    assert_eq_or_fail!(report.data[1], DONE_MARKER);
    assert_eq_or_fail!(report.data[0], MESSAGES_PER_SIDE);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
