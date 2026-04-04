//! PL011 UART driver (TX only) for QEMU virt machine.
//!
//! Writes directly to the physical UART address. Before the MMU is enabled,
//! this works because the hypervisor/QEMU maps device memory at the physical
//! address. After the MMU is enabled, the caller must ensure the UART PA is
//! mapped.

const UART0_PA: usize = 0x0900_0000;
const UART0_FR: usize = UART0_PA + 0x18;
const TXFF: u32 = 1 << 5;

/// Write a `u64` as 16-digit hex to the UART.
pub fn put_hex(v: u64) {
    for i in 0..16 {
        let shift = 60 - (i * 4);
        let nib = ((v >> shift) & 0xF) as u8;
        let c = match nib {
            0..=9 => b'0' + nib,
            _ => b'a' + (nib - 10),
        };

        putc(c);
    }
}

/// Write a single byte to the UART, waiting if the TX FIFO is full.
pub fn putc(c: u8) {
    // Safety: UART0 is a valid device MMIO address on QEMU virt.
    // Volatile access is required — the hardware register has side effects.
    unsafe {
        let mut timeout: u32 = 100_000;

        while core::ptr::read_volatile(UART0_FR as *const u32) & TXFF != 0 {
            timeout -= 1;

            if timeout == 0 {
                break;
            }
        }

        core::ptr::write_volatile(UART0_PA as *mut u32, c as u32);
    }
}

/// Write a string to the UART, converting `\n` to `\r\n`.
pub fn puts(s: &str) {
    for byte in s.bytes() {
        if byte == b'\n' {
            putc(b'\r');
        }

        putc(byte);
    }
}
