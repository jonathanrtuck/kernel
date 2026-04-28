//! SMP integration test: high-volume cross-core IPC stress.
//!
//! Spawns an echo server (migrated to core 1), then hammers it with
//! hundreds of Call/Reply round-trips using varying payloads. Catches
//! data corruption, register clobbering, or IPI delivery failures
//! that only manifest under sustained cross-core load.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const ITERATIONS: u32 = 500;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);

    spawn_echo_server(handler_field, ipc_field);
    setup_reply_field();

    for i in 0..ITERATIONS {
        let v = i as u64;
        let data = [v, v ^ 0xFFFF_FFFF, v.wrapping_mul(0x9E37_79B9), !v];
        let reply = call(ipc_field, v, data, CAP_ABSENT, 0);

        assert_eq_or_fail!(reply.data[0], data[0]);
        assert_eq_or_fail!(reply.data[1], data[1]);
        assert_eq_or_fail!(reply.data[2], data[2]);
        assert_eq_or_fail!(reply.data[3], data[3]);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
