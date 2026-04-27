//! Proves clock_read typed syscall returns a non-zero timestamp.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::*;

const SELF_HANDLE: u64 = 2;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let result = clock_read(SELF_HANDLE);

    assert_or_fail!(result.is_ok());
    assert_or_fail!(result.value() != 0);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
