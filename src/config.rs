//! Kernel-level configuration constants.
//!
//! Policy decisions and capacity limits that are independent of the target
//! architecture. Platform-specific values (device addresses, RAM layout)
//! live in `arch/aarch64/platform.rs`.

/// Default Field queue capacity in messages.
///
/// D13: bounded queue. This is the initial capacity for Fields created
/// without an explicit size. The actual capacity is limited by the
/// Space consumed at creation (D32).
pub const DEFAULT_QUEUE_CAPACITY: u32 = 16;

/// Kernel stack size per core. — link.ld sync: `.bss.stack`
///
/// 256 KiB: KernelState contains IrqRoutingTable (~24 KiB). In debug
/// builds, the compiler may create multiple unoptimized copies of large
/// structs on the stack during construction and moves.
pub const KERNEL_STACK_SIZE: usize = 256 * 1024;

/// Maximum physical pages the bitmap allocator can track.
///
/// Determines the upper bound on system RAM. Two bitmaps (physical +
/// VA) use `MAX_BITMAP_PAGES / 8` bytes each of BSS.
///
/// | Granule | RAM limit | BSS per bitmap |
/// |---------|-----------|----------------|
/// | 4 KiB   | 1 GiB     | 32 KiB         |
/// | 16 KiB  | 4 GiB     | 32 KiB         |
/// | 64 KiB  | 16 GiB    | 32 KiB         |
///
/// Increase if the target system has more RAM. The boot-time assert
/// in `PageBitmap::new` catches undersizing immediately.
pub const MAX_BITMAP_PAGES: usize = 32 * 1024 * 8;

/// Maximum number of CPU cores.
pub const MAX_CORES: usize = 8;
