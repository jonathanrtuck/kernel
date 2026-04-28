//! Audio pipeline — strict 5ms deadline.
//!
//! An audio Observer must process each buffer promptly. Root sends audio
//! ticks and measures round-trip latency. 4 background compute loops
//! simulate other apps competing for CPU. Key metric: p99 and max —
//! any single miss is an audible glitch.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const TAG: u64 = 0x7F0;
const TAG_MAX: u64 = 0x7F7;
const BG_WORKERS: usize = 4;

// Audio worker: receive tick, light compute (1000 iters), send ack, repeat.
// x0 = (ack_field << 32) | audio_field
global_asm!(
    ".global _audio_process",
    "_audio_process:",
    "mov w19, w0",      // audio_field (low 32)
    "lsr x20, x0, #32", // ack_field (high 32)
    "1:",
    "mov x5, x19", // Receive on audio_field
    "svc #2",
    // Simulate audio processing: 1000 * 10 inner iterations
    "mov x21, #1000",
    "2:",
    "mov x22, #10",
    "3:",
    "subs x22, x22, #1",
    "b.ne 3b",
    "subs x21, x21, #1",
    "b.ne 2b",
    // Send ack
    "mov x0, #1",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "b 1b",
);

// Background compute loop
global_asm!(
    ".global _audio_bg_compute",
    "_audio_bg_compute:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn audio_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _audio_process();
    }

    _audio_process as *const () as u64
}

fn bg_compute_entry() -> u64 {
    unsafe extern "C" {
        fn _audio_bg_compute();
    }

    _audio_bg_compute as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let audio_field = alloc_field(8);
    let ack_field = alloc_field(8);
    // Audio worker (precision-dominated)
    let audio_child = create_child(handler_field);
    let child_audio = share_field(audio_child.handle, audio_field);
    let child_ack = share_field(audio_child.handle, ack_field);
    let arg = (child_ack << 32) | child_audio;

    observer_set_scheduling(audio_child.handle, 20, 0);
    start_child(&audio_child, audio_worker_entry(), arg);

    // Background compute workers (throughput-biased)
    let centry = bg_compute_entry();

    for _ in 0..BG_WORKERS {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 10, 110);
        start_child(&child, centry, 0);
    }

    // Warmup
    for _ in 0..WARMUP {
        send(audio_field, 0, [0; 4]);
        receive(ack_field);
    }

    // Measure per-tick latency
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        send(audio_field, 0, [0; 4]);
        receive(ack_field);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);
    bench_emit(TAG_MAX, stats.max, MEASURE as u64, BG_WORKERS as u64);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
