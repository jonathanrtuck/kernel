//! Trivial benchmark for pipeline validation.
//!
//! Emits a single BENCH data point and exits. Validates the full pipeline:
//! build -> hypervisor -> BENCH line -> parse -> display.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::bench_emit;
use userspace_rs::pass;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    bench_emit(0x1, 42, 0, 0);
    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    userspace_rs::fail();
}
