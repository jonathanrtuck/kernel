//! Fanin IPC throughput: N producers, one consumer.
//!
//! N child Observers send to a shared Field as fast as possible. Root
//! (consumer) receives and measures per-message latency. Tests N=2, 4, 8
//! with separate Fields to isolate each configuration.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 100;
const MEASURE: u32 = 1000;
const TAG_N2: u64 = 0x150;
const TAG_N4: u64 = 0x155;
const TAG_N8: u64 = 0x15A;

global_asm!(
    ".global _sender_loop",
    "_sender_loop:",
    "mov x19, x0",
    "1:",
    "mov x0, #0",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "b 1b",
);

fn sender_loop_entry() -> u64 {
    unsafe extern "C" {
        fn _sender_loop();
    }

    _sender_loop as *const () as u64
}

fn create_senders(handler_field: u64, shared_field: u64, n: usize) {
    let entry = sender_loop_entry();

    for _ in 0..n {
        let child = create_child(handler_field);
        let child_field = share_field(child.handle, shared_field);

        start_child(&child, entry, child_field);
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);

    // N=2
    {
        let field = alloc_field(32);

        create_senders(handler_field, field, 2);

        for _ in 0..WARMUP {
            receive(field);
        }

        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            receive(field);

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_N2);
    }
    // N=4
    {
        let field = alloc_field(32);

        create_senders(handler_field, field, 4);

        for _ in 0..WARMUP {
            receive(field);
        }

        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            receive(field);

            stats.record(sw.elapsed());
        }

        stats.emit(TAG_N4);
    }
    // N=8
    {
        let field = alloc_field(32);

        create_senders(handler_field, field, 8);

        for _ in 0..WARMUP {
            receive(field);
        }

        let mut stats = Stats::new();

        for _ in 0..MEASURE {
            let sw = Stopwatch::start();

            receive(field);

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
