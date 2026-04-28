//! SMP integration test: parallel independent work on two cores.
//!
//! Spawns a child Observer (migrated to core 1) that sends N messages
//! to a shared Field, while the parent on core 0 independently sends
//! N messages to the same Field. Then the parent drains all 2N
//! messages and verifies every one arrived with correct data.
//!
//! Proves: per-core scheduler independence, concurrent Field access
//! (enqueue from different cores), no data corruption under SMP load.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const MESSAGES_PER_SIDE: u64 = 20;
const PARENT_TAG: u64 = 0xAAAA;
const CHILD_TAG: u64 = 0xBBBB;

global_asm!(
    ".global _smp_child_sender",
    "_smp_child_sender:",
    // x0 = shared Field handle
    "mov x19, x0",
    "mov x20, #0",  // counter
    "mov x21, #20", // MESSAGES_PER_SIDE
    "1:",
    "cmp x20, x21",
    "b.ge 2f",
    // Send: x0 = counter, x1 = CHILD_TAG, x2-x3 = 0
    "mov x0, x20",
    "mov x1, #0xBBBB",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",  // label = 0
    "mov x5, x19", // handle
    "movn x6, #0", // CAP_ABSENT
    "mov x7, #0",
    "svc #1",
    "add x20, x20, #1",
    "b 1b",
    "2:",
    // Done sending — yield forever so parent can drain.
    "svc #5",
    "b 2b",
);

fn child_sender_entry() -> u64 {
    unsafe extern "C" {
        fn _smp_child_sender();
    }

    _smp_child_sender as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    // Queue capacity must hold all messages (both sides send before drain).
    let shared_field = alloc_field(64);
    let child = create_child(handler_field);
    let child_field_handle = share_field(child.handle, shared_field);

    start_child(&child, child_sender_entry(), child_field_handle);

    // Parent sends its own batch.
    for i in 0..MESSAGES_PER_SIDE {
        let ok = send(shared_field, 0, [i, PARENT_TAG, 0, 0]);

        assert_or_fail!(ok);
    }

    // Yield a few times to let the child finish.
    for _ in 0..10 {
        yield_cpu();
    }

    // Drain and count messages from each side.
    let mut parent_count: u64 = 0;
    let mut child_count: u64 = 0;

    for _ in 0..(2 * MESSAGES_PER_SIDE) {
        let msg = receive(shared_field);

        if msg.data[1] == PARENT_TAG {
            parent_count += 1;
        } else if msg.data[1] == CHILD_TAG {
            child_count += 1;
        } else {
            fail();
        }
    }

    assert_eq_or_fail!(parent_count, MESSAGES_PER_SIDE);
    assert_eq_or_fail!(child_count, MESSAGES_PER_SIDE);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
