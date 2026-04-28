//! Multi-rate sensor fusion.
//!
//! 4 sensor Observers producing data, plus a fusion Observer that
//! combines their outputs. Root simulates sensor rates by sending to
//! each sensor. Each sensor does small compute and forwards to fusion.
//! Fusion receives all 4 and sends ack to root. Measures end-to-end
//! fusion cycle time.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::bench::*;
use userspace_rs::harness::*;
use userspace_rs::*;

const WARMUP: u32 = 10;
const MEASURE: u32 = 100;
const TAG: u64 = 0x810;
const SENSOR_COUNT: usize = 4;

// Sensor worker: receive on input, small compute (500 iters), forward to output, repeat.
// x0 = (output_field << 32) | input_field
global_asm!(
    ".global _sensor_worker",
    "_sensor_worker:",
    "mov w19, w0",      // input_field (low 32)
    "lsr x20, x0, #32", // output_field (high 32)
    "1:",
    "mov x5, x19", // Receive on input_field
    "svc #2",
    // Simulate sensor processing: 500 * 10 inner iterations
    "mov x21, #500",
    "2:",
    "mov x22, #10",
    "3:",
    "subs x22, x22, #1",
    "b.ne 3b",
    "subs x21, x21, #1",
    "b.ne 2b",
    // Forward to fusion
    "mov x0, #1",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "b 1b",
);

// Fusion worker: receive 4 messages from fusion_field, send ack on report_field, repeat.
// x0 = (report_field << 32) | fusion_field
global_asm!(
    ".global _fusion_combiner",
    "_fusion_combiner:",
    "mov w19, w0",      // fusion_field (low 32)
    "lsr x20, x0, #32", // report_field (high 32)
    "1:",
    // Receive 4 sensor inputs
    "mov x5, x19",
    "svc #2",
    "mov x5, x19",
    "svc #2",
    "mov x5, x19",
    "svc #2",
    "mov x5, x19",
    "svc #2",
    // Send ack with count
    "mov x0, #4",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "b 1b",
);

fn sensor_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _sensor_worker();
    }

    _sensor_worker as *const () as u64
}

fn fusion_worker_entry() -> u64 {
    unsafe extern "C" {
        fn _fusion_combiner();
    }

    _fusion_combiner as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let fusion_field = alloc_field(16);
    let report_field = alloc_field(8);
    // Fusion worker
    let fusion_child = create_child(handler_field);
    let child_fusion = share_field(fusion_child.handle, fusion_field);
    let child_report = share_field(fusion_child.handle, report_field);
    let fusion_arg = (child_report << 32) | child_fusion;

    observer_set_scheduling(fusion_child.handle, 40, 40);
    start_child(&fusion_child, fusion_worker_entry(), fusion_arg);

    // 4 sensor workers, each with its own input field forwarding to fusion
    let sensor_entry = sensor_worker_entry();
    let mut sensor_fields = [0u64; SENSOR_COUNT];

    for i in 0..SENSOR_COUNT {
        let input_field = alloc_field(8);

        sensor_fields[i] = input_field;

        let child = create_child(handler_field);
        let child_input = share_field(child.handle, input_field);
        let child_output = share_field(child.handle, fusion_field);
        let arg = (child_output << 32) | child_input;

        observer_set_scheduling(child.handle, 80, 40);
        start_child(&child, sensor_entry, arg);
    }

    // Warmup
    for _ in 0..WARMUP {
        for j in 0..SENSOR_COUNT {
            send(sensor_fields[j], j as u64, [0; 4]);
        }

        receive(report_field);
    }

    // Measure fusion cycle time
    let mut stats = Stats::new();

    for _ in 0..MEASURE {
        let sw = Stopwatch::start();

        for j in 0..SENSOR_COUNT {
            send(sensor_fields[j], j as u64, [0; 4]);
        }

        receive(report_field);

        stats.record(sw.elapsed());
    }

    stats.emit(TAG);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
