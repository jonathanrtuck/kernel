//! Microkernel.
//!
//! This is the kernel entry crate. It owns the boot path, panic handler, and
//! top-level module wiring. Subsystem logic lives in child modules.

#![no_std]
#![no_main]

mod arch;
mod config;
mod print;

use core::panic::PanicInfo;

/// Kernel entry point, called from boot assembly after stack and BSS are set up.
///
/// `dtb_ptr` is the physical address of the device tree blob, passed in x0
/// by the hypervisor/firmware and preserved through boot.S.
#[unsafe(no_mangle)]
extern "C" fn kernel_main(dtb_ptr: usize) -> ! {
    arch::exception::init();
    arch::platform::init(dtb_ptr);
    arch::mmu::init();
    arch::entropy::init();
    arch::interrupts::init();
    arch::timer::init();

    println!("alive");

    loop {
        arch::halt();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    arch::disable_interrupts();

    println!();
    println!("panic: {info}");
    println!();

    arch::dump_panic_registers();
    arch::signal_panic();

    loop {
        arch::halt();
    }
}
