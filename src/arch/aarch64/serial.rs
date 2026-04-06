//! PL011 UART driver (TX only).
//!
//! Writes directly to the physical UART address. Before the MMU is enabled,
//! this works because the hypervisor/QEMU maps device memory at the physical
//! address. After the MMU is enabled, the caller must ensure the UART PA is
//! mapped.
//!
//! All output goes through [`Uart`]'s [`core::fmt::Write`] implementation.

use super::mmio;

const TX_TIMEOUT: u32 = 1_000_000;
const TXFF: u32 = 1 << 5;
const UART0_PA: usize = 0x0900_0000;

/// PL011 UART output. Use with [`core::fmt::Write`]:
///
/// ```ignore
/// use core::fmt::Write;
/// writeln!(serial::Uart, "hello {}", 42).ok();
/// ```
pub struct Uart;

impl core::fmt::Write for Uart {
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

    mmio::write32(uart0_dr(), c as u32);
}

#[inline(always)]
fn uart0_dr() -> usize {
    UART0_PA
}

#[inline(always)]
fn uart0_fr() -> usize {
    UART0_PA + 0x18
}
