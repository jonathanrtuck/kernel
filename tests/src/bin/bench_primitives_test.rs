//! Integration test for benchmark measurement primitives.
//!
//! Exercises cycles(), bench_emit(), Stats, Stopwatch, and burn_cycles()
//! on bare metal. Requires root Observer (clock_access=true).

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::*;

/// Test each primitive in its own function to keep stack usage per-call
/// within the ~16 KiB stack available to the root Observer.

fn test_cycles_monotonic() {
    let c1 = cycles();

    // Do some work between reads so the counter advances
    // (CNTVCT_EL0 ticks at counter frequency, not CPU frequency).
    for _ in 0..100 {
        core::hint::black_box(0u64);
    }

    let c2 = cycles();

    assert_or_fail!(c2 > c1);
}

fn test_bench_emit_resumes() {
    bench_emit(0xBEEF, 0x1234, 0, 0);
    // If we reach here, BRK #0x48 resumed successfully.
}

fn test_stats_basic() {
    let mut s = Stats::new();

    s.record(100);
    s.record(200);
    s.record(300);

    assert_eq_or_fail!(s.min, 100);
    assert_eq_or_fail!(s.max, 300);
    assert_eq_or_fail!(s.count, 3);
    assert_eq_or_fail!(s.mean(), 200);
}

fn test_stats_emit() {
    let mut s = Stats::new();

    for i in 1..=10 {
        s.record(i * 10);
    }

    // Emits 5 BENCH lines: tag+0=min, tag+1=median, tag+2=p99, tag+3=mean, tag+4=count
    s.emit(0x100);
}

fn test_stopwatch() {
    let sw = Stopwatch::start();

    for _ in 0..100 {
        core::hint::black_box(0u64);
    }

    let elapsed = sw.elapsed();

    assert_or_fail!(elapsed > 0);
}

fn test_burn_cycles() {
    let before = cycles();

    burn_cycles(10_000);

    let after = cycles();
    let elapsed = after - before;

    assert_or_fail!(elapsed > 1000);
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    test_cycles_monotonic();
    test_bench_emit_resumes();
    test_stats_basic();
    test_stats_emit();
    test_stopwatch();
    test_burn_cycles();

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
