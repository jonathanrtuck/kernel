//! Fault types and delivery mechanism.
//!
//! D12: fault delegation to userspace pager Observers.
//! D40: per-fault-type resolution via typed kernel syscalls.
//! D61: four fault types with specific data word assignments.
//!      Faults ARE IPC — kernel-as-sender to handler Field (D7).

/// Memory access type for VM faults (D61).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum AccessType {
    Read = 0,
    Write = 1,
    Execute = 2,
}

/// Resource type for resource-request faults (D31, D61).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum ResourceType {
    Space = 0,
    Time = 1,
}

/// Fault type with per-type data word assignments (D61).
///
/// All fault messages carry: badge from D21 handler cap, fault-type label,
/// and an Observer handle cap with 5 of 9 rights — resume, destroy,
/// install_cap, write_registers, read_registers (D61).
///
/// Delivery is standard queued-Field IPC with the kernel as sender.
/// No separate mechanism (D61).
pub enum FaultType {
    /// Page fault: Observer accessed memory outside owned Spaces (D61).
    /// D26 makes VA kernel-internal → carry Space slot index + byte offset.
    VmFault {
        space_slot: u32,
        byte_offset: u64,
        access: AccessType,
    },

    /// Observer requests resources via D31 pager chain.
    ResourceRequest {
        resource: ResourceType,
        quantity: u64,
    },

    /// Observer's cap table is full (D8 growth protocol, D40).
    CapTableFull,

    /// Hardware exception not handled by the kernel (D61).
    HardwareException {
        esr_el1: u64,
        elr_el1: u64,
        far_el1: u64,
    },
}
