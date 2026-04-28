//! IPC dispatch attribution via kernel trace points.
//!
//! Enables dispatch tracing, runs ONE IPC Call (DirectSwitch path),
//! disables tracing. The kernel emits BENCH lines with stage timestamps
//! (tags 0xF000+). Consecutive differences show time per stage:
//!
//!   Stage 0 → 1: cap resolution (decode + resolve + arena + generation)
//!   Stage 1 → 2: communication::call + DirectSwitch decision
//!   Stage 2 → 3: message delivery (read sender regs, install reply cap, write to receiver)
//!   Stage 3 → 4: scheduler bookkeeping (block sender, dequeue, unblock receiver)
//!
//! Also runs N untraced IPC calls for the total round-trip baseline.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 100;
const MEASURE: u32 = 5_000;
const TOTAL_TAG: u64 = 0x700;
const TRACE_RUNS: usize = 10;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);

    spawn_echo_server(handler_field, ipc_field);
    setup_reply_field();

    // Untraced baseline for total round-trip
    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

        stats.record(sw.elapsed());
    }

    stats.emit(TOTAL_TAG);

    // Traced IPC calls — kernel emits stage timestamps as BENCH lines.
    // Run multiple traced calls; each emits its own set of stage entries.
    // The BEST (lowest stage 0→4 span) represents the cleanest observation.
    for _ in 0..TRACE_RUNS {
        trace_control(1);
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
        trace_control(0);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
