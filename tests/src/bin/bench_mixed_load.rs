//! Mixed IPC + compute load.
//!
//! Half the Observers do tight IPC (ping-pong), half do compute (busy
//! loop). Measures IPC latency under mixed scheduling pressure.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 50;
const MEASURE: u32 = 1000;
const TAG: u64 = 0x230;
const COMPUTE_WORKERS: usize = 2;

// Tight busy loop (no IPC, no syscalls except scheduling)
global_asm!(
    ".global _compute_loop",
    "_compute_loop:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn compute_loop_entry() -> u64 {
    unsafe extern "C" {
        fn _compute_loop();
    }

    _compute_loop as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);
    // Background compute load
    let centry = compute_loop_entry();

    for _ in 0..COMPUTE_WORKERS {
        let child = create_child(handler_field);

        start_child(&child, centry, 0);
    }

    // IPC echo server
    spawn_echo_server(handler_field, ipc_field);
    setup_reply_field();

    // Measure IPC latency under mixed load
    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

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
