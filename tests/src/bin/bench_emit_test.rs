//! Validates that BRK #0x48 emits a BENCH line and resumes execution.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::*;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    // Emit a known BENCH line via BRK #0x48 with x0-x3 set to known values.
    // SAFETY: BRK #0x48 is the benchmark emission protocol. The kernel reads
    // x0-x3, prints them, advances PC, and resumes. No memory side effects.
    unsafe {
        core::arch::asm!(
            "mov x0, #0xBEEF",
            "mov x1, #0x1234",
            "mov x2, #0",
            "mov x3, #0",
            "brk #0x48",
            out("x0") _,
            out("x1") _,
            out("x2") _,
            out("x3") _,
        );
    }

    // If we reach here, BRK #0x48 resumed (didn't exit the Observer).
    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
