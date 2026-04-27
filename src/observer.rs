//! Observer: schedulable execution unit.
//!
//! D6:  single execution unit — one register state, one PC.
//! D14: capability-held kernel object type.
//! D20: per-Observer fault handler.
//! D21: fault handler is a cap-table entry at reserved slot.
//! D23: Observer handles are clonable.
//! D30: one or more Time caps in regular cap-table slots.
//! D39: nine rights — resume, destroy, install-cap, write-registers, clone,
//!      read-registers, suspend, change-handler, modify-scheduling.
//! D42: three-value scheduling profile — responsiveness, throughput, precision.
//!      No priority integer. modify-scheduling gates these values.
//! D43: minimum schema settled. Metadata struct + structural backing split.
//!      No base/effective scheduling split (inheritance is userspace policy).
//!      Core assignment is transient (no struct field).
//!      Reply field is a cap-table reserved slot (D21 pattern).
//! D52: rights — all nine (9 bits).
//! D56: placement mechanism settled. Scored placement reads R/T/P to match
//!      Observer to core. Cache affinity tracked per-core, not per-Observer.
//!      No core ID field here (D43 preserved).
//! D57: budget = 128. Store R and T as u8; P = 128 - R - T (derived).
//!      Default profile: R=43, T=43, P=42.
//!      Self-reference cap at reserved slot 2 with full rights.
//! D66: per-Observer clock_access: bool. Kernel writes
//!      CNTKCTL_EL1.EL0VCTEN on every context switch.
//! D67: generation counter for revocation.

use crate::arena::ObjectId;
use crate::capability;
use crate::field::Field;
use crate::syscall;
use core::ptr::NonNull;
use core::sync::atomic::AtomicU64;

// ── Scheduling constants (D42, D57) ─────────────────────────────────

/// Scheduling budget (D57). R + T <= SCHEDULING_BUDGET.
/// Precision is derived: P = SCHEDULING_BUDGET - R - T.
pub const SCHEDULING_BUDGET: u8 = 128;

/// Default responsiveness (D57). Closest equal distribution on 128.
pub const DEFAULT_RESPONSIVENESS: u8 = 43;

/// Default throughput (D57).
pub const DEFAULT_THROUGHPUT: u8 = 43;

// ---------------------------------------------------------------------------
// Register state handle — opaque reference into structural backing.
// ---------------------------------------------------------------------------

/// Opaque handle to saved register state in structural backing (D6/D35/D32).
///
/// The Observer carries this without knowing the register layout. Arch
/// context-switch code (inside the framekernel core) resolves it to the
/// concrete arch-specific layout for save/restore. This keeps arch types
/// confined to the core boundary (journal 023).
#[derive(Clone, Copy)]
pub struct RegisterStateHandle(#[allow(dead_code)] NonNull<u8>);

#[cfg(any(target_os = "none", test))]
impl RegisterStateHandle {
    pub(crate) fn new(ptr: NonNull<u8>) -> Self {
        RegisterStateHandle(ptr)
    }

    pub(crate) fn as_ptr(&self) -> NonNull<u8> {
        self.0
    }
}

// ---------------------------------------------------------------------------
// Scheduling state — D39 five-state machine.
// ---------------------------------------------------------------------------

/// Primary Observer lifecycle state (D39).
///
/// Transitions:
/// - Inert → Runnable: resume (first start, D35)
/// - Runnable → Blocked: receive() with empty queue (D13)
/// - Blocked → Runnable: message arrives on waited Field
/// - Runnable → Faulted: hardware fault (page fault, invalid cap, etc.)
/// - Faulted → Runnable: resume (after handler resolves, D12)
///
/// Suspension (D39) is orthogonal — tracked by [`Observer::suspended`].
pub enum PrimaryState {
    Inert,
    Runnable,
    Blocked,
    Faulted,
}

// ---------------------------------------------------------------------------
// Wait-state linkage — D18 intrusive list, D19 multi-field accommodation.
// ---------------------------------------------------------------------------

/// Node linking an Observer into a Field's waiters or pending list.
///
/// In [`WaitState::Single`], one entry is stored inline (zero allocation).
/// In [`WaitState::Multi`], entries are allocated (source unsettled — D43
/// defers to downstream derivation).
pub struct WaitEntry {
    pub observer: NonNull<Observer>,
    pub field: NonNull<Field>,
    pub prev: Option<NonNull<WaitEntry>>,
    pub next: Option<NonNull<WaitEntry>>,
}

/// Wait-state for a blocked or fault-pending Observer (D18/D19).
///
/// Only one variant is active at a time (D18: states are mutually exclusive).
/// [`WaitState::None`] when the Observer is not waiting on any Field.
pub enum WaitState {
    None,
    Single(WaitEntry),
    Multi { head: NonNull<WaitEntry> },
}

// ---------------------------------------------------------------------------
// Saved syscall context for cap table growth replay (D-3.1b, D40).
// ---------------------------------------------------------------------------

/// Saved syscall context for transparent retry after cap table growth (D-3.1b).
///
/// When an Observer's cap table is full during a syscall that needs a new slot,
/// the kernel saves the operation context here, delivers a CapTableFull fault
/// to the handler, and blocks the Observer. After the handler grows the table
/// and resumes the Observer, the kernel replays the saved operation transparently
/// -- the Observer never sees the fault.
#[derive(Clone, Copy)]
pub enum SavedSyscallContext {
    /// No pending replay.
    None,
    /// A typed operation that was interrupted by cap table full (D-3.1b).
    /// The Observer's RegisterState still contains the original arguments.
    Typed(syscall::TypedOperation),
}

// ---------------------------------------------------------------------------
// Observer metadata struct — lives in root Space (D32).
// ---------------------------------------------------------------------------

/// The condition under which compute (Time) executes instructions within
/// specific memory (Space).
///
/// Each Observer holds capabilities to one or more Spaces and one or more Times
/// (D30). "Process" is a userspace convention (group of Observers sharing Space
/// caps).
///
/// Physically two regions (D32/D35):
/// - This metadata struct: root Space, ~80 bytes.
/// - Structural backing: consumed Space (register save area, cap table pages,
///   L0 page table root). Referenced via opaque handles/pointers.
///
/// Not in this struct (D43):
/// - Fault handler: cap-table reserved slot (D21).
/// - Reply field: cap-table reserved slot (D16 + D21 pattern).
/// - Time caps: cap-table regular slots (D30).
/// - Algorithm-specific scheduler state: per-core (D2).
/// - Core assignment: transient, re-decided per runnable transition (D31/D56).
/// - Cache affinity: per-core tracker with decay (D56).
pub struct Observer {
    /// Arena slot identifier (D100). Stored on the struct so fault
    /// delivery can construct the TransferredCap without resolving
    /// the self-cap at slot 2.
    pub object_id: ObjectId,

    /// Opaque handle to saved register context in structural backing.
    /// Arch core code resolves this for save/restore on context switch.
    pub register_state: RegisterStateHandle,

    /// Per-Observer hardware ASID (D101). Assigned at creation from the
    /// kernel's AsidAllocator. Encoded in TTBR0 bits[63:48] and used as
    /// the TLB invalidation key on Space unmap.
    ///
    /// May be re-assigned on context switch if `asid_generation` is stale
    /// (the allocator wrapped and this Observer's hardware ASID now aliases
    /// a newer Observer). See `refresh_observer_asid`.
    pub asid: u16,

    /// ASID generation epoch (D101). Compared against
    /// `KernelState::asid_generation` on context switch. If they differ,
    /// this Observer's hardware ASID is stale and must be re-allocated
    /// before writing TTBR0.
    pub asid_generation: u64,

    /// Physical address of the per-Observer page table root (D5/D26).
    /// Hot path: loaded into the hardware translation base on context switch.
    /// Encodes ASID in bits[63:48] via `make_ttbr0(asid, l1_root_pa)`.
    pub page_table_root: u64,

    /// Pointer to the flat capability array in structural backing (D4/D8).
    /// Hot path: indexed on every syscall to resolve capability handles.
    /// Updatable: table can grow via D8 table-full fault. Always valid.
    pub cap_table: NonNull<capability::Entry>,

    /// Number of entries in the cap table array (D8, D77).
    ///
    /// Required for bounds-checking handle resolution on the hot path.
    /// D8 put capacity on Table, but the hot path indexes through
    /// Observer's raw pointer — it needs the bound here. Updated on
    /// table growth (D8 table-full fault handler provides more Space).
    pub cap_table_capacity: u32,

    /// Head of the intrusive freelist through empty cap table entries (D8, D96).
    ///
    /// Empty entries store the next-free index in `stored_generation`.
    /// None when all user slots are occupied — triggers table-full fault (D40).
    /// Updated on cap extraction (move out → freed slot enters list) and
    /// cap installation (incoming cap → slot removed from list).
    pub cap_table_free_head: Option<u32>,

    /// Number of occupied entries in the cap table (D8).
    pub cap_table_count: u32,

    /// Primary lifecycle state (D39).
    pub state: PrimaryState,

    /// External suspension overlay (D39). Co-occurs with Blocked or Faulted.
    /// Resume clears this; underlying state remains.
    pub suspended: bool,

    /// Cached sum of held Time compute units (D30/D31/D36).
    /// Hot path: read by per-core scheduler.
    /// Cold path: updated on Time cap install/remove.
    pub compute_aggregate: u32,

    /// Three-value scheduling profile (D42, D57).
    /// Budget = 128. Store R and T; derive P = 128 - R - T.
    /// R + T <= SCHEDULING_BUDGET enforced at creation and modification.
    /// One set of values — no base/effective split (D43).
    /// Modified via modify-scheduling right (D39).
    pub responsiveness: u8,
    pub throughput: u8,

    /// Per-Observer clock access flag (D66).
    /// Kernel writes CNTKCTL_EL1.EL0VCTEN on every context switch.
    /// True = Observer can read CNTVCT_EL0 directly (~1 cycle).
    /// False = must use clock_read() typed kernel operation (D48).
    pub clock_access: bool,

    /// Wait-state linkage for blocked/pending states (D18/D19).
    pub wait_state: WaitState,

    /// Saved syscall context for cap table growth replay (D-3.1b, D40).
    ///
    /// Set when a syscall triggers CapTableFull fault. The Observer's
    /// RegisterState still contains the original syscall arguments.
    /// On resume after table growth, the kernel re-dispatches from here.
    pub saved_syscall: SavedSyscallContext,

    /// D32/D98: VA base of the Space consumed at creation.
    /// Used by Destroy to reconstruct the Space cap (reverse type conversion).
    pub backing_va_base: usize,

    /// D32/D98: size in bytes of the Space consumed at creation.
    pub backing_size: usize,

    /// Outstanding capability references to this Observer (D11/D33).
    /// Decremented on cap close; object eligible for destruction at zero.
    pub refcount: u32,

    /// Revocation generation counter (D67). AtomicU64 per D67.
    pub generation: AtomicU64,
}

// ── Error types ────────────────────────────────────────────────────

/// Errors from Observer operations (D39).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObserverError {
    /// State machine violation — the requested transition is not valid
    /// from the Observer's current state. D39 five-state machine with
    /// explicit transition rules.
    InvalidTransition,
    /// D57: R + T > SCHEDULING_BUDGET. Invalid scheduling profile.
    InvalidProfile,
}

// ── Observer methods ───────────────────────────────────────────────

impl PrimaryState {
    /// Whether this state means the Observer is not currently scheduled.
    pub const fn is_stopped(&self) -> bool {
        matches!(self, PrimaryState::Inert | PrimaryState::Faulted)
    }
}

impl Observer {
    /// Validate a scheduling profile before applying it.
    ///
    /// D57: R + T must not exceed SCHEDULING_BUDGET (128). The kernel
    /// enforces this at creation and on every modify-scheduling call.
    /// Storing two values and deriving the third (P = 128 - R - T)
    /// eliminates the three-way sum invariant by construction — invalid
    /// states are unrepresentable (A1 applied to data representation).
    pub fn validate_profile(responsiveness: u8, throughput: u8) -> Result<(), ObserverError> {
        if responsiveness as u16 + throughput as u16 > SCHEDULING_BUDGET as u16 {
            return Err(ObserverError::InvalidProfile);
        }

        Ok(())
    }

    /// Derived precision value (D57).
    ///
    /// P = SCHEDULING_BUDGET - R - T. The scheduler reads this to
    /// determine hard-RT eligibility (D42 EDF admission).
    pub const fn precision(&self) -> u8 {
        SCHEDULING_BUDGET - self.responsiveness - self.throughput
    }

    /// Transition from a stopped state to Runnable (D14, D35, D39).
    ///
    /// Valid from: Inert (first start), Faulted (after handler resolves).
    /// Also clears the suspension flag if set.
    ///
    /// The caller must enqueue this Observer into the per-core scheduler
    /// (D2) and make a placement decision (D56) after this returns.
    ///
    /// Security: requires RESUME right (D39).
    pub fn resume(&mut self) -> Result<(), ObserverError> {
        match self.state {
            PrimaryState::Inert | PrimaryState::Faulted => {
                self.state = PrimaryState::Runnable;
                self.suspended = false;

                Ok(())
            }
            _ => Err(ObserverError::InvalidTransition),
        }
    }

    /// Set the external suspension overlay (D39).
    ///
    /// Can co-occur with Blocked or Faulted. The Observer is removed
    /// from the run queue (if Runnable) or remains in its current
    /// non-runnable state. Resume clears the suspension; the underlying
    /// state remains.
    ///
    /// Use cases: debugging, checkpointing, resource pressure (A3).
    /// Security: requires SUSPEND right (D39).
    pub fn suspend(&mut self) {
        self.suspended = true;
    }

    /// Transition to Blocked when waiting on a Field (D13).
    ///
    /// Valid from Runnable only. The Observer is removed from the run
    /// queue and linked into the Field's waiters list via wait_state.
    pub fn block(&mut self, wait_state: WaitState) -> Result<(), ObserverError> {
        match self.state {
            PrimaryState::Runnable => {
                self.state = PrimaryState::Blocked;
                self.wait_state = wait_state;

                Ok(())
            }
            _ => Err(ObserverError::InvalidTransition),
        }
    }

    /// Transition from Blocked to Runnable when a message arrives.
    ///
    /// D39: suspension co-occurs with Blocked. When a message arrives
    /// for a blocked+suspended Observer, the blocking condition IS
    /// resolved (primary state → Runnable), but the suspension overlay
    /// stays. The caller checks `self.suspended` afterward: if true,
    /// do NOT enqueue into the scheduler — the Observer remains off
    /// the run queue until explicitly resumed.
    ///
    /// Returns `true` if the Observer should be enqueued (not suspended),
    /// `false` if it transitioned to Runnable but remains suspended.
    pub fn unblock(&mut self) -> Result<bool, ObserverError> {
        match self.state {
            PrimaryState::Blocked => {
                self.state = PrimaryState::Runnable;
                self.wait_state = WaitState::None;

                Ok(!self.suspended)
            }
            _ => Err(ObserverError::InvalidTransition),
        }
    }

    /// Transition to Faulted (D12, D39, D61).
    ///
    /// The Observer is descheduled. Fault delivery proceeds via the
    /// handler Field at cap-table slot 0 (D21). The Observer's
    /// wait_state may be reused for D18 pending-list linkage if the
    /// handler Field is full.
    pub fn fault(&mut self) -> Result<(), ObserverError> {
        match self.state {
            PrimaryState::Runnable => {
                self.state = PrimaryState::Faulted;

                Ok(())
            }
            _ => Err(ObserverError::InvalidTransition),
        }
    }

    /// Update the scheduling profile (D39, D42, D57).
    ///
    /// Validates R + T <= 128 before applying. The cached precision
    /// (P = 128 - R - T) is derived, not stored. The per-core scheduler
    /// reads the new values on the next scheduling decision.
    ///
    /// D43: one set of values — no base/effective split. Scheduling
    /// adjustment during IPC (priority inheritance) is a userspace
    /// policy concern via modify-scheduling, not kernel policy.
    ///
    /// Security: requires MODIFY_SCHEDULING right (D39).
    pub fn set_scheduling(
        &mut self,
        responsiveness: u8,
        throughput: u8,
    ) -> Result<(), ObserverError> {
        Self::validate_profile(responsiveness, throughput)?;

        self.responsiveness = responsiveness;
        self.throughput = throughput;

        Ok(())
    }

    /// Add compute units to the cached aggregate (D30).
    ///
    /// Called when a Time cap is installed into this Observer's table.
    /// D36: the scheduler reads the aggregate on the hot path to
    /// determine the Observer's scheduling quantum.
    pub fn add_compute(&mut self, units: u32) {
        self.compute_aggregate += units;
    }

    /// Remove compute units from the cached aggregate (D30).
    ///
    /// Called when a Time cap is removed from this Observer's table
    /// (close, transfer via IPC, or destroy cascade).
    pub fn remove_compute(&mut self, units: u32) {
        self.compute_aggregate = self.compute_aggregate.saturating_sub(units);
    }

    /// Construct a temporary Table from this Observer's raw cap-table
    /// fields, run `f`, and write back the mutable fields (free_head,
    /// count). Hot-path cap resolution bypasses this — it reads entries
    /// and capacity directly (D77). Cold-path operations (D96 cap
    /// transfer, D11 close, D33 cascade) use this to delegate to Table
    /// methods without reimplementing freelist logic.
    pub fn with_cap_table<R>(&mut self, f: impl FnOnce(&mut capability::Table) -> R) -> R {
        let mut table = capability::Table {
            entries: self.cap_table,
            capacity: self.cap_table_capacity,
            free_head: self.cap_table_free_head,
            count: self.cap_table_count,
        };
        let result = f(&mut table);

        self.cap_table_free_head = table.free_head;
        self.cap_table_count = table.count;

        result
    }

    /// D67: atomically increment the generation counter, revoking all
    /// capabilities that stored the previous generation value.
    pub fn revoke(&self) {
        self.generation
            .fetch_add(1, core::sync::atomic::Ordering::Release);
    }
}

#[cfg(test)]
impl Observer {
    /// Base test Observer: Runnable, dangling pointers, 100 compute.
    ///
    /// Use this for tests that only need a valid Observer struct without
    /// interacting with register state or capabilities. Override individual
    /// fields after construction for specific test scenarios.
    pub(crate) fn test_default() -> Self {
        Observer {
            object_id: ObjectId(0),
            asid: 0,
            asid_generation: 0,
            register_state: RegisterStateHandle::new(NonNull::dangling()),
            page_table_root: 0,
            cap_table: NonNull::dangling(),
            cap_table_capacity: 0,
            cap_table_free_head: None,
            cap_table_count: 0,
            state: PrimaryState::Runnable,
            suspended: false,
            compute_aggregate: 100,
            responsiveness: DEFAULT_RESPONSIVENESS,
            throughput: DEFAULT_THROUGHPUT,
            clock_access: false,
            wait_state: WaitState::None,
            saved_syscall: SavedSyscallContext::None,
            refcount: 1,
            generation: AtomicU64::new(0),
            backing_va_base: 0,
            backing_size: 0,
        }
    }

    /// Test Observer with a real register state allocation.
    ///
    /// Use for IPC dispatch tests that read/write register contexts.
    pub(crate) fn test_with_registers() -> Self {
        let rs = crate::frame::cores::alloc_test_register_state();
        let mut obs = Self::test_default();

        obs.register_state = RegisterStateHandle::new(rs);

        obs
    }

    /// Test Observer with registers and an allocated cap table.
    ///
    /// Freelist is initialized from SLOT_USER_START. Use for tests that
    /// install, transfer, or close capabilities.
    pub(crate) fn test_with_cap_table(capacity: u32) -> Self {
        let entries =
            crate::frame::capabilities::allocate_cap_table(capacity).expect("test cap table alloc");
        let mut obs = Self::test_with_registers();

        obs.cap_table = entries;
        obs.cap_table_capacity = capacity;
        obs.cap_table_free_head = Some(crate::capability::SLOT_USER_START);

        obs
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observer_layout() {
        assert_eq!(core::mem::size_of::<Observer>(), 136);
    }

    #[test]
    fn default_profile_is_valid() {
        assert!(Observer::validate_profile(DEFAULT_RESPONSIVENESS, DEFAULT_THROUGHPUT).is_ok());
    }

    #[test]
    fn profile_rejects_overflow() {
        assert!(Observer::validate_profile(100, 100).is_err());
        assert!(Observer::validate_profile(129, 0).is_err());
    }

    #[test]
    fn precision_is_derived() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Inert;
        observer.compute_aggregate = 0;
        observer.responsiveness = 43;
        observer.throughput = 43;

        assert_eq!(observer.precision(), 42);
    }

    #[test]
    fn resume_from_inert() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Inert;
        observer.compute_aggregate = 0;

        assert!(observer.resume().is_ok());
        assert!(matches!(observer.state, PrimaryState::Runnable));
    }

    #[test]
    fn resume_from_runnable_fails() {
        let mut observer = Observer::test_default();

        observer.compute_aggregate = 0;

        assert!(observer.resume().is_err());
    }

    #[test]
    fn compute_aggregate_tracking() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Inert;
        observer.compute_aggregate = 0;

        observer.add_compute(100);
        observer.add_compute(50);

        assert_eq!(observer.compute_aggregate, 150);

        observer.remove_compute(30);

        assert_eq!(observer.compute_aggregate, 120);
    }

    #[test]
    fn unblock_while_suspended_transitions_but_signals_no_enqueue() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Blocked;
        observer.suspended = true;
        observer.compute_aggregate = 0;

        let should_enqueue = observer.unblock().unwrap();

        assert!(!should_enqueue, "suspended Observer should not be enqueued");
        assert!(matches!(observer.state, PrimaryState::Runnable));
        assert!(observer.suspended, "suspension overlay preserved");
    }

    #[test]
    fn unblock_without_suspension_signals_enqueue() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Blocked;
        observer.compute_aggregate = 0;

        let should_enqueue = observer.unblock().unwrap();

        assert!(should_enqueue, "non-suspended Observer should be enqueued");
        assert!(matches!(observer.state, PrimaryState::Runnable));
    }

    // ── block() coverage ─────────────────────────────────────────────

    #[test]
    fn block_from_runnable_sets_blocked_and_wait_state() {
        let mut observer = Observer::test_default();
        let wait = WaitState::Single(WaitEntry {
            observer: NonNull::from(&observer),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        });

        assert!(observer.block(wait).is_ok());
        assert!(matches!(observer.state, PrimaryState::Blocked));
        assert!(matches!(observer.wait_state, WaitState::Single(_)));
    }

    #[test]
    fn block_from_inert_fails() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Inert;

        assert_eq!(
            observer.block(WaitState::None).unwrap_err(),
            ObserverError::InvalidTransition
        );
    }

    #[test]
    fn block_from_blocked_fails() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Blocked;

        assert_eq!(
            observer.block(WaitState::None).unwrap_err(),
            ObserverError::InvalidTransition
        );
    }

    #[test]
    fn block_from_faulted_fails() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Faulted;

        assert_eq!(
            observer.block(WaitState::None).unwrap_err(),
            ObserverError::InvalidTransition
        );
    }

    #[test]
    fn block_then_unblock_roundtrip() {
        let mut observer = Observer::test_default();

        observer.block(WaitState::None).unwrap();

        assert!(matches!(observer.state, PrimaryState::Blocked));

        let should_enqueue = observer.unblock().unwrap();

        assert!(should_enqueue);
        assert!(matches!(observer.state, PrimaryState::Runnable));
        assert!(matches!(observer.wait_state, WaitState::None));
    }

    // ── fault() coverage ─────────────────────────────────────────────

    #[test]
    fn fault_from_runnable_succeeds() {
        let mut observer = Observer::test_default();

        assert!(observer.fault().is_ok());
        assert!(matches!(observer.state, PrimaryState::Faulted));
    }

    #[test]
    fn fault_from_inert_fails() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Inert;

        assert_eq!(
            observer.fault().unwrap_err(),
            ObserverError::InvalidTransition
        );
    }

    #[test]
    fn fault_from_blocked_fails() {
        let mut observer = Observer::test_default();

        observer.state = PrimaryState::Blocked;

        assert_eq!(
            observer.fault().unwrap_err(),
            ObserverError::InvalidTransition
        );
    }

    #[test]
    fn fault_then_resume_roundtrip() {
        let mut observer = Observer::test_default();

        observer.fault().unwrap();

        assert!(matches!(observer.state, PrimaryState::Faulted));

        observer.resume().unwrap();

        assert!(matches!(observer.state, PrimaryState::Runnable));
    }

    // ── suspend() + resume() interactions ────────────────────────────

    #[test]
    fn suspend_sets_flag() {
        let mut observer = Observer::test_default();

        assert!(!observer.suspended);

        observer.suspend();

        assert!(observer.suspended);
    }

    #[test]
    fn suspend_is_idempotent() {
        let mut observer = Observer::test_default();

        observer.suspend();
        observer.suspend();

        assert!(observer.suspended);
        assert!(matches!(observer.state, PrimaryState::Runnable));
    }

    #[test]
    fn resume_from_faulted_clears_suspension() {
        let mut observer = Observer::test_default();

        observer.fault().unwrap();
        observer.suspend();

        assert!(observer.suspended);
        assert!(matches!(observer.state, PrimaryState::Faulted));

        observer.resume().unwrap();

        assert!(!observer.suspended);
        assert!(matches!(observer.state, PrimaryState::Runnable));
    }

    // ── set_scheduling() coverage ────────────────────────────────────

    #[test]
    fn set_scheduling_valid_profile() {
        let mut observer = Observer::test_default();

        assert!(observer.set_scheduling(60, 60).is_ok());
        assert_eq!(observer.responsiveness, 60);
        assert_eq!(observer.throughput, 60);
        assert_eq!(observer.precision(), 8);
    }

    #[test]
    fn set_scheduling_all_responsiveness() {
        let mut observer = Observer::test_default();

        assert!(observer.set_scheduling(128, 0).is_ok());
        assert_eq!(observer.precision(), 0);
    }

    #[test]
    fn set_scheduling_all_throughput() {
        let mut observer = Observer::test_default();

        assert!(observer.set_scheduling(0, 128).is_ok());
        assert_eq!(observer.precision(), 0);
    }

    #[test]
    fn set_scheduling_even_split() {
        let mut observer = Observer::test_default();

        assert!(observer.set_scheduling(64, 64).is_ok());
        assert_eq!(observer.precision(), 0);
    }

    #[test]
    fn set_scheduling_overflow_fails() {
        let mut observer = Observer::test_default();
        let original_r = observer.responsiveness;
        let original_t = observer.throughput;

        assert_eq!(
            observer.set_scheduling(100, 100).unwrap_err(),
            ObserverError::InvalidProfile
        );
        assert_eq!(
            observer.responsiveness, original_r,
            "must not mutate on error"
        );
        assert_eq!(observer.throughput, original_t, "must not mutate on error");
    }

    // ── revoke() coverage ────────────────────────────────────────────

    #[test]
    fn revoke_increments_generation() {
        let observer = Observer::test_default();
        let gen_before = observer
            .generation
            .load(core::sync::atomic::Ordering::Acquire);

        observer.revoke();

        let gen_after = observer
            .generation
            .load(core::sync::atomic::Ordering::Acquire);

        assert_eq!(gen_after, gen_before + 1);
    }

    #[test]
    fn revoke_is_cumulative() {
        let observer = Observer::test_default();

        observer.revoke();
        observer.revoke();
        observer.revoke();

        assert_eq!(
            observer
                .generation
                .load(core::sync::atomic::Ordering::Acquire),
            3
        );
    }

    // ── remove_compute() saturation ──────────────────────────────────

    #[test]
    fn remove_compute_saturates_at_zero() {
        let mut observer = Observer::test_default();

        observer.compute_aggregate = 10;
        observer.remove_compute(100);

        assert_eq!(observer.compute_aggregate, 0);
    }

    // ── is_stopped() coverage ────────────────────────────────────────

    #[test]
    fn is_stopped_matches_inert_and_faulted() {
        assert!(PrimaryState::Inert.is_stopped());
        assert!(PrimaryState::Faulted.is_stopped());
        assert!(!PrimaryState::Runnable.is_stopped());
        assert!(!PrimaryState::Blocked.is_stopped());
    }
}
