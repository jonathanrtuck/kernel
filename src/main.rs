#![no_std]
#![no_main]

mod arch;

use core::panic::PanicInfo;

core::arch::global_asm!(include_str!("arch/aarch64/boot.S"));

/// Kernel entry point, called from boot.S after stack and BSS are set up.
#[unsafe(no_mangle)]
extern "C" fn kernel_main() -> ! {
    arch::serial::puts("kernel: alive\n");

    loop {
        // Halt until an interrupt arrives (saves power, proves we got here).
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    arch::serial::puts("kernel panic: ");

    if let Some(location) = info.location() {
        arch::serial::puts(location.file());
        arch::serial::putc(b':');

        // Print line number as decimal.
        let mut line = location.line();

        if line == 0 {
            arch::serial::putc(b'0');
        } else {
            let mut buf = [0u8; 10];
            let mut i = buf.len();

            while line > 0 {
                i -= 1;
                buf[i] = b'0' + (line % 10) as u8;
                line /= 10;
            }

            for &b in &buf[i..] {
                arch::serial::putc(b);
            }
        }
    }

    arch::serial::puts("\n");

    loop {
        unsafe {
            core::arch::asm!("wfe");
        }
    }
}
