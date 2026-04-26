//! Kernel-level configuration constants.
//!
//! Policy decisions and capacity limits that are independent of the target
//! architecture. Platform-specific values (device addresses, RAM layout)
//! live in `arch/aarch64/platform.rs`.

/// Kernel stack size per core. — link.ld sync: `.bss.stack`
///
/// 256 KiB: KernelState contains IrqRoutingTable (~24 KiB). In debug
/// builds, the compiler may create multiple unoptimized copies of large
/// structs on the stack during construction and moves.
pub const KERNEL_STACK_SIZE: usize = 256 * 1024;

/// Maximum number of CPU cores.
pub const MAX_CORES: usize = 8;

/// Default Field queue capacity in messages.
///
/// D13: bounded queue. This is the initial capacity for Fields created
/// without an explicit size. The actual capacity is limited by the
/// Space consumed at creation (D32).
pub const DEFAULT_QUEUE_CAPACITY: u32 = 16;
