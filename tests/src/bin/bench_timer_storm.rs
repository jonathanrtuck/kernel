//! Timer storm: many Pulsars expiring in the same tick window.
//!
//! Creates N Pulsars all set to fire at the same time. Measures delivery
//! spread — time between first and last delivery. Stresses the per-core
//! deadline array (D83).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::alloc_field;
use userspace_rs::*;

const N: usize = 8;
const FIRE_DELAY_NS: u64 = 5_000_000;
const PERIOD_NS: u64 = 100_000_000;
const TAG_SPREAD: u64 = 0x410;
const TAG_PER_TIMER: u64 = 0x411;

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let timer_field = alloc_field(64);

    for i in 0..N {
        let ps = space_split(ROOT_SPACE_HANDLE, 4096);

        if !ps.is_ok() {
            fail();
        }

        let r = create_pulsar(
            ps.value(),
            timer_field,
            i as u64 + 1,
            FIRE_DELAY_NS,
            PERIOD_NS,
        );

        if !r.is_ok() {
            fail();
        }
    }

    // Receive all N timer deliveries from the first burst
    let start = cycles();

    for _ in 0..N {
        receive(timer_field);
    }

    let spread = cycles() - start;

    bench_emit(TAG_SPREAD, spread, N as u64, 0);
    bench_emit(TAG_PER_TIMER, spread / N as u64, N as u64, 0);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
