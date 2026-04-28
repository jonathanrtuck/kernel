//! SMP integration test: concurrent typed operations from two cores.
//!
//! Parent (core 0) and child (core 1) simultaneously run SpaceSplit →
//! CreateField → Close loops on independent Space caps. Both cores hit
//! the shared arenas (Space, Field) and SpaceManager under real lock
//! contention. Catches lock ordering violations, arena corruption,
//! and cap table races in the typed operation dispatch path.
//!
//! Exercises: concurrent arena alloc/free, concurrent cap table mutation,
//! SpaceManager page accounting under SMP contention.

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const ITERATIONS: u64 = 50;
const SPLIT_SIZE: u64 = 16384;
const DONE_MARKER: u64 = 0xBEEF;

// ── Child entry (assembly) ───────────────────────────────────────
//
// x0 = packed: bits[31:0] = space handle, bits[63:32] = sync field
//
// Runs ITERATIONS of SpaceSplit → CreateField → Close on its own
// Space, then sends DONE_MARKER on the sync Field.
global_asm!(
    ".global _smp_typed_child",
    "_smp_typed_child:",
    "and x19, x0, #0xFFFFFFFF",
    "lsr x20, x0, #32",
    "mov x21, #50",
    "1:",
    "cbz x21, 2f",
    // SpaceSplit(x19, 16384) → new handle in x0
    "mov x0, #16384",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #11",
    "mov x5, x19",
    "svc #0",
    // Error → send error marker on sync field so parent sees it
    "tbnz x0, #63, 3f",
    "mov x22, x0",
    // CreateField(x22, 4) → consumes Space, installs Field at x22
    "mov x0, #4",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #13",
    "mov x5, x22",
    "svc #0",
    "tbnz x0, #63, 3f",
    // Close(x22) → releases Field
    "mov x0, #0",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #9",
    "mov x5, x22",
    "svc #0",
    "sub x21, x21, #1",
    "b 1b",
    // Done — send marker on sync field.
    "2:",
    "mov x0, #0xBEEF",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "4:",
    "svc #5",
    "b 4b",
    // Error — send error code on sync field instead of brk.
    // brk from a child doesn't cause FATAL FAULT — it routes to
    // the handler Field, silently deadlocking if nobody receives.
    "3:",
    "mov x0, #0xDEAD",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x20",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "5:",
    "svc #5",
    "b 5b",
);

fn child_entry() -> u64 {
    unsafe extern "C" {
        fn _smp_typed_child();
    }

    _smp_typed_child as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let sync_field = alloc_field(4);
    // +1 page: split requires parent to keep at least one page.
    let child_space = space_split(ROOT_SPACE_HANDLE, (ITERATIONS + 1) * SPLIT_SIZE);

    assert_or_fail!(child_space.is_ok());

    let child = create_child(handler_field);
    let child_space_handle = share_field(child.handle, child_space.value());
    let child_sync_handle = share_field(child.handle, sync_field);

    close(child_space.value());

    let packed = child_space_handle | (child_sync_handle << 32);

    start_child(&child, child_entry(), packed);

    // Parent runs its own concurrent loop on root Space.
    for _ in 0..ITERATIONS {
        let space = space_split(ROOT_SPACE_HANDLE, SPLIT_SIZE);

        assert_or_fail!(space.is_ok());

        let handle = space.value();
        let result = create_field(handle, 4);

        assert_or_fail!(result.is_ok());

        let c = close(handle);

        assert_or_fail!(c.is_ok());
    }

    // Wait for child completion.
    let msg = receive(sync_field);

    assert_eq_or_fail!(msg.data[0], DONE_MARKER);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
