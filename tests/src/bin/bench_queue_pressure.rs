//! Queue pressure: fast sender, slow receiver.
//!
//! One child sends as fast as possible. Root receives with deliberate
//! delays (burn_cycles between receives). Measures queue-full rate and
//! pending-list behavior (D18).

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const MEASURE: u32 = 500;
const TAG: u64 = 0x190;
const DELAY_TICKS: u64 = 5000;

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

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(8);
    let child = create_child(handler_field);
    let child_field = share_field(child.handle, ipc_field);

    start_child(&child, sender_loop_entry(), child_field);

    // Drain initial burst
    for _ in 0..50 {
        receive(ipc_field);
        burn_cycles(DELAY_TICKS);
    }

    // Measure: receive with deliberate delays
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        burn_cycles(DELAY_TICKS);

        let sw = Stopwatch::start();

        receive(ipc_field);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
