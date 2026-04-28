//! Allocation storm under load.
//!
//! System running at high utilization, then burst of new Observer
//! creation. Measures: baseline IPC latency under 4-worker load,
//! observer creation burst time (8 new observers), and degraded IPC
//! latency with 12 total observers.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const INITIAL_WORKERS: usize = 4;
const BURST_WORKERS: usize = 8;
const TAG_BASELINE: u64 = 0x840;
const TAG_BURST: u64 = 0x847;
const TAG_DEGRADED: u64 = 0x848;

// Background compute loop
global_asm!(
    ".global _storm_bg_compute",
    "_storm_bg_compute:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn bg_compute_entry() -> u64 {
    unsafe extern "C" {
        fn _storm_bg_compute();
    }

    _storm_bg_compute as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    resource_request(ROOT_SPACE_HANDLE, 0, 2_097_152);

    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);
    let centry = bg_compute_entry();

    // Phase 1: initial compute load
    for _ in 0..INITIAL_WORKERS {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 10, 110);
        start_child(&child, centry, 0);
    }

    // Echo server for IPC measurement
    spawn_echo_server(handler_field, ipc_field);
    setup_reply_field();

    // Baseline IPC latency under initial load
    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    {
        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_BASELINE);
    }

    // Phase 2: burst creation of 8 new observers
    let burst_start = cycles();

    for _ in 0..BURST_WORKERS {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 10, 110);
        start_child(&child, centry, 0);
    }

    let burst_elapsed = cycles() - burst_start;

    bench_emit(TAG_BURST, burst_elapsed, BURST_WORKERS as u64, 0);

    // Phase 3: degraded IPC latency with 12 total observers
    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    {
        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_DEGRADED);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
