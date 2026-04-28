//! Fault handling round-trip latency.
//!
//! Child Observer repeatedly accesses an unmapped address. Root receives
//! VmFault messages, skips the faulting instruction, and resumes the
//! child. Measures the full fault -> deliver -> handle -> resume cycle.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const TAG: u64 = 0x420;

// Repeatedly loads from unmapped address 0xDEAD0000. After the fault
// handler advances PC past the LDR, execution hits the branch back.
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

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let child = create_child(handler_field);

    start_child(&child, fault_looper_entry(), 0);
    // Prime: receive first fault
    receive(handler_field);

    // Warmup
    for _ in 0..WARMUP {
        let regs = observer_read_registers(child.handle);

        observer_write_registers(child.handle, regs.pc + 4, regs.sp, regs.x0, regs.pstate);
        observer_resume(child.handle);
        receive(handler_field);
    }

    // Measure: handle fault + resume + child-runs-to-next-fault + delivery
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();
        let regs = observer_read_registers(child.handle);

        observer_write_registers(child.handle, regs.pc + 4, regs.sp, regs.x0, regs.pstate);
        observer_resume(child.handle);
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
