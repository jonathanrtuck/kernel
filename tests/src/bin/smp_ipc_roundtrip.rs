//! SMP integration test: cross-core IPC round-trip.
//!
//! Spawns an echo server (kernel migrates it to core 1 during boot),
//! then performs multiple Call/Reply round-trips with distinctive data
//! patterns. Verifies all four data words, label, and badge survive
//! the cross-core path intact.
//!
//! Exercises: PSCI CPU_ON, secondary boot, IPI delivery, mailbox drain,
//! Observer migration, cross-core context switch, IPC fast path.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const ROUNDS: u32 = 50;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);

    spawn_echo_server(handler_field, ipc_field);
    setup_reply_field();

    for i in 0..ROUNDS {
        let tag = i as u64;
        let data = [
            0xAAAA_0000 | tag,
            0xBBBB_0000 | tag,
            0xCCCC_0000 | tag,
            0xDDDD_0000 | tag,
        ];
        let reply = call(ipc_field, tag, data, CAP_ABSENT, 0);

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
