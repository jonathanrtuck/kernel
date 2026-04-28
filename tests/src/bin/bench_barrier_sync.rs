//! Barrier synchronization.
//!
//! 4 compute workers with different iteration counts (100, 150, 200, 250)
//! creating natural skew. Root starts all simultaneously, collects
//! per-worker completion times, and reports the barrier round time
//! (= slowest worker) and skew ratio (max/min). Measures how much the
//! slowest worker penalizes the group — the scheduling effect on barrier
//! performance.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const N: usize = 4;
const TAG_BASE: u64 = 0x830;
const TAG_ROUND: u64 = 0x834;
const TAG_SKEW: u64 = 0x835;

const ITERS: [u64; N] = [100, 150, 200, 250];

// Compute worker: x0 = (iterations << 32) | field_handle
// Does iterations * 10000 inner-loop ops, sends count on field, yields forever.
global_asm!(
    ".global _barrier_compute",
    "_barrier_compute:",
    "mov w19, w0",      // field_handle (low 32)
    "lsr x20, x0, #32", // iterations (high 32)
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
    // Send completion with iteration count
    "mov x0, x21",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    // Yield forever
    "4:",
    "svc #5",
    "b 4b",
);

fn barrier_compute_entry() -> u64 {
    unsafe extern "C" {
        fn _barrier_compute();
    }

    _barrier_compute as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let report_field = alloc_field(8);
    let entry = barrier_compute_entry();
    // Start all workers simultaneously
    let start = cycles();

    for i in 0..N {
        let child = create_child(handler_field);
        let child_rf = share_field(child.handle, report_field);
        let arg = (ITERS[i] << 32) | child_rf;

        start_child(&child, entry, arg);
    }

    // Collect per-worker completion times in arrival order
    let mut times = [0u64; N];

    for time in times.iter_mut() {
        receive(report_field);

        *time = cycles() - start;
    }

    // Report per-worker completion times
    for (i, &t) in times.iter().enumerate() {
        bench_emit(TAG_BASE + i as u64, t, ITERS[i], 0);
    }

    // Barrier round time = last worker
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

    bench_emit(TAG_ROUND, max_t, N as u64, 0);

    // Skew ratio: max/min * 100 (100 = perfect, higher = worse)
    if min_t > 0 {
        bench_emit(TAG_SKEW, max_t * 100 / min_t, max_t, min_t);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
