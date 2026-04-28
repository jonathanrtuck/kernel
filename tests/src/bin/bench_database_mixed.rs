//! Database mixed workload.
//!
//! Two "OLTP" Observers (R=100, T=20): rapid IPC ping-pong with root
//! (short transactions). Two "OLAP" Observers (R=20, T=100): long
//! compute (300 iterations). Root alternates: IPC round-trips with OLTP
//! servers, then checks for OLAP completions. Measures whether both
//! workload types get served.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const OLTP_COUNT: usize = 2;
const OLAP_COUNT: usize = 2;
const OLAP_ITERS: u64 = 300;
const OLTP_ROUNDS: u32 = 100;
const TAG_OLTP: u64 = 0x750;
const TAG_OLAP: u64 = 0x758;

// OLAP compute worker: x0 = (iterations << 32) | field_handle
global_asm!(
    ".global _olap_worker",
    "_olap_worker:",
    "mov w19, w0",
    "lsr x20, x0, #32",
    "mov x21, #0",
    "1:",
    "cmp x21, x20",
    "b.ge 2f",
    "mov x22, #10000",
    "3:",
    "subs x22, x22, #1",
    "b.ne 3b",
    "add x21, x21, #1",
    "b 1b",
    "2:",
    "mov x0, x21",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "4:",
    "svc #5",
    "b 4b",
);

fn olap_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _olap_worker();
    }

    _olap_worker as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let olap_report = alloc_field(8);
    // OLAP workers (batch profile)
    let olap_entry = olap_worker_entry();
    let olap_start = cycles();

    for _ in 0..OLAP_COUNT {
        let child = create_child(handler_field);
        let child_rf = share_field(child.handle, olap_report);
        let arg = (OLAP_ITERS << 32) | child_rf;

        observer_set_scheduling(child.handle, 20, 100);
        start_child(&child, olap_entry, arg);
    }

    // OLTP echo servers (interactive profile)
    let mut oltp_fields = [0u64; OLTP_COUNT];

    for field in oltp_fields.iter_mut() {
        let f = alloc_field(8);
        let server = spawn_echo_server(handler_field, f);

        observer_set_scheduling(server, 100, 20);

        *field = f;
    }

    setup_reply_field();

    // Warmup OLTP path
    for _ in 0..20 {
        call(oltp_fields[0], 0, [0; 4], CAP_ABSENT, 0);
    }

    // Measure OLTP latency (alternating between servers)
    let mut stats = Stats::new();

    for i in 0..OLTP_ROUNDS {
        let server_idx = i as usize % OLTP_COUNT;
        let sw = Stopwatch::start();

        call(oltp_fields[server_idx], 0, [0; 4], CAP_ABSENT, 0);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG_OLTP);

    // Collect OLAP completions
    for i in 0..OLAP_COUNT {
        receive(olap_report);

        let elapsed = cycles() - olap_start;

        bench_emit(TAG_OLAP + i as u64, elapsed, OLAP_ITERS, 0);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
