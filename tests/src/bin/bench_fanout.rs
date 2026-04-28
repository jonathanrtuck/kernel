//! Fanout IPC latency: one client, N echo servers.
//!
//! Root calls N echo servers sequentially per iteration. Measures
//! aggregate latency for N=2, N=4, N=8 to show fanout scaling.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 20;
const MEASURE: u32 = 500;
const TAG_N2: u64 = 0x130;
const TAG_N4: u64 = 0x135;
const TAG_N8: u64 = 0x13A;
const MAX_N: usize = 8;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let mut fields = [0u64; MAX_N];

    for field in fields.iter_mut() {
        let f = alloc_field(8);

        spawn_echo_server(handler_field, f);

        *field = f;
    }

    setup_reply_field();

    // N=2: call 2 servers per iteration
    {
        for _ in 0..WARMUP {
            for i in 0..2 {
                call(fields[i], 0, [0; 4], CAP_ABSENT, 0);
            }
        }

        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            for i in 0..2 {
                call(fields[i], 0, [0; 4], CAP_ABSENT, 0);
            }

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_N2);
    }
    // N=4
    {
        for _ in 0..WARMUP {
            for i in 0..4 {
                call(fields[i], 0, [0; 4], CAP_ABSENT, 0);
            }
        }

        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            for i in 0..4 {
                call(fields[i], 0, [0; 4], CAP_ABSENT, 0);
            }

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_N4);
    }
    // N=8 (reduced iterations — 8x calls per iteration)
    {
        for _ in 0..10 {
            for i in 0..8 {
                call(fields[i], 0, [0; 4], CAP_ABSENT, 0);
            }
        }

        let mut stats = Stats::new();

        for _ in 0..200 {
            let sw = Stopwatch::start();

            for i in 0..8 {
                call(fields[i], 0, [0; 4], CAP_ABSENT, 0);
            }

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_N8);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
