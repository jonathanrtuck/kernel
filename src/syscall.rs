//! Syscall ABI types and encoding constants.
//!
//! D47: SVC #imm16 trap, IPC-optimized registers, two-level numbering.
//! D48: 5 IPC + 20 typed = 25 operations.
//! D49: error signaling, cap-present sentinel, SVC/op-code assignments.

/// IPC operations — nonzero SVC immediates (D48, D49).
///
/// The kernel dispatches IPC operations from ESR_EL1 alone — before
/// reading any GPR (D47).
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum IpcOperation {
    Send = 1,
    Receive = 2,
    Call = 3,
    ReplyRecv = 4,
    Yield = 5,
}

/// Typed kernel operations — SVC #0, operation code in x4 (D48, D49).
///
/// Dense table dispatch. Grouped by type for self-documentation.
/// Future rights-mask additions append to their respective type groups.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum TypedOperation {
    // Observer operations (D39 — nine rights)
    ObserverResume = 0,
    ObserverInstallCap = 1,
    ObserverWriteRegisters = 2,
    ObserverReadRegisters = 3,
    ObserverSuspend = 4,
    ObserverChangeHandler = 5,
    ObserverSetScheduling = 6,

    // Generic cap operations (cross-type)
    Destroy = 7,
    Clone = 8,
    Close = 9,
    Mint = 10,

    // Space operations (D41)
    SpaceSplit = 11,
    SpaceMerge = 12,

    // Field operations (D32, D45)
    CreateField = 13,
    FieldSplit = 14,

    // Time operations (D38)
    TimeSplit = 15,

    // Pulsar operations (D44, D62)
    CreatePulsar = 16,
    ClockRead = 17,

    // Observer creation (D35)
    CreateObserver = 18,

    // Resource acquisition (D31)
    ResourceRequest = 19,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_operations_are_contiguous_1_through_5() {
        assert_eq!(IpcOperation::Send as u16, 1);
        assert_eq!(IpcOperation::Receive as u16, 2);
        assert_eq!(IpcOperation::Call as u16, 3);
        assert_eq!(IpcOperation::ReplyRecv as u16, 4);
        assert_eq!(IpcOperation::Yield as u16, 5);
    }

    #[test]
    fn typed_operations_are_contiguous_0_through_19() {
        assert_eq!(TypedOperation::ObserverResume as u16, 0);
        assert_eq!(TypedOperation::ResourceRequest as u16, 19);
    }

    #[test]
    fn total_operations_is_25() {
        let ipc_count = IpcOperation::Yield as u16;
        let typed_count = TypedOperation::ResourceRequest as u16 + 1;

        assert_eq!(ipc_count + typed_count, 25, "D48: 5 IPC + 20 typed = 25");
    }
}
