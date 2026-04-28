//! Render loop + workers — gaming frame deadline.
//!
//! One "render" Observer on a strict 16ms frame cycle. 3 worker Observers
//! doing compute (physics/AI). Root acts as the frame clock: sends a
//! frame-start to the render Observer, times until frame-done ack.
//! Workers add background load that the scheduler must manage around
//! the latency-sensitive render path.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 20;
const MEASURE: u32 = 200;
const TAG: u64 = 0x7E0;
const TAG_P99: u64 = 0x7E7;
const COMPUTE_WORKERS: usize = 3;

// Render worker: receive frame tick, do moderate compute, send ack, repeat.
// x0 = (ack_field << 32) | frame_field
global_asm!(
    ".global _render_frame_worker",
    "_render_frame_worker:",
    "mov w19, w0",      // frame_field (low 32)
    "lsr x20, x0, #32", // ack_field (high 32)
    "1:",
    "mov x5, x19", // Receive on frame_field
    "svc #2",
    // Simulate frame work: 5000 outer * 10 inner iterations
    "mov x21, #5000",
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
    ".global _render_bg_compute",
    "_render_bg_compute:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn render_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _render_frame_worker();
    }

    _render_frame_worker as *const () as u64
}

fn bg_compute_entry() -> u64 {
    unsafe extern "C" {
        fn _render_bg_compute();
    }

    _render_bg_compute as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let frame_field = alloc_field(8);
    let ack_field = alloc_field(8);
    // Render worker (high precision)
    let render_child = create_child(handler_field);
    let child_frame = share_field(render_child.handle, frame_field);
    let child_ack = share_field(render_child.handle, ack_field);
    let arg = (child_ack << 32) | child_frame;

    observer_set_scheduling(render_child.handle, 10, 10);
    start_child(&render_child, render_worker_entry(), arg);

    // Background compute workers (throughput-biased)
    let centry = bg_compute_entry();

    for _ in 0..COMPUTE_WORKERS {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 10, 110);
        start_child(&child, centry, 0);
    }

    // Warmup
    for _ in 0..WARMUP {
        send(frame_field, 0, [0; 4]);
        receive(ack_field);
    }

    // Measure frame times
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        send(frame_field, 0, [0; 4]);
        receive(ack_field);

        stats.record(sw.elapsed());
    }

    // Emit full stats and standalone p99 for stutter analysis
    stats.emit(TAG);
    bench_emit(TAG_P99, stats.max, MEASURE as u64, COMPUTE_WORKERS as u64);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
