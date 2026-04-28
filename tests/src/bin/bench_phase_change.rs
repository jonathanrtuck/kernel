//! Phase change: interactive → batch → interactive workload transition.
//!
//! Phase A1: 6 echo servers, measure IPC latency.
//! Phase B:  Destroy echo servers, create 4 compute workers, measure
//!           total completion time.
//! Phase A2: Destroy compute workers, create 6 new echo servers, measure
//!           IPC latency again.
//!
//! Tests whether the scheduler adapts back after a phase change — the
//! second interactive phase should have similar performance to the first.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const ECHO_COUNT: usize = 6;
const COMPUTE_COUNT: usize = 4;
const COMPUTE_ITERS: u64 = 200;
const WARMUP: u32 = 50;
const MEASURE: u32 = 500;
const TAG_A1: u64 = 0x7B0;
const TAG_B: u64 = 0x7B7;
const TAG_A2: u64 = 0x7B8;

// Compute worker: fixed iterations, then report on field.
// x0 = (iterations << 32) | field_handle
global_asm!(
    ".global _phase_worker",
    "_phase_worker:",
    "mov w19, w0",
    "lsr x20, x0, #32",
    "mov x21, #0",
    "1:",
    "cmp x21, x20",
    "b.ge 2f",
    "mov x22, #10000",
    "3:",
    "subs x22, x22, #1",
    "b.ne 3b",
    "add x21, x21, #1",
    "b 1b",
    "2:",
    "mov x0, x21",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "4:",
    "svc #5",
    "b 4b",
);

fn phase_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _phase_worker();
    }

    _phase_worker as *const () as u64
}

fn measure_ipc_phase(handler_field: u64, warmup: u32, measure: u32, handles: &mut [u64], tag: u64) {
    let ipc_field = alloc_field(64);

    for handle in handles.iter_mut() {
        *handle = spawn_echo_server(handler_field, ipc_field);
    }

    for _ in 0..warmup {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    let mut stats = Stats::new();

    for _ in 0..measure {
        let sw = Stopwatch::start();

        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

        stats.record(sw.elapsed());
    }

    stats.emit(tag);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    resource_request(ROOT_SPACE_HANDLE, 0, 1_048_576);

    let handler_field = alloc_field(8);

    setup_reply_field();

    // Phase A1: interactive (6 echo servers)
    let mut echo_handles = [0u64; ECHO_COUNT];

    measure_ipc_phase(handler_field, WARMUP, MEASURE, &mut echo_handles, TAG_A1);

    // Tear down echo servers
    for &handle in &echo_handles {
        observer_suspend(handle);
        destroy(handle);
    }

    // Phase B: batch (4 compute workers)
    let report_field = alloc_field(8);
    let entry = phase_worker_entry();
    let mut compute_handles = [0u64; COMPUTE_COUNT];
    let batch_start = cycles();

    for handle in compute_handles.iter_mut() {
        let child = create_child(handler_field);
        let child_rf = share_field(child.handle, report_field);
        let arg = (COMPUTE_ITERS << 32) | child_rf;

        start_child(&child, entry, arg);

        *handle = child.handle;
    }

    for _ in 0..COMPUTE_COUNT {
        receive(report_field);
    }

    let batch_elapsed = cycles() - batch_start;

    bench_emit(TAG_B, batch_elapsed, COMPUTE_COUNT as u64, COMPUTE_ITERS);

    // Tear down compute workers
    for &handle in &compute_handles {
        destroy(handle);
    }

    // Phase A2: interactive again (6 new echo servers)
    let mut echo_handles2 = [0u64; ECHO_COUNT];

    measure_ipc_phase(handler_field, WARMUP, MEASURE, &mut echo_handles2, TAG_A2);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
