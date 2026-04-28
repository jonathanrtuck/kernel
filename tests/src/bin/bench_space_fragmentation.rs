//! Space allocation latency under fragmentation.
//!
//! Many small SpaceSplit calls, then SpaceMerge to recombine. Measures
//! whether allocation latency degrades as the free list fragments.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::*;

const FRAG_COUNT: usize = 32;
const FRAG_SIZE: u64 = 4096;
const TAG_SPLIT: u64 = 0x330;
const TAG_MERGE: u64 = 0x335;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let mut spaces = [0u64; FRAG_COUNT];

    // Phase 1: measure split latency as fragmentation increases
    {
        let mut stats = Stats::new();

        for space in spaces.iter_mut() {
            let sw = Stopwatch::start();
            let s = space_split(ROOT_SPACE_HANDLE, FRAG_SIZE);

            stats.record(sw.elapsed());

            if !s.is_ok() {
                fail();
            }

            *space = s.value();
        }

        stats.emit(TAG_SPLIT);
    }

    // Phase 2: measure merge latency (adjacent pairs, reverse order)
    {
        let mut stats = Stats::new();
        let mut i = FRAG_COUNT;

        while i > 1 {
            i -= 1;

            let sw = Stopwatch::start();
            let r = space_merge(spaces[i - 1], spaces[i]);

            stats.record(sw.elapsed());

            if !r.is_ok() {
                break;
            }
        }

        stats.emit(TAG_MERGE);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
