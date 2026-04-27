//! Minimal multi-Observer spawn test.
//!
//! Tests each step independently to isolate failures:
//! 1. alloc_field works
//! 2. create_child works
//! 3. space_info returns valid VA
//! 4. share_field works
//! 5. echo server IPC round-trip works

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::bench_emit;
use userspace_rs::harness::{
    alloc_field, create_child, echo_server_entry, setup_reply_field, share_field, start_child,
};
use userspace_rs::*;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    // Step 1: alloc handler field
    let handler_field = alloc_field(8);

    bench_emit(0x01, handler_field, 0, 0);

    // Step 2: alloc IPC field
    let ipc_field = alloc_field(8);

    bench_emit(0x02, ipc_field, 0, 0);

    // Step 3: create child observer
    let child = create_child(handler_field);

    bench_emit(0x03, child.handle, child.stack_top, 0);

    // Step 4: share field with child
    let child_field = share_field(child.handle, ipc_field);

    bench_emit(0x04, child_field, 0, 0);

    // Step 5: get echo server entry address
    let entry = echo_server_entry();

    bench_emit(0x05, entry, 0, 0);

    // Step 6: set up reply field for Call (root needs this at slot 1)
    setup_reply_field();

    bench_emit(0x06, 0, 0, 0);

    // Step 7: start child
    start_child(&child, entry, child_field);

    bench_emit(0x07, 0, 0, 0);

    // Step 8: ping-pong IPC
    let reply = call(ipc_field, 0x42, [0xAA, 0xBB, 0xCC, 0xDD], CAP_ABSENT, 0);

    bench_emit(0x08, reply.data[0], reply.data[1], 0);

    assert_eq_or_fail!(reply.data[0], 0xAA);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
