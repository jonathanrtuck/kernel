//! SMP control-plane test: FP/SIMD register isolation across Observers.
//!
//! Parent and child each load distinctive patterns into FP registers
//! (d0-d3), then repeatedly yield and verify their patterns survive
//! context switches. If lazy FP save/restore is broken, one Observer
//! will see the other's values after a context switch.
//!
//! Exercises: FP trap handler (CPACR_EL1.FPEN), eager FP save at
//! exception entry (stp q pairs), lazy FP restore via handle_fp_trap
//! (ldp q pairs from RegisterState), FP state isolation under
//! preemptive scheduling with interleaved Observers.

#![no_std]
#![no_main]

use core::arch::asm;
use core::arch::global_asm;
use core::panic::PanicInfo;
use userspace_rs::harness::*;
use userspace_rs::*;

const ROUNDS: u64 = 100;
const PARENT_PATTERN: u64 = 0xAAAA_AAAA_AAAA_AAAA;
const DONE_MARKER: u64 = 0xBEEF;

// Child entry point:
//   x0 = sync_field handle
//
// Loads d0-d3 with CHILD_PATTERN (0xBBBB...), yields ROUNDS times,
// checks d0-d3 after each yield. Sends DONE or ERROR on sync_field.
global_asm!(
    ".global _smp_fp_child",
    "_smp_fp_child:",
    "mov x19, x0",
    // Load child pattern into x20 and FP regs.
    "mov x20, #0xBBBB",
    "movk x20, #0xBBBB, lsl #16",
    "movk x20, #0xBBBB, lsl #32",
    "movk x20, #0xBBBB, lsl #48",
    "fmov d0, x20",
    "fmov d1, x20",
    "fmov d2, x20",
    "fmov d3, x20",
    // Yield-and-check loop.
    "mov x21, #100",
    "1:",
    "cbz x21, 2f",
    "svc #5",
    // Check d0.
    "fmov x22, d0",
    "cmp x22, x20",
    "b.ne 3f",
    // Check d1.
    "fmov x22, d1",
    "cmp x22, x20",
    "b.ne 3f",
    // Check d2.
    "fmov x22, d2",
    "cmp x22, x20",
    "b.ne 3f",
    // Check d3.
    "fmov x22, d3",
    "cmp x22, x20",
    "b.ne 3f",
    "sub x21, x21, #1",
    "b 1b",
    // Success: send DONE.
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
    "4:",
    "svc #5",
    "b 4b",
    // Error: send ERROR.
    "3:",
    "mov x0, #0xDEAD",
    "mov x1, #0",
    "mov x2, #0",
    "mov x3, #0",
    "mov x4, #0",
    "mov x5, x19",
    "movn x6, #0",
    "mov x7, #0",
    "svc #1",
    "5:",
    "svc #5",
    "b 5b",
);

fn child_entry() -> u64 {
    unsafe extern "C" {
        fn _smp_fp_child();
    }

    _smp_fp_child as *const () as u64
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.start")]
extern "C" fn _start() -> ! {
    let handler_field = alloc_field(8);
    let sync_field = alloc_field(4);
    let child = create_child(handler_field);
    let child_sync = share_field(child.handle, sync_field);

    start_child(&child, child_entry(), child_sync);

    // Load parent pattern into d0-d3.
    // SAFETY: Writing to FP registers. Each Observer gets its own FP
    // state via the kernel's eager-save / lazy-restore mechanism.
    unsafe {
        asm!(
            "fmov d0, {p}",
            "fmov d1, {p}",
            "fmov d2, {p}",
            "fmov d3, {p}",
            p = in(reg) PARENT_PATTERN,
        );
    }

    // Yield-and-check loop for parent.
    for _ in 0..ROUNDS {
        yield_cpu();

        let d0: u64;
        let d1: u64;
        let d2: u64;
        let d3: u64;

        // SAFETY: Reading FP registers to verify isolation.
        unsafe {
            asm!(
                "fmov {d0}, d0",
                "fmov {d1}, d1",
                "fmov {d2}, d2",
                "fmov {d3}, d3",
                d0 = out(reg) d0,
                d1 = out(reg) d1,
                d2 = out(reg) d2,
                d3 = out(reg) d3,
            );
        }

        assert_eq_or_fail!(d0, PARENT_PATTERN);
        assert_eq_or_fail!(d1, PARENT_PATTERN);
        assert_eq_or_fail!(d2, PARENT_PATTERN);
        assert_eq_or_fail!(d3, PARENT_PATTERN);
    }

    // Wait for child's result.
    let msg = receive(sync_field);

    assert_eq_or_fail!(msg.data[0], DONE_MARKER);

    pass();
}

#[panic_handler]
fn panic(_: &PanicInfo) -> ! {
    fail();
}
