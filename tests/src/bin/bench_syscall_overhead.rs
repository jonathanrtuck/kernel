//! Typed syscall entry/exit overhead.
//!
//! Tight ClockRead loop measuring the full SVC round-trip: exception
//! vector → register save → syscall decode → cap resolution →
//! typed operation → register restore → eret. ClockRead is the
//! lightest typed op (one MRS instruction in the kernel).
//!
//! Subtract bench_yield_cost to isolate the typed dispatch overhead
//! from the bare SVC entry/exit cost.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::*;

const SELF_HANDLE: u64 = 2;
const WARMUP: u32 = 100;
const MEASURE: u32 = 10_000;
const TAG: u64 = 0x600;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    for _ in 0..WARMUP {
        clock_read(SELF_HANDLE);
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        clock_read(SELF_HANDLE);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
