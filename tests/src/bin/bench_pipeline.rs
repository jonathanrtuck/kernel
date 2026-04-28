//! Pipeline IPC latency: chain of N forwarding stages.
//!
//! Each stage receives from its input Field, sends to its output Field.
//! Root sends to the first stage, receives from the last. Measures
//! end-to-end latency and per-hop cost for N=2, 4, 8 stages.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 50;
const MEASURE: u32 = 1000;
const TAG_N2: u64 = 0x170;
const TAG_N4: u64 = 0x175;
const TAG_N8: u64 = 0x17A;

// Pipeline stage: receive from input, send to output, repeat.
// x0 = (output_handle << 32) | input_handle
global_asm!(
    ".global _pipeline_stage",
    "_pipeline_stage:",
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

fn pipeline_stage_entry() -> u64 {
    unsafe extern "C" {
        fn _pipeline_stage();
    }

    _pipeline_stage as *const () as u64
}

const MAX_STAGES: usize = 8;
const MAX_FIELDS: usize = MAX_STAGES + 1;

fn build_pipeline(handler_field: u64, fields: &[u64], stages: usize) {
    let entry = pipeline_stage_entry();

    for i in 0..stages {
        let child = create_child(handler_field);
        let in_f = share_field(child.handle, fields[i]);
        let out_f = share_field(child.handle, fields[i + 1]);
        let arg = (out_f << 32) | in_f;

        start_child(&child, entry, arg);
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    // Allocate all fields upfront
    let mut fields = [0u64; MAX_FIELDS];

    for field in fields.iter_mut() {
        *field = alloc_field(8);
    }

    // N=2 stages: Root -> fields[0] -> S0 -> fields[1] -> S1 -> fields[2] -> Root
    build_pipeline(handler_field, &fields, 2);
    {
        for _ in 0..WARMUP {
            send(fields[0], 0x42, [1, 2, 3, 4]);
            receive(fields[2]);
        }

        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            send(fields[0], 0x42, [1, 2, 3, 4]);
            receive(fields[2]);

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_N2);
    }

    // N=4 and N=8 need separate field chains (existing stages are still running)
    let mut fields4 = [0u64; 5];
    for field in fields4.iter_mut() {
        *field = alloc_field(8);
    }

    build_pipeline(handler_field, &fields4, 4);

    {
        for _ in 0..WARMUP {
            send(fields4[0], 0x42, [1, 2, 3, 4]);
            receive(fields4[4]);
        }

        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            send(fields4[0], 0x42, [1, 2, 3, 4]);
            receive(fields4[4]);

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_N4);
    }

    let mut fields8 = [0u64; MAX_FIELDS];

    for field in fields8.iter_mut() {
        *field = alloc_field(8);
    }

    build_pipeline(handler_field, &fields8, 8);

    {
        for _ in 0..WARMUP {
            send(fields8[0], 0x42, [1, 2, 3, 4]);
            receive(fields8[8]);
        }

        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            send(fields8[0], 0x42, [1, 2, 3, 4]);
            receive(fields8[8]);

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_N8);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
