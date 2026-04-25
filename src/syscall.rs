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
