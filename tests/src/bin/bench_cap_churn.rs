//! Capability create/destroy throughput.
//!
//! Tight loop: SpaceSplit -> CreateField -> Destroy. Stresses the arena
//! allocator and freelist management.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::*;

const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const TAG: u64 = 0x300;
const FIELD_SPACE: u64 = 16384;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    for _ in 0..WARMUP {
        let space = space_split(ROOT_SPACE_HANDLE, FIELD_SPACE);

        if !space.is_ok() {
            fail();
        }

        let handle = space.value();
        let cf = create_field(handle, 2);

        if !cf.is_ok() {
            fail();
        }

        let ret = destroy(handle);

        close(handle);

        if ret.is_ok() && ret.value() != 0 {
            close(ret.value());
        }
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();
        let space = space_split(ROOT_SPACE_HANDLE, FIELD_SPACE);

        if !space.is_ok() {
            fail();
        }

        let handle = space.value();
        let cf = create_field(handle, 2);

        if !cf.is_ok() {
            fail();
        }

        let ret = destroy(handle);

        close(handle);

        if ret.is_ok() && ret.value() != 0 {
            close(ret.value());
        }

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
