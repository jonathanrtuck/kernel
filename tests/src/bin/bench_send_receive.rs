//! Raw Send + Receive latency on a single Observer.
//!
//! Self-send: root enqueues then dequeues on the same Field. Isolates
//! queue enqueue/dequeue cost from cross-Observer context-switch cost.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::alloc_field;
use userspace_rs::*;

const WARMUP: u32 = 100;
const MEASURE: u32 = 10_000;
const TAG: u64 = 0x120;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let field = alloc_field(16);

    for _ in 0..WARMUP {
        send(field, 0, [0; 4]);
        receive(field);
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        send(field, 0, [0; 4]);
        receive(field);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
