//! Controlled experiment: lazy FP restore cost measurement.
//!
//! Runs two back-to-back IPC ping-pong benchmarks with the same client:
//!   Tag 0x500: integer-only echo server (no FP — restore is skipped)
//!   Tag 0x510: FP-touching echo server (touches d0 each iteration —
//!              triggers FP trap on every context switch, forcing load)
//!
//! The delta between 0x500 and 0x510 is the per-roundtrip cost of the
//! FP restore path (trap entry + 16 ldp q + 2 msr + CPACR write).

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 200;
const MEASURE: u32 = 10_000;

global_asm!(
    ".global _echo_server_fp",
    "_echo_server_fp:",
    "mov x19, x0",
    "mov x5, x19",
    "svc #2",
    "1:",
    "fmov d0, xzr",
    "mov x5, x7",
    "movn x6, #0",
    "mov x7, x19",
    "svc #4",
    "b 1b",
);

fn echo_server_fp_entry() -> u64 {
    unsafe extern "C" {
        fn _echo_server_fp();
    }

    _echo_server_fp as *const () as u64
}

/// Phase 1: integer-only echo server (no FP use).
/// Separate function so its Stats frame is freed before phase 2.
#[inline(never)]
fn run_nofp(ipc_field: u64) {
    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

        stats.record(sw.elapsed());
    }

    stats.emit(0x500);
}

/// Phase 2: FP-touching echo server (fmov d0 each iteration).
/// Separate function so its Stats frame doesn't overlap phase 1.
#[inline(never)]
fn run_fp(ipc_field: u64) {
    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

        stats.record(sw.elapsed());
    }

    stats.emit(0x510);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    // Phase 1: integer-only echo server
    let handler1 = alloc_field(8);
    let ipc1 = alloc_field(8);

    spawn_echo_server(handler1, ipc1);
    setup_reply_field();
    run_nofp(ipc1);

    // Phase 2: FP-touching echo server
    let handler2 = alloc_field(8);
    let ipc2 = alloc_field(8);
    let child2 = create_child(handler2);
    let child2_field = share_field(child2.handle, ipc2);

    start_child(&child2, echo_server_fp_entry(), child2_field);
    run_fp(ipc2);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
