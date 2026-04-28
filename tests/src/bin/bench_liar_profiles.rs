//! The liar — profile mismatch.
//!
//! Four Observers all doing identical compute (200 iterations each),
//! but with wildly different declared profiles. Under round-robin,
//! profiles are ignored and all finish at the same time. Under a
//! profile-aware scheduler, the "liar" (claims interactive, actually
//! compute-bound) might get unfair advantage.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const N: usize = 4;
const COMPUTE_ITERS: u64 = 200;
const TAG_BASE: u64 = 0x740;
const TAG_RATIO: u64 = 0x748;

// Compute worker: x0 = (iterations << 32) | field_handle
global_asm!(
    ".global _liar_worker",
    "_liar_worker:",
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

fn liar_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _liar_worker();
    }

    _liar_worker as *const () as u64
}

// Profiles: all LIES (all do identical compute)
const PROFILES: [(u64, u64); N] = [
    (120, 0), // claims interactive — actually compute-bound
    (0, 120), // claims batch — actually same compute
    (60, 60), // balanced
    (0, 0),   // default
];

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let report_field = alloc_field(8);
    let entry = liar_worker_entry();

    for i in 0..N {
        let child = create_child(handler_field);
        let child_rf = share_field(child.handle, report_field);
        let arg = (COMPUTE_ITERS << 32) | child_rf;
        let (r, t) = PROFILES[i];

        observer_set_scheduling(child.handle, r, t);
        start_child(&child, entry, arg);
    }

    let start = cycles();
    let mut times = [0u64; N];

    for time in times.iter_mut() {
        receive(report_field);

        *time = cycles() - start;
    }

    // Report per-observer completion times
    for (i, &t) in times.iter().enumerate() {
        bench_emit(TAG_BASE + i as u64, t, COMPUTE_ITERS, 0);
    }

    // Fairness ratio: max/min (1.0 = profiles had no effect)
    let mut min_t = u64::MAX;
    let mut max_t = 0;

    for &t in &times {
        if t < min_t {
            min_t = t;
        }
        if t > max_t {
            max_t = t;
        }
    }

    if min_t > 0 {
        bench_emit(TAG_RATIO, max_t * 100 / min_t, max_t, min_t);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
