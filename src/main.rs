//! Microkernel.
//!
//! This is the kernel entry crate. It owns the boot path, panic handler, and
//! top-level module wiring. Subsystem logic lives in child modules.

#![no_std]
#![no_main]

mod arch;
mod print;

use core::panic::PanicInfo;

/// Kernel entry point, called from boot assembly after stack and BSS are set up.
#[unsafe(no_mangle)]
extern "C" fn kernel_main() -> ! {
    println!("alive…");

    loop {
        arch::halt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("panic: {info}");

    loop {
        arch::halt();
    }
}
