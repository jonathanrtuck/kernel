//! Control loop — 1kHz periodic.
//!
//! The tightest deadline: a control Observer must respond every ~1ms.
//! Root uses a Pulsar at 1ms period. On each tick, root sends to the
//! control Observer, times until ack. Reports schedule latency per tick
//! and counts deadline misses (>500us equivalent at 24MHz = 12000 ticks).

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const PERIOD_NS: u64 = 1_000_000;
const TAG: u64 = 0x800;
const TAG_MISSES: u64 = 0x807;
const DEADLINE_TICKS: u64 = 12000;
const BG_WORKERS: usize = 2;

// Control worker: receive tick, minimal compute (200 iters), send ack, repeat.
// x0 = (ack_field << 32) | control_field
global_asm!(
    ".global _control_tick_worker",
    "_control_tick_worker:",
    "mov w19, w0",      // control_field (low 32)
    "lsr x20, x0, #32", // ack_field (high 32)
    "1:",
    "mov x5, x19", // Receive on control_field
    "svc #2",
    // Minimal compute: 200 * 10 inner iterations
    "mov x21, #200",
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
    ".global _control_bg_compute",
    "_control_bg_compute:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn control_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _control_tick_worker();
    }

    _control_tick_worker as *const () as u64
}

fn bg_compute_entry() -> u64 {
    unsafe extern "C" {
        fn _control_bg_compute();
    }

    _control_bg_compute as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let control_field = alloc_field(8);
    let ack_field = alloc_field(8);
    // Control worker (maximum precision: R=0, T=0 -> P=128)
    let ctrl_child = create_child(handler_field);
    let child_ctrl = share_field(ctrl_child.handle, control_field);
    let child_ack = share_field(ctrl_child.handle, ack_field);
    let arg = (child_ack << 32) | child_ctrl;

    observer_set_scheduling(ctrl_child.handle, 0, 0);
    start_child(&ctrl_child, control_worker_entry(), arg);

    // Background compute workers
    let centry = bg_compute_entry();

    for _ in 0..BG_WORKERS {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 10, 110);
        start_child(&child, centry, 0);
    }

    // Pulsar at 1ms period
    let timer_field = alloc_field(64);
    let pulsar_space = space_split(ROOT_SPACE_HANDLE, 4096);

    if !pulsar_space.is_ok() {
        fail();
    }

    let r = create_pulsar(pulsar_space.value(), timer_field, 1, PERIOD_NS, PERIOD_NS);

    if !r.is_ok() {
        fail();
    }

    // Drain first delivery
    receive(timer_field);

    // Warmup
    for _ in 0..WARMUP {
        receive(timer_field);
        send(control_field, 0, [0; 4]);
        receive(ack_field);
    }

    // Measure per-tick latency
    let mut stats = Stats::new();
    let mut misses: u64 = 0;

    for _ in 0..MEASURE {
        receive(timer_field);

        let sw = Stopwatch::start();

        send(control_field, 0, [0; 4]);
        receive(ack_field);

        let elapsed = sw.elapsed();

        stats.record(elapsed);

        if elapsed > DEADLINE_TICKS {
            misses += 1;
        }
    }

    stats.emit(TAG);
    bench_emit(TAG_MISSES, misses, MEASURE as u64, DEADLINE_TICKS);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
