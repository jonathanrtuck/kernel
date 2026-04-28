//! SMP control-plane test: Observer lifecycle typed operations.
//!
//! Part 1: exercises the Observer state machine on an Inert Observer —
//! create → write registers → suspend → read registers → resume.
//! Verifies that register state survives the Inert suspend/resume cycle.
//!
//! Part 2: creates multiple child Observers under preemptive scheduling,
//! each doing independent work on a shared Field. The parent monitors
//! all children by receiving completion signals. Verifies that multiple
//! Observers with overlapping lifetimes don't corrupt each other's state.
//!
//! Exercises: CreateObserver, WriteRegisters, ReadRegisters, Suspend,
//! Resume, concurrent Observer creation under scheduling contention,
//! Observer state machine transitions (D39).

#![no_std]
#![no_main]

use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const CHILDREN: u64 = 4;
const DONE_MARKER: u64 = 0xBEEF;

global_asm!(
    ".global _smp_lifecycle_child",
    "_smp_lifecycle_child:",
    // x0 = sync_field handle
    "mov x19, x0",
    // Yield loop — give other children time to be created and run.
    "mov x20, #50",
    "1:",
    "cbz x20, 2f",
    "svc #5",
    "sub x20, x20, #1",
    "b 1b",
    // Done — send DONE_MARKER on sync_field.
    "2:",
    "mov x0, #0xBEEF",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "3:",
    "svc #5",
    "b 3b",
);

fn child_entry() -> u64 {
    unsafe extern "C" {
        fn _smp_lifecycle_child();
    }

    _smp_lifecycle_child as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let sync_field = alloc_field(64);
    // ── Part 1: Inert Observer lifecycle ──────────────────────────
    //
    // Create a child Observer, exercise typed operations BEFORE
    // starting it: WriteRegisters → Suspend → ReadRegisters → Resume.
    let child0 = create_child(handler_field);
    let child0_sync = share_field(child0.handle, sync_field);
    let entry = child_entry();
    let wr = observer_write_registers(child0.handle, entry, child0.stack_top, child0_sync, 0);

    assert_or_fail!(wr.is_ok());

    let s = observer_suspend(child0.handle);

    assert_or_fail!(s.is_ok());

    let regs = observer_read_registers(child0.handle);

    assert_or_fail!(regs.ok);
    assert_eq_or_fail!(regs.pc, entry);
    assert_eq_or_fail!(regs.sp, child0.stack_top);
    assert_eq_or_fail!(regs.x0, child0_sync);

    let r = observer_resume(child0.handle);

    assert_or_fail!(r.is_ok());

    // ── Part 2: Concurrent Observer creation ──────────────────────
    //
    // Create additional children while child0 is running. This
    // exercises Observer creation typed operations under preemptive
    // scheduling (timer interrupts arrive while we're mid-syscall).
    for _ in 1..CHILDREN {
        let child = create_child(handler_field);
        let child_sync = share_field(child.handle, sync_field);

        start_child(&child, child_entry(), child_sync);
    }

    // Wait for all children to complete.
    for _ in 0..CHILDREN {
        let msg = receive(sync_field);

        assert_eq_or_fail!(msg.data[0], DONE_MARKER);
    }

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
