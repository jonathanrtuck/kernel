//! Cross-core IPC latency.
//!
//! Ping-pong between two Observers with scheduling hints for different
//! cores. Compare against same-core bench_pingpong to quantify the
//! cross-core tax (D56 mailbox + IPI path).
//!
//! NOTE: currently measures same-core fast-path because:
//! (1) DirectSwitch (D50) has no same-core check — fires regardless
//!     of which core the receiver is on (communication.rs)
//! (2) ObserverResume always enqueues to the local core's scheduler —
//!     the Placement trait (D56) is defined but not wired into dispatch
//! (3) observer_set_scheduling only updates profile, doesn't trigger
//!     placement
//! Until placement is wired in and DirectSwitch gains a same-core
//! guard, this benchmark reports the same latency as bench_pingpong.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 100;
const MEASURE: u32 = 5000;
const TAG: u64 = 0x440;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);
    let child = spawn_echo_server(handler_field, ipc_field);

    // Hint the echo server toward high-throughput (different placement score)
    observer_set_scheduling(child, 0, 100);
    setup_reply_field();

    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

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
