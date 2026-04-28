//! Mixed criticality — 3 levels.
//!
//! Tests isolation between criticality levels. 1 safety-critical Observer
//! (R=0, T=0, P=128), 2 important Observers (R=80, T=40), 4 best-effort
//! compute loops (R=10, T=110). Measures IPC round-trip to each level
//! independently. The safety observer must maintain low latency regardless
//! of the 7 other observers competing.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const TAG_SAFETY: u64 = 0x820;
const TAG_IMPORTANT: u64 = 0x827;
const BE_WORKERS: usize = 4;

// Background compute loop
global_asm!(
    ".global _criticality_bg_compute",
    "_criticality_bg_compute:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn bg_compute_entry() -> u64 {
    unsafe extern "C" {
        fn _criticality_bg_compute();
    }

    _criticality_bg_compute as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    resource_request(ROOT_SPACE_HANDLE, 0, 1_048_576);

    let handler_field = alloc_field(8);
    let safety_field = alloc_field(8);
    let important_field = alloc_field(8);
    // Safety-critical echo server (P=128, maximum precision)
    let safety = spawn_echo_server(handler_field, safety_field);

    observer_set_scheduling(safety, 0, 0);

    // Important echo servers (R=80, T=40)
    let important = spawn_echo_server(handler_field, important_field);

    observer_set_scheduling(important, 80, 40);

    // Second important observer shares the same field
    let important2 = spawn_echo_server(handler_field, important_field);

    observer_set_scheduling(important2, 80, 40);

    // Best-effort compute loops
    let centry = bg_compute_entry();

    for _ in 0..BE_WORKERS {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 10, 110);
        start_child(&child, centry, 0);
    }

    setup_reply_field();

    // Measure safety-critical latency
    for _ in 0..WARMUP {
        call(safety_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    {
        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            call(safety_field, 0, [0; 4], CAP_ABSENT, 0);

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_SAFETY);
    }

    // Measure important-level latency
    for _ in 0..WARMUP {
        call(important_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    {
        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            call(important_field, 0, [0; 4], CAP_ABSENT, 0);

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_IMPORTANT);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
