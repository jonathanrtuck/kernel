//! Fault types and delivery mechanism.
//!
//! D12: fault delegation to userspace pager Observers.
//! D40: per-fault-type resolution via typed kernel syscalls.
//! D61: four fault types with specific data word assignments.
//!      Faults ARE IPC — kernel-as-sender to handler Field (D7).

use crate::capability::{Badge, Rights, TransferredCap};
use crate::field;

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
#[derive(Clone, Copy)]
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

// ── FaultType methods ──────────────────────────────────────────────

impl FaultType {
    /// Return the kernel-reserved label for this fault type (D61).
    ///
    /// Labels are in the 0xFFFF_FFFF_FFFF_xxxx range to avoid
    /// collision with user-chosen labels. Values are provisional
    /// pending a label assignment derivation.
    pub const fn label(&self) -> u64 {
        match self {
            FaultType::VmFault { .. } => field::LABEL_VM_FAULT,
            FaultType::ResourceRequest { .. } => field::LABEL_RESOURCE_REQUEST,
            FaultType::CapTableFull => field::LABEL_CAP_TABLE_FULL,
            FaultType::HardwareException { .. } => field::LABEL_HARDWARE_EXCEPTION,
        }
    }

    /// Encode this fault into the D28 message data words (D61).
    ///
    /// D61 data word assignments:
    /// | Type               | data[0]         | data[1]     | data[2]     | data[3] |
    /// | VM_FAULT           | Space slot idx  | byte offset | access type | 0       |
    /// | RESOURCE_REQUEST   | resource type   | quantity    | 0           | 0       |
    /// | CAP_TABLE_FULL     | 0               | 0           | 0           | 0       |
    /// | HARDWARE_EXCEPTION | ESR_EL1         | ELR_EL1     | FAR_EL1     | 0       |
    const fn data_words(&self) -> [u64; 4] {
        match *self {
            FaultType::VmFault {
                space_slot,
                byte_offset,
                access,
            } => [space_slot as u64, byte_offset, access as u64, 0],
            FaultType::ResourceRequest { resource, quantity } => [resource as u64, quantity, 0, 0],
            FaultType::CapTableFull => [0, 0, 0, 0],
            FaultType::HardwareException {
                esr_el1,
                elr_el1,
                far_el1,
            } => [esr_el1, elr_el1, far_el1, 0],
        }
    }

    /// Construct the fault message for delivery to the handler Field.
    ///
    /// D61: fault delivery is standard queued-Field IPC with the kernel
    /// as sender. No separate mechanism. The message carries:
    /// - badge from the D21 handler cap entry
    /// - fault-type label
    /// - data words per the D61 table
    /// - Observer handle cap with 5 of 9 rights (D61)
    ///
    /// The 5-right subset (resume, destroy, install_cap, write_registers,
    /// read_registers) is the minimum the handler needs to inspect and
    /// resolve the fault. Excludes suspend, change_handler,
    /// modify_scheduling, and clone — the handler does not need these
    /// for fault resolution.
    ///
    /// D26 divergence from prior art: VM_FAULT carries Space slot index
    /// + byte offset instead of raw VA (VA is kernel-internal under D26).
    pub fn to_message(&self, handler_badge: Badge, observer_cap: TransferredCap) -> field::Message {
        debug_assert!(
            observer_cap.rights == Rights::FAULT_OBSERVER,
            "D61: fault message Observer cap must carry exactly FAULT_OBSERVER rights"
        );

        field::Message {
            data: self.data_words(),
            label: self.label(),
            badge: handler_badge,
            user_cap: Some(observer_cap),
            reply_cap: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vm_fault_label() {
        let fault = FaultType::VmFault {
            space_slot: 3,
            byte_offset: 0x100,
            access: AccessType::Read,
        };

        assert_eq!(fault.label(), field::LABEL_VM_FAULT);
    }

    #[test]
    fn vm_fault_data_words() {
        let fault = FaultType::VmFault {
            space_slot: 5,
            byte_offset: 0x200,
            access: AccessType::Write,
        };
        let words = fault.data_words();

        assert_eq!(words[0], 5);
        assert_eq!(words[1], 0x200);
        assert_eq!(words[2], 1); // Write = 1
        assert_eq!(words[3], 0);
    }

    #[test]
    fn cap_table_full_is_all_zeros() {
        let words = FaultType::CapTableFull.data_words();

        assert_eq!(words, [0, 0, 0, 0]);
    }

    #[test]
    fn all_fault_types_have_distinct_labels() {
        let faults: [FaultType; 4] = [
            FaultType::VmFault {
                space_slot: 0,
                byte_offset: 0,
                access: AccessType::Read,
            },
            FaultType::ResourceRequest {
                resource: ResourceType::Space,
                quantity: 0,
            },
            FaultType::CapTableFull,
            FaultType::HardwareException {
                esr_el1: 0,
                elr_el1: 0,
                far_el1: 0,
            },
        ];

        for (i, a) in faults.iter().enumerate() {
            for (j, b) in faults.iter().enumerate() {
                if i != j {
                    assert_ne!(a.label(), b.label());
                }
            }
        }
    }
}
