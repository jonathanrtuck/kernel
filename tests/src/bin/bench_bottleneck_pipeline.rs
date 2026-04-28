//! Producer-consumer pipeline with bottleneck stage.
//!
//! 3-stage pipeline where stage 1 does 3x the work per message (30000
//! inner iterations vs instant forwarding). The bottleneck dominates
//! end-to-end latency. A proportional-share scheduler giving stage 1
//! more CPU time would reduce latency.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 20;
const MEASURE: u32 = 200;
const TAG: u64 = 0x790;

// Fast stage: receive from input, send to output immediately, repeat.
// x0 = (output_handle << 32) | input_handle
global_asm!(
    ".global _bottleneck_fast",
    "_bottleneck_fast:",
    "mov w19, w0",
    "lsr x20, x0, #32",
    "mov x5, x19",
    "svc #2",
    "1:",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "mov x5, x19",
    "svc #2",
    "b 1b",
);

// Slow stage: receive, burn 30000 inner iterations, then send.
// x0 = (output_handle << 32) | input_handle
global_asm!(
    ".global _bottleneck_slow",
    "_bottleneck_slow:",
    "mov w19, w0",
    "lsr x20, x0, #32",
    "mov x5, x19",
    "svc #2",
    "1:",
    "mov x22, #30000",
    "2:",
    "subs x22, x22, #1",
    "b.ne 2b",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "mov x5, x19",
    "svc #2",
    "b 1b",
);

fn bottleneck_fast_entry() -> u64 {
    unsafe extern "C" {
        fn _bottleneck_fast();
    }

    _bottleneck_fast as *const () as u64
}

fn bottleneck_slow_entry() -> u64 {
    unsafe extern "C" {
        fn _bottleneck_slow();
    }

    _bottleneck_slow as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    // 4 Fields: root→S0, S0→S1, S1→S2, S2→root
    let mut fields = [0u64; 4];

    for field in fields.iter_mut() {
        *field = alloc_field(8);
    }

    // Stage 0: fast (receive from fields[0], send to fields[1])
    let s0 = create_child(handler_field);
    let s0_in = share_field(s0.handle, fields[0]);
    let s0_out = share_field(s0.handle, fields[1]);

    start_child(&s0, bottleneck_fast_entry(), (s0_out << 32) | s0_in);

    // Stage 1: SLOW bottleneck (receive from fields[1], send to fields[2])
    let s1 = create_child(handler_field);
    let s1_in = share_field(s1.handle, fields[1]);
    let s1_out = share_field(s1.handle, fields[2]);

    start_child(&s1, bottleneck_slow_entry(), (s1_out << 32) | s1_in);

    // Stage 2: fast (receive from fields[2], send to fields[3])
    let s2 = create_child(handler_field);
    let s2_in = share_field(s2.handle, fields[2]);
    let s2_out = share_field(s2.handle, fields[3]);

    start_child(&s2, bottleneck_fast_entry(), (s2_out << 32) | s2_in);

    // Warmup
    for _ in 0..WARMUP {
        send(fields[0], 0x42, [1, 2, 3, 4]);
        receive(fields[3]);
    }

    // Measure end-to-end latency (bottleneck-dominated)
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        send(fields[0], 0x42, [1, 2, 3, 4]);
        receive(fields[3]);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
