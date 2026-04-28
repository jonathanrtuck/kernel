//! Object destruction cost.
//!
//! Creates N Fields from split Spaces, then destroys them all.
//! Measures per-object destruction cost and total cascade time.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::*;

const DEPTH: usize = 16;
const OBJ_SPACE: u64 = 16384;
const TAG_TOTAL: u64 = 0x320;
const TAG_PER_OBJ: u64 = 0x321;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let mut handles = [0u64; DEPTH];

    for handle in handles.iter_mut() {
        let s = space_split(ROOT_SPACE_HANDLE, OBJ_SPACE);

        if !s.is_ok() {
            fail();
        }

        *handle = s.value();

        let cf = create_field(*handle, 2);

        if !cf.is_ok() {
            fail();
        }
    }

    let start = cycles();

    for &handle in &handles {
        let ret = destroy(handle);
        close(handle);
        if ret.is_ok() && ret.value() != 0 {
            close(ret.value());
        }
    }

    let elapsed = cycles() - start;

    bench_emit(TAG_TOTAL, elapsed, DEPTH as u64, 0);
    bench_emit(TAG_PER_OBJ, elapsed / DEPTH as u64, DEPTH as u64, 0);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
