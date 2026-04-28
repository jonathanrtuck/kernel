//! Thundering herd.
//!
//! 8 Observers all blocked on Receive on a shared Field. Root sends 8
//! messages in a tight loop (scatter), then receives all 8 acks
//! (gather). Measures total scatter-gather time and individual ack
//! arrival timing.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const N: usize = 8;
const ROUNDS: u32 = 50;
const TAG: u64 = 0x730;
const TAG_SCATTER: u64 = 0x731;

// Herd worker: receive on wait_field, send ack on ack_field, repeat.
// x0 = (ack_field << 32) | wait_field
global_asm!(
    ".global _herd_worker",
    "_herd_worker:",
    "mov w19, w0",      // wait_field (low 32)
    "lsr x20, x0, #32", // ack_field (high 32)
    "1:",
    "mov x5, x19", // Receive on wait_field
    "svc #2",
    "mov x0, #1", // ack label
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20", // Send on ack_field
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "b 1b",
);

fn herd_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _herd_worker();
    }

    _herd_worker as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let wait_field = alloc_field(16);
    let ack_field = alloc_field(16);
    let entry = herd_worker_entry();

    for _ in 0..N {
        let child = create_child(handler_field);
        let child_wait = share_field(child.handle, wait_field);
        let child_ack = share_field(child.handle, ack_field);
        let arg = (child_ack << 32) | child_wait;

        start_child(&child, entry, arg);
    }

    // Let workers settle on Receive
    for _ in 0..10 {
        yield_cpu();
    }

    let mut stats = Stats::new();

    for _ in 0..ROUNDS {
        let sw = Stopwatch::start();

        // Scatter: send N messages
        for _ in 0..N {
            send(wait_field, 0, [0; 4]);
        }

        let scatter_time = sw.elapsed();

        // Gather: receive N acks
        for _ in 0..N {
            receive(ack_field);
        }

        let total = sw.elapsed();

        stats.record(total);
        bench_emit(TAG_SCATTER, scatter_time, N as u64, 0);
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
