//! Syscall ABI types and encoding constants.
//!
//! D47: SVC #imm16 trap, IPC-optimized registers, two-level numbering.
//! D48: 5 IPC + 20 typed = 25 operations.
//! D49: error signaling, cap-present sentinel, SVC/op-code assignments.

/// IPC operations — nonzero SVC immediates (D48, D49).
///
/// The kernel dispatches IPC operations from ESR_EL1 alone — before
/// reading any GPR (D47).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

// ── IPC register layout (D47, D49) ─────────────────────────────────

/// IPC message registers as seen at the syscall boundary (D47, D49).
///
/// On entry: x0–x3 = data words, x4 = label, x5 = target handle,
/// x6 = user cap handle (u64::MAX = absent), x7 = reply info.
///
/// On exit (Receive/ReplyRecv): x0–x3 = data words, x4 = label,
/// x5 = badge, x6 = user cap slot (u64::MAX = absent),
/// x7 = reply cap handle (u64::MAX = absent).
///
/// D49: IPC errors signaled via ARM64 carry flag in SPSR_EL1.
/// carry clear = success, carry set = error (x0 = error code).
#[derive(Clone, Copy, Debug)]
pub struct IpcRegisters {
    pub data: [u64; 4],
    pub label: u64,
    pub handle_or_badge: u64,
    pub user_cap: u64,
    pub reply_info: u64,
}

/// Typed operation registers as seen at the syscall boundary (D47, D49).
///
/// x4 = operation code, x5 = target cap handle.
/// Remaining registers carry operation-specific arguments.
///
/// D49: errors signaled via negative x0 (x0 < 0 = error code,
/// x0 >= 0 = success/return value). Return values are bounded
/// non-negative integers (slot indices, timestamps, zero-for-void).
#[derive(Clone, Copy, Debug)]
pub struct TypedRegisters {
    pub op_code: u16,
    pub target_handle: u64,
    pub args: [u64; 4],
}

// ── Syscall error domain (D49) ─────────────────────────────────────

/// Kernel error codes returned from syscalls (D49).
///
/// For IPC: delivered via carry flag set + x0. For typed operations:
/// delivered as negative x0. Specific numeric values are an
/// implementation detail of the ABI encoding layer in frame/.
///
/// These correspond to CapError variants and operation-specific
/// failures, translated to the ABI at the frame/ boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyscallError {
    /// Invalid or empty capability handle. Also covers D11 slot-tag
    /// mismatch (stale handle to reused slot) — userspace cannot
    /// distinguish ABA from "never existed," and the recovery is
    /// identical (re-acquire the cap through IPC).
    InvalidCap,
    /// D67: revoked capability (generation mismatch). Distinct from
    /// InvalidCap because the object still exists — the caller's
    /// access was explicitly revoked, not lost to slot reuse.
    StaleCap,
    /// D52: insufficient rights for this operation.
    NoRight,
    /// Wrong object type for this operation.
    WrongType,
    /// D18: Field queue is full (Send error-to-sender).
    QueueFull,
    /// D8: Observer's cap table is full.
    TableFull,
    /// D51: send-once cap already consumed.
    AlreadyConsumed,
    /// D38: clone forbidden for linear types (Time).
    CloneForbidden,
    /// D39: invalid state transition for the Observer.
    InvalidState,
    /// D57: invalid scheduling profile (R + T > 128).
    InvalidProfile,
    /// D60: zero-size Space split.
    ZeroSize,
    /// Insufficient resource for the requested operation.
    InsufficientResource,
    /// D41: merge requires adjacent VA space.
    NotAdjacent,
}

// ── Conversion methods ─────────────────────────────────────────────

impl IpcOperation {
    /// Decode an IPC operation from the SVC immediate value (D49).
    ///
    /// D47: the kernel dispatches IPC operations from ESR_EL1 alone —
    /// before reading any GPR. SVC #1–#5 map directly to Send through
    /// Yield. Any other value is not an IPC operation.
    pub const fn from_svc(imm: u16) -> Option<IpcOperation> {
        match imm {
            1 => Some(IpcOperation::Send),
            2 => Some(IpcOperation::Receive),
            3 => Some(IpcOperation::Call),
            4 => Some(IpcOperation::ReplyRecv),
            5 => Some(IpcOperation::Yield),
            _ => None,
        }
    }

    /// Whether this operation can take the direct-switch fast path (D50).
    ///
    /// D50 condition 1: only Call and ReplyRecv qualify. The sender
    /// voluntarily blocks — Send is fire-and-forget (sender continues),
    /// Receive and Yield don't send.
    pub const fn is_fast_path_eligible(&self) -> bool {
        matches!(self, IpcOperation::Call | IpcOperation::ReplyRecv)
    }
}

impl TypedOperation {
    /// Decode a typed operation from the x4 operation code (D49).
    ///
    /// D49: SVC #0, dense sequential codes 0–19. The kernel reads x4
    /// after determining the SVC immediate was #0.
    pub const fn from_code(code: u16) -> Option<TypedOperation> {
        match code {
            0 => Some(TypedOperation::ObserverResume),
            1 => Some(TypedOperation::ObserverInstallCap),
            2 => Some(TypedOperation::ObserverWriteRegisters),
            3 => Some(TypedOperation::ObserverReadRegisters),
            4 => Some(TypedOperation::ObserverSuspend),
            5 => Some(TypedOperation::ObserverChangeHandler),
            6 => Some(TypedOperation::ObserverSetScheduling),
            7 => Some(TypedOperation::Destroy),
            8 => Some(TypedOperation::Clone),
            9 => Some(TypedOperation::Close),
            10 => Some(TypedOperation::Mint),
            11 => Some(TypedOperation::SpaceSplit),
            12 => Some(TypedOperation::SpaceMerge),
            13 => Some(TypedOperation::CreateField),
            14 => Some(TypedOperation::FieldSplit),
            15 => Some(TypedOperation::TimeSplit),
            16 => Some(TypedOperation::CreatePulsar),
            17 => Some(TypedOperation::ClockRead),
            18 => Some(TypedOperation::CreateObserver),
            19 => Some(TypedOperation::ResourceRequest),
            _ => None,
        }
    }

    /// The object type this operation targets, if type-specific.
    ///
    /// Generic operations (destroy, clone, close, mint) work on any
    /// type — the target type is determined by the cap entry.
    /// Creation operations target the consumed Space cap, not the
    /// type being created.
    pub const fn target_type(&self) -> Option<crate::capability::ObjectType> {
        use crate::capability::ObjectType;

        match self {
            TypedOperation::ObserverResume
            | TypedOperation::ObserverInstallCap
            | TypedOperation::ObserverWriteRegisters
            | TypedOperation::ObserverReadRegisters
            | TypedOperation::ObserverSuspend
            | TypedOperation::ObserverChangeHandler
            | TypedOperation::ObserverSetScheduling => Some(ObjectType::Observer),
            TypedOperation::SpaceSplit | TypedOperation::SpaceMerge => Some(ObjectType::Space),
            TypedOperation::FieldSplit => Some(ObjectType::Field),
            TypedOperation::TimeSplit => Some(ObjectType::Time),
            // Generic ops: type from cap entry. Creation ops: target is Space.
            _ => None,
        }
    }
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

    #[test]
    fn from_svc_roundtrips() {
        for imm in 1..=5u16 {
            let op = IpcOperation::from_svc(imm).unwrap();

            assert_eq!(op as u16, imm);
        }

        assert!(IpcOperation::from_svc(0).is_none());
        assert!(IpcOperation::from_svc(6).is_none());
    }

    #[test]
    fn from_code_roundtrips() {
        for code in 0..=19u16 {
            let op = TypedOperation::from_code(code).unwrap();

            assert_eq!(op as u16, code);
        }

        assert!(TypedOperation::from_code(20).is_none());
    }

    #[test]
    fn fast_path_eligible_only_call_and_reply_recv() {
        assert!(!IpcOperation::Send.is_fast_path_eligible());
        assert!(!IpcOperation::Receive.is_fast_path_eligible());
        assert!(IpcOperation::Call.is_fast_path_eligible());
        assert!(IpcOperation::ReplyRecv.is_fast_path_eligible());
        assert!(!IpcOperation::Yield.is_fast_path_eligible());
    }

    #[test]
    fn observer_ops_target_observer_type() {
        use crate::capability::ObjectType;

        assert_eq!(
            TypedOperation::ObserverResume.target_type(),
            Some(ObjectType::Observer)
        );
        assert_eq!(
            TypedOperation::ObserverSetScheduling.target_type(),
            Some(ObjectType::Observer)
        );
    }

    #[test]
    fn generic_ops_have_no_fixed_target_type() {
        assert!(TypedOperation::Destroy.target_type().is_none());
        assert!(TypedOperation::Clone.target_type().is_none());
        assert!(TypedOperation::Close.target_type().is_none());
        assert!(TypedOperation::Mint.target_type().is_none());
    }
}
