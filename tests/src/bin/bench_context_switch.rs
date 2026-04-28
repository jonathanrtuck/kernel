//! Context switch cost with two Observers.
//!
//! Root and child alternate via yield. Each measured yield from root's
//! perspective spans: root save → scheduler picks child → child restore
//! → child yields → child save → scheduler picks root → root restore.
//! That's 2 full context switches with TTBR0 change each time.
//!
//! Compare to bench_yield_cost (1 Observer, no actual context switch —
//! same TTBR0, scheduler picks self). The delta isolates the cost of
//! switching address spaces and register sets.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 200;
const MEASURE: u32 = 5_000;
const TAG: u64 = 0x610;

global_asm!(
    ".global _yield_loop",
    "_yield_loop:",
    "svc #5",
    "b _yield_loop",
);

fn yield_loop_entry() -> u64 {
    unsafe extern "C" {
        fn _yield_loop();
    }

    _yield_loop as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let child = create_child(handler_field);

    start_child(&child, yield_loop_entry(), 0);

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
