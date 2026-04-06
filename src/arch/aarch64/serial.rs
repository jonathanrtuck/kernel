//! PL011 UART driver (TX only).
//!
//! Writes directly to the physical UART address. Before the MMU is enabled,
//! this works because the hypervisor/QEMU maps device memory at the physical
//! address. After the MMU is enabled, the caller must ensure the UART PA is
//! mapped.
//!
//! All output goes through [`Writer`]'s [`core::fmt::Write`] implementation.

use super::mmio;

use super::platform;

const TX_TIMEOUT: u32 = 1_000_000;
const TXFF: u32 = 1 << 5;

/// PL011 UART output. Use with [`core::fmt::Write`]:
///
/// ```ignore
/// use core::fmt::Write;
/// writeln!(serial::Writer, "hello {}", 42).ok();
/// ```
pub struct Writer;

impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.bytes() {
            if byte == b'\n' {
                putc(b'\r');
            }
            putc(byte);
        }

        Ok(())
    }
}

/// Write a single byte to the UART, waiting if the TX FIFO is full.
fn putc(c: u8) {
    let mut timeout = TX_TIMEOUT;

    while mmio::read32(uart0_fr()) & TXFF != 0 {
        timeout -= 1;
        if timeout == 0 {
            break;
        }
    }

    // Write even if the FIFO is full after timeout — losing a character
    // is better than hanging the kernel (this is often a panic dump path).
    mmio::write32(uart0_dr(), c as u32);
}

#[inline(always)]
fn uart0_dr() -> usize {
    platform::UART_BASE
}

#[inline(always)]
fn uart0_fr() -> usize {
    platform::UART_BASE + 0x18
}
