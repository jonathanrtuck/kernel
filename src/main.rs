//! Microkernel.
//!
//! This is the kernel entry point. Subsystem logic lives in the library
//! modules declared by `lib.rs`.

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use kernel::{arch, println};

/// Kernel entry point, called from boot assembly after stack and BSS are set up.
///
/// `dtb_ptr` is the physical address of the device tree blob, passed in x0
/// by the hypervisor/firmware and preserved through boot.S.
#[unsafe(no_mangle)]
extern "C" fn kernel_main(dtb_ptr: usize) -> ! {
    arch::exception::init();
    arch::platform::init(dtb_ptr);
    arch::mmu::init();
    arch::serial::enable_lock();
    arch::entropy::init();
    arch::interrupts::init();
    arch::timer::init();

    println!("alive");

    arch::cpu::activate_secondaries();

    loop {
        arch::halt();
    }
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
