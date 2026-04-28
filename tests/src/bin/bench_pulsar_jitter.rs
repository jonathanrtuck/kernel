//! Pulsar timer jitter measurement.
//!
//! Creates a Pulsar at 1ms period, measures actual inter-delivery time
//! over many cycles. Reports jitter as the spread of period errors.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::alloc_field;
use userspace_rs::*;

const ITERATIONS: usize = 100;
const PERIOD_NS: u64 = 1_000_000;
const TAG: u64 = 0x400;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let timer_field = alloc_field(64);
    let pulsar_space = space_split(ROOT_SPACE_HANDLE, 4096);

    if !pulsar_space.is_ok() {
        fail();
    }

    let r = create_pulsar(pulsar_space.value(), timer_field, 1, PERIOD_NS, PERIOD_NS);

    if !r.is_ok() {
        fail();
    }

    // Drain first delivery (initial delay)
    receive(timer_field);

    let mut stats = Stats::new();
    let mut prev = cycles();

    for _ in 0..ITERATIONS {
        receive(timer_field);

        let now = cycles();

        stats.record(now - prev);

        prev = now;
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
