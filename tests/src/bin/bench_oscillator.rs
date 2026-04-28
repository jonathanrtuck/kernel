//! Oscillator: rapid phase alternation between light and heavy load.
//!
//! 2 echo servers run throughout. Root alternates between measuring IPC
//! latency with no background load and with 4 compute loops, across 4
//! rounds. Tests whether IPC latency degrades under load and recovers
//! when load is removed.
//!
//! Round 1: no background → Stats at TAG
//! Round 2: 4 compute loops → Stats at TAG+7
//! Round 3: no background → Stats at TAG+14
//! Round 4: 4 compute loops → Stats at TAG+21

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const MEASURE: u32 = 200;
const WARMUP: u32 = 20;
const COMPUTE_COUNT: usize = 4;
const TAG: u64 = 0x7C0;

// Infinite compute loop
global_asm!(
    ".global _oscillator_compute",
    "_oscillator_compute:",
    "1:",
    "add x0, x0, #1",
    "b 1b",
);

fn oscillator_compute_entry() -> u64 {
    unsafe extern "C" {
        fn _oscillator_compute();
    }

    _oscillator_compute as *const () as u64
}

fn measure_ipc(ipc_field: u64, warmup: u32, measure: u32, tag: u64) {
    for _ in 0..warmup {
        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);
    }

    let mut stats = Stats::new();

    for _ in 0..measure {
        let sw = Stopwatch::start();

        call(ipc_field, 0, [0; 4], CAP_ABSENT, 0);

        stats.record(sw.elapsed());
    }

    stats.emit(tag);
}

fn spawn_compute_batch(handler_field: u64, handles: &mut [u64; COMPUTE_COUNT]) {
    let entry = oscillator_compute_entry();

    for handle in handles.iter_mut() {
        let child = create_child(handler_field);

        observer_set_scheduling(child.handle, 10, 110);
        start_child(&child, entry, 0);

        *handle = child.handle;
    }
}

fn destroy_compute_batch(handles: &[u64; COMPUTE_COUNT]) {
    for &handle in handles {
        observer_suspend(handle);
        destroy(handle);
    }
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    resource_request(ROOT_SPACE_HANDLE, 0, 1_048_576);

    let handler_field = alloc_field(8);
    let ipc_field = alloc_field(64);

    // Persistent echo servers
    spawn_echo_server(handler_field, ipc_field);
    spawn_echo_server(handler_field, ipc_field);
    setup_reply_field();
    // Round 1: no background load
    measure_ipc(ipc_field, WARMUP, MEASURE, TAG);

    // Round 2: heavy background load
    let mut compute_handles = [0u64; COMPUTE_COUNT];

    spawn_compute_batch(handler_field, &mut compute_handles);
    measure_ipc(ipc_field, WARMUP, MEASURE, TAG + 7);
    destroy_compute_batch(&compute_handles);
    // Round 3: no background load (recovery)
    measure_ipc(ipc_field, WARMUP, MEASURE, TAG + 14);
    // Round 4: heavy background load again
    spawn_compute_batch(handler_field, &mut compute_handles);
    measure_ipc(ipc_field, WARMUP, MEASURE, TAG + 21);
    destroy_compute_batch(&compute_handles);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
