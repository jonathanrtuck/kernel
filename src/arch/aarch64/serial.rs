//! PL011 UART driver (TX only).
//!
//! Writes directly to the physical UART address. Before the MMU is enabled,
//! this works because the hypervisor/QEMU maps device memory at the physical
//! address. After the MMU is enabled, the caller must ensure the UART PA is
//! mapped.

use super::mmio;

const TX_TIMEOUT: u32 = 1_000_000;
const TXFF: u32 = 1 << 5;
const UART0_PA: usize = 0x0900_0000;

#[inline(always)]
fn uart0_dr() -> usize {
    UART0_PA
}

#[inline(always)]
fn uart0_fr() -> usize {
    UART0_PA + 0x18
}

/// Write a single byte to the UART, waiting if the TX FIFO is full.
pub fn putc(c: u8) {
    let mut timeout = TX_TIMEOUT;

    while mmio::read32(uart0_fr()) & TXFF != 0 {
        timeout -= 1;

        if timeout == 0 {
            break;
        }
    }

    mmio::write32(uart0_dr(), c as u32);
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
