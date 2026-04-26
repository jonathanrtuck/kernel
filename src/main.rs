//! Microkernel.
//!
//! This is the kernel entry point. Subsystem logic lives in the library
//! modules declared by `lib.rs`.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use kernel::frame::arch;
use kernel::kernel_state::KernelState;
use kernel::println;
use kernel::space_manager::{RootPool, SpaceManager};

/// Kernel entry point, called from boot assembly after stack and BSS are set up.
///
/// `dtb_ptr` is the physical address of the device tree blob, passed in x0
/// by the hypervisor/firmware and preserved through boot.S.
#[unsafe(no_mangle)]
extern "C" fn kernel_main(dtb_ptr: usize) -> ! {
    // ── Phase 1: Hardware init ───────────────────────────────────────
    arch::exception::init();
    arch::platform::init(dtb_ptr);
    arch::mmu::init();
    arch::serial::enable_lock();
    arch::entropy::init();
    arch::interrupts::init();
    arch::timer::init();

    println!("alive");

    // ── Phase 2: Kernel state (D82, D93) ─────────────────────────────
    //
    // D93: physical memory partitioning. Usable memory runs from the end
    // of the kernel image to the end of RAM. Everything before __kernel_end
    // (kernel image, DTB, boot stack) is reserved.
    let page_size = arch::mmu::page_size();
    let usable_start = arch::mmu::kernel_end_address();
    let ram_end = arch::platform::ram_base() + arch::platform::ram_size();
    let usable_bytes = ram_end.saturating_sub(usable_start);

    println!(
        "pool: {:#x}..{:#x} ({} KiB, {} pages)",
        usable_start,
        ram_end,
        usable_bytes / 1024,
        usable_bytes / page_size,
    );

    let space_manager = SpaceManager {
        root_pool: RootPool {
            total_bytes: usable_bytes,
            free_bytes: usable_bytes,
            page_size,
        },
        next_physical_base: usable_start,
        next_va_base: usable_start,
    };
    let asid_width = arch::mmu::asid_width();

    println!("asid: {}-bit", asid_width);

    let kernel_state = KernelState::new(space_manager, asid_width);

    kernel::frame::init_kernel_state(kernel_state);

    // ── Phase 3: Root Observer and EL0 entry (D94) ─────────────────
    //
    // Create the root Observer with a test binary, initialize per-core
    // data, and context switch to EL0. This function does not return.
    let ks = kernel::frame::kernel_state();

    kernel::frame::boot::enter_first_observer(ks);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    arch::disable_interrupts();
    arch::serial::break_lock();

    println!();
    println!("panic: {info}");
    println!();

    arch::dump_panic_registers();
    arch::signal_panic();

    loop {
        arch::halt();
    }
}
