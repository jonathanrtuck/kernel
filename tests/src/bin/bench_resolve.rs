//! Cap table resolution cost at different occupancy levels.
//!
//! Fills the cap table to ~1/3 and ~2/3 capacity via clone_cap, then
//! measures typed syscall cost at each level. D77 resolution is a fixed
//! 5-check sequence — this verifies it's truly O(1).
//!
//! Fill counts are conservative (100 per phase) to stay well within
//! the root cap table capacity (~340 slots on 16 KiB pages with 48-byte
//! entries). Each measurement phase does clone+close (net-neutral).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::*;

const WARMUP: u32 = 50;
const MEASURE: u32 = 500;
const TAG_BASELINE: u64 = 0x310;
const TAG_MID: u64 = 0x315;
const TAG_HIGH: u64 = 0x31A;
const FILL_PER_PHASE: usize = 100;

fn measure_resolve(probe: u64, tag: u64) {
    for _ in 0..WARMUP {
        let r = clone_cap(probe);

        if r.is_ok() {
            close(r.value());
        }
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();
        let r = clone_cap(probe);

        if r.is_ok() {
            close(r.value());
        }

        stats.record(sw.elapsed());
    }

    stats.emit(tag);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let probe = clone_cap(ROOT_SPACE_HANDLE);

    if !probe.is_ok() {
        fail();
    }

    let probe_handle = probe.value();

    // Phase 1: sparse table
    measure_resolve(probe_handle, TAG_BASELINE);

    // Fill +100 entries
    for _ in 0..FILL_PER_PHASE {
        let _ = clone_cap(ROOT_SPACE_HANDLE);
    }

    // Phase 2: moderate occupancy
    measure_resolve(probe_handle, TAG_MID);

    // Fill +100 more (total +200)
    for _ in 0..FILL_PER_PHASE {
        let _ = clone_cap(ROOT_SPACE_HANDLE);
    }

    // Phase 3: higher occupancy
    measure_resolve(probe_handle, TAG_HIGH);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
