//! Cascading migration: communication partners with divergent profiles.
//!
//! 3 pairs of 2-stage mini-pipelines. Each pair's partners have different
//! R/T profiles, forcing a profile-aware scheduler to potentially place
//! them on different cores (cross-core IPC penalty). The control pair has
//! identical profiles and should stay co-located.
//!
//! Pair A: R=120,T=0 / R=0,T=120  (maximum divergence)
//! Pair B: R=100,T=20 / R=20,T=100 (moderate divergence)
//! Pair C: R=60,T=60  / R=60,T=60  (control — identical)

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 50;
const MEASURE: u32 = 500;
const TAG_A: u64 = 0x7D0;
const TAG_B: u64 = 0x7D7;
const TAG_C: u64 = 0x7DE;

// Pipeline stage: receive from input, send to output, repeat.
// x0 = (output_handle << 32) | input_handle
global_asm!(
    ".global _cascade_stage",
    "_cascade_stage:",
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

fn cascade_stage_entry() -> u64 {
    unsafe extern "C" {
        fn _cascade_stage();
    }

    _cascade_stage as *const () as u64
}

struct PairProfile {
    r0: u64,
    t0: u64,
    r1: u64,
    t1: u64,
}

const PAIRS: [PairProfile; 3] = [
    PairProfile {
        r0: 120,
        t0: 0,
        r1: 0,
        t1: 120,
    }, // pair A: max divergence
    PairProfile {
        r0: 100,
        t0: 20,
        r1: 20,
        t1: 100,
    }, // pair B: moderate
    PairProfile {
        r0: 60,
        t0: 60,
        r1: 60,
        t1: 60,
    }, // pair C: control
];

const TAGS: [u64; 3] = [TAG_A, TAG_B, TAG_C];

fn build_and_measure_pair(handler_field: u64, profile: &PairProfile, tag: u64) {
    let entry = cascade_stage_entry();
    // 3 Fields: root→stage0, stage0→stage1, stage1→root
    let f_in = alloc_field(8);
    let f_mid = alloc_field(8);
    let f_out = alloc_field(8);
    // Stage 0
    let s0 = create_child(handler_field);
    let s0_in = share_field(s0.handle, f_in);
    let s0_out = share_field(s0.handle, f_mid);

    observer_set_scheduling(s0.handle, profile.r0, profile.t0);
    start_child(&s0, entry, (s0_out << 32) | s0_in);

    // Stage 1
    let s1 = create_child(handler_field);
    let s1_in = share_field(s1.handle, f_mid);
    let s1_out = share_field(s1.handle, f_out);

    observer_set_scheduling(s1.handle, profile.r1, profile.t1);
    start_child(&s1, entry, (s1_out << 32) | s1_in);

    // Warmup
    for _ in 0..WARMUP {
        send(f_in, 0x42, [1, 2, 3, 4]);
        receive(f_out);
    }

    // Measure round-trip through the pair
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        send(f_in, 0x42, [1, 2, 3, 4]);
        receive(f_out);

        stats.record(sw.elapsed());
    }

    stats.emit(tag);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    resource_request(ROOT_SPACE_HANDLE, 0, 1_048_576);

    let handler_field = alloc_field(8);

    // Measure each pair sequentially to isolate their latency
    for i in 0..3 {
        build_and_measure_pair(handler_field, &PAIRS[i], TAGS[i]);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
