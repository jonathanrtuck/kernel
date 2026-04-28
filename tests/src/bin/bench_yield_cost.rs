//! Voluntary preemption (Yield) overhead.
//!
//! Batched timing: each measurement runs BATCH yields in one timed
//! window, divides by BATCH. This amortizes counter-read overhead,
//! enabling sub-tick precision for this very fast syscall.
//!
//! Inlined (not using benchmark_batched harness) to avoid an extra
//! call frame — Stats is ~8 KiB and the stack page is only 16 KiB.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::*;

const WARMUP: u32 = 100;
const MEASURE: u32 = 1_000;
const BATCH: u32 = 100;
const TAG: u64 = 0x200;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    for _ in 0..WARMUP * BATCH {
        yield_cpu();
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        for _ in 0..BATCH {
            yield_cpu();
        }

        stats.record(sw.elapsed() / BATCH as u64);
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
