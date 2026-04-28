//! Voluntary preemption (Yield) overhead.
//!
//! Tight yield loop measuring the cost of a no-op reschedule when the
//! yielding Observer is the only runnable one.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::*;

const WARMUP: u32 = 100;
const MEASURE: u32 = 10_000;
const TAG: u64 = 0x200;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    for _ in 0..WARMUP {
        yield_cpu();
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        yield_cpu();

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
