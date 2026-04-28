//! Priority inversion.
//!
//! Three Observers:
//! - High (R=120, T=0): blocked on Receive, wakes and sends ack
//! - Medium (R=60, T=60): infinite compute loop (never yields)
//! - Low (R=0, T=120): receives trigger, computes, then wakes High
//!
//! The inversion: root triggers Low, which starts computing. Medium
//! runs continuously. Under a priority scheduler, Medium preempts Low,
//! delaying High's wakeup. Root measures trigger-to-ack latency.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const ROUNDS: u32 = 50;
const TAG: u64 = 0x760;

// High-priority worker: receive on wait_field, send ack on
// report_field, loop forever.
// x0 = (report_field << 32) | wait_field
global_asm!(
    ".global _inversion_high",
    "_inversion_high:",
    "mov w19, w0",      // wait_field (low 32)
    "lsr x20, x0, #32", // report_field (high 32)
    "1:",
    "mov x5, x19", // Receive on wait_field
    "svc #2",
    "mov x0, #1", // ack label
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20", // Send on report_field
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "b 1b",
);

// Low-priority worker: receive trigger, compute, send to wake_field,
// loop forever.
// x0 = (wake_field << 32) | trigger_field, x1 = iterations
global_asm!(
    ".global _inversion_low",
    "_inversion_low:",
    "mov w19, w0",      // trigger_field (low 32)
    "lsr x20, x0, #32", // wake_field (high 32)
    "mov x23, x1",      // iteration count
    "1:",
    "mov x5, x19", // Receive trigger
    "svc #2",
    "mov x21, #0", // compute loop
    "2:",
    "cmp x21, x23",
    "b.ge 3f",
    "mov x22, #10000",
    "4:",
    "subs x22, x22, #1",
    "b.ne 4b",
    "add x21, x21, #1",
    "b 2b",
    "3:",
    "mov x0, #1", // wake High
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

// Medium-priority: infinite compute (never yields)
global_asm!(
    ".global _inversion_medium",
    "_inversion_medium:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn high_entry() -> u64 {
    unsafe extern "C" {
        fn _inversion_high();
    }

    _inversion_high as *const () as u64
}

fn low_entry() -> u64 {
    unsafe extern "C" {
        fn _inversion_low();
    }

    _inversion_low as *const () as u64
}

fn medium_entry() -> u64 {
    unsafe extern "C" {
        fn _inversion_medium();
    }

    _inversion_medium as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let trigger_field = alloc_field(8);
    let wake_field = alloc_field(8);
    let report_field = alloc_field(8);
    // High-priority Observer
    let high = create_child(handler_field);
    let high_wait = share_field(high.handle, wake_field);
    let high_report = share_field(high.handle, report_field);
    let high_arg = (high_report << 32) | high_wait;

    observer_set_scheduling(high.handle, 120, 0);
    start_child(&high, high_entry(), high_arg);

    // Medium-priority Observer (infinite compute)
    let medium = create_child(handler_field);

    observer_set_scheduling(medium.handle, 60, 60);
    start_child(&medium, medium_entry(), 0);

    // Low-priority Observer
    let low = create_child(handler_field);
    let low_trigger = share_field(low.handle, trigger_field);
    let low_wake = share_field(low.handle, wake_field);
    let low_arg = (low_wake << 32) | low_trigger;

    observer_set_scheduling(low.handle, 0, 120);
    start_child(&low, low_entry(), low_arg);

    // Let everyone settle
    for _ in 0..10 {
        yield_cpu();
    }

    // Measure trigger-to-ack latency
    let mut stats = Stats::new();

    for _ in 0..ROUNDS {
        let sw = Stopwatch::start();

        // Trigger low-priority (starts computing, eventually wakes high)
        send(trigger_field, 0, [0; 4]);
        // Wait for high-priority ack
        receive(report_field);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
