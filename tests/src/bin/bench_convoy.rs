//! Convoy effect.
//!
//! One "long" Observer runs a large compute loop (800 outer iterations),
//! four "short" Observers run small compute loops (50 each). Under
//! round-robin, short jobs wait behind the long job — the convoy effect.
//! Root timestamps each completion to measure stragglers.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const LONG_ITERS: u64 = 800;
const SHORT_ITERS: u64 = 50;
const N_SHORT: usize = 4;
const N_TOTAL: usize = 5;
const TAG_BASE: u64 = 0x710;
const TAG_RATIO: u64 = 0x718;

// Compute worker: outer loops of 10000 inner iterations each, then
// sends counter on field. x0 = (iterations << 32) | field_handle
global_asm!(
    ".global _convoy_worker",
    "_convoy_worker:",
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

fn convoy_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _convoy_worker();
    }

    _convoy_worker as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let report_field = alloc_field(8);
    let entry = convoy_worker_entry();
    // One long worker
    let long_child = create_child(handler_field);
    let long_rf = share_field(long_child.handle, report_field);
    let long_arg = (LONG_ITERS << 32) | long_rf;

    start_child(&long_child, entry, long_arg);

    // Four short workers
    for _ in 0..N_SHORT {
        let child = create_child(handler_field);
        let child_rf = share_field(child.handle, report_field);
        let arg = (SHORT_ITERS << 32) | child_rf;

        start_child(&child, entry, arg);
    }

    // Collect completion times in arrival order
    let start = cycles();
    let mut times = [0u64; N_TOTAL];

    for time in times.iter_mut() {
        receive(report_field);

        *time = cycles() - start;
    }

    // Report each completion time
    for (i, &t) in times.iter().enumerate() {
        bench_emit(TAG_BASE + i as u64, t, 0, 0);
    }

    // Convoy ratio: last/first (1.0 = no convoy)
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
