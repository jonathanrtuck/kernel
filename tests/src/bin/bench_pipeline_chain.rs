//! Pipeline with heterogeneous scheduling profiles.
//!
//! 5-stage pipeline where each stage has a different R/T profile, plus
//! 2 background compute observers adding contention. Under round-robin
//! all stages wait equally; a profile-aware scheduler should prioritize
//! front-end (high-R) stages for lower end-to-end latency.
//!
//! Stage profiles:
//!   0: R=100,T=20 (front-end)   3: R=60,T=60  (middle)
//!   1: R=60,T=60  (middle)      4: R=100,T=20 (front-end)
//!   2: R=20,T=100 (back-end)

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const STAGES: usize = 5;
const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const TAG: u64 = 0x780;

const PROFILES: [(u64, u64); STAGES] = [
    (100, 20), // stage 0: front-end
    (60, 60),  // stage 1: middle
    (20, 100), // stage 2: back-end
    (60, 60),  // stage 3: middle
    (100, 20), // stage 4: front-end
];

// Pipeline stage: receive from input, send to output, repeat.
// x0 = (output_handle << 32) | input_handle
global_asm!(
    ".global _chain_stage",
    "_chain_stage:",
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

fn chain_stage_entry() -> u64 {
    unsafe extern "C" {
        fn _chain_stage();
    }

    _chain_stage as *const () as u64
}

// Background compute loop (contention)
global_asm!(
    ".global _chain_compute",
    "_chain_compute:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn chain_compute_entry() -> u64 {
    unsafe extern "C" {
        fn _chain_compute();
    }

    _chain_compute as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    resource_request(ROOT_SPACE_HANDLE, 0, 1_048_576);

    let handler_field = alloc_field(8);
    let entry = chain_stage_entry();
    // Allocate inter-stage Fields (STAGES + 1)
    let mut fields = [0u64; STAGES + 1];

    for field in fields.iter_mut() {
        *field = alloc_field(8);
    }

    // Create pipeline stages with profiles
    for i in 0..STAGES {
        let child = create_child(handler_field);
        let in_f = share_field(child.handle, fields[i]);
        let out_f = share_field(child.handle, fields[i + 1]);
        let arg = (out_f << 32) | in_f;
        let (r, t) = PROFILES[i];

        observer_set_scheduling(child.handle, r, t);
        start_child(&child, entry, arg);
    }

    // Background compute contention (batch profile)
    let centry = chain_compute_entry();

    for _ in 0..2 {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 10, 110);
        start_child(&child, centry, 0);
    }

    // Warmup
    for _ in 0..WARMUP {
        send(fields[0], 0x42, [1, 2, 3, 4]);
        receive(fields[STAGES]);
    }

    // Measure end-to-end pipeline latency
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        send(fields[0], 0x42, [1, 2, 3, 4]);
        receive(fields[STAGES]);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
