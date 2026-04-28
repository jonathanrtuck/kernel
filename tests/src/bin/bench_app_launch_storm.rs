//! App launch storm: burst creation of 10 observers then profile settling.
//!
//! Simulates rapid app startup: root creates 10 observers (5 echo servers
//! + 5 compute loops), all initially interactive (R=100,T=20). Then half
//! settle to background (R=20,T=100). Root measures creation burst time
//! and post-settle IPC latency to an interactive echo server.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const ECHO_COUNT: usize = 5;
const COMPUTE_COUNT: usize = 5;
const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const TAG_BURST: u64 = 0x7A0;
const TAG_IPC: u64 = 0x7A1;

// Infinite compute loop
global_asm!(
    ".global _launch_compute",
    "_launch_compute:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn launch_compute_entry() -> u64 {
    unsafe extern "C" {
        fn _launch_compute();
    }

    _launch_compute as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    resource_request(ROOT_SPACE_HANDLE, 0, 1_048_576);

    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(64);
    let centry = launch_compute_entry();
    // Measure burst creation of 10 observers
    let burst_start = cycles();
    // 5 echo servers (stay interactive)
    let mut echo_handles = [0u64; ECHO_COUNT];

    for handle in echo_handles.iter_mut() {
        let server = spawn_echo_server(handler_field, ipc_field);

        observer_set_scheduling(server, 100, 20);

        *handle = server;
    }

    // 5 compute loops (will settle to background)
    let mut compute_handles = [0u64; COMPUTE_COUNT];

    for handle in compute_handles.iter_mut() {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 100, 20);
        start_child(&child, centry, 0);

        *handle = child.handle;
    }

    let burst_elapsed = cycles() - burst_start;

    bench_emit(TAG_BURST, burst_elapsed, 10, 0);

    // Settle: change compute workers to background profile
    for &handle in &compute_handles {
        observer_set_scheduling(handle, 20, 100);
    }

    // Measure IPC latency to one of the interactive echo servers
    setup_reply_field();

    for _ in 0..WARMUP {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG_IPC);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
