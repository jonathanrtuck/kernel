//! Slow-path IPC round-trip latency.
//!
//! Same topology as bench_pingpong but attaches a user_cap on each Call
//! to force the slow path. Measures the cost difference vs fast-path.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 20;
const MEASURE: u32 = 200;
const TAG: u64 = 0x110;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);

    spawn_echo_server(handler_field, ipc_field);
    setup_reply_field();

    // Pre-clone caps for slow-path calls (each Call consumes the user_cap)
    const TOTAL: usize = (WARMUP + MEASURE) as usize;
    let mut caps = [0u64; TOTAL];

    for cap in caps.iter_mut() {
        let r = clone_cap(ROOT_SPACE_HANDLE);

        if !r.is_ok() {
            fail();
        }

        *cap = r.value();
    }

    let mut idx = 0usize;

    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], caps[idx], 0);
        idx += 1;
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        call(ipc_field, 0, [0; 4], caps[idx], 0);

        idx += 1;

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
