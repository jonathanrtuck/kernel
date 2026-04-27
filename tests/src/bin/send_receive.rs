//! Proves Send + Receive IPC round-trip (Rust equivalent of send_receive.S).
//!
//! Splits a Space from the root, creates a Field, sends a message with
//! known data, receives it back, and verifies the contents match.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::*;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let split = space_split(ROOT_SPACE_HANDLE, 4096);
    assert_or_fail!(split.is_ok());
    let space_handle = split.value();

    let create = create_field(space_handle, 8);
    assert_or_fail!(create.is_ok());
    let field_handle = space_handle;

    let sent_data: [u64; 4] = [0xBEEF, 0xCAFE, 0xDEAD, 0xF00D];
    let sent_label: u64 = 0x42;

    assert_or_fail!(send(field_handle, sent_label, sent_data));

    let msg = receive(field_handle);

    assert_eq_or_fail!(msg.data[0], sent_data[0]);
    assert_eq_or_fail!(msg.data[1], sent_data[1]);
    assert_eq_or_fail!(msg.data[2], sent_data[2]);
    assert_eq_or_fail!(msg.data[3], sent_data[3]);
    assert_eq_or_fail!(msg.label, sent_label);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
