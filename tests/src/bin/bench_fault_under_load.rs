//! Fault handling latency with concurrent IPC traffic.
//!
//! Same as bench_fault_roundtrip but with background compute and IPC
//! workers. Measures whether fault delivery introduces latency
//! interference under load.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const TAG: u64 = 0x430;
const BG_WORKERS: usize = 2;

global_asm!(
    ".global _fault_looper",
    "_fault_looper:",
    "mov x1, #0xDEAD",
    "lsl x1, x1, #16",
    "1:",
    "ldr x2, [x1]",
    "b 1b",
);

fn fault_looper_entry() -> u64 {
    unsafe extern "C" {
        fn _fault_looper();
    }

    _fault_looper as *const () as u64
}

global_asm!(
    ".global _compute_loop",
    "_compute_loop:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn compute_loop_entry() -> u64 {
    unsafe extern "C" {
        fn _compute_loop();
    }

    _compute_loop as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    // Background compute load
    let centry = compute_loop_entry();

    for _ in 0..BG_WORKERS {
        let child = create_child(handler_field);

        start_child(&child, centry, 0);
    }

    // Faulting child
    let fault_child = create_child(handler_field);

    start_child(&fault_child, fault_looper_entry(), 0);
    // Prime
    receive(handler_field);

    // Warmup
    for _ in 0..WARMUP {
        let regs = observer_read_registers(fault_child.handle);

        observer_write_registers(
            fault_child.handle,
            regs.pc + 4,
            regs.sp,
            regs.x0,
            regs.pstate,
        );
        observer_resume(fault_child.handle);
        receive(handler_field);
    }

    // Measure
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();
        let regs = observer_read_registers(fault_child.handle);

        observer_write_registers(
            fault_child.handle,
            regs.pc + 4,
            regs.sp,
            regs.x0,
            regs.pstate,
        );
        observer_resume(fault_child.handle);
        receive(handler_field);

        stats.record(sw.elapsed());
    }
    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
