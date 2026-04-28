//! Foreground/background split.
//!
//! One interactive Observer (R=110, T=10) doing IPC with root. Four
//! compute Observers (R=10, T=110) doing infinite tight loops. Measures
//! IPC round-trip latency to the interactive server — the "can I use my
//! text editor while compiling" test.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 20;
const MEASURE: u32 = 200;
const TAG: u64 = 0x700;
const COMPUTE_WORKERS: usize = 4;

// Tight busy loop (no IPC, no syscalls except scheduling)
global_asm!(
    ".global _fg_bg_compute",
    "_fg_bg_compute:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn compute_loop_entry() -> u64 {
    unsafe extern "C" {
        fn _fg_bg_compute();
    }

    _fg_bg_compute as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);
    // Background compute load (batch profile)
    let centry = compute_loop_entry();

    for _ in 0..COMPUTE_WORKERS {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 10, 110);
        start_child(&child, centry, 0);
    }

    // Interactive echo server (foreground profile)
    let server = spawn_echo_server(handler_field, ipc_field);

    observer_set_scheduling(server, 110, 10);
    setup_reply_field();

    // Warmup
    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    // Measure IPC latency to interactive server under compute load
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
