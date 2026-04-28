//! Embarrassingly parallel — throughput ceiling.
//!
//! N identical compute Observers, each doing ITERS iterations of
//! compute. No communication, no yielding. Measures total elapsed time.
//! Compares N=1 (same total work, single Observer) against N=4 to
//! isolate scheduling overhead in the best-case parallel scenario.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const ITERS: u64 = 400;
const TAG_SINGLE: u64 = 0x720;
const TAG_PARALLEL: u64 = 0x727;

// Compute worker: x0 = (iterations << 32) | field_handle
global_asm!(
    ".global _parallel_worker",
    "_parallel_worker:",
    "mov w19, w0",
    "lsr x20, x0, #32",
    "mov x21, #0",
    "1:",
    "cmp x21, x20",
    "b.ge 2f",
    "mov x22, #10000",
    "3:",
    "subs x22, x22, #1",
    "b.ne 3b",
    "add x21, x21, #1",
    "b 1b",
    "2:",
    "mov x0, x21",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "4:",
    "svc #5",
    "b 4b",
);

fn parallel_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _parallel_worker();
    }

    _parallel_worker as *const () as u64
}

fn run_workers(handler_field: u64, n: usize, iters_each: u64) -> u64 {
    let report_field = alloc_field(8);
    let entry = parallel_worker_entry();
    let start = cycles();

    for _ in 0..n {
        let child = create_child(handler_field);
        let child_rf = share_field(child.handle, report_field);
        let arg = (iters_each << 32) | child_rf;

        start_child(&child, entry, arg);
    }

    for _ in 0..n {
        receive(report_field);
    }

    cycles() - start
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    // N=1: single Observer doing all 4*ITERS work
    let elapsed_single = run_workers(handler_field, 1, ITERS * 4);

    bench_emit(TAG_SINGLE, elapsed_single, ITERS * 4, 1);

    // N=4: four Observers doing ITERS each (same total work)
    let elapsed_parallel = run_workers(handler_field, 4, ITERS);

    bench_emit(TAG_PARALLEL, elapsed_parallel, ITERS, 4);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
