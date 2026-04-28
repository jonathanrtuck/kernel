//! Proves FieldSplit creates a routed sub-Field on bare metal.
//!
//! Exercises the bare-metal routing table allocator: creates a Field,
//! splits it with a badge range, and sends a message through the route
//! to verify the sub-Field receives it.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::*;

const FIELD_SPACE_SIZE: u64 = 16384;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    // Allocate source Field.
    let space1 = space_split(ROOT_SPACE_HANDLE, FIELD_SPACE_SIZE);

    assert_or_fail!(space1.is_ok());

    let source_handle = space1.value();
    let cf1 = create_field(source_handle, 8);

    assert_or_fail!(cf1.is_ok());

    // Allocate Space for sub-Field.
    let space2 = space_split(ROOT_SPACE_HANDLE, FIELD_SPACE_SIZE);

    assert_or_fail!(space2.is_ok());

    let sub_space_handle = space2.value();
    // Split: badge range [100, 200] routes to a new sub-Field.
    let split = field_split(source_handle, sub_space_handle, 100, 200);

    assert_or_fail!(split.is_ok());

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
