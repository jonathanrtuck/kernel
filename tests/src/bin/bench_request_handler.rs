//! Request dispatcher: 8 competing handlers serve root's requests.
//!
//! Simulates a web server with a shared request Field. 8 echo servers
//! (4 fast R=100,T=20, 4 slow R=20,T=100) compete to handle incoming
//! Calls. Root measures per-request round-trip latency — the "how fast
//! does my request get serviced" test.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 50;
const MEASURE: u32 = 500;
const TAG: u64 = 0x770;
const FAST_HANDLERS: usize = 4;
const SLOW_HANDLERS: usize = 4;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    resource_request(ROOT_SPACE_HANDLE, 0, 1_048_576);

    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(64);

    // Fast handlers: interactive profile (cache hits)
    for _ in 0..FAST_HANDLERS {
        let server = spawn_echo_server(handler_field, ipc_field);

        observer_set_scheduling(server, 100, 20);
    }
    // Slow handlers: batch profile (cache misses)
    for _ in 0..SLOW_HANDLERS {
        let server = spawn_echo_server(handler_field, ipc_field);

        observer_set_scheduling(server, 20, 100);
    }

    setup_reply_field();

    // Warmup
    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    // Measure per-request latency
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
