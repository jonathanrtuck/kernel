//! Proves Yield syscall round-trips correctly (Rust equivalent of yield_returns.S).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::{fail, pass, yield_cpu};

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    yield_cpu();
    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
