//! Integration test for the multi-Observer harness (Layer 5).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::bench_emit;
use userspace_rs::harness::{alloc_field, setup_reply_field, spawn_echo_server};
use userspace_rs::*;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);

    bench_emit(0x10, handler_field, ipc_field, 0);

    // Test: child first, then reply field — should pass
    let child1 = spawn_echo_server(handler_field, ipc_field);

    bench_emit(0x11, child1, 0, 0);

    setup_reply_field();

    bench_emit(0x12, 0, 0, 0);

    let reply = call(ipc_field, 1, [0x11, 0x22, 0x33, 0x44], CAP_ABSENT, 0);

    bench_emit(0x13, reply.data[0], 0, 0);

    assert_eq_or_fail!(reply.data[0], 0x11);

    // Test: second child + second call — reuses same reply field
    let ipc_field2 = alloc_field(8);
    let child2 = spawn_echo_server(handler_field, ipc_field2);

    bench_emit(0x14, child2, 0, 0);

    let reply2 = call(ipc_field2, 2, [0xAA, 0xBB, 0xCC, 0xDD], CAP_ABSENT, 0);

    bench_emit(0x15, reply2.data[0], 0, 0);

    assert_eq_or_fail!(reply2.data[0], 0xAA);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
