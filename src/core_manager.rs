//! Core manager: per-core hot path and exception dispatch.
//!
//! Named after the graph.d2 top-level kernel component. Each logical
//! core owns one core manager instance. The core manager is the kernel's
//! "main loop" equivalent — it runs in exception context (A4) and
//! coordinates all per-core work.
//!
//! D1:  per-core hot path, shared cold path. Hot-path data is per-core
//!      with no cross-core sharing. Cold-path operations (object creation,
//!      cross-core migration) may touch shared state under locks.
//! D7:  split interaction model — IPC vs typed kernel operations.
//! D46: core lifecycle is kernel-internal. All cores activate at boot
//!      (PSCI CPU_ON). Idle cores sleep (WFI/CPU_SUSPEND).
//! D81: hardware event protocol — handle_timer checks deadlines and fires
//!      expired Pulsars; handle_irq routes device interrupts to Fields.
//! D83: per-core data organization — DeadlineEntry and deadline array
//!      for Pulsar timer checking (D44). Max 32 deadlines per core.
//! D79: scheduling decision matrix — for each (IPC operation x outcome)
//!      pair, which state transitions, scheduler calls, register writes,
//!      and DispatchResult. Ten rows covering all 5 IPC operations.
//! A4:  purely reactive — runs only in response to hardware exceptions.

use crate::arena::ObjectId;
use crate::field;
use crate::kernel_state::KernelState;
use crate::observer::Observer;
use crate::time_manager::{CoreId, Scheduler};
use core::ptr::NonNull;
use core::sync::atomic::Ordering;

// ── Per-core Pulsar deadline data (D83, D44) ──────────────────────

/// Maximum number of active Pulsar deadlines per core (D83).
///
/// Hard cap — reject CreatePulsar if the core's deadline array is full.
/// 32 is generous for per-core timer count: typical interactive workloads
/// use 2–5 timers per application, and a core runs one Observer at a time.
/// The fixed array avoids dynamic allocation on the hot path.
pub const MAX_DEADLINES_PER_CORE: usize = 32;

/// A pending Pulsar deadline entry in the per-core deadline array (D83, D44).
///
/// Each entry represents one active Pulsar timer assigned to this core.
/// `handle_timer` scans these on every timer interrupt, comparing
/// `deadline_ticks` against the current counter value.
#[derive(Clone, Copy)]
pub struct DeadlineEntry {
    /// Absolute deadline in counter ticks (kernel-internal).
    /// Compared against `current_ticks` in `handle_timer`.
    pub deadline_ticks: u64,

    /// ObjectId of the Pulsar in the global arena (D67).
    /// Used to look up the Pulsar for fire_message, rearm (repeating),
    /// or removal (one-shot).
    pub pulsar_id: ObjectId,

    /// ObjectId of the delivery Field (D44).
    /// The fire message is enqueued here.
    pub field_id: ObjectId,
}

// ── Per-core state access ──────────────────────────────────────────

/// Access the current core's state.
///
/// On ARM64, each core stores a pointer to its `CoreState` in
/// TPIDR_EL1 at boot. This function reads that register and returns
/// a reference. The implementation lives in frame/arch/ (the register
/// read is unsafe); this function is the safe boundary.
///
/// A4: called at exception entry to find the per-core scheduler,
/// current Observer, and dispatch context. This is the bridge between
/// the hardware exception vector (frame/arch/) and the safe kernel
/// logic in this module.
///
/// D1: the returned reference is core-local — no cross-core sharing.
/// The caller never needs to lock per-core state for local access.
#[cfg(target_os = "none")]
pub fn current_core<S: Scheduler>() -> &'static CoreState<S> {
    crate::frame::cores::read_core_state()
}

/// Mutable access to the current core's state.
///
/// Same as `current_core` but returns a mutable reference. Safe
/// because A4 guarantees the kernel is non-reentrant on a single
/// core — only one exception handler runs at a time, so there can
/// be no aliasing.
#[cfg(target_os = "none")]
pub fn current_core_mut<S: Scheduler>() -> &'static mut CoreState<S> {
    crate::frame::cores::read_core_state_mut()
}

// ── Cross-core IPI dispatch (D56) ─────────────────────────────────

/// Send an IPI request to a target core (D56).
///
/// Writes the request to the target core's mailbox and triggers an SGI.
/// Fire-and-forget: if the mailbox is full the request is silently
/// dropped. The SGI wakes the target from WFI if idle.
///
/// Must be called with a valid `kernel_state` reference. The SGI
/// trigger is bare-metal only (test builds skip the hardware write).
pub fn send_ipi(
    kernel_state: &KernelState,
    target_core: crate::time_manager::CoreId,
    request: crate::kernel_state::IpiRequest,
) {
    // Push to the target core's mailbox. If full, silently drop.
    let pushed = kernel_state.ipi_mailboxes.push_to(target_core, request);

    if pushed {
        // Trigger SGI on the target core to wake it and process the mailbox.
        #[cfg(target_os = "none")]
        crate::frame::arch::gic::send_sgi(
            crate::kernel_state::IPI_SGI_NUMBER,
            target_core.0 as usize,
        );
    }
}

// ── Per-core state ─────────────────────────────────────────────────

/// Per-core kernel state (D1, D83).
///
/// Each logical core owns one of these. The hot path — exception entry,
/// cap resolution, scheduling decision, context switch — operates
/// entirely within this struct's data. No cross-core shared state
/// on the hot path (D1).
///
/// The time manager (D2) is nested inside the core manager, matching
/// graph.d2's structural hierarchy: `kernel.core-manager.time-manager`.
///
/// D83: includes per-core Pulsar deadline array. `handle_timer` scans
/// this array on every timer interrupt — no lock needed (D1: per-core).
///
/// Generic over `S: Scheduler` because D2 allows different scheduling
/// algorithms per core (e.g., throughput on big cores, fixed-priority
/// on LITTLE, deadline on RT-dedicated). The concrete type is chosen
/// at boot based on core classification.
pub struct CoreState<S: Scheduler> {
    /// Which core this state belongs to (D46: kernel-internal, not
    /// exposed to Observers).
    pub core_id: CoreId,

    /// The Observer currently executing on this core, if any.
    /// None when the core is idle (WFI, D46).
    pub current: Option<NonNull<Observer>>,

    /// Per-core scheduling algorithm instance (D2, D59).
    /// Owns the run queue and algorithm-specific state.
    pub scheduler: S,

    /// Per-core Pulsar deadline entries (D83, D44).
    ///
    /// Fixed-size array — `deadline_count` tracks how many entries are
    /// active. Entries are dense-packed: active entries occupy indices
    /// 0..deadline_count, removal swaps with the last entry.
    ///
    /// Per-core (D1), no lock needed. `handle_timer` scans this on
    /// every timer interrupt.
    pub deadlines: [Option<DeadlineEntry>; MAX_DEADLINES_PER_CORE],

    /// Number of active deadline entries (D83).
    ///
    /// Invariant: `deadline_count <= MAX_DEADLINES_PER_CORE`.
    /// Invariant: `deadlines[0..deadline_count]` are all `Some`.
    /// Invariant: `deadlines[deadline_count..]` are all `None`.
    pub deadline_count: usize,

    /// D98: preemptible cascade continuation state.
    ///
    /// Some when a destroy cascade is in progress and was preempted by
    /// the timer. None when no cascade is active. The destroying Observer
    /// is blocked while this is Some (D39).
    pub cascade_continuation: Option<crate::capability::CascadeContinuation>,
}

// ── Dispatch outcomes ──────────────────────────────────────────────

/// Result of exception dispatch — what the core manager tells frame/
/// to do next (D76).
///
/// A4: every exception handler invocation ends with a scheduling
/// decision. The core manager always returns which Observer to resume
/// (or None for idle). frame/ handles the actual context switch.
///
/// D76: safe dispatch writes syscall results (error codes, message data)
/// to RegisterState via frame/ helpers *before* returning. DispatchResult
/// carries only the scheduling decision — frame/ gets a uniform restore
/// path with no error-specific branching.
pub enum DispatchResult {
    /// Resume an Observer. frame/ loads all registers from RegisterState.
    Resume(NonNull<Observer>),
    /// Resume with IPC fast-path optimization (D50, D74).
    /// frame/ loads registers EXCEPT x0–x3, which pass through in
    /// physical registers carrying data words from sender to receiver.
    ResumeFastPath(NonNull<Observer>),
    /// No runnable Observer on this core — enter idle (D46: WFI).
    Idle,
    /// D100: root Observer faulted with no handler. The kernel has
    /// already logged diagnostics. frame/ calls PSCI SYSTEM_OFF.
    FatalFault,
}

// ── CoreState methods ──────────────────────────────────────────────

impl<S: Scheduler> CoreState<S> {
    /// Map a CapError to the corresponding SyscallError (D49, D80).
    ///
    /// Used by dispatch_ipc to translate capability resolution failures
    /// into error codes that userspace receives via carry flag + x0.
    #[cfg(any(target_os = "none", test))]
    fn cap_error_to_syscall_error(
        cap_err: crate::capability::CapError,
    ) -> crate::syscall::SyscallError {
        match cap_err {
            crate::capability::CapError::InvalidHandle
            | crate::capability::CapError::SlotTagMismatch => {
                crate::syscall::SyscallError::InvalidCap
            }
            crate::capability::CapError::StaleGeneration => crate::syscall::SyscallError::StaleCap,
            crate::capability::CapError::InsufficientRights => {
                crate::syscall::SyscallError::NoRight
            }
            crate::capability::CapError::TypeMismatch => crate::syscall::SyscallError::WrongType,
            crate::capability::CapError::TableFull => crate::syscall::SyscallError::TableFull,
            crate::capability::CapError::SendOnceConsumed => {
                crate::syscall::SyscallError::AlreadyConsumed
            }
            crate::capability::CapError::CloneForbidden => {
                crate::syscall::SyscallError::CloneForbidden
            }
        }
    }

    /// D96: install an optional transferred cap, returning the encoded
    /// handle or CAP_ABSENT. Table-full silently produces CAP_ABSENT
    /// (D40 fault delivery is wired in D100).
    #[cfg(any(target_os = "none", test))]
    fn install_cap_or_absent(
        observer_ptr: NonNull<Observer>,
        cap: Option<&crate::capability::TransferredCap>,
    ) -> u64 {
        cap.map_or(crate::capability::CAP_ABSENT, |tc| {
            crate::frame::cores::observer_install_transferred_cap(observer_ptr, tc)
                .unwrap_or(crate::capability::CAP_ABSENT)
        })
    }

    /// D96: decode a user-cap handle and extract the cap from the sender's
    /// table (move semantics). Returns Ok(None) for CAP_ABSENT, Ok(Some)
    /// for a valid cap, or Err(()) if the slot is empty/invalid.
    #[cfg(any(target_os = "none", test))]
    fn try_extract_user_cap(
        sender_ptr: NonNull<Observer>,
        user_cap_raw: u64,
    ) -> Result<Option<crate::capability::TransferredCap>, ()> {
        if user_cap_raw == crate::capability::CAP_ABSENT {
            return Ok(None);
        }

        let handle = crate::capability::Handle::decode(user_cap_raw);

        crate::frame::cores::observer_extract_cap(sender_ptr, handle.index)
            .map(Some)
            .ok_or(())
    }

    /// Write a message to an Observer's saved registers and clear its IPC carry flag.
    ///
    /// D76/D96: slow-path delivery — writes all x0–x7 registers.
    /// If the message carries transferred caps (user_cap, reply_cap), they
    /// are installed into the receiver's cap table and the encoded handles
    /// written to x6/x7. If the receiver's table is full, the cap slot is
    /// written as CAP_ABSENT (D40 fault delivery is wired in D100).
    #[cfg(any(target_os = "none", test))]
    fn deliver_message(observer_ptr: NonNull<Observer>, message: &field::Message) {
        let user_cap_slot = Self::install_cap_or_absent(observer_ptr, message.user_cap.as_ref());
        let reply_cap_slot = Self::install_cap_or_absent(observer_ptr, message.reply_cap.as_ref());

        crate::frame::cores::write_message_to_registers(
            observer_ptr,
            &message.data,
            message.label,
            message.badge.0,
            user_cap_slot,
            reply_cap_slot,
        );
        crate::frame::cores::clear_ipc_carry(observer_ptr);
    }

    /// Handle an IPC syscall (SVC #1–#5).
    ///
    /// D7: IPC is one of two syscall families. The core manager
    /// resolves the cap handle, checks rights, and delegates to the
    /// communication module for the actual Send/Receive/Call/ReplyRecv/Yield
    /// operation. Returns which Observer to resume next.
    ///
    /// D50: for Call/ReplyRecv, the fast path may direct-switch to the
    /// receiver without going through the run queue. The scheduler's
    /// `should_switch_to` callback approves or denies.
    ///
    /// D69: frame/ masks DAIF.I for the fast-path window (~400 cycles).
    ///
    /// D79: scheduling decision matrix — for each (operation x outcome) pair,
    /// the method performs the correct state transitions, scheduler calls,
    /// register writes, and returns the correct DispatchResult.
    #[cfg(any(target_os = "none", test))]
    pub fn dispatch_ipc(
        &mut self,
        operation: crate::syscall::IpcOperation,
        kernel_state: &KernelState,
    ) -> DispatchResult {
        let sender_ptr = match self.current {
            Some(ptr) => ptr,
            None => return DispatchResult::Idle,
        };

        // D48: Yield is fire-and-forget — no cap resolution needed.
        // D79: rotate current Observer to tail, then pick_next.
        // The running Observer is always in the scheduler queue (boot
        // enqueues it). on_preempt rotates head to tail — same as the
        // timer preemption path.
        if operation == crate::syscall::IpcOperation::Yield {
            crate::communication::yield_cpu();

            self.scheduler.on_preempt();

            return self.schedule_next();
        }

        // ── Cap resolution and IPC dispatch ──────────────────────────
        //
        // D77: 8-step cap resolution sequence. D76: pull IPC registers.
        // 1. read_ipc_registers(sender_ptr) for handle + message data
        // 2. Read cap table from Observer (framekernel helper)
        // 3. resolve_cap_entry(handle) — check bounds, slot tag, occupancy
        // 4. Look up target Field in KernelState.fields arena
        // 5. Check generation (D67), rights (D52), type (Field)
        // 6. Construct Message from registers + badge from cap entry
        // 7. Call the communication function
        // 8. Handle outcome per the D79 matrix
        // On error: write_ipc_error(sender_ptr, error), Resume(sender)

        // Step 1: read IPC registers from sender's saved state.
        let ipc_regs = crate::frame::cores::read_ipc_registers(sender_ptr);
        // Step 2: read cap table pointer and capacity from the Observer.
        let (cap_entries, cap_capacity) = crate::frame::cores::observer_cap_table(sender_ptr);
        // Step 3: resolve the primary handle (x5) to a cap entry.
        let raw_handle = ipc_regs.handle_or_badge;
        let entry =
            match crate::capability::resolve_cap_entry(raw_handle, cap_entries, cap_capacity) {
                Ok(e) => e,
                Err(cap_err) => {
                    crate::frame::cores::write_ipc_error(
                        sender_ptr,
                        Self::cap_error_to_syscall_error(cap_err),
                    );

                    return DispatchResult::Resume(sender_ptr);
                }
            };
        // Extract object info from the resolved entry.
        let (object_type, object_id) = match entry.object {
            Some(pair) => pair,
            None => {
                crate::frame::cores::write_ipc_error(
                    sender_ptr,
                    crate::syscall::SyscallError::InvalidCap,
                );

                return DispatchResult::Resume(sender_ptr);
            }
        };

        // Check that the cap targets a Field.
        if object_type != crate::capability::ObjectType::Field {
            crate::frame::cores::write_ipc_error(
                sender_ptr,
                crate::syscall::SyscallError::WrongType,
            );

            return DispatchResult::Resume(sender_ptr);
        }

        // Check rights based on the operation.
        let required_rights = match operation {
            crate::syscall::IpcOperation::Send | crate::syscall::IpcOperation::Call => {
                crate::capability::Rights::SEND
            }
            crate::syscall::IpcOperation::Receive => crate::capability::Rights::RECEIVE,
            crate::syscall::IpcOperation::ReplyRecv => {
                // ReplyRecv: x5 handle is the reply field (needs SEND).
                // Recv field handle is in x7 — checked separately below.
                crate::capability::Rights::SEND
            }
            crate::syscall::IpcOperation::Yield => unreachable!("Yield handled above"),
        };

        if !entry.check_rights(required_rights) {
            crate::frame::cores::write_ipc_error(sender_ptr, crate::syscall::SyscallError::NoRight);

            return DispatchResult::Resume(sender_ptr);
        }

        // Step 4-5: acquire fields lock, look up the Field, check generation.
        let mut fields_guard = kernel_state.fields.acquire();

        // D67: validate reply field generation for Call before taking &mut target.
        if matches!(operation, crate::syscall::IpcOperation::Call)
            && let Some((_, reply_id, stored_gen)) = crate::frame::cores::observer_read_cap_entry(
                sender_ptr,
                crate::capability::SLOT_REPLY_FIELD,
            )
        {
            match fields_guard.get(reply_id) {
                Some(f) if f.generation.load(Ordering::Acquire) != stored_gen => {
                    drop(fields_guard);

                    crate::frame::cores::write_ipc_error(
                        sender_ptr,
                        crate::syscall::SyscallError::StaleCap,
                    );

                    return DispatchResult::Resume(sender_ptr);
                }
                None => {
                    drop(fields_guard);

                    crate::frame::cores::write_ipc_error(
                        sender_ptr,
                        crate::syscall::SyscallError::InvalidCap,
                    );

                    return DispatchResult::Resume(sender_ptr);
                }
                _ => {}
            }
        }

        let target_field = match fields_guard.get_mut(object_id) {
            Some(f) => f,
            None => {
                drop(fields_guard);

                crate::frame::cores::write_ipc_error(
                    sender_ptr,
                    crate::syscall::SyscallError::InvalidCap,
                );

                return DispatchResult::Resume(sender_ptr);
            }
        };

        // D67: generation check.
        let live_gen = target_field.generation.load(Ordering::Acquire);

        if entry.stored_generation != live_gen {
            drop(fields_guard);

            crate::frame::cores::write_ipc_error(
                sender_ptr,
                crate::syscall::SyscallError::StaleCap,
            );

            return DispatchResult::Resume(sender_ptr);
        }

        let badge = entry.badge;
        let is_send_once = entry.is_send_once();
        let handle_index = crate::capability::Handle::decode(raw_handle).index;

        // ── Dispatch per operation ──────────────────────────────────
        match operation {
            crate::syscall::IpcOperation::Send => {
                // D96: extract user cap from sender's table (move semantics).
                let user_cap = match Self::try_extract_user_cap(sender_ptr, ipc_regs.user_cap) {
                    Ok(cap) => cap,
                    Err(()) => {
                        drop(fields_guard);

                        crate::frame::cores::write_ipc_error(
                            sender_ptr,
                            crate::syscall::SyscallError::InvalidCap,
                        );

                        return DispatchResult::Resume(sender_ptr);
                    }
                };

                // Step 6: construct Message from IPC registers.
                let message = crate::field::Message {
                    data: ipc_regs.data,
                    label: ipc_regs.label,
                    badge,
                    user_cap,
                    reply_cap: None,
                };
                // Step 7: call send.
                let outcome = match crate::communication::send(target_field, message) {
                    Ok(outcome) => outcome,
                    Err(crate::field::FieldError::QueueFull) => {
                        drop(fields_guard);

                        crate::frame::cores::write_ipc_error(
                            sender_ptr,
                            crate::syscall::SyscallError::QueueFull,
                        );

                        return DispatchResult::Resume(sender_ptr);
                    }
                    Err(_) => {
                        drop(fields_guard);

                        crate::frame::cores::write_ipc_error(
                            sender_ptr,
                            crate::syscall::SyscallError::QueueFull,
                        );

                        return DispatchResult::Resume(sender_ptr);
                    }
                };

                drop(fields_guard);

                // D51: consume send-once cap after successful Send.
                if is_send_once {
                    crate::frame::cores::observer_free_cap_slot(sender_ptr, handle_index);
                }

                // Step 8: dispatch outcome per D79 matrix.
                self.dispatch_send_outcome(sender_ptr, outcome)
            }

            crate::syscall::IpcOperation::Receive => {
                // Get a NonNull<Field> for the WaitEntry.
                let field_ptr = NonNull::from(&*target_field);
                // Set up WaitEntry in the Observer's wait_state.
                let wait_entry = crate::frame::cores::observer_prepare_wait(sender_ptr, field_ptr);
                // Call receive.
                let outcome = crate::communication::receive(target_field, wait_entry);
                // If received (not blocking), clear the wait_state.
                let is_received =
                    matches!(outcome, crate::communication::ReceiveOutcome::Received(_));

                if is_received {
                    crate::frame::cores::observer_clear_wait(sender_ptr);
                }

                drop(fields_guard);

                // Dispatch outcome per D79 matrix.
                self.dispatch_receive_outcome(sender_ptr, outcome)
            }

            crate::syscall::IpcOperation::Call => {
                // D96: extract user cap from sender's table (move semantics).
                let user_cap = match Self::try_extract_user_cap(sender_ptr, ipc_regs.user_cap) {
                    Ok(cap) => cap,
                    Err(()) => {
                        drop(fields_guard);

                        crate::frame::cores::write_ipc_error(
                            sender_ptr,
                            crate::syscall::SyscallError::InvalidCap,
                        );

                        return DispatchResult::Resume(sender_ptr);
                    }
                };
                // D96: mint reply cap from sender's SLOT_REPLY_FIELD (slot 1).
                // reply_badge from x7 (D65).
                let reply_badge = crate::capability::Badge(ipc_regs.reply_info);
                let reply_cap = crate::frame::cores::observer_read_cap_entry(
                    sender_ptr,
                    crate::capability::SLOT_REPLY_FIELD,
                )
                .map(|(object_type, object_id, stored_generation)| {
                    crate::capability::TransferredCap {
                        object_type,
                        object_id,
                        rights: crate::capability::Rights::SEND
                            .union(crate::capability::Rights::DESTROY)
                            .union(crate::capability::Rights::CLONE),
                        badge: reply_badge,
                        send_once: true,
                        stored_generation,
                    }
                });
                let message = crate::field::Message {
                    data: ipc_regs.data,
                    label: ipc_regs.label,
                    badge,
                    user_cap,
                    reply_cap,
                };
                let outcome = match crate::communication::call(target_field, message, reply_badge) {
                    Ok(outcome) => outcome,
                    Err(crate::field::FieldError::QueueFull) => {
                        drop(fields_guard);

                        crate::frame::cores::write_ipc_error(
                            sender_ptr,
                            crate::syscall::SyscallError::QueueFull,
                        );

                        return DispatchResult::Resume(sender_ptr);
                    }
                    Err(_) => {
                        drop(fields_guard);

                        crate::frame::cores::write_ipc_error(
                            sender_ptr,
                            crate::syscall::SyscallError::QueueFull,
                        );

                        return DispatchResult::Resume(sender_ptr);
                    }
                };

                drop(fields_guard);

                // D51: consume send-once cap after successful Call.
                if is_send_once {
                    crate::frame::cores::observer_free_cap_slot(sender_ptr, handle_index);
                }

                // Pass label, badge, and reply_cap for outcome handling.
                self.dispatch_call_outcome_with_metadata(
                    sender_ptr,
                    outcome,
                    ipc_regs.label,
                    badge.0,
                    reply_cap,
                )
            }

            crate::syscall::IpcOperation::ReplyRecv => {
                // ReplyRecv needs TWO fields:
                // - reply_field: from x5 handle (already resolved above as target_field)
                // - recv_field: from x7 (reply_info) handle
                //
                // The reply_field is what we resolved above. Now resolve the
                // recv_field from x7.
                let recv_raw_handle = ipc_regs.reply_info;
                let recv_entry = match crate::capability::resolve_cap_entry(
                    recv_raw_handle,
                    cap_entries,
                    cap_capacity,
                ) {
                    Ok(e) => e,
                    Err(cap_err) => {
                        drop(fields_guard);

                        crate::frame::cores::write_ipc_error(
                            sender_ptr,
                            Self::cap_error_to_syscall_error(cap_err),
                        );

                        return DispatchResult::Resume(sender_ptr);
                    }
                };

                // Check recv entry is a Field with RECEIVE right.
                let (recv_type, recv_id) = match recv_entry.object {
                    Some(pair) => pair,
                    None => {
                        drop(fields_guard);

                        crate::frame::cores::write_ipc_error(
                            sender_ptr,
                            crate::syscall::SyscallError::InvalidCap,
                        );

                        return DispatchResult::Resume(sender_ptr);
                    }
                };

                if recv_type != crate::capability::ObjectType::Field {
                    drop(fields_guard);

                    crate::frame::cores::write_ipc_error(
                        sender_ptr,
                        crate::syscall::SyscallError::WrongType,
                    );

                    return DispatchResult::Resume(sender_ptr);
                }

                if !recv_entry.check_rights(crate::capability::Rights::RECEIVE) {
                    drop(fields_guard);

                    crate::frame::cores::write_ipc_error(
                        sender_ptr,
                        crate::syscall::SyscallError::NoRight,
                    );

                    return DispatchResult::Resume(sender_ptr);
                }

                // reply_field is target_field (already resolved, from x5).
                // We need a separate mutable reference to recv_field.
                // If they're the same ObjectId, that's a protocol error (you
                // can't reply and receive on the same field).
                if object_id == recv_id {
                    drop(fields_guard);

                    crate::frame::cores::write_ipc_error(
                        sender_ptr,
                        crate::syscall::SyscallError::InvalidCap,
                    );

                    return DispatchResult::Resume(sender_ptr);
                }

                // Check recv_field generation.
                let recv_field = match fields_guard.get_mut(recv_id) {
                    Some(f) => f,
                    None => {
                        drop(fields_guard);

                        crate::frame::cores::write_ipc_error(
                            sender_ptr,
                            crate::syscall::SyscallError::InvalidCap,
                        );

                        return DispatchResult::Resume(sender_ptr);
                    }
                };

                let recv_live_gen = recv_field.generation.load(Ordering::Acquire);

                if recv_entry.stored_generation != recv_live_gen {
                    drop(fields_guard);

                    crate::frame::cores::write_ipc_error(
                        sender_ptr,
                        crate::syscall::SyscallError::StaleCap,
                    );

                    return DispatchResult::Resume(sender_ptr);
                }

                // Get NonNull<Field> for the recv_field WaitEntry.
                let recv_field_ptr = NonNull::from(&*recv_field);
                // Set up WaitEntry for potential blocking on recv_field.
                let wait_entry =
                    crate::frame::cores::observer_prepare_wait(sender_ptr, recv_field_ptr);
                // Construct reply message.
                let reply_message = crate::field::Message {
                    data: ipc_regs.data,
                    label: ipc_regs.label,
                    badge,
                    user_cap: None,
                    reply_cap: None,
                };
                // We need two &mut Field references simultaneously.
                // Arena::get_mut borrows the entire arena mutably, so we
                // cannot hold two &mut references from it. Instead, we obtain
                // NonNull pointers to each field (from &mut references, which
                // is safe) and pass them to a frame/ helper that performs the
                // actual dereference. The two ObjectIds are different (checked
                // above), so no aliasing occurs.
                let reply_field_ref = fields_guard.get_mut(object_id).unwrap();
                let reply_field_ptr = NonNull::from(&mut *reply_field_ref);
                let recv_field_ref = fields_guard.get_mut(recv_id).unwrap();
                let recv_field_ptr = NonNull::from(&mut *recv_field_ref);
                let outcome = crate::frame::cores::call_reply_recv(
                    reply_field_ptr,
                    recv_field_ptr,
                    reply_message,
                    wait_entry,
                );
                // If received (not blocking), clear the wait_state.
                let is_received = matches!(
                    outcome.receive_outcome,
                    crate::communication::ReceiveOutcome::Received(_)
                );

                if is_received {
                    crate::frame::cores::observer_clear_wait(sender_ptr);
                }

                drop(fields_guard);

                // D51: consume send-once cap on the reply field (x5)
                // after the reply is sent.
                if is_send_once {
                    crate::frame::cores::observer_free_cap_slot(sender_ptr, handle_index);
                }

                self.dispatch_reply_recv_outcome(sender_ptr, outcome)
            }

            crate::syscall::IpcOperation::Yield => unreachable!("Yield handled above"),
        }
    }

    fn deliver_kernel_message(
        &mut self,
        field: &mut field::Field,
        message: field::Message,
    ) -> Result<(), field::FieldError> {
        if field.is_full() && field.waiters_head.is_none() {
            field.pending_kernel_message = Some(message);

            return Ok(());
        }

        self.deliver_kernel_message_inner(field, message)
    }

    #[cfg(any(target_os = "none", test))]
    fn deliver_kernel_message_inner(
        &mut self,
        field: &mut field::Field,
        message: field::Message,
    ) -> Result<(), field::FieldError> {
        match crate::communication::send(field, message) {
            Ok(crate::communication::SendOutcome::Enqueued) => Ok(()),
            Ok(crate::communication::SendOutcome::WokeReceiver(receiver_ptr, msg)) => {
                Self::deliver_message(receiver_ptr, &msg);

                let _ = crate::frame::cores::observer_unblock(receiver_ptr);

                self.scheduler.enqueue(receiver_ptr);

                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    #[cfg(not(any(target_os = "none", test)))]
    fn deliver_kernel_message_inner(
        &mut self,
        field: &mut field::Field,
        message: field::Message,
    ) -> Result<(), field::FieldError> {
        field.enqueue(message)
    }

    #[cfg(any(target_os = "none", test))]
    pub fn dispatch_send_outcome(
        &mut self,
        sender_ptr: NonNull<Observer>,
        outcome: crate::communication::SendOutcome,
    ) -> DispatchResult {
        match outcome {
            crate::communication::SendOutcome::Enqueued => {
                // Row 1: message in queue, sender continues.
                crate::frame::cores::clear_ipc_carry(sender_ptr);

                DispatchResult::Resume(sender_ptr)
            }
            crate::communication::SendOutcome::WokeReceiver(receiver_ptr, message) => {
                // Row 2: deliver message to receiver's registers, enqueue receiver.
                crate::frame::cores::clear_ipc_carry(sender_ptr);
                Self::deliver_message(receiver_ptr, &message);

                let _ = crate::frame::cores::observer_unblock(receiver_ptr);

                self.scheduler.enqueue(receiver_ptr);

                // D50: Send is NOT fast-path eligible. Sender always continues.
                DispatchResult::Resume(sender_ptr)
            }
        }
    }

    #[cfg(any(target_os = "none", test))]
    /// D79 Row 3-4: Handle Receive outcome.
    ///
    /// Row 3 (Received): message available, receiver continues with it.
    /// Row 4 (Blocked): queue empty, receiver blocks on Field.
    pub fn dispatch_receive_outcome(
        &mut self,
        receiver_ptr: NonNull<Observer>,
        outcome: crate::communication::ReceiveOutcome,
    ) -> DispatchResult {
        match outcome {
            crate::communication::ReceiveOutcome::Received(message) => {
                // Row 3: message available, deliver to receiver's registers.
                Self::deliver_message(receiver_ptr, &message);

                DispatchResult::Resume(receiver_ptr)
            }
            crate::communication::ReceiveOutcome::Blocked => {
                // Row 4: queue empty, receiver blocks.
                let _ = crate::frame::cores::observer_set_blocked(receiver_ptr);

                self.scheduler.dequeue(receiver_ptr);
                self.schedule_next()
            }
        }
    }

    #[cfg(any(target_os = "none", test))]
    /// D79 Row 5-7: Handle Call outcome — caller always blocks.
    ///
    /// Convenience wrapper without label/badge/reply_cap. Used by tests
    /// that exercise scheduling behavior without cap transfer. Delegates
    /// to dispatch_call_outcome_with_metadata with zero metadata and no caps.
    pub fn dispatch_call_outcome(
        &mut self,
        sender_ptr: NonNull<Observer>,
        outcome: crate::communication::CallOutcome,
    ) -> DispatchResult {
        self.dispatch_call_outcome_with_metadata(sender_ptr, outcome, 0, 0, None)
    }

    #[cfg(any(target_os = "none", test))]
    /// D79 Row 5-7 with metadata and D96 cap transfer.
    ///
    /// Handles Call outcome with label, badge, and reply cap from the
    /// dispatch path. The reply_cap (minted from sender's SLOT_REPLY_FIELD)
    /// is installed into the receiver's table on all delivery paths.
    ///
    /// D96 DirectSwitch denial: reads sender's saved registers, constructs
    /// a Message, and delivers via slow path — no enum change needed.
    pub fn dispatch_call_outcome_with_metadata(
        &mut self,
        sender_ptr: NonNull<Observer>,
        outcome: crate::communication::CallOutcome,
        label: u64,
        badge: u64,
        reply_cap: Option<crate::capability::TransferredCap>,
    ) -> DispatchResult {
        // D16: caller always blocks on Call, regardless of outcome.

        match outcome {
            crate::communication::CallOutcome::Enqueued => {
                // Row 5: message in queue, no receiver woken. Caller blocks.
                // The reply_cap was already in the Message (installed on dequeue).
                let _ = crate::frame::cores::observer_set_blocked(sender_ptr);

                self.scheduler.dequeue(sender_ptr);
                self.schedule_next()
            }
            crate::communication::CallOutcome::DirectSwitch(receiver_ptr) => {
                // Row 6: D50 fast path. Consult scheduler.
                if self.scheduler.should_switch_to(receiver_ptr) {
                    // Approved: direct-switch to receiver.
                    // D74: x0-x3 pass through in physical registers.
                    // D50: no user cap (0-cap gate).
                    let user_cap_slot = crate::capability::CAP_ABSENT;
                    let reply_cap_slot =
                        Self::install_cap_or_absent(receiver_ptr, reply_cap.as_ref());

                    crate::frame::cores::write_metadata_to_registers(
                        receiver_ptr,
                        label,
                        badge,
                        user_cap_slot,
                        reply_cap_slot,
                    );

                    // Dequeue sender (it's blocking). Receiver bypasses queue.
                    let _ = crate::frame::cores::observer_set_blocked(sender_ptr);

                    self.scheduler.dequeue(sender_ptr);

                    // Unblock receiver (D39: Blocked -> Runnable).
                    let _ = crate::frame::cores::observer_unblock(receiver_ptr);

                    DispatchResult::ResumeFastPath(receiver_ptr)
                } else {
                    // D96: DirectSwitch denied — fall back to slow path.
                    // Read sender's data words; label and badge are already
                    // available as function parameters.
                    let sender_regs = crate::frame::cores::read_ipc_registers(sender_ptr);
                    let denial_message = crate::field::Message {
                        data: sender_regs.data,
                        label,
                        badge: crate::capability::Badge(badge),
                        user_cap: None, // D50: 0-cap gate, no user cap
                        reply_cap,
                    };
                    let _ = crate::frame::cores::observer_set_blocked(sender_ptr);

                    self.scheduler.dequeue(sender_ptr);
                    // Deliver via slow path (installs reply cap in receiver's table).
                    Self::deliver_message(receiver_ptr, &denial_message);

                    // Unblock receiver and enqueue it.
                    let _ = crate::frame::cores::observer_unblock(receiver_ptr);

                    self.scheduler.enqueue(receiver_ptr);
                    self.schedule_next()
                }
            }
            crate::communication::CallOutcome::WokeReceiverSlowPath(receiver_ptr, message) => {
                // Row 7: waiter found but user cap forces slow path.
                // Message already carries user_cap and reply_cap.
                let _ = crate::frame::cores::observer_set_blocked(sender_ptr);

                self.scheduler.dequeue(sender_ptr);
                Self::deliver_message(receiver_ptr, &message);

                // Unblock receiver and enqueue it.
                let _ = crate::frame::cores::observer_unblock(receiver_ptr);

                self.scheduler.enqueue(receiver_ptr);
                self.schedule_next()
            }
        }
    }

    #[cfg(any(target_os = "none", test))]
    /// D79 Row 8-9: Handle ReplyRecv outcome.
    ///
    /// Reply phase: if a client was waiting on the reply field, deliver the
    /// reply message to the client and enqueue the client.
    ///
    /// Receive phase:
    /// Row 8 (Received): new message available, server continues.
    /// Row 9 (Blocked): recv_field empty, server blocks.
    pub fn dispatch_reply_recv_outcome(
        &mut self,
        server_ptr: NonNull<Observer>,
        outcome: crate::communication::ReplyRecvOutcome,
    ) -> DispatchResult {
        // ── Reply phase: deliver reply to client if waiting ──────────
        if let Some(delivery) = outcome.reply_delivery {
            Self::deliver_message(delivery.client, &delivery.message);

            let _ = crate::frame::cores::observer_unblock(delivery.client);

            self.scheduler.enqueue(delivery.client);
        }

        // ── Receive phase ────────────────────────────────────────────
        match outcome.receive_outcome {
            crate::communication::ReceiveOutcome::Received(message) => {
                // Row 8: new message available, server continues.
                Self::deliver_message(server_ptr, &message);

                DispatchResult::Resume(server_ptr)
            }
            crate::communication::ReceiveOutcome::Blocked => {
                // Row 9: recv_field empty, server blocks.
                let _ = crate::frame::cores::observer_set_blocked(server_ptr);

                self.scheduler.dequeue(server_ptr);
                self.schedule_next()
            }
        }
    }

    /// Handle a typed kernel operation (SVC #0, code in x4).
    ///
    /// D7: typed ops are the other syscall family. The core manager
    /// resolves the target cap, verifies type and rights (D4/D52),
    /// then dispatches to the type-specific operation.
    ///
    /// D49: 20 operations in dense table dispatch (codes 0–19).
    /// Typed operations never block — they always return Resume(sender).
    #[cfg(any(target_os = "none", test))]
    pub fn dispatch_typed(
        &mut self,
        operation: crate::syscall::TypedOperation,
        kernel_state: &KernelState,
    ) -> DispatchResult {
        use crate::capability::{self, ObjectType, Rights};
        use crate::syscall::{SyscallError, TypedOperation};

        let sender_ptr = match self.current {
            Some(ptr) => ptr,
            None => return DispatchResult::Idle,
        };
        // D76: read typed registers from sender's saved register state.
        let regs = crate::frame::cores::read_typed_registers(sender_ptr);
        // Read sender's cap table pointer and capacity for handle resolution.
        let (cap_entries, cap_capacity) = crate::frame::cores::observer_cap_table(sender_ptr);
        // Helper: write a typed error result and resume the sender.
        // Typed operations never block (D49).
        let typed_error = |error: SyscallError| -> DispatchResult {
            crate::frame::cores::write_typed_result(sender_ptr, error.error_code());

            DispatchResult::Resume(sender_ptr)
        };
        // Helper: write a success result (0 for void, positive for values).
        let typed_ok = |value: u64| -> DispatchResult {
            crate::frame::cores::write_typed_result(sender_ptr, value);

            DispatchResult::Resume(sender_ptr)
        };
        // ── Step 1: Resolve the target cap entry (D77 steps 1-5) ────
        let entry =
            match capability::resolve_cap_entry(regs.target_handle, cap_entries, cap_capacity) {
                Ok(entry) => entry,
                Err(cap_err) => return typed_error(SyscallError::from(cap_err)),
            };
        // Read the object type and id from the entry.
        let (object_type, object_id) = match entry.object {
            Some(pair) => pair,
            None => return typed_error(SyscallError::InvalidCap),
        };

        // ── Step 2: Type check (D49) ─────────────────────────────────
        // Type-specific operations must target the correct type.
        // Generic operations (Destroy, Clone, Close, Mint) accept any type.
        if let Some(expected_type) = operation.target_type()
            && object_type != expected_type
        {
            return typed_error(SyscallError::WrongType);
        }

        // ── Step 3: Rights check (D52) ──────────────────────────────
        let required = Self::required_rights(operation, object_type);

        if !entry.check_rights(required) {
            return typed_error(SyscallError::NoRight);
        }

        // Extract entry fields needed by creation operations (D95/D32).
        // Creation handlers overwrite this cap entry via write_entry (raw
        // pointer), so they must not access `entry` after that point.
        // These copies ensure all reads happen before any writes.
        let entry_slot_tag = entry.slot_tag;
        let entry_stored_generation = entry.stored_generation;

        // ── Step 4: Dispatch to per-operation logic ─────────────────
        match operation {
            // ── Observer operations ──────────────────────────────────
            TypedOperation::ObserverResume => {
                let obs_ptr = match validated_observer_ptr(kernel_state, object_id, entry) {
                    Ok(ptr) => ptr,
                    Err(e) => return typed_error(e),
                };

                if crate::frame::cores::observer_resume(obs_ptr).is_err() {
                    return typed_error(SyscallError::InvalidState);
                }

                self.scheduler.enqueue(obs_ptr);

                typed_ok(0)
            }
            TypedOperation::ObserverSuspend => {
                match with_validated_observer_mut(kernel_state, object_id, entry, |observer| {
                    observer.suspend();
                }) {
                    Ok(()) => typed_ok(0),
                    Err(e) => typed_error(e),
                }
            }
            TypedOperation::ObserverSetScheduling => {
                let responsiveness = regs.args[0] as u8;
                let throughput = regs.args[1] as u8;

                match with_validated_observer_mut(kernel_state, object_id, entry, |observer| {
                    observer.set_scheduling(responsiveness, throughput)
                }) {
                    Ok(Ok(())) => typed_ok(0),
                    Ok(Err(_)) => typed_error(SyscallError::InvalidProfile),
                    Err(e) => typed_error(e),
                }
            }
            TypedOperation::ObserverInstallCap => {
                // D97: install a cap into the target Observer's cap table.
                // args[0] = source cap handle (in caller's table).
                let source_handle = regs.args[0];
                let source_entry =
                    match capability::resolve_cap_entry(source_handle, cap_entries, cap_capacity) {
                        Ok(e) => e,
                        Err(cap_err) => return typed_error(SyscallError::from(cap_err)),
                    };
                let (source_type, source_id) = match source_entry.object {
                    Some(pair) => pair,
                    None => return typed_error(SyscallError::InvalidCap),
                };
                let target_ptr = match validated_observer_ptr(kernel_state, object_id, entry) {
                    Ok(ptr) => ptr,
                    Err(e) => return typed_error(e),
                };
                let transferred = capability::TransferredCap {
                    object_type: source_type,
                    object_id: source_id,
                    rights: source_entry.rights,
                    badge: source_entry.badge,
                    send_once: source_entry.send_once,
                    stored_generation: source_entry.stored_generation,
                };

                match crate::frame::cores::observer_install_transferred_cap(
                    target_ptr,
                    &transferred,
                ) {
                    Ok(encoded_handle) => typed_ok(encoded_handle),
                    Err(_) => typed_error(SyscallError::TableFull),
                }
            }
            TypedOperation::ObserverWriteRegisters => {
                // D103: inline register transfer — PC, SP, x0, PSTATE.
                let target_ptr = match validated_observer_ptr(kernel_state, object_id, entry) {
                    Ok(ptr) => ptr,
                    Err(e) => return typed_error(e),
                };
                // ABI: x0=PC, x1=SP, x2=target's x0, x3=PSTATE (masked).
                const PSTATE_USER_MASK: u64 = 0xF000_0000;
                let pc = regs.args[0];
                let sp = regs.args[1];
                let x0 = regs.args[2];
                let safe_pstate = regs.args[3] & PSTATE_USER_MASK;

                if crate::frame::cores::observer_write_registers(
                    target_ptr,
                    pc,
                    sp,
                    x0,
                    safe_pstate,
                ) {
                    typed_ok(0)
                } else {
                    typed_error(SyscallError::InvalidState)
                }
            }
            TypedOperation::ObserverReadRegisters => {
                // D103: inline register read — PC, SP, x0, PSTATE.
                let target_ptr = match validated_observer_ptr(kernel_state, object_id, entry) {
                    Ok(ptr) => ptr,
                    Err(e) => return typed_error(e),
                };

                match crate::frame::cores::observer_read_registers(target_ptr) {
                    Some((pc, sp, x0, pstate)) => {
                        // Write all four values to caller's registers.
                        crate::frame::cores::write_typed_result(sender_ptr, pc);
                        crate::frame::cores::write_read_registers_result(
                            sender_ptr, sp, x0, pstate,
                        );

                        DispatchResult::Resume(sender_ptr)
                    }
                    None => typed_error(SyscallError::InvalidState),
                }
            }
            TypedOperation::ObserverChangeHandler => {
                // D97: replace the fault handler Field cap at SLOT_FAULT_HANDLER
                // in the target Observer's cap table.
                // args[0] = new handler Field cap handle (in caller's table).
                let handler_handle = regs.args[0];
                let handler_entry = match capability::resolve_cap_entry(
                    handler_handle,
                    cap_entries,
                    cap_capacity,
                ) {
                    Ok(e) => e,
                    Err(cap_err) => return typed_error(SyscallError::from(cap_err)),
                };
                let (handler_type, handler_id) = match handler_entry.object {
                    Some(pair) => pair,
                    None => return typed_error(SyscallError::InvalidCap),
                };

                if handler_type != ObjectType::Field {
                    return typed_error(SyscallError::WrongType);
                }

                let handler_badge = capability::Badge(regs.args[1]);
                let handler_stored_gen = handler_entry.stored_generation;
                let target_ptr = match validated_observer_ptr(kernel_state, object_id, entry) {
                    Ok(ptr) => ptr,
                    Err(e) => return typed_error(e),
                };
                let new_handler = capability::Entry {
                    object: Some((ObjectType::Field, handler_id)),
                    rights: Rights::SEND,
                    badge: handler_badge,
                    slot_tag: capability::SlotTag(0),
                    send_once: false,
                    stored_generation: handler_stored_gen,
                };

                if crate::frame::cores::observer_write_cap_at(
                    target_ptr,
                    capability::SLOT_FAULT_HANDLER,
                    new_handler,
                ) {
                    typed_ok(0)
                } else {
                    typed_error(SyscallError::InvalidCap)
                }
            }

            // ── Generic operations ──────────────────────────────────
            TypedOperation::Destroy => {
                // D98: destroy cascade and return.
                //
                // Observer: table-full check → revoke → cascade → return Space.
                // Field/Pulsar: revoke → return Space (reverse type conversion).
                // Space: revoke → free → return pages to root pool.
                // Time: revoke → free (no Space involved).
                match object_type {
                    ObjectType::Observer => {
                        let (live_gen, cap_capacity_target, backing_va, backing_sz) = {
                            let observers = kernel_state.observers.acquire();
                            let observer = match observers.get(object_id) {
                                Some(o) => o,
                                None => return typed_error(SyscallError::InvalidCap),
                            };
                            let live_gen = observer.generation.load(Ordering::Acquire);

                            (
                                live_gen,
                                observer.cap_table_capacity,
                                observer.backing_va_base,
                                observer.backing_size,
                            )
                        };

                        if !entry.check_generation(live_gen) {
                            return typed_error(SyscallError::StaleCap);
                        }

                        // D98: table-full check BEFORE marking dead. Need a free
                        // slot for the return Space cap (if backing exists).
                        if let Err(e) = check_destroy_backing(sender_ptr, backing_sz) {
                            return typed_error(e);
                        }

                        {
                            let observers = kernel_state.observers.acquire();

                            if let Some(observer) = observers.get(object_id) {
                                observer.revoke();
                            }
                        }

                        // D98: preemptible cascade — initiate and block destroyer.
                        if cap_capacity_target == 0 {
                            // No caps to cascade — complete immediately.
                            {
                                let mut observers = kernel_state.observers.acquire();

                                observers.free(object_id);
                            }

                            typed_ok(return_backing_space(
                                kernel_state,
                                sender_ptr,
                                backing_va,
                                backing_sz,
                            ))
                        } else {
                            let mut cascade = crate::capability::CascadeContinuation::new();

                            cascade.push(object_id);
                            cascade.destroyer_ptr = Some(sender_ptr);
                            cascade.backing_va = backing_va;
                            cascade.backing_size = backing_sz;
                            cascade.target_id = object_id;

                            // Run first batch synchronously before blocking.
                            let target_ptr = crate::frame::cores::observer_ptr_from_arena(
                                kernel_state,
                                object_id,
                            );

                            if let Some(ptr) = target_ptr {
                                let done = crate::frame::cores::observer_cascade_step(
                                    ptr,
                                    &mut cascade,
                                    16,
                                );

                                if done {
                                    cascade.pop();
                                }
                            }

                            if cascade.is_empty() {
                                // Cascade completed in the first batch.
                                {
                                    let mut observers = kernel_state.observers.acquire();

                                    observers.free(object_id);
                                }

                                typed_ok(return_backing_space(
                                    kernel_state,
                                    sender_ptr,
                                    backing_va,
                                    backing_sz,
                                ))
                            } else {
                                // More work remains — block destroyer, defer to timer.
                                let _ = crate::frame::cores::observer_set_blocked(sender_ptr);

                                self.scheduler.dequeue(sender_ptr);
                                self.cascade_continuation = Some(cascade);
                                self.schedule_next()
                            }
                        }
                    }
                    ObjectType::Field => {
                        let (live_gen, backing_va, backing_sz) = {
                            let fields = kernel_state.fields.acquire();
                            let field = match fields.get(object_id) {
                                Some(f) => f,
                                None => return typed_error(SyscallError::InvalidCap),
                            };
                            let live_gen = field.generation.load(Ordering::Acquire);

                            (live_gen, field.backing_va_base, field.backing_size)
                        };

                        if !entry.check_generation(live_gen) {
                            return typed_error(SyscallError::StaleCap);
                        }

                        if let Err(e) = check_destroy_backing(sender_ptr, backing_sz) {
                            return typed_error(e);
                        }

                        {
                            let mut fields = kernel_state.fields.acquire();

                            if let Some(field) = fields.get(object_id) {
                                field.revoke();
                            }

                            fields.free(object_id);
                        }

                        typed_ok(return_backing_space(
                            kernel_state,
                            sender_ptr,
                            backing_va,
                            backing_sz,
                        ))
                    }
                    ObjectType::Pulsar => {
                        let (live_gen, backing_va, backing_sz) = {
                            let pulsars = kernel_state.pulsars.acquire();
                            let pulsar = match pulsars.get(object_id) {
                                Some(p) => p,
                                None => return typed_error(SyscallError::InvalidCap),
                            };
                            let live_gen = pulsar.generation.load(Ordering::Acquire);

                            (live_gen, pulsar.backing_va_base, pulsar.backing_size)
                        };

                        if !entry.check_generation(live_gen) {
                            return typed_error(SyscallError::StaleCap);
                        }

                        if let Err(e) = check_destroy_backing(sender_ptr, backing_sz) {
                            return typed_error(e);
                        }

                        // D99: remove Pulsar from per-core deadline array.
                        for i in (0..self.deadline_count).rev() {
                            if let Some(de) = &self.deadlines[i]
                                && de.pulsar_id == object_id
                            {
                                self.deadline_count -= 1;

                                if i < self.deadline_count {
                                    self.deadlines[i] = self.deadlines[self.deadline_count].take();
                                } else {
                                    self.deadlines[i] = None;
                                }

                                break;
                            }
                        }

                        {
                            let mut pulsars = kernel_state.pulsars.acquire();

                            if let Some(pulsar) = pulsars.get(object_id) {
                                pulsar.revoke();
                            }

                            pulsars.free(object_id);
                        }

                        typed_ok(return_backing_space(
                            kernel_state,
                            sender_ptr,
                            backing_va,
                            backing_sz,
                        ))
                    }
                    ObjectType::Space => {
                        let (live_gen, size, content_pa, l3_table_pa, va_base) = {
                            let spaces = kernel_state.spaces.acquire();
                            let space = match spaces.get(object_id) {
                                Some(s) => s,
                                None => return typed_error(SyscallError::InvalidCap),
                            };
                            let live_gen = space.generation.load(Ordering::Acquire);

                            (
                                live_gen,
                                space.size,
                                space.content_pa as usize,
                                space.l3_table_pa as usize,
                                space.va_base,
                            )
                        };

                        if !entry.check_generation(live_gen) {
                            return typed_error(SyscallError::StaleCap);
                        }

                        {
                            let mut sm = kernel_state.space_manager.acquire();

                            sm.destroy_space(content_pa, l3_table_pa, va_base, size);
                        }

                        consume_space(kernel_state, object_id);

                        typed_ok(0)
                    }
                    ObjectType::Time => {
                        // D98: Time is asymmetric — no Space involved.
                        // Revoke and free. Compute returns to per-core pool.
                        let mut times = kernel_state.times.acquire();
                        let time = match times.get(object_id) {
                            Some(t) => t,
                            None => return typed_error(SyscallError::InvalidCap),
                        };
                        let live_gen = time.generation.load(Ordering::Acquire);

                        if !entry.check_generation(live_gen) {
                            return typed_error(SyscallError::StaleCap);
                        }

                        time.revoke();
                        times.free(object_id);

                        typed_ok(0)
                    }
                }
            }
            TypedOperation::Clone => {
                // D38: Time is linear — clone forbidden.
                if object_type == ObjectType::Time {
                    return typed_error(SyscallError::CloneForbidden);
                }

                // D97: duplicate entry in caller's own cap table.
                // No generation check needed — Clone is a cap-table
                // operation. The stored_generation is copied; validation
                // happens when the cloned cap is used.
                let transferred = capability::TransferredCap {
                    object_type,
                    object_id,
                    rights: entry.rights,
                    badge: entry.badge,
                    send_once: entry.send_once,
                    stored_generation: entry.stored_generation,
                };

                match crate::frame::cores::observer_install_transferred_cap(
                    sender_ptr,
                    &transferred,
                ) {
                    Ok(encoded_handle) => typed_ok(encoded_handle),
                    Err(_) => typed_error(SyscallError::TableFull),
                }
            }
            TypedOperation::Close => {
                // D97: free slot in caller's cap table.
                let handle = capability::Handle::decode(regs.target_handle);
                let close_result =
                    crate::frame::cores::observer_close_cap(sender_ptr, handle.index);

                match close_result {
                    capability::CloseResult::Closed {
                        object_type: _closed_type,
                        object_id: _closed_id,
                        ..
                    } => {
                        // D26 (bare-metal): when closing a Space cap, check
                        // whether the Observer still holds another cap to the
                        // same Space. If not, unwire the page table mapping.
                        #[cfg(target_os = "none")]
                        if _closed_type == ObjectType::Space {
                            let still_has = crate::frame::cores::observer_has_cap_to_object(
                                sender_ptr,
                                ObjectType::Space,
                                _closed_id,
                                u32::MAX,
                            );

                            if !still_has {
                                unwire_space_for_observer(sender_ptr, _closed_id, kernel_state);
                            }
                        }

                        typed_ok(0)
                    }
                    capability::CloseResult::ClosedWithBadgeClosure { .. } => {
                        // D17: badge-closure tracking map not yet built.
                        // The close succeeded; badge-closure delivery is
                        // deferred.
                        typed_ok(0)
                    }
                    capability::CloseResult::AlreadyEmpty => typed_error(SyscallError::InvalidCap),
                }
            }
            TypedOperation::Mint => {
                // D97: create attenuated cap with optional badge.
                // args[0] = requested rights mask, args[1] = badge value.
                // If badge == CAP_ABSENT, keep the source badge.
                let requested_rights = Rights::from_bits(regs.args[0] as u16);
                let badge = if regs.args[1] == capability::CAP_ABSENT {
                    entry.badge
                } else {
                    capability::Badge(regs.args[1])
                };
                let attenuated = entry.rights.attenuate(requested_rights);
                let transferred = capability::TransferredCap {
                    object_type,
                    object_id,
                    rights: attenuated,
                    badge,
                    send_once: entry.send_once,
                    stored_generation: entry.stored_generation,
                };

                match crate::frame::cores::observer_install_transferred_cap(
                    sender_ptr,
                    &transferred,
                ) {
                    Ok(encoded_handle) => typed_ok(encoded_handle),
                    Err(_) => typed_error(SyscallError::TableFull),
                }
            }

            // ── Space operations ────────────────────────────────────
            TypedOperation::SpaceSplit => {
                let split_size = regs.args[0] as usize;
                let mut spaces = kernel_state.spaces.acquire();
                let space = match spaces.get_mut(object_id) {
                    Some(s) => s,
                    None => return typed_error(SyscallError::InvalidCap),
                };
                let live_gen = space.generation.load(Ordering::Acquire);

                if !entry.check_generation(live_gen) {
                    return typed_error(SyscallError::StaleCap);
                }

                let page_size = {
                    let sm = kernel_state.space_manager.acquire();

                    sm.root_pool.page_size
                };

                match space.split(split_size, page_size) {
                    Ok((new_va, rounded_size)) => {
                        drop(spaces);

                        let new_space_id = {
                            let mut spaces = kernel_state.spaces.acquire();

                            match spaces.allocate() {
                                Ok((id, new_space)) => {
                                    new_space.va_base = new_va;
                                    new_space.size = rounded_size;
                                    new_space.l3_table_pa = 0;
                                    new_space.refcount = 1;
                                    new_space.generation = core::sync::atomic::AtomicU64::new(0);

                                    id
                                }
                                Err(_) => {
                                    // Rollback: restore source Space size.
                                    if let Some(s) = spaces.get_mut(object_id) {
                                        s.size += rounded_size;
                                    }

                                    return typed_error(SyscallError::InsufficientResource);
                                }
                            }
                        };
                        let transferred = capability::TransferredCap {
                            object_type: ObjectType::Space,
                            object_id: new_space_id,
                            rights: Rights::SPACE_ALL,
                            badge: capability::Badge(0),
                            send_once: false,
                            stored_generation: 0,
                        };

                        match crate::frame::cores::observer_install_transferred_cap(
                            sender_ptr,
                            &transferred,
                        ) {
                            Ok(encoded_handle) => typed_ok(encoded_handle),
                            Err(_) => {
                                // Rollback: free new Space and restore source.
                                let mut spaces = kernel_state.spaces.acquire();

                                spaces.free(new_space_id);

                                if let Some(s) = spaces.get_mut(object_id) {
                                    s.size += rounded_size;
                                }

                                typed_error(SyscallError::TableFull)
                            }
                        }
                    }
                    Err(crate::space::SpaceError::ZeroSize) => typed_error(SyscallError::ZeroSize),
                    Err(crate::space::SpaceError::InsufficientSpace) => {
                        typed_error(SyscallError::InsufficientResource)
                    }
                    Err(crate::space::SpaceError::NotAdjacent) => {
                        typed_error(SyscallError::NotAdjacent)
                    }
                }
            }
            TypedOperation::SpaceMerge => {
                // args[0] = handle to the source Space cap to merge.
                let source_handle = regs.args[0];
                let source_entry =
                    match capability::resolve_cap_entry(source_handle, cap_entries, cap_capacity) {
                        Ok(e) => e,
                        Err(cap_err) => return typed_error(SyscallError::from(cap_err)),
                    };
                let (source_type, source_id) = match source_entry.object {
                    Some(pair) => pair,
                    None => return typed_error(SyscallError::InvalidCap),
                };

                if source_type != ObjectType::Space {
                    return typed_error(SyscallError::WrongType);
                }
                if !source_entry.check_rights(Rights::MERGE) {
                    return typed_error(SyscallError::NoRight);
                }
                if object_id == source_id {
                    // Cannot merge with self.
                    return typed_error(SyscallError::InvalidState);
                }

                // Two-phase approach to avoid needing simultaneous mutable +
                // shared references into the same arena (framekernel discipline:
                // no unsafe outside frame/). Phase 1: read source fields via
                // shared reference. Phase 2: mutate target.
                let spaces = kernel_state.spaces.acquire();
                // Phase 1: read source fields.
                let (source_va_base, source_size, source_gen) = match spaces.get(source_id) {
                    Some(s) => (s.va_base, s.size, s.generation.load(Ordering::Acquire)),
                    None => return typed_error(SyscallError::InvalidCap),
                };

                if !source_entry.check_generation(source_gen) {
                    return typed_error(SyscallError::StaleCap);
                }

                // Check target generation via shared reference.
                let target_gen = match spaces.get(object_id) {
                    Some(t) => t.generation.load(Ordering::Acquire),
                    None => return typed_error(SyscallError::InvalidCap),
                };

                if !entry.check_generation(target_gen) {
                    return typed_error(SyscallError::StaleCap);
                }

                drop(spaces);

                // Phase 2: merge using a temporary Space value.
                // Surrogate for adjacency check only. merge() reads only
                // va_base and size. l3_table_pa, refcount, and generation
                // are fabricated — never read by merge().
                let source_snapshot = crate::space::Space {
                    va_base: source_va_base,
                    size: source_size,
                    refcount: 0,
                    content_pa: 0,
                    l3_table_pa: 0,
                    generation: core::sync::atomic::AtomicU64::new(0),
                };
                let mut spaces = kernel_state.spaces.acquire();
                let target = match spaces.get_mut(object_id) {
                    Some(t) => t,
                    None => return typed_error(SyscallError::InvalidCap),
                };

                match target.merge(&source_snapshot) {
                    Ok(()) => typed_ok(0),
                    Err(crate::space::SpaceError::NotAdjacent) => {
                        typed_error(SyscallError::NotAdjacent)
                    }
                    Err(crate::space::SpaceError::ZeroSize) => typed_error(SyscallError::ZeroSize),
                    Err(crate::space::SpaceError::InsufficientSpace) => {
                        typed_error(SyscallError::InsufficientResource)
                    }
                }
            }

            // ── Field operations ────────────────────────────────────
            TypedOperation::CreateField => {
                if object_type != ObjectType::Space {
                    return typed_error(SyscallError::WrongType);
                }

                let (backing_va, space_size) =
                    match verify_space(kernel_state, object_id, entry_stored_generation) {
                        Ok(pair) => pair,
                        Err(e) => return typed_error(e),
                    };
                let queue_capacity = space_size / core::mem::size_of::<crate::field::Message>();

                if queue_capacity == 0 {
                    return typed_error(SyscallError::InsufficientResource);
                }

                let queue_ptr =
                    match crate::frame::fields::allocate_field_queue(queue_capacity as u32) {
                        Some(ptr) => ptr,
                        None => return typed_error(SyscallError::InsufficientResource),
                    };

                let field_id = {
                    let mut fields = kernel_state.fields.acquire();
                    let (id, new_field) = match fields.allocate() {
                        Ok(pair) => pair,
                        Err(_) => return typed_error(SyscallError::InsufficientResource),
                    };

                    *new_field = crate::field::Field::new(
                        queue_ptr,
                        queue_capacity as u32,
                        backing_va,
                        space_size,
                    );

                    id
                };

                consume_space(kernel_state, object_id);

                let handle = capability::Handle::decode(regs.target_handle);
                let wrote = crate::frame::capabilities::write_entry(
                    cap_entries,
                    cap_capacity,
                    handle.index,
                    capability::Entry {
                        object: Some((ObjectType::Field, field_id)),
                        rights: Rights::FIELD_ALL,
                        badge: capability::Badge(0),
                        slot_tag: entry_slot_tag,
                        send_once: false,
                        stored_generation: 0,
                    },
                );

                debug_assert!(wrote);

                typed_ok(0)
            }
            TypedOperation::FieldSplit => {
                // D99/D45: split a Field by badge range.
                //
                // target_handle = source Field cap (SPLIT right, already checked).
                // args[0] = Space cap handle (consumed for new sub-Field).
                // args[1] = badge_low (inclusive).
                // args[2] = badge_high (inclusive).
                //
                // Creates a new sub-Field backed by the consumed Space. Adds a
                // routing rule to the source Field. Updates IrqRoutingTable
                // entries whose badge falls in the split range. Installs the
                // new Field's cap in the Space cap's former slot.

                let source_field_id = object_id;
                let badge_low = regs.args[1];
                let badge_high = regs.args[2];

                if badge_low > badge_high {
                    return typed_error(SyscallError::InvalidState);
                }

                let (space_id, backing_va, space_size) = match resolve_space_argument(
                    regs.args[0],
                    cap_entries,
                    cap_capacity,
                    kernel_state,
                ) {
                    Ok(tuple) => tuple,
                    Err(e) => return typed_error(e),
                };
                let queue_capacity = space_size / core::mem::size_of::<crate::field::Message>();

                if queue_capacity == 0 {
                    return typed_error(SyscallError::InsufficientResource);
                }

                let queue_ptr =
                    match crate::frame::fields::allocate_field_queue(queue_capacity as u32) {
                        Some(ptr) => ptr,
                        None => return typed_error(SyscallError::InsufficientResource),
                    };

                let mut fields = kernel_state.fields.acquire();
                let new_field_id = {
                    let (id, new_field) = match fields.allocate() {
                        Ok(pair) => pair,
                        Err(_) => return typed_error(SyscallError::InsufficientResource),
                    };

                    *new_field = crate::field::Field::new(
                        queue_ptr,
                        queue_capacity as u32,
                        backing_va,
                        space_size,
                    );

                    id
                };

                if let Some(source_field) = fields.get_mut(source_field_id) {
                    if source_field
                        .add_route(badge_low, badge_high, new_field_id, 0)
                        .is_err()
                    {
                        fields.free(new_field_id);

                        return typed_error(SyscallError::InsufficientResource);
                    }
                } else {
                    fields.free(new_field_id);

                    return typed_error(SyscallError::InvalidCap);
                }

                drop(fields);
                consume_space(kernel_state, space_id);

                {
                    let mut irq_routes = kernel_state.irq_routes.acquire();

                    irq_routes.update_routes_for_split(badge_low, badge_high, new_field_id, 0);
                }

                let space_handle = crate::capability::Handle::decode(regs.args[0]);
                let wrote = crate::frame::capabilities::write_entry(
                    cap_entries,
                    cap_capacity,
                    space_handle.index,
                    capability::Entry {
                        object: Some((ObjectType::Field, new_field_id)),
                        rights: Rights::FIELD_ALL,
                        badge: capability::Badge(0),
                        slot_tag: capability::SlotTag(space_handle.slot_tag.0),
                        send_once: false,
                        stored_generation: 0,
                    },
                );

                debug_assert!(wrote);

                typed_ok(0)
            }

            // ── Time operations ─────────────────────────────────────
            TypedOperation::TimeSplit => {
                let amount = regs.args[0] as u32;
                let mut times = kernel_state.times.acquire();
                let time = match times.get_mut(object_id) {
                    Some(t) => t,
                    None => return typed_error(SyscallError::InvalidCap),
                };
                let live_gen = time.generation.load(Ordering::Acquire);

                if !entry.check_generation(live_gen) {
                    return typed_error(SyscallError::StaleCap);
                }

                match time.split(amount) {
                    Ok(new_units) => {
                        drop(times);

                        let new_time_id = {
                            let mut times = kernel_state.times.acquire();

                            match times.allocate() {
                                Ok((id, new_time)) => {
                                    new_time.compute_units = new_units;
                                    new_time.refcount = 1;
                                    new_time.generation = core::sync::atomic::AtomicU64::new(0);

                                    id
                                }
                                Err(_) => {
                                    if let Some(t) = times.get_mut(object_id) {
                                        t.compute_units += amount;
                                    }

                                    return typed_error(SyscallError::InsufficientResource);
                                }
                            }
                        };

                        let transferred = capability::TransferredCap {
                            object_type: ObjectType::Time,
                            object_id: new_time_id,
                            rights: Rights::TIME_ALL,
                            badge: capability::Badge(0),
                            send_once: false,
                            stored_generation: 0,
                        };
                        match crate::frame::cores::observer_install_transferred_cap(
                            sender_ptr,
                            &transferred,
                        ) {
                            Ok(encoded_handle) => typed_ok(encoded_handle),
                            Err(_) => {
                                let mut times = kernel_state.times.acquire();

                                times.free(new_time_id);

                                if let Some(t) = times.get_mut(object_id) {
                                    t.compute_units += amount;
                                }

                                typed_error(SyscallError::TableFull)
                            }
                        }
                    }
                    Err(crate::time::TimeError::ZeroAmount) => typed_error(SyscallError::ZeroSize),
                    Err(crate::time::TimeError::InsufficientUnits) => {
                        typed_error(SyscallError::InsufficientResource)
                    }
                }
            }

            // ── Pulsar operations ───────────────────────────────────
            TypedOperation::CreatePulsar => {
                if object_type != ObjectType::Space {
                    return typed_error(SyscallError::WrongType);
                }

                let (backing_va, backing_size) =
                    match verify_space(kernel_state, object_id, entry_stored_generation) {
                        Ok(pair) => pair,
                        Err(e) => return typed_error(e),
                    };
                let (delivery_field_id, _) = match resolve_field_argument(
                    regs.args[0],
                    cap_entries,
                    cap_capacity,
                    kernel_state,
                ) {
                    Ok(pair) => pair,
                    Err(e) => return typed_error(e),
                };
                let badge = capability::Badge(regs.args[1]);
                let duration_ns = regs.args[2];
                let period_ns = regs.args[3];

                if self.deadline_count >= MAX_DEADLINES_PER_CORE {
                    return typed_error(SyscallError::InsufficientResource);
                }

                let counter_freq = crate::frame::cores::read_counter_freq();
                let now_ticks = crate::frame::cores::read_counter_ticks();
                let (pulsar_id, deadline_ticks) = {
                    let mut pulsars = kernel_state.pulsars.acquire();
                    let (id, pulsar) = match pulsars.allocate() {
                        Ok(pair) => pair,
                        Err(_) => return typed_error(SyscallError::InsufficientResource),
                    };

                    *pulsar = crate::pulsar::Pulsar::new(
                        delivery_field_id,
                        badge,
                        duration_ns,
                        period_ns,
                        counter_freq,
                        now_ticks,
                    );
                    pulsar.backing_va_base = backing_va;
                    pulsar.backing_size = backing_size;

                    (id, pulsar.next_deadline_ticks)
                };

                consume_space(kernel_state, object_id);

                let idx = self.deadline_count;

                self.deadlines[idx] = Some(DeadlineEntry {
                    deadline_ticks,
                    pulsar_id,
                    field_id: delivery_field_id,
                });
                self.deadline_count += 1;

                let handle = capability::Handle::decode(regs.target_handle);
                let wrote = crate::frame::capabilities::write_entry(
                    cap_entries,
                    cap_capacity,
                    handle.index,
                    capability::Entry {
                        object: Some((ObjectType::Pulsar, pulsar_id)),
                        rights: Rights::PULSAR_ALL,
                        badge: capability::Badge(0),
                        slot_tag: entry_slot_tag,
                        send_once: false,
                        stored_generation: 0,
                    },
                );

                debug_assert!(wrote);

                typed_ok(0)
            }
            TypedOperation::ClockRead => {
                // D66: enable direct EL0 counter access on this Observer.
                // CNTKCTL_EL1.EL0VCTEN will be set on the next context restore.
                crate::frame::cores::observer_set_clock_access(sender_ptr);

                let ticks = crate::frame::cores::read_counter_ticks();

                typed_ok(ticks)
            }

            // ── Observer creation ───────────────────────────────────
            TypedOperation::CreateObserver => {
                if object_type != ObjectType::Space {
                    return typed_error(SyscallError::WrongType);
                }

                let (backing_va, space_size) =
                    match verify_space(kernel_state, object_id, entry_stored_generation) {
                        Ok(pair) => pair,
                        Err(e) => return typed_error(e),
                    };
                let (handler_field_id, handler_stored_gen) = match resolve_field_argument(
                    regs.args[0],
                    cap_entries,
                    cap_capacity,
                    kernel_state,
                ) {
                    Ok(pair) => pair,
                    Err(e) => return typed_error(e),
                };
                let handler_badge = capability::Badge(regs.args[1]);
                // D95: structural backing from consumed Space.
                let register_state_size =
                    core::mem::size_of::<crate::frame::arch::register_state::RegisterState>();
                let l1_root_size = 16384usize; // ARM64 16 KiB granule L1 table
                let min_structural = register_state_size + l1_root_size;

                if space_size < min_structural {
                    return typed_error(SyscallError::InsufficientResource);
                }

                let cap_table_bytes = space_size - min_structural;
                let entry_size = core::mem::size_of::<capability::Entry>();
                let min_cap_slots = capability::SLOT_USER_START + 1;

                if cap_table_bytes / entry_size < min_cap_slots as usize {
                    return typed_error(SyscallError::InsufficientResource);
                }

                let cap_capacity_new = (cap_table_bytes / entry_size) as u32;
                let rs_ptr = match crate::frame::cores::allocate_register_state() {
                    Some(ptr) => ptr,
                    None => return typed_error(SyscallError::InsufficientResource),
                };
                let cap_entries_new =
                    match crate::frame::capabilities::allocate_cap_table(cap_capacity_new) {
                        Some(ptr) => ptr,
                        None => return typed_error(SyscallError::InsufficientResource),
                    };

                let observer_id = {
                    let mut observers = kernel_state.observers.acquire();
                    let (id, obs) = match observers.allocate() {
                        Ok(pair) => pair,
                        Err(_) => return typed_error(SyscallError::InsufficientResource),
                    };
                    let (asid, asid_gen) = crate::frame::cores::allocate_asid(kernel_state);

                    // D26: allocate per-Observer L1 page table with L1[0] → kernel L2_ROOT.
                    // Host tests skip this — no MMU.
                    #[cfg(target_os = "none")]
                    let page_table_root = {
                        match crate::frame::boot::allocate_observer_l1(kernel_state) {
                            Ok(l1_pa) => crate::frame::arch::mmu::make_ttbr0(asid, l1_pa as u64),
                            Err(_) => {
                                observers.free(id);

                                return typed_error(SyscallError::InsufficientResource);
                            }
                        }
                    };
                    #[cfg(not(target_os = "none"))]
                    let page_table_root = 0u64;

                    obs.object_id = id;
                    obs.asid = asid;
                    obs.asid_generation = asid_gen;
                    obs.register_state = crate::observer::RegisterStateHandle::new(rs_ptr);
                    obs.page_table_root = page_table_root;
                    obs.cap_table = cap_entries_new;
                    obs.cap_table_capacity = cap_capacity_new;
                    obs.cap_table_free_head = Some(crate::capability::SLOT_USER_START);
                    obs.cap_table_count = 0;
                    obs.state = crate::observer::PrimaryState::Inert;
                    obs.suspended = false;
                    obs.compute_aggregate = 0;
                    obs.responsiveness = crate::observer::DEFAULT_RESPONSIVENESS;
                    obs.throughput = crate::observer::DEFAULT_THROUGHPUT;
                    obs.clock_access = false;
                    obs.wait_state = crate::observer::WaitState::None;
                    obs.backing_va_base = backing_va;
                    obs.backing_size = space_size;
                    obs.refcount = 1;
                    obs.generation = core::sync::atomic::AtomicU64::new(0);

                    id
                };

                consume_space(kernel_state, object_id);

                // D57/D95: populate reserved cap table slots.
                let wrote = crate::frame::capabilities::write_entry(
                    cap_entries_new,
                    cap_capacity_new,
                    capability::SLOT_FAULT_HANDLER,
                    capability::Entry {
                        object: Some((ObjectType::Field, handler_field_id)),
                        rights: Rights::SEND,
                        badge: handler_badge,
                        slot_tag: capability::SlotTag(0),
                        send_once: false,
                        stored_generation: handler_stored_gen,
                    },
                );

                debug_assert!(wrote);

                let wrote = crate::frame::capabilities::write_entry(
                    cap_entries_new,
                    cap_capacity_new,
                    capability::SLOT_SELF,
                    capability::Entry {
                        object: Some((ObjectType::Observer, observer_id)),
                        rights: Rights::OBSERVER_ALL,
                        badge: capability::Badge(0),
                        slot_tag: capability::SlotTag(0),
                        send_once: false,
                        stored_generation: 0,
                    },
                );

                debug_assert!(wrote);

                let handle = capability::Handle::decode(regs.target_handle);
                let wrote = crate::frame::capabilities::write_entry(
                    cap_entries,
                    cap_capacity,
                    handle.index,
                    capability::Entry {
                        object: Some((ObjectType::Observer, observer_id)),
                        rights: Rights::OBSERVER_ALL,
                        badge: capability::Badge(0),
                        slot_tag: entry_slot_tag,
                        send_once: false,
                        stored_generation: 0,
                    },
                );

                debug_assert!(wrote);

                typed_ok(0)
            }

            // ── Resource ────────────────────────────────────────────
            TypedOperation::ResourceRequest => {
                // D104: dual-path resource request.
                // args[0] = resource type (0=Space, 1=Time), args[1] = quantity.
                let resource = match regs.args[0] {
                    0 => crate::fault::ResourceType::Space,
                    1 => crate::fault::ResourceType::Time,
                    _ => return typed_error(SyscallError::InvalidState),
                };
                let quantity = regs.args[1];
                // Detect root vs non-root by checking handler cap at slot 0.
                let handler_entry = crate::frame::cores::observer_read_full_cap_entry(
                    sender_ptr,
                    crate::capability::SLOT_FAULT_HANDLER,
                );
                let has_valid_handler = handler_entry.as_ref().is_some_and(|e| e.object.is_some());

                if has_valid_handler {
                    // Non-root: fault-route to handler Field (D31 pager chain).
                    let fault = crate::fault::FaultType::ResourceRequest { resource, quantity };

                    self.dispatch_fault(fault, kernel_state)
                } else {
                    // Root: kernel allocates directly from SpaceManager pool.
                    if !matches!(resource, crate::fault::ResourceType::Space) {
                        return typed_error(SyscallError::InvalidState);
                    }

                    let page_count = quantity as usize;

                    if page_count == 0 {
                        return typed_error(SyscallError::ZeroSize);
                    }

                    let page_size = {
                        let sm = kernel_state.space_manager.acquire();

                        sm.root_pool.page_size
                    };
                    let split_size = page_count * page_size;
                    // D31/D104: root allocates by splitting the target Space.
                    let (new_va, rounded_size) = {
                        let mut spaces = kernel_state.spaces.acquire();
                        let space = match spaces.get_mut(object_id) {
                            Some(s) => s,
                            None => return typed_error(SyscallError::InvalidCap),
                        };

                        match space.split(split_size, page_size) {
                            Ok(pair) => pair,
                            Err(_) => return typed_error(SyscallError::InsufficientResource),
                        }
                    };

                    let new_space_id = {
                        let mut spaces = kernel_state.spaces.acquire();

                        match spaces.allocate() {
                            Ok((id, space)) => {
                                space.va_base = new_va;
                                space.size = rounded_size;
                                space.l3_table_pa = 0;
                                space.refcount = 1;
                                space.generation = core::sync::atomic::AtomicU64::new(0);

                                id
                            }
                            Err(_) => return typed_error(SyscallError::InsufficientResource),
                        }
                    };
                    let transferred = capability::TransferredCap {
                        object_type: ObjectType::Space,
                        object_id: new_space_id,
                        rights: Rights::SPACE_ALL,
                        badge: capability::Badge(0),
                        send_once: false,
                        stored_generation: 0,
                    };

                    match crate::frame::cores::observer_install_transferred_cap(
                        sender_ptr,
                        &transferred,
                    ) {
                        Ok(encoded_handle) => typed_ok(encoded_handle),
                        Err(_) => {
                            let mut spaces = kernel_state.spaces.acquire();

                            spaces.free(new_space_id);

                            typed_error(SyscallError::TableFull)
                        }
                    }
                }
            }
        }
    }

    /// Determine the required rights for a typed operation (D52).
    ///
    /// Maps each operation to the specific Rights bit(s) that the caller's
    /// capability must contain. Generic operations use type-appropriate
    /// rights.
    #[cfg(any(target_os = "none", test))]
    fn required_rights(
        operation: crate::syscall::TypedOperation,
        object_type: crate::capability::ObjectType,
    ) -> crate::capability::Rights {
        use crate::capability::{ObjectType, Rights};
        use crate::syscall::TypedOperation;

        match operation {
            // Observer operations — each has a specific right.
            TypedOperation::ObserverResume => Rights::RESUME,
            TypedOperation::ObserverInstallCap => Rights::INSTALL_CAP,
            TypedOperation::ObserverWriteRegisters => Rights::WRITE_REGISTERS,
            TypedOperation::ObserverReadRegisters => Rights::READ_REGISTERS,
            TypedOperation::ObserverSuspend => Rights::SUSPEND,
            TypedOperation::ObserverChangeHandler => Rights::CHANGE_HANDLER,
            TypedOperation::ObserverSetScheduling => Rights::MODIFY_SCHEDULING,
            // Generic operations — type-appropriate right.
            TypedOperation::Destroy => Rights::DESTROY,
            TypedOperation::Clone => Rights::CLONE,
            TypedOperation::Close => Rights::empty(), // Close always allowed.
            TypedOperation::Mint => Rights::MINT,
            // Space operations.
            TypedOperation::SpaceSplit => Rights::SPLIT,
            TypedOperation::SpaceMerge => Rights::MERGE,
            // Field operations — creation uses SPLIT on the Space cap.
            TypedOperation::CreateField => Rights::SPLIT,
            TypedOperation::FieldSplit => Rights::SPLIT,
            // Time operations.
            TypedOperation::TimeSplit => Rights::SPLIT,
            // Pulsar — creation from Time uses SPLIT.
            TypedOperation::CreatePulsar => Rights::SPLIT,
            // ClockRead — no cap right needed (operation targets self).
            TypedOperation::ClockRead => Rights::empty(),
            // Observer creation — from Space, uses SPLIT.
            TypedOperation::CreateObserver => Rights::SPLIT,
            // ResourceRequest — DESTROY on the Space (privileged).
            TypedOperation::ResourceRequest => {
                // ResourceRequest targets a Space cap. Use DESTROY as the
                // privilege check — only root-level Spaces should have DESTROY.
                match object_type {
                    ObjectType::Space => Rights::DESTROY,
                    _ => Rights::DESTROY,
                }
            }
        }
    }

    /// Handle a timer interrupt (preemption tick).
    ///
    /// D2: calls `scheduler.on_preempt()` for accounting, then
    /// `scheduler.pick_next()` to decide whether to switch.
    /// D44/D81: checks pending Pulsar deadlines. For each expired deadline:
    /// constructs `Message::timer_fire`, enqueues to target Field, rearms
    /// repeating Pulsars, removes one-shot deadlines.
    ///
    /// D76: `current_ticks` is a single consistent snapshot of the
    /// timer counter, read by frame/ before calling. The counter is
    /// volatile hardware state — pushing it as a parameter keeps
    /// handle_timer pure and testable.
    ///
    /// D81: counter_freq is needed for rearm (ns -> ticks conversion).
    /// Passed as parameter for testability (same rationale as current_ticks).
    pub fn handle_timer(
        &mut self,
        current_ticks: u64,
        kernel_state: &KernelState,
        counter_freq: u64,
    ) -> DispatchResult {
        // D81: scan deadline array for expired entries.
        // Linear scan, swap-remove for expired entries.
        //
        // D53 lock ordering: Field (2) < Pulsar (4). Both locks acquired
        // before the loop to respect the ordering and avoid per-iteration
        // spinlock overhead. The scan is bounded by MAX_DEADLINES_PER_CORE (32).
        let mut fields = kernel_state.fields.acquire();
        let mut pulsars = kernel_state.pulsars.acquire();
        let mut i = 0;

        while i < self.deadline_count {
            let entry = self.deadlines[i].expect("D83: invariant — active slot must be Some");

            if entry.deadline_ticks <= current_ticks {
                if let Some(pulsar) = pulsars.get_mut(entry.pulsar_id) {
                    let message = pulsar.fire_message(current_ticks);

                    if let Some(target_field) = fields.get_mut(entry.field_id)
                        && self.deliver_kernel_message(target_field, message).is_err()
                    {
                        pulsar.record_overrun();
                    }

                    if pulsar.is_repeating() {
                        pulsar.rearm(counter_freq);

                        self.deadlines[i] = Some(DeadlineEntry {
                            deadline_ticks: pulsar.next_deadline_ticks,
                            pulsar_id: entry.pulsar_id,
                            field_id: entry.field_id,
                        });

                        i += 1;
                    } else {
                        self.swap_remove_deadline(i);
                    }
                } else {
                    self.swap_remove_deadline(i);
                }
            } else {
                i += 1;
            }
        }

        drop(pulsars);
        drop(fields);

        self.continue_cascade(kernel_state);
        self.scheduler.on_preempt();
        self.schedule_next()
    }

    /// Continue a preemptible cascade if one is in progress (D98).
    ///
    /// Called from handle_timer after the Pulsar deadline scan and before
    /// schedule_next(). Runs one batch of cap-close steps (16 slots).
    /// When the cascade completes, frees the destroyed Observer, installs
    /// the return Space cap in the destroyer's table, and unblocks the
    /// destroyer.
    #[cfg(any(target_os = "none", test))]
    fn continue_cascade(&mut self, kernel_state: &KernelState) {
        if let Some(ref mut cascade) = self.cascade_continuation {
            let target_id = cascade.target_id;
            let target_ptr = crate::frame::cores::observer_ptr_from_arena(kernel_state, target_id);

            if let Some(ptr) = target_ptr {
                let done = crate::frame::cores::observer_cascade_step(ptr, cascade, 16);

                if done {
                    cascade.pop();
                }
            }

            if self
                .cascade_continuation
                .as_ref()
                .is_some_and(|c| c.is_empty())
            {
                let cascade = self.cascade_continuation.take().unwrap();
                let destroyer_ptr = cascade.destroyer_ptr;

                {
                    let mut observers = kernel_state.observers.acquire();

                    observers.free(cascade.target_id);
                }

                if let Some(dptr) = destroyer_ptr {
                    let return_handle = return_backing_space(
                        kernel_state,
                        dptr,
                        cascade.backing_va,
                        cascade.backing_size,
                    );

                    crate::frame::cores::write_typed_result(dptr, return_handle);

                    let _ = crate::frame::cores::observer_unblock(dptr);

                    self.scheduler.enqueue(dptr);
                }
            }
        }
    }

    #[cfg(not(any(target_os = "none", test)))]
    fn continue_cascade(&mut self, _kernel_state: &KernelState) {}

    /// Handle a device interrupt routed to this core (D22, D81).
    ///
    /// D22: the kernel reads the GIC IAR, masks the interrupt,
    /// constructs a message with a per-IRQ badge and send-once ack cap
    /// (D16), and enqueues to the registered driver Field. The driver
    /// acks by using the send-once cap, which unmasks the interrupt.
    ///
    /// D81 flow:
    /// 1. Look up route in IRQ routing table by INTID.
    /// 2. If route exists: check generation against live Field.
    /// 3. Construct Message::device_irq with route's badge.
    /// 4. Enqueue to target Field. If full, log and drop (D18: no pending
    ///    list for IRQ messages — they are edge-triggered notifications).
    /// 5. If no route or stale: log and ignore.
    /// 6. Return schedule_next().
    pub fn handle_irq(&mut self, irq: u32, kernel_state: &KernelState) -> DispatchResult {
        // D81: look up IRQ route.
        let irq_routes = kernel_state.irq_routes.acquire();

        if let Some(route) = irq_routes.lookup(irq) {
            let field_id = route.field_id;
            let badge = route.badge;
            let route_generation = route.generation;

            drop(irq_routes);

            // Acquire field arena to deliver the message.
            let mut fields = kernel_state.fields.acquire();

            if let Some(target_field) = fields.get_mut(field_id) {
                // D67: check generation — stale route detection.
                let live_gen = target_field.generation.load(Ordering::Acquire);

                if live_gen == route_generation {
                    // Route is valid — construct and deliver message.
                    let message = field::Message::device_irq(badge, irq);
                    // D18: if the queue is full, the message is dropped.
                    // IRQ messages are edge-triggered — the interrupt stays
                    // masked until explicitly acked. No pending list needed.
                    let _ = target_field.enqueue(message);
                }
                // else: stale route — generation mismatch, silently ignore.
            }
            // else: Field freed — silently ignore.
        }
        // else: no route for this INTID — silently ignore.

        self.schedule_next()
    }

    /// Handle an IPI (Software Generated Interrupt) from a remote core (D56).
    ///
    /// Drains the local core's IPI mailbox and processes each request:
    /// - WorkSteal: hint to check for runnable Observers (currently a no-op
    ///   since the scheduler already picks the next runnable Observer).
    /// - ObserverMigration: look up the Observer by ObjectId, enqueue it
    ///   into the local scheduler.
    /// - TlbInvalidation: execute a local TLB invalidation barrier.
    /// - RoutingEntryCleanup: no-op (routing table cleanup is handled by
    ///   the core that modifies the routing table under lock).
    ///
    /// Returns schedule_next() to pick the best Observer after processing.
    pub fn handle_ipi(&mut self, kernel_state: &KernelState) -> DispatchResult {
        use crate::kernel_state::IpiRequest;

        // Drain all pending IPI requests from this core's mailbox.
        loop {
            let request = kernel_state.ipi_mailboxes.pop_from(self.core_id);

            match request {
                None => break,
                Some(IpiRequest::WorkSteal) => {
                    // Work-steal hint: the scheduler will pick the next
                    // runnable Observer in schedule_next(). No additional
                    // action needed — the IPI woke us from WFI.
                }
                Some(IpiRequest::ObserverMigration(observer_id)) => {
                    // Look up the Observer in the global arena and enqueue
                    // it into this core's local scheduler.
                    let observers = kernel_state.observers.acquire();

                    if let Some(observer) = observers.get(observer_id) {
                        let observer_ptr = core::ptr::NonNull::from(observer);

                        drop(observers);

                        self.scheduler.enqueue(observer_ptr);
                    }
                }
                Some(IpiRequest::TlbInvalidation) => {
                    // The requesting core already issued broadcast TLBI with
                    // the IS (inner-shareable) suffix, followed by DSB ISH.
                    // That sequence ensures all cores in the shareable domain
                    // see the invalidation. This IPI's purpose is to wake the
                    // target core from WFI so it takes any pending exceptions
                    // before resuming userspace with potentially stale TLB
                    // entries. No additional barrier needed here — the
                    // exception entry/exit sequence includes the necessary
                    // context synchronization.
                }
                Some(IpiRequest::RoutingEntryCleanup) => {
                    // Routing table cleanup is handled centrally by the
                    // modifying core under the irq_routes lock. The IPI
                    // serves as a notification that routing state changed.
                }
            }
        }

        self.schedule_next()
    }

    /// Build a CoreSnapshot for placement decisions (D56, D59).
    ///
    /// Captures the current core's state as a snapshot: idle status,
    /// queue depth, and capacity factor. Used by the Placement trait
    /// to compare cores and decide where to schedule a new Observer.
    pub fn build_core_snapshot(&self) -> crate::time_manager::CoreSnapshot {
        let next = self.scheduler.pick_next();

        crate::time_manager::CoreSnapshot {
            core_id: self.core_id,
            idle: self.current.is_none() && next.is_none(),
            queue_depth: if next.is_some() { 1 } else { 0 },
            capacity_factor: 100,
        }
    }

    /// Remove a deadline entry by swap-removing with the last active entry (D83).
    ///
    /// Maintains the dense-packing invariant: active entries in
    /// `0..deadline_count`, None entries beyond. O(1) removal.
    fn swap_remove_deadline(&mut self, index: usize) {
        let last = self.deadline_count - 1;

        if index != last {
            self.deadlines[index] = self.deadlines[last];
        }

        self.deadlines[last] = None;
        self.deadline_count -= 1;
    }

    /// Select the next Observer to run after handling an exception.
    ///
    /// D2/D59: delegates to `scheduler.pick_next()`. Returns `Idle`
    /// if no Observer is runnable (D46: core enters WFI).
    pub fn schedule_next(&mut self) -> DispatchResult {
        match self.scheduler.pick_next() {
            Some(observer) => {
                self.current = Some(observer);

                DispatchResult::Resume(observer)
            }
            None => {
                self.current = None;
                DispatchResult::Idle
            }
        }
    }

    // ── Fault delivery (D100) ────────────────────────────────────────

    /// Deliver a fault to the current Observer's handler Field (D100).
    ///
    /// D68/D100: HandlerUnavailable for the root Observer means the
    /// system has no recovery path — returns FatalFault so frame/ can
    /// call PSCI SYSTEM_OFF. Observer transitions to Faulted (D39)
    /// before delivery so it cannot be re-scheduled during the fault.
    #[cfg(any(target_os = "none", test))]
    pub fn dispatch_fault(
        &mut self,
        fault: crate::fault::FaultType,
        kernel_state: &KernelState,
    ) -> DispatchResult {
        let observer_ptr = match self.current {
            Some(ptr) => ptr,
            None => return DispatchResult::Idle,
        };
        let (observer_id, observer_generation) =
            crate::frame::cores::observer_fault_info(observer_ptr);
        let handler_entry = match crate::frame::cores::observer_read_full_cap_entry(
            observer_ptr,
            crate::capability::SLOT_FAULT_HANDLER,
        ) {
            Some(e) => e,
            None => return self.fatal_fault(observer_ptr, &fault),
        };
        // Need handler Field's ObjectId for the arena lookup below.
        let obj_id = match handler_entry.object {
            Some((_obj_type, id)) => id,
            None => return self.fatal_fault(observer_ptr, &fault),
        };

        if crate::frame::cores::observer_set_faulted(observer_ptr).is_err() {
            return self.fatal_fault(observer_ptr, &fault);
        }

        // Single lock scope: validate generation and deliver atomically.
        let mut fields = kernel_state.fields.acquire();
        let live_gen = match fields.get(obj_id) {
            Some(f) => f.generation.load(Ordering::Acquire),
            None => {
                drop(fields);

                return self.fatal_fault(observer_ptr, &fault);
            }
        };
        let (handler_field_id, handler_badge) =
            match crate::fault::validate_handler_cap(&handler_entry, live_gen) {
                Some(pair) => pair,
                None => {
                    drop(fields);

                    return self.fatal_fault(observer_ptr, &fault);
                }
            };
        let handler_field = match fields.get_mut(handler_field_id) {
            Some(f) => f,
            None => {
                drop(fields);

                return self.fatal_fault(observer_ptr, &fault);
            }
        };
        let outcome = crate::fault::deliver_fault(
            fault,
            handler_field,
            handler_badge,
            observer_id,
            observer_generation,
        );

        drop(fields);

        // The faulted Observer must be removed from the scheduler queue
        // regardless of delivery outcome. It stays in Faulted state until
        // the handler resumes it.
        self.scheduler.dequeue(observer_ptr);

        match outcome {
            crate::fault::FaultDeliveryOutcome::Enqueued => self.schedule_next(),
            crate::fault::FaultDeliveryOutcome::WokeReceiver(receiver_ptr, message) => {
                Self::deliver_message(receiver_ptr, &message);

                let _ = crate::frame::cores::observer_unblock(receiver_ptr);

                self.scheduler.enqueue(receiver_ptr);
                self.schedule_next()
            }
            crate::fault::FaultDeliveryOutcome::Deferred => {
                // D18: the faulting Observer should be linked into the
                // handler Field's pending list. Pending list linkage is
                // not yet wired — the fault stays deferred until the
                // handler Field drains a slot.
                self.schedule_next()
            }
            crate::fault::FaultDeliveryOutcome::HandlerUnavailable => {
                self.fatal_fault(observer_ptr, &fault)
            }
        }
    }

    /// D68 chain terminus: no handler available, system has no recovery path.
    #[cfg(any(target_os = "none", test))]
    #[cfg_attr(not(target_os = "none"), allow(unused_variables))]
    fn fatal_fault(
        &self,
        observer_ptr: NonNull<Observer>,
        fault: &crate::fault::FaultType,
    ) -> DispatchResult {
        #[cfg(target_os = "none")]
        {
            let pc = crate::frame::cores::observer_read_pc(observer_ptr);
            let data = fault.data_words();
            let label = fault.label();

            crate::println!();
            crate::println!("FATAL FAULT: handler unavailable (D68/D100)");
            crate::println!("  label: 0x{label:016x}");
            crate::println!(
                "  data:  [{:016x}, {:016x}, {:016x}, {:016x}]",
                data[0],
                data[1],
                data[2],
                data[3]
            );
            crate::println!("  PC:    0x{pc:016x}");
            crate::println!();
        }

        DispatchResult::FatalFault
    }
}

// ── Validation helpers (D67, D77) ─────────────────────────────────
//
// These extract the repeated "acquire arena, look up object, check
// generation" pattern used by typed operation dispatch. Each is a
// single verification boundary: one function, one Verus contract.

/// Acquire the observers arena, validate the target exists and the cap
/// entry's generation matches, extract a NonNull pointer, and release
/// the lock.
///
/// The pointer remains valid after lock release because arena slots are
/// stable for the object's lifetime (D70: no compaction).
///
/// Verus contract:
///   requires: object_id from a resolved cap entry
///   ensures:  Ok(ptr) => ptr refers to an Observer whose generation
///             matches entry.stored_generation
///             Err(InvalidCap) => arena slot is empty
///             Err(StaleCap) => object exists but generation mismatch
#[cfg(any(target_os = "none", test))]
fn validated_observer_ptr(
    kernel_state: &KernelState,
    object_id: ObjectId,
    entry: &crate::capability::Entry,
) -> Result<NonNull<Observer>, crate::syscall::SyscallError> {
    let mut observers = kernel_state.observers.acquire();
    let observer = observers
        .get_mut(object_id)
        .ok_or(crate::syscall::SyscallError::InvalidCap)?;
    let live_gen = observer.generation.load(Ordering::Acquire);

    if !entry.check_generation(live_gen) {
        return Err(crate::syscall::SyscallError::StaleCap);
    }

    Ok(NonNull::from(&mut *observer))
}

/// Acquire the observers arena, validate the target, and call `f` with
/// a mutable reference to the Observer while the lock is held.
///
/// For operations that need to mutate the Observer under the lock
/// (ObserverSuspend, ObserverSetScheduling) rather than extracting a
/// pointer for post-lock use.
#[cfg(any(target_os = "none", test))]
fn with_validated_observer_mut<R>(
    kernel_state: &KernelState,
    object_id: ObjectId,
    entry: &crate::capability::Entry,
    f: impl FnOnce(&mut Observer) -> R,
) -> Result<R, crate::syscall::SyscallError> {
    let mut observers = kernel_state.observers.acquire();
    let observer = observers
        .get_mut(object_id)
        .ok_or(crate::syscall::SyscallError::InvalidCap)?;
    let live_gen = observer.generation.load(Ordering::Acquire);

    if !entry.check_generation(live_gen) {
        return Err(crate::syscall::SyscallError::StaleCap);
    }

    Ok(f(observer))
}

/// Check that the sender has a free cap slot for the return Space cap
/// before committing to a destroy operation (D98).
#[cfg(any(target_os = "none", test))]
fn check_destroy_backing(
    sender_ptr: NonNull<Observer>,
    backing_size: usize,
) -> Result<(), crate::syscall::SyscallError> {
    if backing_size > 0 && !crate::frame::cores::observer_has_free_slot(sender_ptr) {
        return Err(crate::syscall::SyscallError::TableFull);
    }

    Ok(())
}

// ── Creation-path helpers (D95, D32) ────────────────────────────────

/// Verify a Space's generation and return its backing address and size.
///
/// Shared by CreateField, CreatePulsar, and CreateObserver — all three
/// creation operations require a valid, non-stale Space as their target.
#[cfg(any(target_os = "none", test))]
fn verify_space(
    kernel_state: &KernelState,
    space_id: ObjectId,
    stored_gen: u64,
) -> Result<(usize, usize), crate::syscall::SyscallError> {
    let spaces = kernel_state.spaces.acquire();
    let space = spaces
        .get(space_id)
        .ok_or(crate::syscall::SyscallError::InvalidCap)?;
    let live_gen = space.generation.load(Ordering::Acquire);

    if stored_gen != live_gen {
        return Err(crate::syscall::SyscallError::StaleCap);
    }

    Ok((space.va_base, space.size))
}

/// Consume a Space: bump generation (revoke all caps) and free the slot.
///
/// D32 type conversion: the Space's backing memory is repurposed for the
/// new object type. Generation bump invalidates all outstanding caps.
/// D26 auto-unmapping: remove a Space's page table entries from an Observer.
///
/// Called after Close determines the Observer no longer holds any cap to
/// this Space. Looks up the Space's metadata and delegates to
/// `unwire_space_mapping` for the actual page table update + TLB invalidation.
#[cfg(target_os = "none")]
fn unwire_space_for_observer(
    observer_ptr: NonNull<Observer>,
    space_id: ObjectId,
    kernel_state: &KernelState,
) {
    let (va_base, l3_pa, page_count) = {
        let spaces = kernel_state.spaces.acquire();

        match spaces.get(space_id) {
            Some(space) => {
                let pc = space.size / crate::frame::arch::mmu::page_size();
                (space.va_base, space.l3_table_pa, pc)
            }
            None => return,
        }
    };

    let (pt_root, asid) = crate::frame::cores::observer_page_table_info(observer_ptr);

    if pt_root == 0 {
        return;
    }

    crate::frame::mapping::unwire_space_mapping(
        pt_root,
        va_base,
        l3_pa,
        page_count,
        asid,
        kernel_state,
    );
}

#[cfg(any(target_os = "none", test))]
fn consume_space(kernel_state: &KernelState, space_id: ObjectId) {
    let mut spaces = kernel_state.spaces.acquire();

    if let Some(space) = spaces.get(space_id) {
        space.generation.fetch_add(1, Ordering::Release);
    }

    spaces.free(space_id);
}

/// D98 reverse type conversion: allocate a Space from the freed backing
/// memory and install it in the sender's cap table. Returns the encoded
/// handle, or 0 if backing_size is zero or allocation/install fails.
#[cfg(any(target_os = "none", test))]
fn return_backing_space(
    kernel_state: &KernelState,
    sender_ptr: NonNull<Observer>,
    backing_va: usize,
    backing_size: usize,
) -> u64 {
    if backing_size == 0 {
        return 0;
    }

    let return_space_id = {
        let mut spaces = kernel_state.spaces.acquire();

        match spaces.allocate() {
            Ok((id, space)) => {
                space.va_base = backing_va;
                space.size = backing_size;
                space.l3_table_pa = 0;
                space.refcount = 1;
                space.generation = core::sync::atomic::AtomicU64::new(0);

                id
            }
            Err(_) => return 0,
        }
    };
    let tcap = crate::capability::TransferredCap {
        object_type: crate::capability::ObjectType::Space,
        object_id: return_space_id,
        rights: crate::capability::Rights::SPACE_ALL,
        badge: crate::capability::Badge(0),
        send_once: false,
        stored_generation: 0,
    };

    crate::frame::cores::observer_install_transferred_cap(sender_ptr, &tcap).unwrap_or_default()
}

/// Resolve a secondary Field cap argument, verify type/rights/generation.
///
/// Shared by CreatePulsar (delivery field) and CreateObserver (handler field).
/// Returns (field_id, stored_generation) on success.
#[cfg(any(target_os = "none", test))]
fn resolve_field_argument(
    handle: u64,
    cap_entries: NonNull<crate::capability::Entry>,
    cap_capacity: u32,
    kernel_state: &KernelState,
) -> Result<(ObjectId, u64), crate::syscall::SyscallError> {
    use crate::capability::{self, ObjectType, Rights};
    use crate::syscall::SyscallError;

    let entry = capability::resolve_cap_entry(handle, cap_entries, cap_capacity)
        .map_err(SyscallError::from)?;
    let (obj_type, field_id) = entry.object.ok_or(SyscallError::InvalidCap)?;

    if obj_type != ObjectType::Field {
        return Err(SyscallError::WrongType);
    }
    if !entry.check_rights(Rights::SEND) {
        return Err(SyscallError::NoRight);
    }

    let fields = kernel_state.fields.acquire();
    let field = fields.get(field_id).ok_or(SyscallError::InvalidCap)?;
    let live_gen = field.generation.load(Ordering::Acquire);

    if !entry.check_generation(live_gen) {
        return Err(SyscallError::StaleCap);
    }

    Ok((field_id, entry.stored_generation))
}

/// Resolve a secondary Space cap argument for FieldSplit (D99).
///
/// Verifies the cap entry is a Space with SPLIT right, checks generation,
/// and returns the Space's backing address and size in a single lock
/// acquisition. The Space is being consumed for the new sub-Field.
#[cfg(any(target_os = "none", test))]
fn resolve_space_argument(
    handle: u64,
    cap_entries: NonNull<crate::capability::Entry>,
    cap_capacity: u32,
    kernel_state: &KernelState,
) -> Result<(ObjectId, usize, usize), crate::syscall::SyscallError> {
    use crate::capability::{self, ObjectType, Rights};
    use crate::syscall::SyscallError;

    let entry = capability::resolve_cap_entry(handle, cap_entries, cap_capacity)
        .map_err(SyscallError::from)?;
    let (obj_type, space_id) = entry.object.ok_or(SyscallError::InvalidCap)?;

    if obj_type != ObjectType::Space {
        return Err(SyscallError::WrongType);
    }
    if !entry.check_rights(Rights::SPLIT) {
        return Err(SyscallError::NoRight);
    }

    let spaces = kernel_state.spaces.acquire();
    let space = spaces.get(space_id).ok_or(SyscallError::InvalidCap)?;
    let live_gen = space.generation.load(Ordering::Acquire);

    if !entry.check_generation(live_gen) {
        return Err(SyscallError::StaleCap);
    }

    Ok((space_id, space.va_base, space.size))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{Badge, ObjectType, Rights};
    use crate::kernel_state::{IrqRoute, IrqRoutingTable, MAX_IRQS};
    use crate::observer::Observer;
    use crate::space_manager::SpaceManager;
    use crate::syscall::{IpcOperation, TypedOperation};
    use crate::time_manager::round_robin::RoundRobin;

    /// 24 MHz — typical ARM generic timer frequency.
    const TEST_COUNTER_FREQ: u64 = 24_000_000;

    fn make_observer() -> Observer {
        Observer::test_default()
    }

    fn make_core_state() -> CoreState<RoundRobin> {
        CoreState {
            core_id: CoreId(0),
            current: None,
            scheduler: RoundRobin::new(),
            deadlines: [None; MAX_DEADLINES_PER_CORE],
            deadline_count: 0,
            cascade_continuation: None,
        }
    }

    fn make_space_manager() -> SpaceManager {
        SpaceManager::test_default()
    }

    fn make_kernel_state() -> KernelState {
        KernelState::new(make_space_manager(), 16)
    }

    // ── Spec verifier tests ──────────────────────────────────────────

    #[test]
    fn test_d46_schedule_next_returns_idle_when_empty() {
        let mut core = make_core_state();
        let result = core.schedule_next();

        assert!(
            matches!(result, DispatchResult::Idle),
            "D46: empty scheduler must return Idle (WFI)"
        );
    }

    #[test]
    fn test_d59_schedule_next_returns_resume_when_runnable() {
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.scheduler.enqueue(ptr);

        let result = core.schedule_next();

        match result {
            DispatchResult::Resume(resumed) | DispatchResult::ResumeFastPath(resumed) => {
                assert_eq!(
                    resumed, ptr,
                    "D59: schedule_next must resume the head of the run queue"
                );
            }
            DispatchResult::Idle => panic!("must resume when run queue is non-empty"),
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    #[test]
    fn test_d2_handle_timer_calls_on_preempt_and_schedules() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        core.scheduler.enqueue(ptr_a);
        core.scheduler.enqueue(ptr_b);

        assert_eq!(core.scheduler.pick_next(), Some(ptr_a));

        let result = core.handle_timer(1000, &ks, TEST_COUNTER_FREQ);

        match result {
            DispatchResult::Resume(resumed) | DispatchResult::ResumeFastPath(resumed) => {
                assert_eq!(
                    resumed, ptr_b,
                    "D2: handle_timer must rotate (on_preempt) then pick next — B after A"
                );
            }
            DispatchResult::Idle => panic!("must resume after timer with runnable Observers"),
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    #[test]
    fn test_d46_handle_timer_returns_idle_when_no_runnable() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let result = core.handle_timer(1000, &ks, TEST_COUNTER_FREQ);

        assert!(
            matches!(result, DispatchResult::Idle),
            "D46: handle_timer with empty queue must return Idle"
        );
    }

    #[test]
    fn test_d2_handle_timer_single_observer_no_switch() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.scheduler.enqueue(ptr);

        let result = core.handle_timer(1000, &ks, TEST_COUNTER_FREQ);

        match result {
            DispatchResult::Resume(resumed) | DispatchResult::ResumeFastPath(resumed) => {
                assert_eq!(
                    resumed, ptr,
                    "D2: single Observer must continue running after timer"
                );
            }
            DispatchResult::Idle => panic!("must resume the single runnable Observer"),
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    #[test]
    fn test_d2_handle_timer_preempts_current_observer() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        core.current = Some(ptr_a);

        core.scheduler.enqueue(ptr_a);
        core.scheduler.enqueue(ptr_b);

        let result = core.handle_timer(1000, &ks, TEST_COUNTER_FREQ);

        match result {
            DispatchResult::Resume(resumed) | DispatchResult::ResumeFastPath(resumed) => {
                assert_eq!(
                    resumed, ptr_b,
                    "D2: timer must preempt current Observer (A at queue head) and switch to B"
                );
            }
            DispatchResult::Idle => {
                panic!("must not idle with current Observer and runnable queue")
            }
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    #[test]
    fn test_d48_dispatch_ipc_yield_schedules_next() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut current = make_observer();
        let mut next = make_observer();
        let current_ptr = NonNull::from(&mut current);
        let next_ptr = NonNull::from(&mut next);

        // Scheduler invariant: current is in queue at head.
        core.scheduler.enqueue(current_ptr);
        core.scheduler.enqueue(next_ptr);

        core.current = Some(current_ptr);

        let result = core.dispatch_ipc(IpcOperation::Yield, &ks);

        match result {
            DispatchResult::Resume(resumed) | DispatchResult::ResumeFastPath(resumed) => {
                // D79: Yield rotates current to tail via on_preempt.
                // Queue was [current, next], becomes [next, current].
                // pick_next returns next.
                assert_eq!(
                    resumed, next_ptr,
                    "D79: Yield must rotate current to tail and schedule next"
                );
            }
            DispatchResult::Idle => {
                panic!("D48: Yield with runnable Observer in queue must not Idle")
            }
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    #[test]
    fn test_dispatch_ipc_no_current_returns_idle() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let result = core.dispatch_ipc(IpcOperation::Send, &ks);

        assert!(
            matches!(result, DispatchResult::Idle),
            "dispatch_ipc with no current Observer must return Idle"
        );
    }

    #[test]
    fn test_dispatch_typed_no_current_returns_idle() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let result = core.dispatch_typed(TypedOperation::ObserverResume, &ks);

        assert!(
            matches!(result, DispatchResult::Idle),
            "dispatch_typed with no current Observer must return Idle"
        );
    }

    #[test]
    fn test_handle_irq_schedules_next() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.scheduler.enqueue(ptr);

        let result = core.handle_irq(42, &ks);

        match result {
            DispatchResult::Resume(resumed) | DispatchResult::ResumeFastPath(resumed) => {
                assert_eq!(resumed, ptr, "handle_irq must schedule_next after handling");
            }
            DispatchResult::Idle => panic!("handle_irq with runnable Observer must not Idle"),
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    // ── Adversarial tests ────────────────────────────────────────────

    #[test]
    fn test_adversarial_handle_timer_rotation_cycle() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut observers: [Observer; 3] = core::array::from_fn(|_| make_observer());
        let ptrs: [NonNull<Observer>; 3] =
            core::array::from_fn(|i| NonNull::from(&mut observers[i]));

        for ptr in &ptrs {
            core.scheduler.enqueue(*ptr);
        }

        let expected_after_each_timer = [ptrs[1], ptrs[2], ptrs[0]];

        for (i, expected) in expected_after_each_timer.iter().enumerate() {
            let result = core.handle_timer(1000, &ks, TEST_COUNTER_FREQ);

            match result {
                DispatchResult::Resume(resumed) | DispatchResult::ResumeFastPath(resumed) => {
                    assert_eq!(
                        resumed, *expected,
                        "timer tick {i}: rotation must advance to the next Observer"
                    );
                }
                DispatchResult::Idle => panic!("timer tick {i}: must not Idle with 3 Observers"),
                DispatchResult::FatalFault => panic!("unexpected FatalFault"),
            }
        }
    }

    #[test]
    fn test_adversarial_dispatch_ipc_all_operations_with_no_current() {
        let ks = make_kernel_state();
        let mut core = make_core_state();

        for op in [
            IpcOperation::Send,
            IpcOperation::Receive,
            IpcOperation::Call,
            IpcOperation::ReplyRecv,
            IpcOperation::Yield,
        ] {
            let result = core.dispatch_ipc(op, &ks);

            assert!(
                matches!(result, DispatchResult::Idle),
                "dispatch_ipc({op:?}) with no current must return Idle"
            );
        }
    }

    #[test]
    fn test_adversarial_dispatch_typed_all_operations_with_no_current() {
        let ks = make_kernel_state();
        let mut core = make_core_state();

        for code in 0..=19u16 {
            let op = TypedOperation::from_code(code).unwrap();
            let result = core.dispatch_typed(op, &ks);

            assert!(
                matches!(result, DispatchResult::Idle),
                "dispatch_typed(code={code}) with no current must return Idle"
            );
        }
    }

    #[test]
    fn test_adversarial_handle_irq_with_no_runnable() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let result = core.handle_irq(0, &ks);

        assert!(
            matches!(result, DispatchResult::Idle),
            "handle_irq with empty queue must return Idle"
        );
    }

    #[test]
    fn test_adversarial_repeated_timer_ticks_cycle_correctly() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        core.scheduler.enqueue(ptr_a);
        core.scheduler.enqueue(ptr_b);

        for i in 0..10 {
            let result = core.handle_timer(1000, &ks, TEST_COUNTER_FREQ);
            let expected = if i % 2 == 0 { ptr_b } else { ptr_a };

            match result {
                DispatchResult::Resume(resumed) => {
                    assert_eq!(
                        resumed, expected,
                        "tick {i}: 2-Observer rotation must alternate"
                    );
                }
                DispatchResult::Idle => panic!("tick {i}: must not Idle"),
                DispatchResult::ResumeFastPath(_) => {
                    panic!("tick {i}: timer must not use fast path")
                }
                DispatchResult::FatalFault => panic!("unexpected FatalFault"),
            }
        }
    }

    // ── D76 dispatch entry contract tests ───────────────────────────

    #[test]
    fn test_d76_dispatch_result_resume_fast_path_is_distinct() {
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);
        let normal = DispatchResult::Resume(ptr);
        let fast = DispatchResult::ResumeFastPath(ptr);
        let idle = DispatchResult::Idle;

        assert!(matches!(normal, DispatchResult::Resume(_)));
        assert!(matches!(fast, DispatchResult::ResumeFastPath(_)));
        assert!(matches!(idle, DispatchResult::Idle));
        assert!(!matches!(normal, DispatchResult::ResumeFastPath(_)));
        assert!(!matches!(fast, DispatchResult::Resume(_)));
    }

    #[test]
    fn test_d76_yield_returns_without_register_access() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        // Scheduler invariant: current is in the queue.
        core.scheduler.enqueue(ptr);

        core.current = Some(ptr);

        let result = core.dispatch_ipc(IpcOperation::Yield, &ks);

        match result {
            DispatchResult::Resume(_) => {}
            DispatchResult::Idle => {}
            DispatchResult::ResumeFastPath(_) => {
                panic!("D76: Yield must not use fast path")
            }
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    #[test]
    fn test_d76_handle_timer_receives_tick_snapshot() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs_a = make_observer();
        let mut obs_b = make_observer();
        let ptr_a = NonNull::from(&mut obs_a);
        let ptr_b = NonNull::from(&mut obs_b);

        core.scheduler.enqueue(ptr_a);
        core.scheduler.enqueue(ptr_b);

        let result_low = core.handle_timer(100, &ks, TEST_COUNTER_FREQ);

        assert!(matches!(result_low, DispatchResult::Resume(_)));

        let result_high = core.handle_timer(u64::MAX, &ks, TEST_COUNTER_FREQ);

        assert!(matches!(result_high, DispatchResult::Resume(_)));
    }

    // ── D83 per-core data organization tests ───────────────────────

    #[test]
    fn test_d83_deadline_entry_size() {
        // DeadlineEntry must be reasonably compact for a 32-element array.
        // 8 (u64) + 4 (ObjectId) + 4 (ObjectId) = 16 bytes.
        assert_eq!(
            core::mem::size_of::<DeadlineEntry>(),
            16,
            "D83: DeadlineEntry must be 16 bytes"
        );
    }

    #[test]
    fn test_d83_max_deadlines_per_core_is_32() {
        assert_eq!(
            MAX_DEADLINES_PER_CORE, 32,
            "D83: hard cap of 32 deadlines per core"
        );
    }

    #[test]
    fn test_d83_core_state_deadlines_initialized_empty() {
        let core = make_core_state();

        assert_eq!(
            core.deadline_count, 0,
            "D83: new CoreState must have zero deadlines"
        );

        for (i, slot) in core.deadlines.iter().enumerate() {
            assert!(
                slot.is_none(),
                "D83: deadline slot {i} must be None on initialization"
            );
        }
    }

    #[test]
    fn test_d83_deadline_entry_fields_roundtrip() {
        let entry = DeadlineEntry {
            deadline_ticks: 1_000_000,
            pulsar_id: crate::arena::ObjectId(42),
            field_id: crate::arena::ObjectId(7),
        };

        assert_eq!(entry.deadline_ticks, 1_000_000);
        assert_eq!(entry.pulsar_id, crate::arena::ObjectId(42));
        assert_eq!(entry.field_id, crate::arena::ObjectId(7));
    }

    #[test]
    fn test_d83_deadline_array_dense_packing_invariant() {
        // Verify the dense-packing invariant: active entries in
        // 0..deadline_count, None entries in deadline_count..MAX.
        let mut core = make_core_state();

        // Add 3 deadlines.
        for i in 0..3 {
            core.deadlines[i] = Some(DeadlineEntry {
                deadline_ticks: (i as u64 + 1) * 1000,
                pulsar_id: crate::arena::ObjectId(i as u32),
                field_id: crate::arena::ObjectId(0),
            });
        }

        core.deadline_count = 3;

        // Verify invariant: 0..3 are Some, 3..32 are None.
        for i in 0..core.deadline_count {
            assert!(
                core.deadlines[i].is_some(),
                "D83: deadline[{i}] must be Some (within deadline_count)"
            );
        }
        for i in core.deadline_count..MAX_DEADLINES_PER_CORE {
            assert!(
                core.deadlines[i].is_none(),
                "D83: deadline[{i}] must be None (beyond deadline_count)"
            );
        }
    }

    #[test]
    fn test_d83_deadline_entry_is_copy() {
        // DeadlineEntry must be Copy for efficient array operations
        // (swap-remove on expiry).
        let entry = DeadlineEntry {
            deadline_ticks: 500,
            pulsar_id: crate::arena::ObjectId(1),
            field_id: crate::arena::ObjectId(2),
        };
        let copied = entry;

        assert_eq!(copied.deadline_ticks, entry.deadline_ticks);
        assert_eq!(copied.pulsar_id, entry.pulsar_id);
        assert_eq!(copied.field_id, entry.field_id);
    }

    #[test]
    fn test_d83_deadline_array_total_size() {
        // 32 entries * Option<DeadlineEntry> size.
        // Option<DeadlineEntry> should be 32 bytes (24 + discriminant + padding).
        let option_size = core::mem::size_of::<Option<DeadlineEntry>>();
        let array_size = option_size * MAX_DEADLINES_PER_CORE;

        // The array must fit comfortably in per-core state.
        // 32 * 32 = 1024 bytes. Acceptable for per-core data.
        assert!(
            array_size <= 2048,
            "D83: deadline array must be under 2 KiB (got {array_size} bytes)"
        );
    }

    #[test]
    fn test_d83_core_state_with_deadlines_still_dispatches() {
        // Verify that adding deadline fields does not break existing
        // dispatch behavior.
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        // Add a deadline entry.
        core.deadlines[0] = Some(DeadlineEntry {
            deadline_ticks: 5000,
            pulsar_id: crate::arena::ObjectId(0),
            field_id: crate::arena::ObjectId(0),
        });
        core.deadline_count = 1;

        // Dispatch must still work normally.
        core.scheduler.enqueue(ptr);

        let result = core.schedule_next();

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, ptr,
                    "D83: dispatch must work with deadlines present"
                );
            }
            _ => panic!("D83: must resume with runnable Observer"),
        }
    }

    // ── D81 hardware event protocol tests ───────────────────────────

    // ── D81: IRQ routing table ──────────────────────────────────────

    #[test]
    fn test_d81_irq_routing_table_new_is_empty() {
        let table = IrqRoutingTable::new();

        for i in 0..MAX_IRQS {
            assert!(
                table.routes[i].is_none(),
                "D81: new routing table must have all None entries"
            );
        }
    }

    #[test]
    fn test_d81_irq_routing_table_max_irqs_is_1024() {
        assert_eq!(MAX_IRQS, 1024, "D81: max IRQs must be 1024");
    }

    #[test]
    fn test_d81_irq_route_lookup_empty_returns_none() {
        let table = IrqRoutingTable::new();
        let result = table.lookup(42);

        assert!(
            result.is_none(),
            "D81: lookup on empty table must return None"
        );
    }

    #[test]
    fn test_d81_irq_route_install_and_lookup() {
        let mut table = IrqRoutingTable::new();
        let route = IrqRoute {
            field_id: ObjectId(7),
            badge: Badge(0xDEAD),
            generation: 0,
        };
        let was_occupied = table.install(42, route);

        assert_eq!(
            was_occupied,
            Some(false),
            "D81: first install must return false"
        );

        let found = table
            .lookup(42)
            .expect("D81: installed route must be found");

        assert_eq!(found.field_id, ObjectId(7));
        assert_eq!(found.badge, Badge(0xDEAD));
        assert_eq!(found.generation, 0);
    }

    #[test]
    fn test_d81_irq_route_install_overwrites() {
        let mut table = IrqRoutingTable::new();

        table.install(
            100,
            IrqRoute {
                field_id: ObjectId(1),
                badge: Badge(1),
                generation: 0,
            },
        );

        let was_occupied = table.install(
            100,
            IrqRoute {
                field_id: ObjectId(2),
                badge: Badge(2),
                generation: 1,
            },
        );

        assert_eq!(was_occupied, Some(true), "D81: overwrite must return true");

        let found = table.lookup(100).unwrap();

        assert_eq!(found.field_id, ObjectId(2));
        assert_eq!(found.badge, Badge(2));
    }

    #[test]
    fn test_d81_irq_route_remove() {
        let mut table = IrqRoutingTable::new();

        table.install(
            50,
            IrqRoute {
                field_id: ObjectId(3),
                badge: Badge(3),
                generation: 0,
            },
        );

        let removed = table.remove(50);

        assert!(
            removed.is_some(),
            "D81: remove of installed route must return Some"
        );
        assert_eq!(removed.unwrap().field_id, ObjectId(3));
        assert!(
            table.lookup(50).is_none(),
            "D81: removed route must not be found"
        );
    }

    #[test]
    fn test_d81_irq_route_out_of_range() {
        let mut table = IrqRoutingTable::new();

        assert!(
            table.lookup(1024).is_none(),
            "D81: INTID 1024 must be out of range"
        );
        assert!(
            table.lookup(u32::MAX).is_none(),
            "D81: INTID u32::MAX must be out of range"
        );
        assert_eq!(
            table.install(
                1024,
                IrqRoute {
                    field_id: ObjectId(0),
                    badge: Badge(0),
                    generation: 0,
                }
            ),
            None,
            "D81: install at INTID 1024 must return None"
        );
        assert_eq!(
            table.remove(1024),
            None,
            "D81: remove at INTID 1024 must return None"
        );
    }

    #[test]
    fn test_d81_irq_route_boundary_intids() {
        let mut table = IrqRoutingTable::new();

        // INTID 0 — valid.
        table.install(
            0,
            IrqRoute {
                field_id: ObjectId(0),
                badge: Badge(0),
                generation: 0,
            },
        );

        assert!(table.lookup(0).is_some());

        // INTID 1023 — last valid.
        table.install(
            1023,
            IrqRoute {
                field_id: ObjectId(1023),
                badge: Badge(1023),
                generation: 0,
            },
        );

        assert!(table.lookup(1023).is_some());
        assert_eq!(table.lookup(1023).unwrap().field_id, ObjectId(1023));
    }

    // ── D81: handle_irq with routing ────────────────────────────────

    #[test]
    // Field allocation requires a non-zero queue pointer at construction
    // time. The slab allocator zeroes slots. Fixing this requires either
    // an Arena::allocate_with(init_fn) pattern or making Field's queue
    // pointer nullable. Pre-existing issue from D81 — not introduced by D77.
    fn test_d81_handle_irq_delivers_to_routed_field() {
        let ks = make_kernel_state();
        // Allocate a Field in the arena.
        let field_id = {
            let mut fields = ks.fields.acquire();
            let (id, field) = fields.allocate().expect("allocate field");

            // Initialize queue so enqueue works.
            field.queue = crate::frame::fields::alloc_test_queue(8);
            field.queue_capacity = 8;

            id
        };

        // Install IRQ route.
        {
            let mut routes = ks.irq_routes.acquire();

            routes.install(
                42,
                IrqRoute {
                    field_id,
                    badge: Badge(0xBEEF),
                    generation: 0,
                },
            );
        }

        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.scheduler.enqueue(ptr);

        let result = core.handle_irq(42, &ks);

        assert!(matches!(result, DispatchResult::Resume(_)));

        // Verify message was enqueued.
        let mut fields = ks.fields.acquire();
        let target = fields.get_mut(field_id).unwrap();

        assert_eq!(target.queue_length, 1, "D81: IRQ message must be enqueued");

        let msg = target.dequeue().unwrap();

        assert_eq!(msg.label, field::LABEL_DEVICE_IRQ);
        assert_eq!(msg.badge, Badge(0xBEEF));
        assert_eq!(msg.data[0], 42, "D81: data[0] must carry INTID");
    }

    #[test]
    fn test_d81_handle_irq_no_route_ignores() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.scheduler.enqueue(ptr);

        // No route installed for INTID 99.
        let result = core.handle_irq(99, &ks);

        assert!(
            matches!(result, DispatchResult::Resume(_)),
            "D81: unrouted IRQ must still schedule_next"
        );
    }

    #[test]
    fn test_d81_handle_irq_generation_mismatch_skips() {
        let ks = make_kernel_state();
        let field_id = {
            let mut fields = ks.fields.acquire();
            let (id, field) = fields.allocate().expect("allocate field");

            field.queue = crate::frame::fields::alloc_test_queue(8);
            field.queue_capacity = 8;

            // Revoke the field — generation becomes 1.
            field.revoke();

            id
        };

        // Install route with generation 0 (now stale).
        {
            let mut routes = ks.irq_routes.acquire();

            routes.install(
                10,
                IrqRoute {
                    field_id,
                    badge: Badge(1),
                    generation: 0,
                },
            );
        }

        let mut core = make_core_state();
        let mut obs = make_observer();

        core.scheduler.enqueue(NonNull::from(&mut obs));
        core.handle_irq(10, &ks);

        // Message must NOT have been enqueued (stale route).
        let fields = ks.fields.acquire();
        let target = fields.get(field_id).unwrap();

        assert_eq!(
            target.queue_length, 0,
            "D81: stale route (generation mismatch) must not deliver"
        );
    }

    // ── D81: handle_timer with deadlines ────────────────────────────

    #[test]
    fn test_d81_handle_timer_fires_expired_one_shot() {
        let ks = make_kernel_state();
        // Create a Pulsar in the arena (one-shot: period_ns = 0).
        let (pulsar_id, field_id) = {
            let mut pulsars = ks.pulsars.acquire();
            let (pid, pulsar) = pulsars.allocate().expect("allocate pulsar");

            *pulsar = crate::pulsar::Pulsar::new(
                ObjectId(0), // placeholder delivery field
                Badge(0x42),
                1_000_000, // 1ms duration
                0,         // one-shot
                TEST_COUNTER_FREQ,
                0, // now_ticks
            );

            let mut fields = ks.fields.acquire();
            let (fid, field) = fields.allocate().expect("allocate field");

            field.queue = crate::frame::fields::alloc_test_queue(8);
            field.queue_capacity = 8;
            // Update pulsar delivery field.
            pulsar.delivery_field = fid;

            (pid, fid)
        };

        let mut core = make_core_state();
        let mut obs = make_observer();

        core.scheduler.enqueue(NonNull::from(&mut obs));

        // Install a deadline for the pulsar.
        core.deadlines[0] = Some(DeadlineEntry {
            deadline_ticks: 24, // 1ms at 24MHz = 24000, but use small value
            pulsar_id,
            field_id,
        });
        core.deadline_count = 1;

        // Fire timer with current_ticks > deadline.
        core.handle_timer(100, &ks, TEST_COUNTER_FREQ);

        // One-shot must be removed.
        assert_eq!(
            core.deadline_count, 0,
            "D81: one-shot deadline must be removed after firing"
        );

        // Message must have been enqueued.
        let mut fields = ks.fields.acquire();
        let target = fields.get_mut(field_id).unwrap();

        assert_eq!(target.queue_length, 1, "D81: fire message must be enqueued");

        let msg = target.dequeue().unwrap();

        assert_eq!(msg.label, field::LABEL_TIMER_FIRE);
        assert_eq!(msg.badge, Badge(0x42));
    }

    #[test]
    fn test_d81_handle_timer_rearms_repeating_pulsar() {
        let ks = make_kernel_state();
        let (pulsar_id, field_id) = {
            let mut pulsars = ks.pulsars.acquire();
            let (pid, pulsar) = pulsars.allocate().expect("allocate pulsar");

            *pulsar = crate::pulsar::Pulsar::new(
                ObjectId(0),
                Badge(0x99),
                1_000_000,  // 1ms duration
                10_000_000, // 10ms period (repeating)
                TEST_COUNTER_FREQ,
                0,
            );

            let mut fields = ks.fields.acquire();
            let (fid, field) = fields.allocate().expect("allocate field");

            field.queue = crate::frame::fields::alloc_test_queue(8);
            field.queue_capacity = 8;
            pulsar.delivery_field = fid;

            (pid, fid)
        };

        let mut core = make_core_state();
        let mut obs = make_observer();

        core.scheduler.enqueue(NonNull::from(&mut obs));

        // Compute expected first deadline.
        let first_deadline = {
            let pulsars = ks.pulsars.acquire();

            pulsars.get(pulsar_id).unwrap().next_deadline_ticks
        };

        core.deadlines[0] = Some(DeadlineEntry {
            deadline_ticks: first_deadline,
            pulsar_id,
            field_id,
        });
        core.deadline_count = 1;

        // Fire timer after first deadline.
        core.handle_timer(first_deadline + 1, &ks, TEST_COUNTER_FREQ);

        // Repeating: deadline must still exist but with updated tick.
        assert_eq!(
            core.deadline_count, 1,
            "D81: repeating pulsar must rearm (deadline_count stays 1)"
        );

        let updated_entry = core.deadlines[0].expect("D81: rearmed entry must be Some");

        assert!(
            updated_entry.deadline_ticks > first_deadline,
            "D81: rearmed deadline must be in the future"
        );
    }

    #[test]
    fn test_d81_handle_timer_not_expired_leaves_deadline() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs = make_observer();

        core.scheduler.enqueue(NonNull::from(&mut obs));

        // Deadline far in the future.
        core.deadlines[0] = Some(DeadlineEntry {
            deadline_ticks: 1_000_000,
            pulsar_id: ObjectId(0),
            field_id: ObjectId(0),
        });
        core.deadline_count = 1;

        // Timer tick before deadline.
        core.handle_timer(100, &ks, TEST_COUNTER_FREQ);

        assert_eq!(
            core.deadline_count, 1,
            "D81: non-expired deadline must not be removed"
        );
        assert_eq!(
            core.deadlines[0].unwrap().deadline_ticks,
            1_000_000,
            "D81: non-expired deadline must not be modified"
        );
    }

    #[test]
    fn test_d81_handle_timer_multiple_expired_fires_all() {
        let ks = make_kernel_state();
        // Create two one-shot pulsars with different fields.
        let (pid_a, fid_a) = {
            let mut pulsars = ks.pulsars.acquire();
            let (pid, pulsar) = pulsars.allocate().expect("allocate pulsar a");

            *pulsar = crate::pulsar::Pulsar::new(
                ObjectId(0),
                Badge(1),
                1_000_000,
                0,
                TEST_COUNTER_FREQ,
                0,
            );

            let mut fields = ks.fields.acquire();
            let (fid, field) = fields.allocate().expect("allocate field a");

            field.queue = crate::frame::fields::alloc_test_queue(8);
            field.queue_capacity = 8;
            pulsar.delivery_field = fid;

            (pid, fid)
        };

        let (pid_b, fid_b) = {
            let mut pulsars = ks.pulsars.acquire();
            let (pid, pulsar) = pulsars.allocate().expect("allocate pulsar b");

            *pulsar = crate::pulsar::Pulsar::new(
                ObjectId(0),
                Badge(2),
                2_000_000,
                0,
                TEST_COUNTER_FREQ,
                0,
            );

            let mut fields = ks.fields.acquire();
            let (fid, field) = fields.allocate().expect("allocate field b");

            field.queue = crate::frame::fields::alloc_test_queue(8);
            field.queue_capacity = 8;
            pulsar.delivery_field = fid;

            (pid, fid)
        };

        let mut core = make_core_state();
        let mut obs = make_observer();

        core.scheduler.enqueue(NonNull::from(&mut obs));

        core.deadlines[0] = Some(DeadlineEntry {
            deadline_ticks: 50,
            pulsar_id: pid_a,
            field_id: fid_a,
        });
        core.deadlines[1] = Some(DeadlineEntry {
            deadline_ticks: 80,
            pulsar_id: pid_b,
            field_id: fid_b,
        });
        core.deadline_count = 2;

        // Both expired at tick 100.
        core.handle_timer(100, &ks, TEST_COUNTER_FREQ);

        // Both one-shot deadlines removed.
        assert_eq!(
            core.deadline_count, 0,
            "D81: both one-shot deadlines must be removed"
        );

        // Both fields must have received messages.
        let fields = ks.fields.acquire();

        assert_eq!(
            fields.get(fid_a).unwrap().queue_length,
            1,
            "D81: field A must have one message"
        );
        assert_eq!(
            fields.get(fid_b).unwrap().queue_length,
            1,
            "D81: field B must have one message"
        );
    }

    #[test]
    fn test_d81_swap_remove_deadline_maintains_invariant() {
        let mut core = make_core_state();

        // Add 3 deadlines.
        for i in 0..3u32 {
            core.deadlines[i as usize] = Some(DeadlineEntry {
                deadline_ticks: (i as u64 + 1) * 100,
                pulsar_id: ObjectId(i),
                field_id: ObjectId(i),
            });
        }

        core.deadline_count = 3;

        // Remove middle (index 1). Last entry (index 2) should swap in.
        core.swap_remove_deadline(1);

        assert_eq!(core.deadline_count, 2);
        assert!(core.deadlines[0].is_some());
        assert!(core.deadlines[1].is_some());
        assert!(core.deadlines[2].is_none());

        // The entry at index 1 should now be what was at index 2.
        let swapped = core.deadlines[1].unwrap();

        assert_eq!(swapped.pulsar_id, ObjectId(2));
    }

    #[test]
    fn test_d81_swap_remove_deadline_last_element() {
        let mut core = make_core_state();

        core.deadlines[0] = Some(DeadlineEntry {
            deadline_ticks: 100,
            pulsar_id: ObjectId(0),
            field_id: ObjectId(0),
        });
        core.deadline_count = 1;

        core.swap_remove_deadline(0);

        assert_eq!(core.deadline_count, 0);
        assert!(core.deadlines[0].is_none());
    }

    // ── D81: IrqRoute in KernelState ────────────────────────────────

    #[test]
    fn test_d81_kernel_state_has_irq_routes() {
        let ks = make_kernel_state();
        // IRQ routes lock must be acquirable.
        let routes = ks.irq_routes.acquire();

        assert!(
            routes.lookup(0).is_none(),
            "D81: new KernelState IRQ routes must be empty"
        );
    }

    #[test]
    fn test_d81_irq_routes_lock_order_is_unordered() {
        let ks = make_kernel_state();

        assert!(
            !ks.irq_routes.order().is_ordered(),
            "D81: IrqRouting must be unordered"
        );
    }

    // ── D79 scheduling decision matrix tests ──────────────────────────

    // Helper: make an Observer with a real (test) RegisterState so
    // register write helpers can operate on it.
    fn make_observer_with_registers() -> Observer {
        crate::observer::Observer::test_with_registers()
    }

    fn make_message(label: u64, badge: u64) -> crate::field::Message {
        crate::field::Message {
            data: [label, 0, 0, 0],
            label,
            badge: Badge(badge),
            user_cap: None,
            reply_cap: None,
        }
    }

    // ── D79 Row 1: Send x Enqueued ──────────────────────────────────

    /// D79 Row 1: Send with Enqueued outcome — sender continues, no
    /// scheduling change.
    #[test]
    fn test_d79_send_enqueued_sender_continues() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let outcome = crate::communication::SendOutcome::Enqueued;
        let result = core.dispatch_send_outcome(sender_ptr, outcome);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, sender_ptr,
                    "D79 Row 1: Send Enqueued must resume the sender"
                );
            }
            _ => panic!("D79 Row 1: Send Enqueued must return Resume(sender)"),
        }

        // Sender must still be in the run queue.
        assert!(
            core.scheduler.contains(sender_ptr),
            "D79 Row 1: sender must remain in run queue"
        );
    }

    // ── D79 Row 2: Send x WokeReceiver ──────────────────────────────

    /// D79 Row 2: Send with WokeReceiver — sender continues, receiver
    /// enqueued, message written to receiver's registers.
    #[test]
    fn test_d79_send_woke_receiver_enqueues_receiver() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let mut receiver = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let message = make_message(42, 0xBEEF);
        let outcome = crate::communication::SendOutcome::WokeReceiver(receiver_ptr, message);
        let result = core.dispatch_send_outcome(sender_ptr, outcome);

        // Sender continues.
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, sender_ptr,
                    "D79 Row 2: Send WokeReceiver must resume the sender"
                );
            }
            _ => panic!("D79 Row 2: Send WokeReceiver must return Resume(sender)"),
        }

        // Receiver must be enqueued.
        assert!(
            core.scheduler.contains(receiver_ptr),
            "D79 Row 2: woken receiver must be enqueued in run queue"
        );
        // Sender must still be in queue.
        assert!(
            core.scheduler.contains(sender_ptr),
            "D79 Row 2: sender must remain in run queue"
        );
    }

    /// D79 Row 2: Verify message is written to receiver's registers.
    #[test]
    fn test_d79_send_woke_receiver_writes_registers() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let mut receiver = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(sender_ptr);

        let message = crate::field::Message {
            data: [0x1111, 0x2222, 0x3333, 0x4444],
            label: 0xABCD,
            badge: Badge(0x5555),
            user_cap: None,
            reply_cap: None,
        };
        let outcome = crate::communication::SendOutcome::WokeReceiver(receiver_ptr, message);

        core.dispatch_send_outcome(sender_ptr, outcome);

        // Read the receiver's registers to verify the message was written.
        let regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_eq!(regs.data[0], 0x1111, "D79: data[0] must be written");
        assert_eq!(regs.data[1], 0x2222, "D79: data[1] must be written");
        assert_eq!(regs.data[2], 0x3333, "D79: data[2] must be written");
        assert_eq!(regs.data[3], 0x4444, "D79: data[3] must be written");
        assert_eq!(regs.label, 0xABCD, "D79: label must be written");
        assert_eq!(regs.handle_or_badge, 0x5555, "D79: badge must be written");
    }

    // ── D79 Row 3: Receive x Received ───────────────────────────────

    /// D79 Row 3: Receive with Received — receiver continues with message.
    #[test]
    fn test_d79_receive_received_continues() {
        let mut core = make_core_state();
        let mut receiver = make_observer_with_registers();
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(receiver_ptr);

        core.scheduler.enqueue(receiver_ptr);

        let message = make_message(99, 0x42);
        let outcome = crate::communication::ReceiveOutcome::Received(message);
        let result = core.dispatch_receive_outcome(receiver_ptr, outcome);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, receiver_ptr,
                    "D79 Row 3: Receive Received must resume the receiver"
                );
            }
            _ => panic!("D79 Row 3: Receive Received must return Resume(receiver)"),
        }
    }

    /// D79 Row 3: Verify message written to receiver's registers.
    #[test]
    fn test_d79_receive_received_writes_registers() {
        let mut core = make_core_state();
        let mut receiver = make_observer_with_registers();
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(receiver_ptr);

        let message = crate::field::Message {
            data: [0xA, 0xB, 0xC, 0xD],
            label: 0xFACE,
            badge: Badge(0xBEEF),
            user_cap: None,
            reply_cap: None,
        };
        let outcome = crate::communication::ReceiveOutcome::Received(message);

        core.dispatch_receive_outcome(receiver_ptr, outcome);

        let regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_eq!(regs.data, [0xA, 0xB, 0xC, 0xD], "D79: data words");
        assert_eq!(regs.label, 0xFACE, "D79: label");
        assert_eq!(regs.handle_or_badge, 0xBEEF, "D79: badge");
    }

    // ── D79 Row 4: Receive x Blocked ────────────────────────────────

    /// D79 Row 4: Receive with Blocked — receiver dequeued, schedule_next.
    #[test]
    fn test_d79_receive_blocked_dequeues_receiver() {
        let mut core = make_core_state();
        let mut receiver = make_observer_with_registers();
        let mut next = make_observer();
        let receiver_ptr = NonNull::from(&mut receiver);
        let next_ptr = NonNull::from(&mut next);

        core.current = Some(receiver_ptr);

        core.scheduler.enqueue(receiver_ptr);
        core.scheduler.enqueue(next_ptr);

        let outcome = crate::communication::ReceiveOutcome::Blocked;
        let result = core.dispatch_receive_outcome(receiver_ptr, outcome);

        // Receiver must be dequeued.
        assert!(
            !core.scheduler.contains(receiver_ptr),
            "D79 Row 4: blocked receiver must be dequeued"
        );

        // Must resume next runnable.
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, next_ptr,
                    "D79 Row 4: must schedule next after receiver blocks"
                );
            }
            _ => panic!("D79 Row 4: must resume next Observer"),
        }
    }

    /// D79 Row 4: Receive Blocked with no other runnable — Idle.
    #[test]
    fn test_d79_receive_blocked_no_runnable_returns_idle() {
        let mut core = make_core_state();
        let mut receiver = make_observer_with_registers();
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(receiver_ptr);

        core.scheduler.enqueue(receiver_ptr);

        let outcome = crate::communication::ReceiveOutcome::Blocked;
        let result = core.dispatch_receive_outcome(receiver_ptr, outcome);

        assert!(
            matches!(result, DispatchResult::Idle),
            "D79 Row 4: Receive Blocked with empty queue must return Idle"
        );
    }

    // ── D79 Row 5: Call x Enqueued ──────────────────────────────────

    /// D79 Row 5: Call with Enqueued — sender dequeued (blocks on reply).
    #[test]
    fn test_d79_call_enqueued_dequeues_sender() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let mut next = make_observer();
        let sender_ptr = NonNull::from(&mut sender);
        let next_ptr = NonNull::from(&mut next);

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.scheduler.enqueue(next_ptr);

        let outcome = crate::communication::CallOutcome::Enqueued;
        let result = core.dispatch_call_outcome(sender_ptr, outcome);

        // Sender must be dequeued (blocking on reply field).
        assert!(
            !core.scheduler.contains(sender_ptr),
            "D79 Row 5: caller must be dequeued (blocking on reply)"
        );

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, next_ptr,
                    "D79 Row 5: must schedule next after caller blocks"
                );
            }
            _ => panic!("D79 Row 5: must resume next Observer"),
        }
    }

    // ── D79 Row 6: Call x DirectSwitch ──────────────────────────────

    /// D79 Row 6 (approved): DirectSwitch — sender dequeued, receiver
    /// direct-switched to with ResumeFastPath.
    #[test]
    fn test_d79_call_direct_switch_approved() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let mut receiver = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        // RoundRobin always approves should_switch_to.
        let outcome = crate::communication::CallOutcome::DirectSwitch(receiver_ptr);
        let result = core.dispatch_call_outcome(sender_ptr, outcome);

        // Must be ResumeFastPath (x0-x3 pass through).
        match result {
            DispatchResult::ResumeFastPath(resumed) => {
                assert_eq!(
                    resumed, receiver_ptr,
                    "D79 Row 6: DirectSwitch approved must resume receiver via fast path"
                );
            }
            _ => panic!("D79 Row 6: approved DirectSwitch must return ResumeFastPath"),
        }

        // Sender must be dequeued.
        assert!(
            !core.scheduler.contains(sender_ptr),
            "D79 Row 6: sender must be dequeued after Call"
        );
        // Receiver must NOT be in the run queue (direct-switched to, bypasses queue).
        assert!(
            !core.scheduler.contains(receiver_ptr),
            "D79 Row 6: receiver must not be in run queue (direct switch bypasses it)"
        );
    }

    // ── D79 Row 7: Call x WokeReceiverSlowPath ──────────────────────

    /// D79 Row 7: Call with user cap — sender dequeued, receiver enqueued,
    /// message written to receiver registers.
    #[test]
    fn test_d79_call_woke_receiver_slow_path() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let mut receiver = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let message = make_message(77, 0xCAFE);
        let outcome =
            crate::communication::CallOutcome::WokeReceiverSlowPath(receiver_ptr, message);
        let result = core.dispatch_call_outcome(sender_ptr, outcome);

        // Sender must be dequeued.
        assert!(
            !core.scheduler.contains(sender_ptr),
            "D79 Row 7: sender must be dequeued after Call"
        );
        // Receiver must be enqueued.
        assert!(
            core.scheduler.contains(receiver_ptr),
            "D79 Row 7: woken receiver must be enqueued"
        );

        // schedule_next picks the receiver (only runnable).
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, receiver_ptr,
                    "D79 Row 7: must schedule woken receiver"
                );
            }
            _ => panic!("D79 Row 7: must resume the woken receiver"),
        }
    }

    /// D79 Row 7: Verify message written to receiver registers on slow path.
    #[test]
    fn test_d79_call_woke_receiver_slow_path_writes_registers() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let mut receiver = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(sender_ptr);

        let message = crate::field::Message {
            data: [0xAA, 0xBB, 0xCC, 0xDD],
            label: 0x1234,
            badge: Badge(0x5678),
            user_cap: None,
            reply_cap: None,
        };
        let outcome =
            crate::communication::CallOutcome::WokeReceiverSlowPath(receiver_ptr, message);

        core.dispatch_call_outcome(sender_ptr, outcome);

        let regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_eq!(regs.data, [0xAA, 0xBB, 0xCC, 0xDD], "D79: data words");
        assert_eq!(regs.label, 0x1234, "D79: label");
        assert_eq!(regs.handle_or_badge, 0x5678, "D79: badge");
    }

    // ── D79 Row 8: ReplyRecv x Received ─────────────────────────────

    /// D79 Row 8: ReplyRecv with Received — server continues, client
    /// enqueued if reply was delivered.
    #[test]
    fn test_d79_reply_recv_received_server_continues() {
        let mut core = make_core_state();
        let mut server = make_observer_with_registers();
        let mut client = make_observer_with_registers();
        let server_ptr = NonNull::from(&mut server);
        let client_ptr = NonNull::from(&mut client);

        core.current = Some(server_ptr);

        core.scheduler.enqueue(server_ptr);

        let reply_message = make_message(100, 0x42);
        let recv_message = make_message(200, 0x99);
        let outcome = crate::communication::ReplyRecvOutcome {
            reply_delivery: Some(crate::communication::ReplyDelivery {
                client: client_ptr,
                message: reply_message,
            }),
            receive_outcome: crate::communication::ReceiveOutcome::Received(recv_message),
        };
        let result = core.dispatch_reply_recv_outcome(server_ptr, outcome);

        // Server continues with the new message.
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, server_ptr,
                    "D79 Row 8: server must continue with received message"
                );
            }
            _ => panic!("D79 Row 8: must return Resume(server)"),
        }

        // Client must be enqueued (reply delivered).
        assert!(
            core.scheduler.contains(client_ptr),
            "D79 Row 8: woken client must be enqueued"
        );
    }

    /// D79 Row 8: ReplyRecv with Received but no client waiting.
    #[test]
    fn test_d79_reply_recv_received_no_client() {
        let mut core = make_core_state();
        let mut server = make_observer_with_registers();
        let server_ptr = NonNull::from(&mut server);

        core.current = Some(server_ptr);

        core.scheduler.enqueue(server_ptr);

        let recv_message = make_message(300, 0x77);
        let outcome = crate::communication::ReplyRecvOutcome {
            reply_delivery: None,
            receive_outcome: crate::communication::ReceiveOutcome::Received(recv_message),
        };
        let result = core.dispatch_reply_recv_outcome(server_ptr, outcome);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, server_ptr,
                    "D79 Row 8: server continues even without reply delivery"
                );
            }
            _ => panic!("D79 Row 8: must Resume server"),
        }
    }

    /// D79 Row 8: Verify both reply and receive messages written correctly.
    #[test]
    fn test_d79_reply_recv_received_writes_registers() {
        let mut core = make_core_state();
        let mut server = make_observer_with_registers();
        let mut client = make_observer_with_registers();
        let server_ptr = NonNull::from(&mut server);
        let client_ptr = NonNull::from(&mut client);

        core.current = Some(server_ptr);

        let reply_message = crate::field::Message {
            data: [0xA1, 0xA2, 0xA3, 0xA4],
            label: 0xAE01,
            badge: Badge(0xCB),
            user_cap: None,
            reply_cap: None,
        };
        let recv_message = crate::field::Message {
            data: [0xB1, 0xB2, 0xB3, 0xB4],
            label: 0xBE01,
            badge: Badge(0xDB),
            user_cap: None,
            reply_cap: None,
        };
        let outcome = crate::communication::ReplyRecvOutcome {
            reply_delivery: Some(crate::communication::ReplyDelivery {
                client: client_ptr,
                message: reply_message,
            }),
            receive_outcome: crate::communication::ReceiveOutcome::Received(recv_message),
        };

        core.dispatch_reply_recv_outcome(server_ptr, outcome);

        // Client gets reply message.
        let client_regs = crate::frame::cores::read_ipc_registers(client_ptr);

        assert_eq!(client_regs.label, 0xAE01, "D79: client gets reply label");

        // Server gets new request message.
        let server_regs = crate::frame::cores::read_ipc_registers(server_ptr);

        assert_eq!(server_regs.label, 0xBE01, "D79: server gets new label");
    }

    // ── D79 Row 9: ReplyRecv x Blocked ──────────────────────────────

    /// D79 Row 9: ReplyRecv with Blocked — server dequeued, client enqueued.
    #[test]
    fn test_d79_reply_recv_blocked_server_dequeued() {
        let mut core = make_core_state();
        let mut server = make_observer_with_registers();
        let mut client = make_observer_with_registers();
        let server_ptr = NonNull::from(&mut server);
        let client_ptr = NonNull::from(&mut client);

        core.current = Some(server_ptr);

        core.scheduler.enqueue(server_ptr);

        let reply_message = make_message(50, 0);
        let outcome = crate::communication::ReplyRecvOutcome {
            reply_delivery: Some(crate::communication::ReplyDelivery {
                client: client_ptr,
                message: reply_message,
            }),
            receive_outcome: crate::communication::ReceiveOutcome::Blocked,
        };
        let result = core.dispatch_reply_recv_outcome(server_ptr, outcome);

        // Server must be dequeued (blocked).
        assert!(
            !core.scheduler.contains(server_ptr),
            "D79 Row 9: blocked server must be dequeued"
        );
        // Client must be enqueued.
        assert!(
            core.scheduler.contains(client_ptr),
            "D79 Row 9: woken client must be enqueued"
        );

        // Result: schedule next (the client).
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, client_ptr,
                    "D79 Row 9: must schedule the woken client"
                );
            }
            _ => panic!("D79 Row 9: must resume the woken client"),
        }
    }

    /// D79 Row 9: ReplyRecv Blocked with no client — server dequeued, Idle.
    #[test]
    fn test_d79_reply_recv_blocked_no_client_idle() {
        let mut core = make_core_state();
        let mut server = make_observer_with_registers();
        let server_ptr = NonNull::from(&mut server);

        core.current = Some(server_ptr);

        core.scheduler.enqueue(server_ptr);

        let outcome = crate::communication::ReplyRecvOutcome {
            reply_delivery: None,
            receive_outcome: crate::communication::ReceiveOutcome::Blocked,
        };
        let result = core.dispatch_reply_recv_outcome(server_ptr, outcome);

        assert!(
            !core.scheduler.contains(server_ptr),
            "D79 Row 9: server must be dequeued"
        );
        assert!(
            matches!(result, DispatchResult::Idle),
            "D79 Row 9: no client, no server, must return Idle"
        );
    }

    // ── D79 Row 10: Yield ────────────────────────────────────────────

    /// D79 Yield: re-enqueue at tail, pick_next returns next Observer.
    #[test]
    fn test_d79_yield_reenqueues_at_tail() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut current = make_observer();
        let mut next = make_observer();
        let current_ptr = NonNull::from(&mut current);
        let next_ptr = NonNull::from(&mut next);

        // Scheduler invariant: current is in queue at head.
        core.scheduler.enqueue(current_ptr);
        core.scheduler.enqueue(next_ptr);

        core.current = Some(current_ptr);

        let result = core.dispatch_ipc(IpcOperation::Yield, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, next_ptr,
                    "D79: Yield rotates current to tail — next Observer runs first"
                );
            }
            _ => panic!("D79: Yield with runnable Observer must not Idle"),
        }

        // Current must still be in the queue (rotated to tail).
        assert!(
            core.scheduler.contains(current_ptr),
            "D79: yielded Observer must remain in the run queue"
        );
    }

    /// D79 Yield: if only one Observer, it runs again.
    /// Scheduler invariant: the running Observer is in the queue.
    #[test]
    fn test_d79_yield_single_observer_runs_again() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        // Scheduler invariant: current is in the queue.
        core.scheduler.enqueue(ptr);

        core.current = Some(ptr);

        let result = core.dispatch_ipc(IpcOperation::Yield, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, ptr,
                    "D79: single Observer Yield must resume the same Observer"
                );
            }
            _ => panic!("D79: single Observer Yield must not Idle"),
        }
    }

    // ── D79 adversarial tests ───────────────────────────────────────

    /// D79: Send x WokeReceiver must NOT use ResumeFastPath (D50 excludes Send).
    #[test]
    fn test_d79_send_woke_receiver_never_fast_path() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let mut receiver = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(sender_ptr);

        let message = make_message(1, 0);
        let outcome = crate::communication::SendOutcome::WokeReceiver(receiver_ptr, message);
        let result = core.dispatch_send_outcome(sender_ptr, outcome);

        assert!(
            !matches!(result, DispatchResult::ResumeFastPath(_)),
            "D79: Send must never return ResumeFastPath (D50 condition 1)"
        );
    }

    /// D79: Call x DirectSwitch approved must return ResumeFastPath.
    #[test]
    fn test_d79_call_direct_switch_uses_fast_path() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let mut receiver = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let outcome = crate::communication::CallOutcome::DirectSwitch(receiver_ptr);
        let result = core.dispatch_call_outcome(sender_ptr, outcome);

        assert!(
            matches!(result, DispatchResult::ResumeFastPath(_)),
            "D79: approved DirectSwitch must return ResumeFastPath"
        );
    }

    /// D79: Call x WokeReceiverSlowPath must NOT use ResumeFastPath.
    #[test]
    fn test_d79_call_slow_path_never_fast_path() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let mut receiver = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let receiver_ptr = NonNull::from(&mut receiver);

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let message = make_message(1, 0);
        let outcome =
            crate::communication::CallOutcome::WokeReceiverSlowPath(receiver_ptr, message);
        let result = core.dispatch_call_outcome(sender_ptr, outcome);

        assert!(
            !matches!(result, DispatchResult::ResumeFastPath(_)),
            "D79: WokeReceiverSlowPath must not return ResumeFastPath"
        );
    }

    /// D79: Call x Enqueued with no other runnable — Idle.
    #[test]
    fn test_d79_call_enqueued_no_runnable_idle() {
        let mut core = make_core_state();
        let mut sender = make_observer_with_registers();
        let sender_ptr = NonNull::from(&mut sender);

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let outcome = crate::communication::CallOutcome::Enqueued;
        let result = core.dispatch_call_outcome(sender_ptr, outcome);

        assert!(
            matches!(result, DispatchResult::Idle),
            "D79: Call Enqueued with no other runnable must return Idle"
        );
    }

    /// D79: ReplyRecv delivers reply message to client registers.
    #[test]
    fn test_d79_reply_recv_delivers_reply_to_client() {
        let mut core = make_core_state();
        let mut server = make_observer_with_registers();
        let mut client = make_observer_with_registers();
        let server_ptr = NonNull::from(&mut server);
        let client_ptr = NonNull::from(&mut client);

        core.current = Some(server_ptr);

        core.scheduler.enqueue(server_ptr);

        let reply_message = crate::field::Message {
            data: [0xA1, 0xB2, 0xC3, 0xD4],
            label: 0xDEAD,
            badge: Badge(0xBEEF),
            user_cap: None,
            reply_cap: None,
        };
        let outcome = crate::communication::ReplyRecvOutcome {
            reply_delivery: Some(crate::communication::ReplyDelivery {
                client: client_ptr,
                message: reply_message,
            }),
            receive_outcome: crate::communication::ReceiveOutcome::Blocked,
        };

        core.dispatch_reply_recv_outcome(server_ptr, outcome);

        let client_regs = crate::frame::cores::read_ipc_registers(client_ptr);

        assert_eq!(
            client_regs.data,
            [0xA1, 0xB2, 0xC3, 0xD4],
            "D79: reply data written to client"
        );
        assert_eq!(client_regs.label, 0xDEAD, "D79: reply label");
        assert_eq!(client_regs.handle_or_badge, 0xBEEF, "D79: reply badge");
    }

    // ── dispatch_typed tests ───────────────────────────────────────

    // Helper: create an Observer with real RegisterState and a real cap table.
    // Installs a single cap entry at slot 0 pointing to the given object.
    fn make_sender_with_cap(
        object_type: crate::capability::ObjectType,
        object_id: ObjectId,
        rights: crate::capability::Rights,
        badge: Badge,
        generation: u64,
    ) -> (Observer, NonNull<crate::capability::Entry>) {
        use crate::capability::{Entry, SlotTag};

        let rs_ptr = crate::frame::cores::alloc_test_register_state();
        let entries = crate::frame::capabilities::alloc_test_entries(16);

        // Initialize freelist for user slots (3..16).
        crate::frame::capabilities::init_freelist(entries, 16, crate::capability::SLOT_USER_START);

        let entry = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        *entry = Entry {
            object: Some((object_type, object_id)),
            rights,
            badge,
            slot_tag: SlotTag(0),
            send_once: false,
            stored_generation: generation,
        };

        let observer = Observer {
            object_id: ObjectId(0),
            asid: 0,
            asid_generation: 0,
            register_state: crate::observer::RegisterStateHandle::new(rs_ptr),
            page_table_root: 0,
            cap_table: entries,
            cap_table_capacity: 16,
            cap_table_free_head: Some(crate::capability::SLOT_USER_START),
            cap_table_count: 1,
            state: crate::observer::PrimaryState::Runnable,
            suspended: false,
            compute_aggregate: 100,
            responsiveness: crate::observer::DEFAULT_RESPONSIVENESS,
            throughput: crate::observer::DEFAULT_THROUGHPUT,
            clock_access: false,
            wait_state: crate::observer::WaitState::None,
            saved_syscall: crate::observer::SavedSyscallContext::None,
            backing_va_base: 0,
            backing_size: 0,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };

        (observer, entries)
    }

    // Helper: set up typed registers on a sender Observer.
    fn setup_typed_regs(
        sender_ptr: NonNull<Observer>,
        op_code: u16,
        target_handle: u64,
        args: [u64; 4],
    ) {
        let regs = crate::syscall::TypedRegisters {
            op_code,
            target_handle,
            args,
        };

        crate::frame::cores::write_test_typed_registers_via_observer(sender_ptr, &regs);
    }

    // Helper: read x0 (typed result) from an Observer's saved registers.
    fn read_typed_result(observer_ptr: NonNull<Observer>) -> u64 {
        let regs = crate::frame::cores::read_typed_registers(observer_ptr);

        regs.args[0]
    }

    /// dispatch_typed with valid Observer cap — ObserverSuspend succeeds.
    #[test]
    fn test_dispatch_typed_observer_suspend_success() {
        let ks = make_kernel_state();
        // Create target Observer in arena.
        let target_id = {
            let mut observers = ks.observers.acquire();
            let (id, observer) = observers.allocate().expect("allocate observer");

            observer.register_state = crate::observer::RegisterStateHandle::new(
                crate::frame::cores::alloc_test_register_state(),
            );
            observer.cap_table = NonNull::dangling();
            observer.cap_table_capacity = 0;
            observer.state = crate::observer::PrimaryState::Runnable;
            observer.suspended = false;
            observer.compute_aggregate = 100;
            observer.responsiveness = crate::observer::DEFAULT_RESPONSIVENESS;
            observer.throughput = crate::observer::DEFAULT_THROUGHPUT;
            observer.clock_access = false;
            observer.wait_state = crate::observer::WaitState::None;
            observer.refcount = 1;
            observer.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        // Create sender with a cap to the target Observer.
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Observer,
            target_id,
            crate::capability::Rights::OBSERVER_ALL,
            Badge(0),
            0, // generation matches
        );
        let sender_ptr = NonNull::from(&mut sender);
        // Set up typed registers: ObserverSuspend (code 4), handle = slot 0 encoded.
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverSuspend as u16,
            handle,
            [0; 4],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::ObserverSuspend, &ks);

        // Must resume sender (typed ops never block).
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "typed op must resume sender");
            }
            _ => panic!("typed op must return Resume(sender)"),
        }

        // Check result is 0 (success).
        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "ObserverSuspend success must write 0 to x0");

        // Verify the target Observer is now suspended.
        let observers = ks.observers.acquire();
        let target = observers.get(target_id).expect("target must exist");

        assert!(target.suspended, "target Observer must be suspended");
    }

    /// dispatch_typed with invalid cap handle returns error.
    #[test]
    fn test_dispatch_typed_invalid_cap_returns_error() {
        let ks = make_kernel_state();
        let mut sender = make_observer_with_registers();
        // Give sender a real but empty cap table.
        let entries = crate::frame::capabilities::alloc_test_entries(16);

        sender.cap_table = entries;
        sender.cap_table_capacity = 16;

        let sender_ptr = NonNull::from(&mut sender);
        // Target handle points to empty slot 5 (not occupied).
        let handle = crate::capability::Handle {
            index: 5,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverResume as u16,
            handle,
            [0; 4],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::ObserverResume, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "must resume sender on error");
            }
            _ => panic!("must return Resume(sender) even on error"),
        }

        // x0 must be negative (error code).
        let x0 = read_typed_result(sender_ptr) as i64;

        assert!(x0 < 0, "invalid cap must produce negative x0 (got {x0})");
    }

    /// dispatch_typed with wrong type returns WrongType error.
    #[test]
    fn test_dispatch_typed_wrong_type_returns_error() {
        let ks = make_kernel_state();
        // Create a Space in the arena.
        let space_id = {
            let mut spaces = ks.spaces.acquire();
            let (id, space) = spaces.allocate().expect("allocate space");

            space.va_base = 0x1000;
            space.size = 0x4000;
            space.refcount = 1;
            space.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        // Create sender with a Space cap — but try to use ObserverResume.
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Space,
            space_id,
            crate::capability::Rights::SPACE_ALL,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverResume as u16,
            handle,
            [0; 4],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::ObserverResume, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "must resume sender on type error");
            }
            _ => panic!("must Resume on error"),
        }

        let x0 = read_typed_result(sender_ptr) as i64;

        assert_eq!(
            x0,
            crate::syscall::SyscallError::WrongType.error_code() as i64,
            "wrong type must return WrongType error code"
        );
    }

    /// dispatch_typed with insufficient rights returns NoRight error.
    /// Uses Space + SpaceSplit to avoid Arena<Observer> zero-init issue.
    #[test]
    fn test_dispatch_typed_insufficient_rights_returns_error() {
        let ks = make_kernel_state();
        let space_id = {
            let mut spaces = ks.spaces.acquire();
            let (id, space) = spaces.allocate().expect("allocate space");

            space.va_base = 0x1000;
            space.size = 0x4000;
            space.refcount = 1;
            space.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        // Cap with DESTROY only — missing SPLIT required for SpaceSplit.
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Space,
            space_id,
            crate::capability::Rights::DESTROY,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::SpaceSplit as u16,
            handle,
            [0x1000, 0, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::SpaceSplit, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr);
            }
            _ => panic!("must Resume on error"),
        }

        let x0 = read_typed_result(sender_ptr) as i64;

        assert_eq!(
            x0,
            crate::syscall::SyscallError::NoRight.error_code() as i64,
            "insufficient rights must return NoRight error code"
        );
    }

    /// dispatch_typed with stale generation returns StaleCap error.
    /// Uses Space + Destroy to avoid Arena<Observer> zero-init issue.
    #[test]
    fn test_dispatch_typed_stale_generation_returns_error() {
        let ks = make_kernel_state();
        let space_id = {
            let mut spaces = ks.spaces.acquire();
            let (id, space) = spaces.allocate().expect("allocate space");

            space.va_base = 0x1000;
            space.size = 0x4000;
            space.refcount = 1;
            // Live generation is 5.
            space.generation = core::sync::atomic::AtomicU64::new(5);

            id
        };
        // Cap stores generation 0 — stale (live is 5).
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Space,
            space_id,
            crate::capability::Rights::SPACE_ALL,
            Badge(0),
            0, // stored generation 0, live is 5
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Destroy as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::Destroy, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr);
            }
            _ => panic!("must Resume on error"),
        }

        let x0 = read_typed_result(sender_ptr) as i64;

        assert_eq!(
            x0,
            crate::syscall::SyscallError::StaleCap.error_code() as i64,
            "stale generation must return StaleCap error code"
        );

        // Verify Space generation was NOT bumped (operation rejected).
        let spaces = ks.spaces.acquire();
        let space = spaces.get(space_id).expect("space must exist");

        assert_eq!(
            space.generation.load(Ordering::Acquire),
            5,
            "generation must remain at 5 after stale cap rejection"
        );
    }

    /// dispatch_typed ObserverResume transitions Inert -> Runnable.
    #[test]
    fn test_dispatch_typed_observer_resume_from_inert() {
        let ks = make_kernel_state();
        let target_id = {
            let mut observers = ks.observers.acquire();
            let (id, observer) = observers.allocate().expect("allocate observer");

            observer.register_state = crate::observer::RegisterStateHandle::new(
                crate::frame::cores::alloc_test_register_state(),
            );
            observer.cap_table = NonNull::dangling();
            observer.cap_table_capacity = 0;
            observer.state = crate::observer::PrimaryState::Inert;
            observer.suspended = false;
            observer.compute_aggregate = 0;
            observer.responsiveness = crate::observer::DEFAULT_RESPONSIVENESS;
            observer.throughput = crate::observer::DEFAULT_THROUGHPUT;
            observer.clock_access = false;
            observer.wait_state = crate::observer::WaitState::None;
            observer.refcount = 1;
            observer.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Observer,
            target_id,
            crate::capability::Rights::OBSERVER_ALL,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverResume as u16,
            handle,
            [0; 4],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::ObserverResume, &ks);

        assert!(matches!(result, DispatchResult::Resume(_)), "must Resume");

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "resume success must write 0");

        // Verify target is now Runnable.
        let observers = ks.observers.acquire();
        let target = observers.get(target_id).expect("target must exist");

        assert!(
            matches!(target.state, crate::observer::PrimaryState::Runnable),
            "target must be Runnable after resume"
        );
    }

    /// dispatch_typed ObserverResume from Runnable returns InvalidState.
    #[test]
    fn test_dispatch_typed_observer_resume_from_runnable_fails() {
        let ks = make_kernel_state();
        let target_id = {
            let mut observers = ks.observers.acquire();
            let (id, observer) = observers.allocate().expect("allocate observer");

            observer.register_state = crate::observer::RegisterStateHandle::new(
                crate::frame::cores::alloc_test_register_state(),
            );
            observer.cap_table = NonNull::dangling();
            observer.cap_table_capacity = 0;
            observer.state = crate::observer::PrimaryState::Runnable;
            observer.suspended = false;
            observer.compute_aggregate = 100;
            observer.responsiveness = crate::observer::DEFAULT_RESPONSIVENESS;
            observer.throughput = crate::observer::DEFAULT_THROUGHPUT;
            observer.clock_access = false;
            observer.wait_state = crate::observer::WaitState::None;
            observer.refcount = 1;
            observer.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Observer,
            target_id,
            crate::capability::Rights::OBSERVER_ALL,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverResume as u16,
            handle,
            [0; 4],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::ObserverResume, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert_eq!(
            x0,
            crate::syscall::SyscallError::InvalidState.error_code() as i64,
            "resume from Runnable must return InvalidState"
        );
    }

    /// dispatch_typed ObserverSetScheduling with valid profile.
    #[test]
    fn test_dispatch_typed_observer_set_scheduling_success() {
        let ks = make_kernel_state();
        let target_id = {
            let mut observers = ks.observers.acquire();
            let (id, observer) = observers.allocate().expect("allocate observer");

            observer.register_state = crate::observer::RegisterStateHandle::new(
                crate::frame::cores::alloc_test_register_state(),
            );
            observer.cap_table = NonNull::dangling();
            observer.cap_table_capacity = 0;
            observer.state = crate::observer::PrimaryState::Runnable;
            observer.suspended = false;
            observer.compute_aggregate = 100;
            observer.responsiveness = crate::observer::DEFAULT_RESPONSIVENESS;
            observer.throughput = crate::observer::DEFAULT_THROUGHPUT;
            observer.clock_access = false;
            observer.wait_state = crate::observer::WaitState::None;
            observer.refcount = 1;
            observer.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Observer,
            target_id,
            crate::capability::Rights::OBSERVER_ALL,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        // args[0] = responsiveness (60), args[1] = throughput (40)
        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverSetScheduling as u16,
            handle,
            [60, 40, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::ObserverSetScheduling, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "set_scheduling with valid profile must succeed");

        let observers = ks.observers.acquire();
        let target = observers.get(target_id).expect("target must exist");

        assert_eq!(target.responsiveness, 60, "responsiveness must be updated");
        assert_eq!(target.throughput, 40, "throughput must be updated");
    }

    /// dispatch_typed ObserverSetScheduling with invalid profile.
    #[test]
    fn test_dispatch_typed_observer_set_scheduling_invalid_profile() {
        let ks = make_kernel_state();
        let target_id = {
            let mut observers = ks.observers.acquire();
            let (id, observer) = observers.allocate().expect("allocate observer");

            observer.register_state = crate::observer::RegisterStateHandle::new(
                crate::frame::cores::alloc_test_register_state(),
            );
            observer.cap_table = NonNull::dangling();
            observer.cap_table_capacity = 0;
            observer.state = crate::observer::PrimaryState::Runnable;
            observer.suspended = false;
            observer.compute_aggregate = 100;
            observer.responsiveness = crate::observer::DEFAULT_RESPONSIVENESS;
            observer.throughput = crate::observer::DEFAULT_THROUGHPUT;
            observer.clock_access = false;
            observer.wait_state = crate::observer::WaitState::None;
            observer.refcount = 1;
            observer.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Observer,
            target_id,
            crate::capability::Rights::OBSERVER_ALL,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        // args[0] = 100, args[1] = 100 => R+T = 200 > 128 budget
        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverSetScheduling as u16,
            handle,
            [100, 100, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::ObserverSetScheduling, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert_eq!(
            x0,
            crate::syscall::SyscallError::InvalidProfile.error_code() as i64,
            "R+T > 128 must return InvalidProfile"
        );
    }

    /// dispatch_typed Destroy bumps generation for revocation.
    #[test]
    fn test_dispatch_typed_destroy_bumps_generation() {
        let ks = make_kernel_state();
        let space_id = {
            let mut spaces = ks.spaces.acquire();
            let (id, space) = spaces.allocate().expect("allocate space");

            space.va_base = 0x1000;
            space.size = 0x4000;
            space.refcount = 1;
            space.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Space,
            space_id,
            crate::capability::Rights::SPACE_ALL,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Destroy as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Destroy, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(
            x0, 0,
            "D98: Space Destroy must succeed with 0 (no return cap)"
        );

        // D98: arena slot must be freed after Destroy.
        let spaces = ks.spaces.acquire();

        assert!(
            spaces.get(space_id).is_none(),
            "D98: Space arena slot must be freed after Destroy"
        );
    }

    /// D98: Observer Destroy runs cascade — closes all caps in target table.
    #[test]
    fn test_d98_observer_destroy_cascades_cap_table() {
        let ks = make_kernel_state();
        // Create a target Observer with caps in its table.
        let target_id = {
            let mut observers = ks.observers.acquire();
            let (id, obs) = observers.allocate().expect("allocate target");
            let rs = crate::frame::cores::alloc_test_register_state();
            let entries = crate::frame::capabilities::alloc_test_entries(8);

            crate::frame::capabilities::init_freelist(
                entries,
                8,
                crate::capability::SLOT_USER_START,
            );

            obs.asid = 0;
            obs.register_state = crate::observer::RegisterStateHandle::new(rs);
            obs.page_table_root = 0;
            obs.cap_table = entries;
            obs.cap_table_capacity = 8;
            obs.cap_table_free_head = Some(crate::capability::SLOT_USER_START);
            obs.cap_table_count = 0;
            obs.state = crate::observer::PrimaryState::Inert;
            obs.suspended = false;
            obs.compute_aggregate = 0;
            obs.responsiveness = crate::observer::DEFAULT_RESPONSIVENESS;
            obs.throughput = crate::observer::DEFAULT_THROUGHPUT;
            obs.clock_access = false;
            obs.wait_state = crate::observer::WaitState::None;
            obs.backing_va_base = 0x2000;
            obs.backing_size = 0x4000;
            obs.refcount = 1;
            obs.generation = core::sync::atomic::AtomicU64::new(0);

            // Install a Space cap at slot 3 (user slot).
            let space_id = {
                let mut spaces = ks.spaces.acquire();
                let (sid, space) = spaces.allocate().expect("allocate space");
                space.va_base = 0x8000;
                space.size = 0x1000;
                space.l3_table_pa = 0;
                space.refcount = 1;
                space.generation = core::sync::atomic::AtomicU64::new(0);
                sid
            };

            crate::frame::capabilities::write_entry(
                entries,
                8,
                crate::capability::SLOT_USER_START,
                crate::capability::Entry {
                    object: Some((crate::capability::ObjectType::Space, space_id)),
                    rights: crate::capability::Rights::SPACE_ALL,
                    badge: crate::capability::Badge(0),
                    slot_tag: crate::capability::SlotTag(0),
                    send_once: false,
                    stored_generation: 0,
                },
            );

            obs.cap_table_count = 1;
            obs.cap_table_free_head = Some(crate::capability::SLOT_USER_START + 1);

            id
        };
        // Sender holds a Destroy cap to the target Observer.
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Observer,
            target_id,
            crate::capability::Rights::DESTROY,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Destroy as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Destroy, &ks);

        let x0 = read_typed_result(sender_ptr);

        // D98: backing_size > 0, so return value is an encoded Space cap handle.
        assert_ne!(
            x0 as i64,
            crate::syscall::SyscallError::InvalidCap.error_code() as i64,
            "D98: Observer Destroy must not return InvalidCap"
        );

        // D98: target Observer arena slot must be freed.
        let observers = ks.observers.acquire();

        assert!(
            observers.get(target_id).is_none(),
            "D98: Observer arena slot must be freed after cascade"
        );
    }

    /// D98: Time Destroy frees arena slot — no Space return.
    #[test]
    fn test_d98_time_destroy_frees_arena() {
        let ks = make_kernel_state();
        let time_id = {
            let mut times = ks.times.acquire();
            let (id, time) = times.allocate().expect("allocate time");

            time.compute_units = 100;
            time.refcount = 1;
            time.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Time,
            time_id,
            crate::capability::Rights::TIME_ALL,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Destroy as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Destroy, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D98: Time Destroy returns 0 (no Space return)");

        // Arena slot must be freed.
        let times = ks.times.acquire();

        assert!(
            times.get(time_id).is_none(),
            "D98: Time arena slot must be freed after Destroy"
        );
    }

    /// D98: Pulsar Destroy removes deadline entry from per-core array.
    #[test]
    fn test_d98_pulsar_destroy_removes_deadline() {
        let ks = make_kernel_state();
        // Create delivery field (required for Pulsar).
        let field_id = {
            let mut fields = ks.fields.acquire();
            let (id, field) = fields.allocate().expect("allocate field");
            let queue_ptr = crate::frame::fields::allocate_field_queue(4).unwrap();

            field.queue = queue_ptr;
            field.queue_capacity = 4;
            field.queue_length = 0;
            field.queue_head = 0;
            field.waiters_head = None;
            field.waiters_tail = None;
            field.routing_table = None;
            field.pending_head = None;
            field.badge_tracking = false;
            field.back_pointer_head = None;
            field.backing_va_base = 0;
            field.backing_size = 0;
            field.refcount = 1;
            field.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let pulsar_id = {
            let mut pulsars = ks.pulsars.acquire();
            let (id, pulsar) = pulsars.allocate().expect("allocate pulsar");

            *pulsar = crate::pulsar::Pulsar::new(
                field_id,
                crate::capability::Badge(42),
                1_000_000,
                0,
                24_000_000,
                0,
            );
            pulsar.backing_va_base = 0;
            pulsar.backing_size = 0;

            id
        };
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Pulsar,
            pulsar_id,
            crate::capability::Rights::PULSAR_ALL,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Destroy as u16, handle, [0; 4]);

        let mut core = make_core_state();

        // Install deadline entry for the Pulsar.
        core.deadlines[0] = Some(DeadlineEntry {
            deadline_ticks: 1000,
            pulsar_id,
            field_id,
        });
        core.deadline_count = 1;
        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Destroy, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D98: Pulsar Destroy with no backing returns 0");
        // D98/D99: deadline entry must be removed.
        assert_eq!(
            core.deadline_count, 0,
            "D98: Pulsar Destroy must remove deadline entry"
        );
        assert!(
            core.deadlines[0].is_none(),
            "D98: deadline slot must be None after removal"
        );

        // Arena slot must be freed.
        let pulsars = ks.pulsars.acquire();

        assert!(
            pulsars.get(pulsar_id).is_none(),
            "D98: Pulsar arena slot must be freed after Destroy"
        );
    }

    /// D98: Field Destroy with backing returns Space cap.
    #[test]
    fn test_d98_field_destroy_returns_space() {
        let ks = make_kernel_state();
        let field_id = {
            let mut fields = ks.fields.acquire();
            let (id, field) = fields.allocate().expect("allocate field");
            let queue_ptr = crate::frame::fields::allocate_field_queue(4).unwrap();

            field.queue = queue_ptr;
            field.queue_capacity = 4;
            field.queue_length = 0;
            field.queue_head = 0;
            field.waiters_head = None;
            field.waiters_tail = None;
            field.routing_table = None;
            field.pending_head = None;
            field.badge_tracking = false;
            field.back_pointer_head = None;
            field.backing_va_base = 0x5000;
            field.backing_size = 0x4000;
            field.refcount = 1;
            field.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            field_id,
            crate::capability::Rights::FIELD_ALL,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Destroy as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Destroy, &ks);

        let x0 = read_typed_result(sender_ptr);
        // D98: return value is an encoded handle to the new Space cap.
        let returned_handle = crate::capability::Handle::decode(x0);

        assert!(
            returned_handle.index >= crate::capability::SLOT_USER_START,
            "D98: returned Space cap must be in a user slot"
        );

        // Verify the Space was created in the arena with the backing info.
        let spaces = ks.spaces.acquire();
        // The new Space should exist — iterate to find one with matching va_base.
        let mut found = false;

        for i in 0..32 {
            if let Some(space) = spaces.get(ObjectId(i)) {
                if space.va_base == 0x5000 && space.size == 0x4000 {
                    found = true;
                    break;
                }
            }
        }

        assert!(
            found,
            "D98: returned Space must have the backing VA and size"
        );

        // Field arena slot must be freed.
        drop(spaces);

        let fields = ks.fields.acquire();

        assert!(
            fields.get(field_id).is_none(),
            "D98: Field arena slot must be freed after Destroy"
        );
    }

    /// dispatch_typed Clone on Time returns CloneForbidden (D38 linear).
    #[test]
    fn test_dispatch_typed_clone_time_forbidden() {
        let ks = make_kernel_state();
        let time_id = {
            let mut times = ks.times.acquire();
            let (id, time) = times.allocate().expect("allocate time");

            time.compute_units = 100;
            time.refcount = 1;
            time.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        // Time caps have no CLONE right in TIME_ALL, but the Clone op
        // checks for CloneForbidden before rights. Let's give CLONE
        // right explicitly to test the type-level rejection.
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Time,
            time_id,
            crate::capability::Rights::CLONE.union(crate::capability::Rights::DESTROY),
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Clone as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Clone, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert_eq!(
            x0,
            crate::syscall::SyscallError::CloneForbidden.error_code() as i64,
            "Clone on Time must return CloneForbidden"
        );
    }

    /// dispatch_typed with out-of-bounds handle index returns InvalidCap.
    #[test]
    fn test_dispatch_typed_out_of_bounds_handle() {
        let ks = make_kernel_state();
        let mut sender = make_observer_with_registers();
        let entries = crate::frame::capabilities::alloc_test_entries(4);

        sender.cap_table = entries;
        sender.cap_table_capacity = 4;

        let sender_ptr = NonNull::from(&mut sender);
        // Handle index 100 is well beyond capacity 4.
        let handle = crate::capability::Handle {
            index: 100,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Destroy as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Destroy, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert_eq!(
            x0,
            crate::syscall::SyscallError::InvalidCap.error_code() as i64,
            "out-of-bounds handle must return InvalidCap"
        );
    }

    // ── dispatch_ipc tests ──────────────────────────────────────────

    // Helper: allocate a Field in the arena with a real queue, fully initialized.
    fn make_field_in_arena(ks: &KernelState, capacity: u32) -> ObjectId {
        let mut fields = ks.fields.acquire();
        let (id, field) = fields.allocate().expect("allocate field");

        *field = crate::field::Field::new(
            crate::frame::fields::alloc_test_queue(capacity),
            capacity,
            0,
            0,
        );

        id
    }

    fn make_space_in_arena(ks: &KernelState, va_base: usize, size: usize) -> ObjectId {
        let mut spaces = ks.spaces.acquire();
        let (id, space) = spaces.allocate().expect("allocate space");

        space.va_base = va_base;
        space.size = size;
        space.refcount = 1;
        space.l3_table_pa = 0;
        space.generation = core::sync::atomic::AtomicU64::new(0);

        id
    }

    // ── Test 1: Send with valid Field cap — message enqueued ────────

    /// D79/D77: Send with a valid Field cap enqueues the message and
    /// resumes the sender. The Field's queue must contain the message
    /// with data from the IPC registers and badge from the cap entry.
    #[test]
    fn test_dispatch_ipc_send_valid_cap_enqueues_message() {
        let ks = make_kernel_state();
        // Allocate a Field in the arena with a real queue.
        let field_id = make_field_in_arena(&ks, 8);
        // Create a sender Observer with a SEND cap to the Field.
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            field_id,
            crate::capability::Rights::SEND,
            Badge(0x5555),
            0, // stored_generation matches live generation (0)
        );
        let sender_ptr = NonNull::from(&mut sender);
        // Write IPC registers: data + handle pointing to slot 0.
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0x1111, 0x2222, 0x3333, 0x4444],
                label: 0xABCD,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Send, &ks);

        // D79 Row 1: sender continues.
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, sender_ptr,
                    "D77/D79: Send valid cap must resume sender"
                );
            }
            _ => panic!("D77/D79: Send valid cap must return Resume(sender)"),
        }

        // Carry must be clear (success).
        let (carry, _) = crate::frame::cores::read_ipc_carry_and_x0(sender_ptr);

        assert!(!carry, "D49: carry must be clear on successful Send");

        // The Field must contain the message.
        let mut fields = ks.fields.acquire();
        let target = fields.get_mut(field_id).unwrap();

        assert_eq!(
            target.queue_length, 1,
            "D13: message must be enqueued in Field"
        );

        let msg = target.dequeue().unwrap();

        assert_eq!(
            msg.data,
            [0x1111, 0x2222, 0x3333, 0x4444],
            "D28: data words must match"
        );
        assert_eq!(msg.label, 0xABCD, "D28: label must match");
        assert_eq!(
            msg.badge,
            Badge(0x5555),
            "D17: badge injected from cap entry"
        );
    }

    // ── D51: Send-once cap consumed after Send ──────────────────────

    /// D51: a send-once cap is consumed (slot freed) after a successful
    /// Send. A second Send with the same handle fails because the slot
    /// is now empty.
    #[test]
    fn test_d51_send_once_cap_consumed_after_send() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let (mut sender, entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            field_id,
            crate::capability::Rights::SEND,
            Badge(0x5555),
            0,
        );
        // Set send_once on the cap entry at slot 0.
        let entry = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        entry.send_once = true;

        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0x1111, 0x2222, 0x3333, 0x4444],
                label: 0xABCD,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        // First Send must succeed.
        let result = core.dispatch_ipc(IpcOperation::Send, &ks);

        assert!(
            matches!(result, DispatchResult::Resume(_)),
            "D51: first send must resume sender"
        );

        let (carry, _) = crate::frame::cores::read_ipc_carry_and_x0(sender_ptr);

        assert!(!carry, "D51: first send must succeed (carry clear)");

        // Cap at slot 0 must now be empty (consumed by D51).
        let entry_after = crate::frame::capabilities::entry_ref(entries, 16, 0).unwrap();

        assert!(
            !entry_after.is_occupied(),
            "D51: send-once cap must be consumed after Send"
        );

        // Second Send with same handle must fail.
        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0x5555, 0x6666, 0x7777, 0x8888],
                label: 0xDEAD,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let result2 = core.dispatch_ipc(IpcOperation::Send, &ks);

        assert!(
            matches!(result2, DispatchResult::Resume(_)),
            "D51: second send must still resume sender (with error)"
        );

        let (carry2, _) = crate::frame::cores::read_ipc_carry_and_x0(sender_ptr);

        assert!(
            carry2,
            "D51: second send with consumed cap must fail (carry set)"
        );
    }

    /// D51: a regular (non-send-once) cap is NOT consumed after Send.
    #[test]
    fn test_d51_regular_cap_not_consumed_after_send() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let (mut sender, entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            field_id,
            crate::capability::Rights::SEND,
            Badge(0x5555),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [1, 2, 3, 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let _ = core.dispatch_ipc(IpcOperation::Send, &ks);

        // Regular cap must still be occupied.
        let entry_after = crate::frame::capabilities::entry_ref(entries, 16, 0).unwrap();

        assert!(
            entry_after.is_occupied(),
            "D51: regular cap must NOT be consumed after Send"
        );
    }

    // ── Test 2: Send with invalid handle → InvalidCap ──────────────

    /// D77: Send with a bad handle (points to empty slot) returns
    /// InvalidCap error to the sender via carry flag + x0.
    #[test]
    fn test_dispatch_ipc_send_invalid_handle_returns_invalid_cap() {
        let ks = make_kernel_state();
        // Sender with real registers and a real (but empty) cap table.
        let mut sender = make_observer_with_registers();
        let entries = crate::frame::capabilities::alloc_test_entries(16);

        sender.cap_table = entries;
        sender.cap_table_capacity = 16;

        let sender_ptr = NonNull::from(&mut sender);
        // Handle pointing to an empty slot (index 5 — never populated).
        let handle = crate::capability::Handle {
            index: 5,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Send, &ks);

        // Must resume sender (never blocks on error).
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "error path must resume sender");
            }
            _ => panic!("error path must return Resume(sender)"),
        }

        // Carry set, x0 = InvalidCap error code.
        let (carry, x0) = crate::frame::cores::read_ipc_carry_and_x0(sender_ptr);

        assert!(carry, "D49: carry must be set on IPC error");
        assert_eq!(
            x0,
            crate::syscall::SyscallError::InvalidCap as u64,
            "D49: x0 must contain InvalidCap error code"
        );
    }

    // ── Test 3: Send with wrong type cap → WrongType ────────────────

    /// D77: Send with a cap pointing to an Observer (not Field) returns
    /// WrongType error. IPC operations require a Field cap.
    #[test]
    fn test_dispatch_ipc_send_wrong_type_returns_wrong_type() {
        let ks = make_kernel_state();
        // Use a Space object — easy to create without Arena zero-init issues.
        let space_id = {
            let mut spaces = ks.spaces.acquire();
            let (id, space) = spaces.allocate().expect("allocate space");

            space.va_base = 0x1000;
            space.size = 0x4000;
            space.refcount = 1;
            space.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        // Create sender with a Space cap (wrong type for IPC).
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Space,
            space_id,
            crate::capability::Rights::SEND.union(crate::capability::Rights::SPACE_ALL),
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Send, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "wrong type error must resume sender");
            }
            _ => panic!("wrong type error must return Resume(sender)"),
        }

        let (carry, x0) = crate::frame::cores::read_ipc_carry_and_x0(sender_ptr);

        assert!(carry, "D49: carry must be set on WrongType error");
        assert_eq!(
            x0,
            crate::syscall::SyscallError::WrongType as u64,
            "D49: x0 must contain WrongType error code"
        );
    }

    // ── Test 4: Send with insufficient rights → NoRight ─────────────

    /// D52: Send with a Field cap that has RECEIVE but not SEND returns
    /// NoRight error.
    #[test]
    fn test_dispatch_ipc_send_insufficient_rights_returns_no_right() {
        let ks = make_kernel_state();
        // Allocate a Field in the arena.
        let field_id = make_field_in_arena(&ks, 8);
        // Cap with RECEIVE only — missing SEND required for Send operation.
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            field_id,
            crate::capability::Rights::RECEIVE, // no SEND
            Badge(0x5555),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Send, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "NoRight error must resume sender");
            }
            _ => panic!("NoRight error must return Resume(sender)"),
        }

        let (carry, x0) = crate::frame::cores::read_ipc_carry_and_x0(sender_ptr);

        assert!(carry, "D49: carry must be set on NoRight error");
        assert_eq!(
            x0,
            crate::syscall::SyscallError::NoRight as u64,
            "D49: x0 must contain NoRight error code"
        );
    }

    // ── Test 5: Send with stale generation → StaleCap ───────────────

    /// D67: Send with a cap whose stored_generation doesn't match the
    /// Field's live generation returns StaleCap error.
    #[test]
    fn test_dispatch_ipc_send_stale_generation_returns_stale_cap() {
        let ks = make_kernel_state();
        // Allocate a Field in the arena with live generation 0.
        let field_id = make_field_in_arena(&ks, 8);
        // Cap with stored_generation = 1 — stale (live is 0).
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            field_id,
            crate::capability::Rights::SEND,
            Badge(0x5555),
            1, // stored_generation 1, live is 0 → stale
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Send, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "StaleCap error must resume sender");
            }
            _ => panic!("StaleCap error must return Resume(sender)"),
        }

        let (carry, x0) = crate::frame::cores::read_ipc_carry_and_x0(sender_ptr);

        assert!(carry, "D49: carry must be set on StaleCap error");
        assert_eq!(
            x0,
            crate::syscall::SyscallError::StaleCap as u64,
            "D49: x0 must contain StaleCap error code"
        );

        // The Field must be unmodified (no message enqueued).
        let fields = ks.fields.acquire();
        let target = fields.get(field_id).unwrap();

        assert_eq!(
            target.queue_length, 0,
            "D67: stale cap must not enqueue any message"
        );
    }

    // ── Test 6: Receive with message available → Received ───────────

    /// D79 Row 3 (via dispatch_ipc): Receive on a Field that has a queued
    /// message delivers the message to the receiver's registers and resumes.
    #[test]
    fn test_dispatch_ipc_receive_with_message_delivers_and_resumes() {
        let ks = make_kernel_state();
        // Allocate a Field in the arena and pre-enqueue a message.
        let field_id = make_field_in_arena(&ks, 8);

        {
            let mut fields = ks.fields.acquire();
            let field = fields.get_mut(field_id).unwrap();

            field
                .enqueue(crate::field::Message {
                    data: [0xAA, 0xBB, 0xCC, 0xDD],
                    label: 0xFACE,
                    badge: Badge(0xCAFE),
                    user_cap: None,
                    reply_cap: None,
                })
                .expect("pre-enqueue must succeed");
        };

        // Create receiver with RECEIVE cap.
        let (mut receiver, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            field_id,
            crate::capability::Rights::RECEIVE,
            Badge(0x5555),
            0,
        );
        let receiver_ptr = NonNull::from(&mut receiver);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            receiver_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(receiver_ptr);

        core.scheduler.enqueue(receiver_ptr);

        let result = core.dispatch_ipc(IpcOperation::Receive, &ks);

        // D79 Row 3: receiver continues.
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, receiver_ptr,
                    "D79 Row 3: Receive Received must resume the receiver"
                );
            }
            _ => panic!("D79 Row 3: Receive Received must return Resume(receiver)"),
        }

        // Message data must appear in receiver's saved registers.
        let regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_eq!(
            regs.data,
            [0xAA, 0xBB, 0xCC, 0xDD],
            "D76: data words delivered"
        );
        assert_eq!(regs.label, 0xFACE, "D76: label delivered");
        assert_eq!(regs.handle_or_badge, 0xCAFE, "D76: badge delivered");

        // Carry must be clear (success).
        let (carry, _) = crate::frame::cores::read_ipc_carry_and_x0(receiver_ptr);

        assert!(!carry, "D49: carry must be clear on successful Receive");

        // The Field must be empty now.
        let mut fields = ks.fields.acquire();
        let target = fields.get_mut(field_id).unwrap();

        assert_eq!(target.queue_length, 0, "D13: message must be consumed");
    }

    // ── Test 7: Receive on empty Field → Blocked ────────────────────

    /// D79 Row 4 (via dispatch_ipc): Receive on an empty Field blocks
    /// the receiver — dequeued from scheduler, schedule_next picks next.
    #[test]
    fn test_dispatch_ipc_receive_empty_field_blocks_receiver() {
        let ks = make_kernel_state();
        // Allocate an empty Field.
        let field_id = make_field_in_arena(&ks, 8);
        // Create receiver with RECEIVE cap.
        let (mut receiver, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            field_id,
            crate::capability::Rights::RECEIVE,
            Badge(0x5555),
            0,
        );
        let receiver_ptr = NonNull::from(&mut receiver);
        // Enqueue another Observer so schedule_next has something to return.
        let mut next_obs = make_observer();
        let next_ptr = NonNull::from(&mut next_obs);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            receiver_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(receiver_ptr);

        core.scheduler.enqueue(receiver_ptr);
        core.scheduler.enqueue(next_ptr);

        let result = core.dispatch_ipc(IpcOperation::Receive, &ks);

        // D79 Row 4: receiver blocks, schedule_next returns next_ptr.
        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(
                    resumed, next_ptr,
                    "D79 Row 4: blocked receiver must yield to next Observer"
                );
            }
            _ => panic!("D79 Row 4: blocked receiver must schedule next"),
        }

        // Receiver must be dequeued from the scheduler.
        assert!(
            !core.scheduler.contains(receiver_ptr),
            "D79 Row 4: blocked receiver must be removed from run queue"
        );
    }

    // ── Test 8: Call with waiter → DirectSwitch (fast path) ─────────

    /// D50/D79 Row 6 (via dispatch_ipc): Call on a Field that has a
    /// waiting receiver triggers the DirectSwitch fast path when the
    /// scheduler approves. The sender blocks; the receiver is direct-
    /// switched to via ResumeFastPath.
    #[test]
    fn test_dispatch_ipc_call_with_waiter_direct_switch() {
        let ks = make_kernel_state();
        // Allocate a Field in the arena.
        let field_id = make_field_in_arena(&ks, 8);
        // Create the sender with a SEND cap.
        let (mut sender, _entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            field_id,
            crate::capability::Rights::SEND,
            Badge(0x5555),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        // Create a waiter Observer with a real RegisterState (needed for
        // unblock which calls observer_unblock on the waiter).
        let mut waiter = make_observer_with_registers();

        // Install the waiter into the Field's waiters list so Call finds it.
        // communication::call checks pop_waiter() — if it finds one, it
        // returns DirectSwitch with that waiter's observer pointer.
        //
        // We must set up the WaitEntry in waiter.wait_state and link it.
        // Use observer_prepare_wait to build a valid WaitEntry, then add it
        // to the Field's waiters list.
        {
            let mut fields = ks.fields.acquire();
            let target_field = fields.get_mut(field_id).unwrap();
            let field_ptr = NonNull::from(&*target_field);
            // Set waiter's wait_state (needed for WaitEntry validity).
            let waiter_ptr = NonNull::from(&mut waiter);
            let wait_entry = crate::frame::cores::observer_prepare_wait(waiter_ptr, field_ptr);

            // Add the wait entry to the Field's waiters list.
            target_field.add_waiter(wait_entry);
        }

        // Write IPC registers: 0-cap message (D50 fast-path condition).
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0x10, 0x20, 0x30, 0x40],
                label: 0x9999,
                handle_or_badge: handle,
                user_cap: u64::MAX, // 0-cap: fast-path eligible
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Call, &ks);
        // D50: RoundRobin always approves should_switch_to, so DirectSwitch
        // approved → ResumeFastPath(waiter_observer).
        let waiter_ptr = NonNull::from(&waiter);

        match result {
            DispatchResult::ResumeFastPath(resumed) => {
                assert_eq!(
                    resumed, waiter_ptr,
                    "D50: DirectSwitch approved must resume the waiting receiver"
                );
            }
            DispatchResult::Resume(_) => {
                // DirectSwitch denied — also acceptable. The receiver was
                // enqueued and schedule_next picks it. Verify sender blocked.
                assert!(
                    !core.scheduler.contains(sender_ptr),
                    "D50 denied path: sender must still be dequeued"
                );
            }
            DispatchResult::Idle => {
                panic!("D50: Call with a waiter must not return Idle");
            }
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }

        // Sender must be dequeued (D16: Call always blocks the sender).
        assert!(
            !core.scheduler.contains(sender_ptr),
            "D16: Call must always dequeue (block) the sender"
        );
    }

    // ── Test 9: Call with stale reply field → StaleCap ─────────────

    /// D67/P4-001: Call validates the reply field's generation before
    /// transferring it to the receiver. A stale reply cap at
    /// SLOT_REPLY_FIELD returns StaleCap to the sender without
    /// delivering any message to the target Field.
    #[test]
    fn test_dispatch_ipc_call_stale_reply_field_returns_stale_cap() {
        let ks = make_kernel_state();
        let target_field_id = make_field_in_arena(&ks, 8);
        let reply_field_id = make_field_in_arena(&ks, 8);
        let (mut sender, entries) = make_sender_with_cap(
            crate::capability::ObjectType::Field,
            target_field_id,
            crate::capability::Rights::SEND,
            Badge(0x5555),
            0,
        );
        // Install stale reply cap at SLOT_REPLY_FIELD (slot 1).
        // Live generation is 0; stored_generation is 1 → stale.
        let reply_entry =
            crate::frame::capabilities::entry_mut(entries, 16, crate::capability::SLOT_REPLY_FIELD)
                .unwrap();

        *reply_entry = crate::capability::Entry {
            object: Some((crate::capability::ObjectType::Field, reply_field_id)),
            rights: crate::capability::Rights::SEND
                .union(crate::capability::Rights::DESTROY)
                .union(crate::capability::Rights::CLONE),
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 1,
        };

        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0x10, 0x20, 0x30, 0x40],
                label: 0x9999,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0xBEEF,
            },
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Call, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "StaleCap error must resume sender");
            }
            _ => panic!("Stale reply field must return Resume(sender), not block"),
        }

        let (carry, x0) = crate::frame::cores::read_ipc_carry_and_x0(sender_ptr);

        assert!(carry, "D49: carry must be set on StaleCap error");
        assert_eq!(
            x0,
            crate::syscall::SyscallError::StaleCap as u64,
            "D67: stale reply field must return StaleCap error"
        );

        // Target Field must be unmodified — stale reply field prevents message delivery.
        let fields = ks.fields.acquire();
        let target = fields.get(target_field_id).unwrap();

        assert_eq!(
            target.queue_length, 0,
            "P4-001: stale reply field must prevent message delivery"
        );
    }

    // ── D95 object creation tests ─────────────────────────────────

    /// D95 CreateField: Space consumed, Field created, cap transformed.
    #[test]
    fn test_d95_create_field_success() {
        let ks = make_kernel_state();
        let space_id = make_space_in_arena(&ks, 0x1000, 4 * 4096);

        let (mut sender, entries) =
            make_sender_with_cap(ObjectType::Space, space_id, Rights::SPACE_ALL, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::CreateField as u16,
            handle,
            [0; 4],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::CreateField, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "must resume sender");
            }
            _ => panic!("CreateField must return Resume(sender)"),
        }

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D95: CreateField success must return 0");

        // Cap entry must now point to a Field.
        let entry =
            crate::frame::capabilities::entry_ref(entries, 16, 0).expect("entry must exist");
        let (obj_type, field_id) = entry.object.expect("entry must be occupied");

        assert_eq!(
            obj_type,
            ObjectType::Field,
            "D32: cap must be Field after type conversion"
        );

        // Field must exist in arena with derived queue capacity.
        let fields = ks.fields.acquire();
        let field = fields.get(field_id).expect("Field must exist in arena");
        let expected_capacity = (4 * 4096) / core::mem::size_of::<crate::field::Message>();

        assert_eq!(
            field.queue_capacity, expected_capacity as u32,
            "D95: queue capacity must be derived from Space size"
        );
        assert_eq!(field.queue_length, 0, "new Field must have empty queue");

        // Space must be consumed (freed from arena).
        drop(fields);

        let spaces = ks.spaces.acquire();

        assert!(
            spaces.get(space_id).is_none(),
            "D32: Space must be freed after type conversion"
        );
    }

    /// D95 CreateField: wrong target type returns WrongType.
    #[test]
    fn test_d95_create_field_wrong_type() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 1);
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::FIELD_ALL, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::CreateField as u16,
            handle,
            [0; 4],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::CreateField, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr);
            }
            _ => panic!("must return Resume"),
        }

        let x0 = read_typed_result(sender_ptr) as i64;

        assert!(x0 < 0, "D95: CreateField on non-Space must return error");
    }

    /// D95 CreateField: insufficient Space size returns error.
    #[test]
    fn test_d95_create_field_insufficient_space() {
        let ks = make_kernel_state();
        let space_id = make_space_in_arena(&ks, 0x1000, 1);
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Space, space_id, Rights::SPACE_ALL, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::CreateField as u16,
            handle,
            [0; 4],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        core.dispatch_typed(TypedOperation::CreateField, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert!(x0 < 0, "D95: Space too small for one Message must fail");
    }

    /// D95 CreatePulsar: Space consumed, Pulsar created, deadline installed.
    #[test]
    fn test_d95_create_pulsar_success() {
        let ks = make_kernel_state();
        let space_id = make_space_in_arena(&ks, 0x2000, 4096);
        let delivery_field_id = make_field_in_arena(&ks, 16);
        let (mut sender, entries) =
            make_sender_with_cap(ObjectType::Space, space_id, Rights::SPACE_ALL, Badge(0), 0);
        let field_entry = crate::frame::capabilities::entry_mut(entries, 16, 1).unwrap();

        *field_entry = crate::capability::Entry {
            object: Some((ObjectType::Field, delivery_field_id)),
            rights: Rights::FIELD_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let sender_ptr = NonNull::from(&mut sender);
        let space_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let field_handle = crate::capability::Handle {
            index: 1,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let duration_ns: u64 = 10_000_000;
        let period_ns: u64 = 50_000_000;

        setup_typed_regs(
            sender_ptr,
            TypedOperation::CreatePulsar as u16,
            space_handle,
            [field_handle, 42, duration_ns, period_ns],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        assert_eq!(core.deadline_count, 0, "precondition: no deadlines");

        let result = core.dispatch_typed(TypedOperation::CreatePulsar, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "must resume sender");
            }
            _ => panic!("CreatePulsar must return Resume(sender)"),
        }

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D95: CreatePulsar success must return 0");

        // Cap entry must now point to a Pulsar.
        let entry =
            crate::frame::capabilities::entry_ref(entries, 16, 0).expect("entry must exist");
        let (obj_type, pulsar_id) = entry.object.expect("entry must be occupied");

        assert_eq!(
            obj_type,
            ObjectType::Pulsar,
            "D32: cap must be Pulsar after type conversion"
        );

        // Pulsar must exist in arena.
        let pulsars = ks.pulsars.acquire();
        let pulsar = pulsars.get(pulsar_id).expect("Pulsar must exist");

        assert_eq!(pulsar.delivery_field, delivery_field_id);
        assert_eq!(pulsar.badge, Badge(42));
        assert_eq!(pulsar.duration_ns, duration_ns);
        assert_eq!(pulsar.period_ns, period_ns);
        assert!(pulsar.is_repeating(), "period_ns > 0 → repeating");

        drop(pulsars);

        // Deadline must be installed in per-core array.
        assert_eq!(
            core.deadline_count, 1,
            "D83: one deadline must be installed"
        );

        let deadline = core.deadlines[0].expect("deadline must be Some");

        assert_eq!(deadline.pulsar_id, pulsar_id);
        assert_eq!(deadline.field_id, delivery_field_id);
    }

    /// D95 CreateObserver: Space consumed, Observer created with reserved slots.
    #[test]
    fn test_d95_create_observer_success() {
        let ks = make_kernel_state();
        let space_id = make_space_in_arena(&ks, 0x4000, 8 * 4096);
        let handler_field_id = make_field_in_arena(&ks, 16);
        let (mut sender, entries) =
            make_sender_with_cap(ObjectType::Space, space_id, Rights::SPACE_ALL, Badge(0), 0);
        let field_entry = crate::frame::capabilities::entry_mut(entries, 16, 1).unwrap();

        *field_entry = crate::capability::Entry {
            object: Some((ObjectType::Field, handler_field_id)),
            rights: Rights::FIELD_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let sender_ptr = NonNull::from(&mut sender);
        let space_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let handler_handle = crate::capability::Handle {
            index: 1,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let handler_badge: u64 = 0xDEAD;

        setup_typed_regs(
            sender_ptr,
            TypedOperation::CreateObserver as u16,
            space_handle,
            [handler_handle, handler_badge, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::CreateObserver, &ks);

        match result {
            DispatchResult::Resume(resumed) => {
                assert_eq!(resumed, sender_ptr, "must resume sender");
            }
            _ => panic!("CreateObserver must return Resume(sender)"),
        }

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D95: CreateObserver success must return 0");

        // Cap entry must now point to an Observer.
        let entry =
            crate::frame::capabilities::entry_ref(entries, 16, 0).expect("entry must exist");
        let (obj_type, observer_id) = entry.object.expect("entry must be occupied");

        assert_eq!(
            obj_type,
            ObjectType::Observer,
            "D32: cap must be Observer after type conversion"
        );

        // Observer must exist in arena with correct state.
        let observers = ks.observers.acquire();
        let obs = observers.get(observer_id).expect("Observer must exist");

        assert!(
            matches!(obs.state, crate::observer::PrimaryState::Inert),
            "D35: new Observer must be Inert"
        );
        assert!(
            obs.cap_table_capacity >= 4,
            "must have room for reserved slots + 1"
        );

        drop(observers);

        // D57: verify reserved slots in the new Observer's cap table.
        // Slot 0: handler field with badge.
        let slot0 = crate::frame::capabilities::entry_ref(
            {
                let observers = ks.observers.acquire();
                let obs = observers.get(observer_id).unwrap();
                obs.cap_table
            },
            {
                let observers = ks.observers.acquire();
                let obs = observers.get(observer_id).unwrap();
                obs.cap_table_capacity
            },
            crate::capability::SLOT_FAULT_HANDLER,
        )
        .expect("slot 0 must exist");
        let (s0_type, s0_id) = slot0.object.expect("handler slot must be occupied");

        assert_eq!(s0_type, ObjectType::Field, "slot 0 must be handler Field");
        assert_eq!(s0_id, handler_field_id, "slot 0 must point to handler");
        assert_eq!(slot0.badge, Badge(handler_badge), "slot 0 badge must match");

        // Slot 2: self-reference.
        let obs_cap_table;
        let obs_cap_capacity;

        {
            let observers = ks.observers.acquire();
            let obs = observers.get(observer_id).unwrap();

            obs_cap_table = obs.cap_table;
            obs_cap_capacity = obs.cap_table_capacity;
        }

        let slot2 = crate::frame::capabilities::entry_ref(
            obs_cap_table,
            obs_cap_capacity,
            crate::capability::SLOT_SELF,
        )
        .expect("slot 2 must exist");
        let (s2_type, s2_id) = slot2.object.expect("self slot must be occupied");

        assert_eq!(s2_type, ObjectType::Observer, "slot 2 must be Observer");
        assert_eq!(s2_id, observer_id, "slot 2 must be self-reference");
        assert_eq!(
            slot2.rights,
            Rights::OBSERVER_ALL,
            "D57: self-cap must have full rights"
        );

        // Space must be consumed.
        let spaces = ks.spaces.acquire();

        assert!(
            spaces.get(space_id).is_none(),
            "D32: Space must be freed after type conversion"
        );
    }

    /// D95 CreateObserver: Space too small for structural backing.
    #[test]
    fn test_d95_create_observer_insufficient_space() {
        let ks = make_kernel_state();
        let space_id = make_space_in_arena(&ks, 0x1000, 100);
        let handler_field_id = make_field_in_arena(&ks, 4);
        let (mut sender, entries) =
            make_sender_with_cap(ObjectType::Space, space_id, Rights::SPACE_ALL, Badge(0), 0);
        let field_entry = crate::frame::capabilities::entry_mut(entries, 16, 1).unwrap();

        *field_entry = crate::capability::Entry {
            object: Some((ObjectType::Field, handler_field_id)),
            rights: Rights::SEND,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let sender_ptr = NonNull::from(&mut sender);
        let space_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let handler_handle = crate::capability::Handle {
            index: 1,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::CreateObserver as u16,
            space_handle,
            [handler_handle, 0, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        core.dispatch_typed(TypedOperation::CreateObserver, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert!(
            x0 < 0,
            "D95: Space too small for RegisterState + L1 must fail"
        );
    }

    // ── D96: IPC cap transfer mechanics ───────────────────────────────

    /// D96: observer_extract_cap moves a cap out of the sender's table.
    /// The slot becomes empty and reenters the freelist.
    #[test]
    fn test_d96_extract_cap_moves_out_of_sender() {
        let (mut sender, entries) = make_sender_with_cap(
            ObjectType::Field,
            ObjectId(42),
            Rights::SEND,
            Badge(0xBEEF),
            7,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let transferred = crate::frame::cores::observer_extract_cap(sender_ptr, 0)
            .expect("extract must succeed for occupied slot");

        assert_eq!(transferred.object_type, ObjectType::Field);
        assert_eq!(transferred.object_id, ObjectId(42));
        assert_eq!(transferred.rights, Rights::SEND);
        assert_eq!(transferred.badge, Badge(0xBEEF));
        assert_eq!(transferred.stored_generation, 7);

        // Slot 0 must now be empty.
        let slot = crate::frame::capabilities::entry_ref(entries, 16, 0).unwrap();

        assert!(
            !slot.is_occupied(),
            "D96: slot must be empty after extract (move semantics)"
        );
        // Sender's count must have decreased.
        assert_eq!(sender.cap_table_count, 0, "D96: count must decrease");
        // Slot must be in the freelist (free_head points to it).
        assert_eq!(
            sender.cap_table_free_head,
            Some(0),
            "D96: extracted slot must be at freelist head"
        );
    }

    /// D96: extract from empty slot returns None.
    #[test]
    fn test_d96_extract_cap_empty_slot_returns_none() {
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Field, ObjectId(1), Rights::SEND, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        // Slot 5 is empty (not in the test entry).
        let result = crate::frame::cores::observer_extract_cap(sender_ptr, 5);

        assert!(
            result.is_none(),
            "D96: extract from empty slot must be None"
        );
    }

    /// D96: observer_install_transferred_cap installs a cap into the
    /// receiver's table, returns the encoded handle.
    #[test]
    fn test_d96_install_transferred_cap_in_receiver() {
        let mut receiver = crate::observer::Observer::test_with_cap_table(16);
        let entries = receiver.cap_table;
        let receiver_ptr = NonNull::from(&mut receiver);
        let transferred = crate::capability::TransferredCap {
            object_type: ObjectType::Field,
            object_id: ObjectId(99),
            rights: Rights::SEND,
            badge: Badge(0xCAFE),
            send_once: false,
            stored_generation: 3,
        };
        let handle_raw =
            crate::frame::cores::observer_install_transferred_cap(receiver_ptr, &transferred)
                .expect("install must succeed with free slots");

        // D77: decode the handle to verify slot index.
        let handle = crate::capability::Handle::decode(handle_raw);

        assert_eq!(
            handle.index,
            crate::capability::SLOT_USER_START,
            "D96: first install must use SLOT_USER_START"
        );

        // Verify the entry was written correctly.
        let entry = crate::frame::capabilities::entry_ref(entries, 16, handle.index).unwrap();

        assert!(entry.is_occupied(), "D96: installed slot must be occupied");

        let (obj_type, obj_id) = entry.object.unwrap();

        assert_eq!(obj_type, ObjectType::Field);
        assert_eq!(obj_id, ObjectId(99));
        assert_eq!(entry.rights, Rights::SEND);
        assert_eq!(entry.badge, Badge(0xCAFE));
        assert!(!entry.send_once);
        assert_eq!(entry.stored_generation, 3);
        assert_eq!(receiver.cap_table_count, 1, "D96: count must increase");
    }

    /// D96: install into a full table returns TableFull.
    #[test]
    fn test_d96_install_cap_table_full_returns_error() {
        // No freelist initialized — free_head is None.
        let mut receiver = crate::observer::Observer::test_with_registers();
        let entries = crate::frame::capabilities::alloc_test_entries(4);

        receiver.cap_table = entries;
        receiver.cap_table_capacity = 4;
        receiver.cap_table_free_head = None;
        receiver.cap_table_count = 4;
        receiver.compute_aggregate = 0;

        let receiver_ptr = NonNull::from(&mut receiver);
        let transferred = crate::capability::TransferredCap {
            object_type: ObjectType::Space,
            object_id: ObjectId(1),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            send_once: false,
            stored_generation: 0,
        };
        let result =
            crate::frame::cores::observer_install_transferred_cap(receiver_ptr, &transferred);

        assert!(
            matches!(result, Err(crate::capability::CapError::TableFull)),
            "D96/D40: install into full table must return TableFull"
        );
    }

    /// D96: reply cap is send-once — the transferred cap carries the flag.
    #[test]
    fn test_d96_reply_cap_is_send_once() {
        let mut receiver = crate::observer::Observer::test_with_cap_table(16);
        let entries = receiver.cap_table;
        let receiver_ptr = NonNull::from(&mut receiver);
        // Simulate a reply cap: send-once = true, reply badge embedded.
        let reply_transferred = crate::capability::TransferredCap {
            object_type: ObjectType::Field,
            object_id: ObjectId(77),
            rights: Rights::SEND.union(Rights::DESTROY).union(Rights::CLONE),
            badge: Badge(0x1234),
            send_once: true,
            stored_generation: 5,
        };
        let handle_raw =
            crate::frame::cores::observer_install_transferred_cap(receiver_ptr, &reply_transferred)
                .expect("install must succeed");
        let handle = crate::capability::Handle::decode(handle_raw);
        let entry = crate::frame::capabilities::entry_ref(entries, 16, handle.index).unwrap();

        assert!(entry.is_send_once(), "D96/D51: reply cap must be send-once");
        assert_eq!(entry.badge, Badge(0x1234), "D96/D65: reply badge preserved");
    }

    /// D96: deliver_message installs user_cap and reply_cap into receiver's
    /// table. Registers x6 and x7 carry the encoded handles.
    #[test]
    fn test_d96_deliver_message_installs_caps() {
        let mut receiver = crate::observer::Observer::test_with_cap_table(16);
        let entries = receiver.cap_table;
        let receiver_ptr = NonNull::from(&mut receiver);
        let message = crate::field::Message {
            data: [0xAA, 0xBB, 0xCC, 0xDD],
            label: 0xFACE,
            badge: Badge(0xBEEF),
            user_cap: Some(crate::capability::TransferredCap {
                object_type: ObjectType::Field,
                object_id: ObjectId(10),
                rights: Rights::SEND,
                badge: Badge(0x111),
                send_once: false,
                stored_generation: 1,
            }),
            reply_cap: Some(crate::capability::TransferredCap {
                object_type: ObjectType::Field,
                object_id: ObjectId(20),
                rights: Rights::SEND,
                badge: Badge(0x222),
                send_once: true,
                stored_generation: 2,
            }),
        };

        CoreState::<RoundRobin>::deliver_message(receiver_ptr, &message);

        // Read receiver's registers to check x6 and x7.
        let regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_eq!(regs.data, [0xAA, 0xBB, 0xCC, 0xDD], "D96: data preserved");
        assert_eq!(regs.label, 0xFACE, "D96: label preserved");
        assert_eq!(regs.handle_or_badge, 0xBEEF, "D96: badge preserved");
        // x6 and x7 must NOT be CAP_ABSENT — caps were installed.
        assert_ne!(
            regs.user_cap,
            crate::capability::CAP_ABSENT,
            "D96: user cap must be installed (x6 != CAP_ABSENT)"
        );
        assert_ne!(
            regs.reply_info,
            crate::capability::CAP_ABSENT,
            "D96: reply cap must be installed (x7 != CAP_ABSENT)"
        );

        // Verify the handles decode to valid slot indices.
        let user_handle = crate::capability::Handle::decode(regs.user_cap);
        let reply_handle = crate::capability::Handle::decode(regs.reply_info);

        assert_eq!(
            user_handle.index,
            crate::capability::SLOT_USER_START,
            "D96: user cap at first free slot"
        );
        assert_eq!(
            reply_handle.index,
            crate::capability::SLOT_USER_START + 1,
            "D96: reply cap at second free slot"
        );

        // Verify cap table entries.
        let user_entry =
            crate::frame::capabilities::entry_ref(entries, 16, user_handle.index).unwrap();
        let (ut, uid) = user_entry.object.unwrap();

        assert_eq!(ut, ObjectType::Field);
        assert_eq!(uid, ObjectId(10));

        let reply_entry =
            crate::frame::capabilities::entry_ref(entries, 16, reply_handle.index).unwrap();
        let (rt, rid) = reply_entry.object.unwrap();

        assert_eq!(rt, ObjectType::Field);
        assert_eq!(rid, ObjectId(20));
        assert!(reply_entry.is_send_once(), "D96: reply cap is send-once");
        assert_eq!(receiver.cap_table_count, 2, "D96: two caps installed");
    }

    /// D96: deliver_message with no caps writes CAP_ABSENT to x6 and x7.
    #[test]
    fn test_d96_deliver_message_no_caps_writes_absent() {
        let mut receiver = crate::observer::Observer::test_with_registers();

        receiver.compute_aggregate = 0;

        let receiver_ptr = NonNull::from(&mut receiver);
        let message = crate::field::Message {
            data: [1, 2, 3, 4],
            label: 0xFF,
            badge: Badge(0),
            user_cap: None,
            reply_cap: None,
        };

        CoreState::<RoundRobin>::deliver_message(receiver_ptr, &message);

        let regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_eq!(
            regs.user_cap,
            crate::capability::CAP_ABSENT,
            "D96: no user cap → x6 = CAP_ABSENT"
        );
        assert_eq!(
            regs.reply_info,
            crate::capability::CAP_ABSENT,
            "D96: no reply cap → x7 = CAP_ABSENT"
        );
    }

    /// D96: extract then install round-trips a cap between two Observers.
    #[test]
    fn test_d96_cap_move_roundtrip_sender_to_receiver() {
        // Sender has a Field cap at slot 0.
        let (mut sender, _sender_entries) = make_sender_with_cap(
            ObjectType::Field,
            ObjectId(55),
            Rights::FIELD_ALL,
            Badge(0xDEAD),
            10,
        );
        let sender_ptr = NonNull::from(&mut sender);
        // Receiver has an empty table with freelist.
        let mut receiver = crate::observer::Observer::test_with_cap_table(16);
        let receiver_entries = receiver.cap_table;
        let receiver_ptr = NonNull::from(&mut receiver);
        // D96 §2: move cap from sender to receiver.
        let transferred =
            crate::frame::cores::observer_extract_cap(sender_ptr, 0).expect("extract must succeed");

        assert_eq!(sender.cap_table_count, 0, "sender count after extract");

        let handle_raw =
            crate::frame::cores::observer_install_transferred_cap(receiver_ptr, &transferred)
                .expect("install must succeed");

        assert_eq!(receiver.cap_table_count, 1, "receiver count after install");

        // Verify the cap arrived intact.
        let handle = crate::capability::Handle::decode(handle_raw);
        let entry =
            crate::frame::capabilities::entry_ref(receiver_entries, 16, handle.index).unwrap();
        let (obj_type, obj_id) = entry.object.unwrap();

        assert_eq!(obj_type, ObjectType::Field, "D96: object type preserved");
        assert_eq!(obj_id, ObjectId(55), "D96: object id preserved");
        assert_eq!(entry.rights, Rights::FIELD_ALL, "D96: rights preserved");
        assert_eq!(entry.badge, Badge(0xDEAD), "D96: badge preserved");
        assert_eq!(entry.stored_generation, 10, "D96: generation preserved");
    }

    /// D96: DirectSwitch approved path installs reply cap in receiver.
    /// RoundRobin always approves, so this tests the fast-path cap install.
    #[test]
    fn test_d96_direct_switch_approved_installs_reply_cap() {
        let mut receiver = crate::observer::Observer::test_with_cap_table(16);
        let receiver_entries = receiver.cap_table;

        receiver.state = crate::observer::PrimaryState::Blocked;

        let receiver_ptr = NonNull::from(&mut receiver);
        let mut sender = crate::observer::Observer::test_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let outcome = crate::communication::CallOutcome::DirectSwitch(receiver_ptr);
        let reply_cap = Some(crate::capability::TransferredCap {
            object_type: ObjectType::Field,
            object_id: ObjectId(88),
            rights: Rights::SEND,
            badge: Badge(0x4567),
            send_once: true,
            stored_generation: 0,
        });
        let result = core
            .dispatch_call_outcome_with_metadata(sender_ptr, outcome, 0x1ABE, 0xBAD6E, reply_cap);

        // RoundRobin approves → fast path.
        assert!(
            matches!(result, DispatchResult::ResumeFastPath(_)),
            "D96: RoundRobin approves DirectSwitch → fast path"
        );

        // D96: reply cap must be installed in receiver's x7.
        let recv_regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_ne!(
            recv_regs.reply_info,
            crate::capability::CAP_ABSENT,
            "D96: reply cap must be installed (x7 != CAP_ABSENT)"
        );

        // Verify reply cap entry in receiver's table.
        let reply_handle = crate::capability::Handle::decode(recv_regs.reply_info);
        let reply_entry =
            crate::frame::capabilities::entry_ref(receiver_entries, 16, reply_handle.index)
                .unwrap();

        assert!(
            reply_entry.is_send_once(),
            "D96/D51: reply cap must be send-once"
        );
        assert_eq!(
            reply_entry.badge,
            Badge(0x4567),
            "D96/D65: reply badge preserved"
        );
        assert_eq!(
            reply_entry.object.unwrap().1,
            ObjectId(88),
            "D96: reply Field ObjectId preserved"
        );
        // x4 (label) and x5 (badge) must be written for metadata path.
        assert_eq!(recv_regs.label, 0x1ABE, "D96: label in x4");
        assert_eq!(recv_regs.handle_or_badge, 0xBAD6E, "D96: badge in x5");
    }

    /// D96 §4: DirectSwitch denied path constructs Message from sender's
    /// saved registers and delivers to receiver via slow path with reply cap.
    /// Uses WokeReceiverSlowPath as a proxy since RoundRobin always approves.
    #[test]
    fn test_d96_slow_path_delivers_reply_cap() {
        let mut receiver = crate::observer::Observer::test_with_cap_table(16);
        let receiver_entries = receiver.cap_table;

        receiver.state = crate::observer::PrimaryState::Blocked;

        let receiver_ptr = NonNull::from(&mut receiver);
        let mut sender = crate::observer::Observer::test_with_registers();
        let sender_ptr = NonNull::from(&mut sender);
        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        // D96 §4 slow path: WokeReceiverSlowPath carries both user and reply cap.
        let message = crate::field::Message {
            data: [0xD1, 0xD2, 0xD3, 0xD4],
            label: 0xFACE,
            badge: Badge(0xBEEF),
            user_cap: Some(crate::capability::TransferredCap {
                object_type: ObjectType::Field,
                object_id: ObjectId(10),
                rights: Rights::SEND,
                badge: Badge(0x111),
                send_once: false,
                stored_generation: 1,
            }),
            reply_cap: Some(crate::capability::TransferredCap {
                object_type: ObjectType::Field,
                object_id: ObjectId(20),
                rights: Rights::SEND,
                badge: Badge(0x222),
                send_once: true,
                stored_generation: 2,
            }),
        };
        let outcome =
            crate::communication::CallOutcome::WokeReceiverSlowPath(receiver_ptr, message);
        let _result = core.dispatch_call_outcome_with_metadata(sender_ptr, outcome, 0, 0, None);
        // Receiver must have both caps installed.
        let recv_regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_ne!(
            recv_regs.user_cap,
            crate::capability::CAP_ABSENT,
            "D96: user cap must be installed on slow path"
        );
        assert_ne!(
            recv_regs.reply_info,
            crate::capability::CAP_ABSENT,
            "D96: reply cap must be installed on slow path"
        );

        // Verify reply cap is send-once.
        let reply_handle = crate::capability::Handle::decode(recv_regs.reply_info);
        let reply_entry =
            crate::frame::capabilities::entry_ref(receiver_entries, 16, reply_handle.index)
                .unwrap();

        assert!(reply_entry.is_send_once(), "D96: reply cap is send-once");
        assert_eq!(receiver.cap_table_count, 2, "D96: two caps installed");
    }

    /// D96: multiple cap installations advance the freelist correctly.
    #[test]
    fn test_d96_multiple_installs_advance_freelist() {
        let mut receiver = crate::observer::Observer::test_with_cap_table(8);

        receiver.compute_aggregate = 0;

        let receiver_ptr = NonNull::from(&mut receiver);

        // Install caps until the table is full (slots 3..8 = 5 slots).
        for i in 0u32..5 {
            let tc = crate::capability::TransferredCap {
                object_type: ObjectType::Space,
                object_id: ObjectId(i),
                rights: Rights::SPACE_ALL,
                badge: Badge(i as u64),
                send_once: false,
                stored_generation: 0,
            };
            let handle_raw =
                crate::frame::cores::observer_install_transferred_cap(receiver_ptr, &tc)
                    .expect("install must succeed");
            let handle = crate::capability::Handle::decode(handle_raw);

            assert_eq!(
                handle.index,
                crate::capability::SLOT_USER_START + i,
                "D96: sequential slot allocation"
            );
        }

        assert_eq!(receiver.cap_table_count, 5, "D96: 5 caps installed");
        assert_eq!(
            receiver.cap_table_free_head, None,
            "D96: freelist exhausted after 5 installs"
        );

        // Next install must fail with TableFull.
        let overflow = crate::capability::TransferredCap {
            object_type: ObjectType::Space,
            object_id: ObjectId(99),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            send_once: false,
            stored_generation: 0,
        };
        let result = crate::frame::cores::observer_install_transferred_cap(receiver_ptr, &overflow);

        assert!(
            matches!(result, Err(crate::capability::CapError::TableFull)),
            "D96: install after freelist exhausted must return TableFull"
        );
    }

    /// D96: extract followed by install reuses the freed slot.
    #[test]
    fn test_d96_extract_then_install_reuses_slot() {
        let (mut sender, entries) =
            make_sender_with_cap(ObjectType::Field, ObjectId(1), Rights::SEND, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        // Extract slot 0.
        let _transferred =
            crate::frame::cores::observer_extract_cap(sender_ptr, 0).expect("extract must succeed");
        // Slot 0 is now at freelist head. Install a new cap — should reuse slot 0.
        let new_cap = crate::capability::TransferredCap {
            object_type: ObjectType::Space,
            object_id: ObjectId(999),
            rights: Rights::SPACE_ALL,
            badge: Badge(0x42),
            send_once: false,
            stored_generation: 5,
        };
        let handle_raw =
            crate::frame::cores::observer_install_transferred_cap(sender_ptr, &new_cap)
                .expect("install must succeed");
        let handle = crate::capability::Handle::decode(handle_raw);

        assert_eq!(
            handle.index, 0,
            "D96: freed slot 0 must be reused by next install"
        );

        // Verify new cap is in place.
        let entry = crate::frame::capabilities::entry_ref(entries, 16, 0).unwrap();
        let (obj_type, obj_id) = entry.object.unwrap();

        assert_eq!(obj_type, ObjectType::Space);
        assert_eq!(obj_id, ObjectId(999));
        assert_eq!(sender.cap_table_count, 1, "count back to 1 after roundtrip");
    }

    // ── D97 cap-table self-mutation tests ────────────────────────────

    /// D97 Clone: cloning a Field cap produces a new entry with identical
    /// object reference, rights, badge, send_once, and stored_generation.
    #[test]
    fn test_d97_clone_field_cap() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 4);
        let (mut sender, entries) = make_sender_with_cap(
            ObjectType::Field,
            field_id,
            Rights::FIELD_ALL,
            Badge(0xCAFE),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Clone as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Clone, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert!(
            (x0 as i64) >= 0,
            "D97: Clone must return non-negative slot handle, got {x0}"
        );

        let new_handle = crate::capability::Handle::decode(x0);

        assert!(
            new_handle.index >= crate::capability::SLOT_USER_START,
            "D97: cloned cap must be in user slot range"
        );

        let new_entry =
            crate::frame::capabilities::entry_ref(entries, 16, new_handle.index).unwrap();
        let (obj_type, obj_id) = new_entry.object.unwrap();

        assert_eq!(obj_type, ObjectType::Field, "D97: clone preserves type");
        assert_eq!(obj_id, field_id, "D97: clone preserves object_id");
        assert_eq!(
            new_entry.rights,
            Rights::FIELD_ALL,
            "D97: clone preserves rights"
        );
        assert_eq!(new_entry.badge, Badge(0xCAFE), "D97: clone preserves badge");
        assert_eq!(
            new_entry.stored_generation, 0,
            "D97: clone preserves stored_generation"
        );
        assert_eq!(sender.cap_table_count, 2, "D97: count incremented to 2");
    }

    /// D97 Clone: send-once flag is preserved through clone (D51).
    #[test]
    fn test_d97_clone_preserves_send_once() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 4);
        let mut sender = crate::observer::Observer::test_with_cap_table(16);
        let entries = sender.cap_table;
        let entry = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        *entry = crate::capability::Entry {
            object: Some((ObjectType::Field, field_id)),
            rights: Rights::FIELD_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: true,
            stored_generation: 0,
        };
        sender.cap_table_count = 1;

        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Clone as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Clone, &ks);

        let x0 = read_typed_result(sender_ptr);
        let new_handle = crate::capability::Handle::decode(x0);
        let new_entry =
            crate::frame::capabilities::entry_ref(entries, 16, new_handle.index).unwrap();

        assert!(
            new_entry.send_once,
            "D51/D97: clone must preserve send_once flag"
        );
    }

    /// D97 Clone: cloning an Observer cap works (non-linear type).
    #[test]
    fn test_d97_clone_observer_cap() {
        let ks = make_kernel_state();
        let target_id = {
            let mut observers = ks.observers.acquire();
            let (id, obs) = observers.allocate().expect("allocate observer");

            obs.state = crate::observer::PrimaryState::Inert;
            obs.refcount = 1;
            obs.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, entries) = make_sender_with_cap(
            ObjectType::Observer,
            target_id,
            Rights::OBSERVER_ALL,
            Badge(42),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Clone as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Clone, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert!((x0 as i64) >= 0, "D97: Clone Observer must succeed");

        let new_handle = crate::capability::Handle::decode(x0);
        let new_entry =
            crate::frame::capabilities::entry_ref(entries, 16, new_handle.index).unwrap();
        let (obj_type, obj_id) = new_entry.object.unwrap();

        assert_eq!(obj_type, ObjectType::Observer);
        assert_eq!(obj_id, target_id);
    }

    /// D97 Close: closing a Field cap succeeds and frees the slot.
    #[test]
    fn test_d97_close_field_cap() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 4);
        let (mut sender, entries) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::FIELD_ALL, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Close as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Close, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D97: Close must return 0 on success");

        let closed_entry = crate::frame::capabilities::entry_ref(entries, 16, 0).unwrap();

        assert!(
            !closed_entry.is_occupied(),
            "D97: closed slot must be empty"
        );
        assert_eq!(
            sender.cap_table_count, 0,
            "D97: count must be 0 after close"
        );
    }

    /// D97 Close: closing a Space cap triggers D24 mapping bridge scan.
    /// Verifies the scan correctly detects remaining Space caps.
    #[test]
    fn test_d97_close_space_cap_mapping_bridge_scan() {
        let ks = make_kernel_state();
        let space_id = {
            let mut spaces = ks.spaces.acquire();
            let (id, space) = spaces.allocate().expect("allocate space");

            space.va_base = 0x1000;
            space.size = 0x4000;
            space.refcount = 2;
            space.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        // Create a sender with TWO caps to the same Space:
        // slot 0 = Space cap (will be closed)
        // slot 3 = Space cap (remains)
        let mut sender = crate::observer::Observer::test_with_cap_table(16);
        let entries = sender.cap_table;
        let entry0 = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        *entry0 = crate::capability::Entry {
            object: Some((ObjectType::Space, space_id)),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let entry3 = crate::frame::capabilities::entry_mut(entries, 16, 3).unwrap();

        *entry3 = crate::capability::Entry {
            object: Some((ObjectType::Space, space_id)),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };
        sender.cap_table_free_head = Some(4);
        sender.cap_table_count = 2;
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Close as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Close, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D97: Close Space cap must succeed");

        let slot0 = crate::frame::capabilities::entry_ref(entries, 16, 0).unwrap();

        assert!(!slot0.is_occupied(), "D97: slot 0 must be freed");

        let slot3 = crate::frame::capabilities::entry_ref(entries, 16, 3).unwrap();

        assert!(
            slot3.is_occupied(),
            "D97: slot 3 (other Space cap) must remain"
        );

        let still_has = crate::frame::cores::observer_has_cap_to_object(
            sender_ptr,
            ObjectType::Space,
            space_id,
            0,
        );

        assert!(
            still_has,
            "D24: Observer still holds another cap to the same Space"
        );
    }

    /// D97 Mint: attenuate rights and assign new badge.
    #[test]
    fn test_d97_mint_attenuates_rights_and_sets_badge() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 4);
        let (mut sender, entries) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::FIELD_ALL, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let requested_rights = Rights::SEND.union(Rights::RECEIVE);

        setup_typed_regs(
            sender_ptr,
            TypedOperation::Mint as u16,
            handle,
            [requested_rights.bits() as u64, 0xBEEF, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Mint, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert!((x0 as i64) >= 0, "D97: Mint must succeed");

        let new_handle = crate::capability::Handle::decode(x0);
        let new_entry =
            crate::frame::capabilities::entry_ref(entries, 16, new_handle.index).unwrap();
        let (obj_type, obj_id) = new_entry.object.unwrap();

        assert_eq!(obj_type, ObjectType::Field, "D97: mint preserves type");
        assert_eq!(obj_id, field_id, "D97: mint preserves object_id");

        let attenuated = Rights::FIELD_ALL.attenuate(requested_rights);

        assert_eq!(
            new_entry.rights, attenuated,
            "D97: mint must attenuate rights to intersection"
        );
        assert_eq!(
            new_entry.badge,
            Badge(0xBEEF),
            "D97: mint must set caller-provided badge"
        );
    }

    /// D97 Mint: badge == CAP_ABSENT preserves source badge.
    #[test]
    fn test_d97_mint_preserves_badge_with_sentinel() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 4);
        let (mut sender, entries) = make_sender_with_cap(
            ObjectType::Field,
            field_id,
            Rights::FIELD_ALL,
            Badge(0xAAAA),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::Mint as u16,
            handle,
            [
                Rights::FIELD_ALL.bits() as u64,
                crate::capability::CAP_ABSENT,
                0,
                0,
            ],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Mint, &ks);

        let x0 = read_typed_result(sender_ptr);
        let new_handle = crate::capability::Handle::decode(x0);
        let new_entry =
            crate::frame::capabilities::entry_ref(entries, 16, new_handle.index).unwrap();

        assert_eq!(
            new_entry.badge,
            Badge(0xAAAA),
            "D97: Mint with CAP_ABSENT badge must preserve source badge"
        );
    }

    /// D97 Mint: cannot escalate rights (attenuate is intersection).
    #[test]
    fn test_d97_mint_cannot_escalate_rights() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 4);
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::SEND, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::Mint as u16,
            handle,
            [Rights::FIELD_ALL.bits() as u64, 0, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        // MINT right is not in the source cap — should fail NoRight.
        core.dispatch_typed(TypedOperation::Mint, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert_eq!(
            x0,
            crate::syscall::SyscallError::NoRight.error_code() as i64,
            "D97: Mint without MINT right must return NoRight"
        );
    }

    /// D97 ObserverInstallCap: install a Field cap into a target Observer's table.
    #[test]
    fn test_d97_observer_install_cap() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 4);
        // Create target Observer in arena with a cap table.
        let (target_id, target_entries) = {
            let mut observers = ks.observers.acquire();
            let (id, obs) = observers.allocate().expect("allocate observer");
            let rs = crate::frame::cores::alloc_test_register_state();
            let cap_entries = crate::frame::capabilities::alloc_test_entries(16);

            crate::frame::capabilities::init_freelist(
                cap_entries,
                16,
                crate::capability::SLOT_USER_START,
            );

            obs.object_id = id;
            obs.asid = 0;
            obs.register_state = crate::observer::RegisterStateHandle::new(rs);
            obs.cap_table = cap_entries;
            obs.cap_table_capacity = 16;
            obs.cap_table_free_head = Some(crate::capability::SLOT_USER_START);
            obs.cap_table_count = 0;
            obs.state = crate::observer::PrimaryState::Inert;
            obs.refcount = 1;
            obs.generation = core::sync::atomic::AtomicU64::new(0);

            (id, cap_entries)
        };
        // Sender has TWO caps: slot 0 = Observer cap (target), slot 3 = Field cap (source).
        let mut sender = crate::observer::Observer::test_with_cap_table(16);
        let entries = sender.cap_table;
        let slot0 = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        *slot0 = crate::capability::Entry {
            object: Some((ObjectType::Observer, target_id)),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let slot3 = crate::frame::capabilities::entry_mut(entries, 16, 3).unwrap();

        *slot3 = crate::capability::Entry {
            object: Some((ObjectType::Field, field_id)),
            rights: Rights::SEND.union(Rights::RECEIVE),
            badge: Badge(0xDEAD),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };
        sender.cap_table_free_head = Some(4);
        sender.cap_table_count = 2;

        let sender_ptr = NonNull::from(&mut sender);
        let observer_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let source_handle = crate::capability::Handle {
            index: 3,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverInstallCap as u16,
            observer_handle,
            [source_handle, 0, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::ObserverInstallCap, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert!(
            (x0 as i64) >= 0,
            "D97: ObserverInstallCap must succeed, got {x0}"
        );

        let installed_handle = crate::capability::Handle::decode(x0);
        let installed_entry =
            crate::frame::capabilities::entry_ref(target_entries, 16, installed_handle.index)
                .unwrap();
        let (obj_type, obj_id) = installed_entry.object.unwrap();

        assert_eq!(
            obj_type,
            ObjectType::Field,
            "D97: installed cap must be Field"
        );
        assert_eq!(
            obj_id, field_id,
            "D97: installed cap must reference the source object"
        );
        assert_eq!(
            installed_entry.rights,
            Rights::SEND.union(Rights::RECEIVE),
            "D97: installed cap preserves source rights"
        );
        assert_eq!(
            installed_entry.badge,
            Badge(0xDEAD),
            "D97: installed cap preserves source badge"
        );
    }

    /// D97 ObserverChangeHandler: replaces the fault handler Field cap at
    /// SLOT_FAULT_HANDLER in the target Observer.
    #[test]
    fn test_d97_observer_change_handler() {
        let ks = make_kernel_state();
        let old_field_id = make_field_in_arena(&ks, 4);
        let new_field_id = make_field_in_arena(&ks, 4);
        // Create target Observer in arena with old handler at slot 0.
        let target_id = {
            let mut observers = ks.observers.acquire();
            let (id, obs) = observers.allocate().expect("allocate observer");
            let rs = crate::frame::cores::alloc_test_register_state();
            let cap_entries = crate::frame::capabilities::alloc_test_entries(16);

            crate::frame::capabilities::init_freelist(
                cap_entries,
                16,
                crate::capability::SLOT_USER_START,
            );

            // Install old handler at slot 0.
            let handler_slot = crate::frame::capabilities::entry_mut(cap_entries, 16, 0).unwrap();

            *handler_slot = crate::capability::Entry {
                object: Some((ObjectType::Field, old_field_id)),
                rights: Rights::SEND,
                badge: Badge(0),
                slot_tag: crate::capability::SlotTag(0),
                send_once: false,
                stored_generation: 0,
            };

            obs.object_id = id;
            obs.asid = 0;
            obs.register_state = crate::observer::RegisterStateHandle::new(rs);
            obs.cap_table = cap_entries;
            obs.cap_table_capacity = 16;
            obs.cap_table_free_head = Some(crate::capability::SLOT_USER_START);
            obs.cap_table_count = 1;
            obs.state = crate::observer::PrimaryState::Inert;
            obs.refcount = 1;
            obs.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        // Sender has: slot 0 = Observer cap, slot 3 = new handler Field cap.
        let mut sender = crate::observer::Observer::test_with_cap_table(16);
        let entries = sender.cap_table;
        let slot0 = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        *slot0 = crate::capability::Entry {
            object: Some((ObjectType::Observer, target_id)),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let slot3 = crate::frame::capabilities::entry_mut(entries, 16, 3).unwrap();

        *slot3 = crate::capability::Entry {
            object: Some((ObjectType::Field, new_field_id)),
            rights: Rights::SEND,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };
        sender.cap_table_free_head = Some(4);
        sender.cap_table_count = 2;

        let sender_ptr = NonNull::from(&mut sender);
        let observer_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let handler_handle = crate::capability::Handle {
            index: 3,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverChangeHandler as u16,
            observer_handle,
            [handler_handle, 0xBBBB, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::ObserverChangeHandler, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D97: ObserverChangeHandler must succeed");

        // Verify the target Observer's handler slot now points to new_field_id.
        let target_ptr = crate::frame::cores::observer_ptr_from_arena(&ks, target_id).unwrap();
        let handler_entry = crate::frame::cores::observer_read_cap_entry(
            target_ptr,
            crate::capability::SLOT_FAULT_HANDLER,
        );
        let (handler_type, handler_id, _gen) =
            handler_entry.expect("handler slot must be occupied");

        assert_eq!(
            handler_type,
            ObjectType::Field,
            "D97: handler must be a Field"
        );
        assert_eq!(
            handler_id, new_field_id,
            "D97: handler must point to new Field"
        );
    }

    /// D97 ObserverChangeHandler: non-Field handler returns WrongType.
    #[test]
    fn test_d97_change_handler_wrong_type() {
        let ks = make_kernel_state();
        let target_id = {
            let mut observers = ks.observers.acquire();
            let (id, obs) = observers.allocate().expect("allocate observer");
            let rs = crate::frame::cores::alloc_test_register_state();
            let cap_entries = crate::frame::capabilities::alloc_test_entries(16);

            crate::frame::capabilities::init_freelist(
                cap_entries,
                16,
                crate::capability::SLOT_USER_START,
            );

            obs.object_id = id;
            obs.asid = 0;
            obs.register_state = crate::observer::RegisterStateHandle::new(rs);
            obs.cap_table = cap_entries;
            obs.cap_table_capacity = 16;
            obs.cap_table_free_head = Some(crate::capability::SLOT_USER_START);
            obs.cap_table_count = 0;
            obs.state = crate::observer::PrimaryState::Inert;
            obs.refcount = 1;
            obs.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let space_id = {
            let mut spaces = ks.spaces.acquire();
            let (id, space) = spaces.allocate().expect("allocate space");

            space.va_base = 0;
            space.size = 4096;
            space.refcount = 1;
            space.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        // Sender: slot 0 = Observer, slot 3 = Space (wrong type for handler).
        let mut sender = crate::observer::Observer::test_with_cap_table(16);
        let entries = sender.cap_table;
        let slot0 = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        *slot0 = crate::capability::Entry {
            object: Some((ObjectType::Observer, target_id)),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let slot3 = crate::frame::capabilities::entry_mut(entries, 16, 3).unwrap();

        *slot3 = crate::capability::Entry {
            object: Some((ObjectType::Space, space_id)),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };
        sender.cap_table_free_head = Some(4);
        sender.cap_table_count = 2;

        let sender_ptr = NonNull::from(&mut sender);
        let observer_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let bad_handler_handle = crate::capability::Handle {
            index: 3,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::ObserverChangeHandler as u16,
            observer_handle,
            [bad_handler_handle, 0, 0, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::ObserverChangeHandler, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert_eq!(
            x0,
            crate::syscall::SyscallError::WrongType.error_code() as i64,
            "D97: non-Field handler must return WrongType"
        );
    }

    /// D24/D97: has_cap_to_object returns false when no remaining caps exist.
    #[test]
    fn test_d24_no_remaining_space_cap_after_close() {
        let ks = make_kernel_state();
        let space_id = {
            let mut spaces = ks.spaces.acquire();
            let (id, space) = spaces.allocate().expect("allocate space");

            space.va_base = 0x1000;
            space.size = 0x4000;
            space.refcount = 1;
            space.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Space, space_id, Rights::SPACE_ALL, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(sender_ptr, TypedOperation::Close as u16, handle, [0; 4]);

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::Close, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D97: Close must succeed");

        let still_has = crate::frame::cores::observer_has_cap_to_object(
            sender_ptr,
            ObjectType::Space,
            space_id,
            0,
        );

        assert!(
            !still_has,
            "D24: after closing the only Space cap, no remaining cap should exist"
        );
    }

    /// D97: has_cap_to_object scans correctly with Table method.
    #[test]
    fn test_d97_table_has_cap_to_object() {
        let entries = crate::frame::capabilities::alloc_test_entries(8);

        crate::frame::capabilities::init_freelist(entries, 8, crate::capability::SLOT_USER_START);

        let mut table = crate::capability::Table {
            entries,
            capacity: 8,
            count: 0,
            free_head: Some(crate::capability::SLOT_USER_START),
        };
        let target_id = ObjectId(42);

        assert!(
            !table.has_cap_to_object(ObjectType::Space, target_id, u32::MAX),
            "empty table has no caps"
        );

        let e3 = crate::frame::capabilities::entry_mut(entries, 8, 3).unwrap();

        *e3 = crate::capability::Entry {
            object: Some((ObjectType::Space, target_id)),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };
        table.count = 1;

        assert!(
            table.has_cap_to_object(ObjectType::Space, target_id, u32::MAX),
            "must find cap at slot 3"
        );
        assert!(
            !table.has_cap_to_object(ObjectType::Space, target_id, 3),
            "must not find cap when slot 3 is excluded"
        );
        assert!(
            !table.has_cap_to_object(ObjectType::Field, target_id, u32::MAX),
            "must not match wrong type"
        );
        assert!(
            !table.has_cap_to_object(ObjectType::Space, ObjectId(99), u32::MAX),
            "must not match wrong id"
        );
    }

    // ── D99 — FieldSplit dispatch tests ──────────────────────────────

    /// D99: FieldSplit creates sub-Field, adds route, updates IRQ routing.
    #[test]
    fn test_d99_field_split_success() {
        let ks = make_kernel_state();
        let source_field_id = make_field_in_arena(&ks, 16);
        let space_id = make_space_in_arena(&ks, 0x3000, 4 * 4096);
        // Sender has: slot 0 = source Field cap, slot 1 = Space cap.
        let (mut sender, entries) = make_sender_with_cap(
            ObjectType::Field,
            source_field_id,
            Rights::FIELD_ALL,
            Badge(0),
            0,
        );
        let space_entry = crate::frame::capabilities::entry_mut(entries, 16, 1).unwrap();

        *space_entry = crate::capability::Entry {
            object: Some((ObjectType::Space, space_id)),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let sender_ptr = NonNull::from(&mut sender);
        let field_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let space_handle = crate::capability::Handle {
            index: 1,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::FieldSplit as u16,
            field_handle,
            [space_handle, 100, 200, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_typed(TypedOperation::FieldSplit, &ks);

        match result {
            DispatchResult::Resume(resumed) => assert_eq!(resumed, sender_ptr),
            _ => panic!("FieldSplit must return Resume(sender)"),
        }

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D99: FieldSplit success must return 0");

        // Space cap slot must now contain a Field cap.
        let slot1 = crate::frame::capabilities::entry_ref(entries, 16, 1).unwrap();
        let (obj_type, new_field_id) = slot1.object.expect("slot 1 must be occupied");

        assert_eq!(obj_type, ObjectType::Field, "D99: new cap must be Field");
        assert_ne!(
            new_field_id, source_field_id,
            "D99: new Field must be distinct from source"
        );

        // New Field must exist in arena with correct queue capacity.
        let fields = ks.fields.acquire();
        let new_field = fields.get(new_field_id).expect("new Field must exist");
        let expected_capacity = (4 * 4096) / core::mem::size_of::<crate::field::Message>();

        assert_eq!(new_field.queue_capacity, expected_capacity as u32);
        assert_eq!(new_field.queue_length, 0);

        // Source Field must have a routing rule.
        let source_field = fields
            .get(source_field_id)
            .expect("source Field must exist");

        assert_eq!(
            source_field.resolve_route(150),
            Some(new_field_id),
            "D99: source Field must route badge 150 to new sub-Field"
        );
        assert!(
            source_field.resolve_route(50).is_none(),
            "D99: badge outside split range must not route"
        );

        // Space must be consumed.
        drop(fields);

        let spaces = ks.spaces.acquire();

        assert!(
            spaces.get(space_id).is_none(),
            "D99: Space must be freed after type conversion"
        );
    }

    /// D99: FieldSplit updates IrqRoutingTable for affected routes.
    #[test]
    fn test_d99_field_split_updates_irq_routing() {
        let ks = make_kernel_state();
        let source_field_id = make_field_in_arena(&ks, 16);
        let space_id = make_space_in_arena(&ks, 0x3000, 4 * 4096);

        // Pre-populate IRQ routing table with routes pointing to source Field.
        {
            let mut irq_routes = ks.irq_routes.acquire();

            irq_routes.populate_device_routes(source_field_id, 0, 32, 48);
        }

        let (mut sender, entries) = make_sender_with_cap(
            ObjectType::Field,
            source_field_id,
            Rights::FIELD_ALL,
            Badge(0),
            0,
        );
        let space_entry = crate::frame::capabilities::entry_mut(entries, 16, 1).unwrap();

        *space_entry = crate::capability::Entry {
            object: Some((ObjectType::Space, space_id)),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let sender_ptr = NonNull::from(&mut sender);
        let field_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let space_handle = crate::capability::Handle {
            index: 1,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        // Split badge range [40, 45] — overlaps with populated INTIDs 40-45.
        setup_typed_regs(
            sender_ptr,
            TypedOperation::FieldSplit as u16,
            field_handle,
            [space_handle, 40, 45, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::FieldSplit, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "FieldSplit must succeed");

        // Get the new Field id from the cap table.
        let slot1 = crate::frame::capabilities::entry_ref(entries, 16, 1).unwrap();
        let (_, new_field_id) = slot1.object.unwrap();
        // IRQ routes in [40,45] must now point to the new Field.
        let irq_routes = ks.irq_routes.acquire();

        for intid in 40..=45u32 {
            let route = irq_routes.lookup(intid).unwrap();

            assert_eq!(
                route.field_id, new_field_id,
                "D99: IRQ route for INTID {intid} must point to new sub-Field"
            );
        }
        // IRQ routes outside [40,45] must still point to source Field.
        for intid in 32..40u32 {
            let route = irq_routes.lookup(intid).unwrap();

            assert_eq!(
                route.field_id, source_field_id,
                "D99: IRQ route for INTID {intid} must remain at source Field"
            );
        }
        for intid in 46..48u32 {
            let route = irq_routes.lookup(intid).unwrap();

            assert_eq!(
                route.field_id, source_field_id,
                "D99: IRQ route for INTID {intid} must remain at source Field"
            );
        }
    }

    /// D99: FieldSplit on wrong target type returns WrongType.
    #[test]
    fn test_d99_field_split_wrong_type() {
        let ks = make_kernel_state();
        let space_id = make_space_in_arena(&ks, 0x1000, 4096);
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Space, space_id, Rights::SPACE_ALL, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::FieldSplit as u16,
            handle,
            [0, 10, 20, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::FieldSplit, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert!(x0 < 0, "D99: FieldSplit on non-Field must return error");
    }

    /// D99: FieldSplit with inverted badge range returns error.
    #[test]
    fn test_d99_field_split_inverted_range() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::FIELD_ALL, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        // badge_low (200) > badge_high (100).
        setup_typed_regs(
            sender_ptr,
            TypedOperation::FieldSplit as u16,
            handle,
            [0, 200, 100, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::FieldSplit, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert!(x0 < 0, "D99: inverted badge range must return error");
    }

    /// D99: FieldSplit without SPLIT right returns NoRight.
    #[test]
    fn test_d99_field_split_no_split_right() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        // Give only SEND right, not SPLIT.
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::SEND, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::FieldSplit as u16,
            handle,
            [0, 10, 20, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::FieldSplit, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert!(x0 < 0, "D99: FieldSplit without SPLIT right must fail");
    }

    /// D99: FieldSplit with Space too small for one Message returns error.
    #[test]
    fn test_d99_field_split_insufficient_space() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let space_id = make_space_in_arena(&ks, 0x1000, 1);
        let (mut sender, entries) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::FIELD_ALL, Badge(0), 0);
        let space_entry = crate::frame::capabilities::entry_mut(entries, 16, 1).unwrap();

        *space_entry = crate::capability::Entry {
            object: Some((ObjectType::Space, space_id)),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let sender_ptr = NonNull::from(&mut sender);
        let field_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let space_handle = crate::capability::Handle {
            index: 1,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        setup_typed_regs(
            sender_ptr,
            TypedOperation::FieldSplit as u16,
            field_handle,
            [space_handle, 10, 20, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::FieldSplit, &ks);

        let x0 = read_typed_result(sender_ptr) as i64;

        assert!(x0 < 0, "D99: Space too small for one Message must fail");
    }

    /// D99: FieldSplit with exact-match badge range (low == high).
    #[test]
    fn test_d99_field_split_exact_badge_range() {
        let ks = make_kernel_state();
        let source_field_id = make_field_in_arena(&ks, 16);
        let space_id = make_space_in_arena(&ks, 0x4000, 4 * 4096);
        let (mut sender, entries) = make_sender_with_cap(
            ObjectType::Field,
            source_field_id,
            Rights::FIELD_ALL,
            Badge(0),
            0,
        );
        let space_entry = crate::frame::capabilities::entry_mut(entries, 16, 1).unwrap();

        *space_entry = crate::capability::Entry {
            object: Some((ObjectType::Space, space_id)),
            rights: Rights::SPACE_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let sender_ptr = NonNull::from(&mut sender);
        let field_handle = crate::capability::Handle {
            index: 0,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();
        let space_handle = crate::capability::Handle {
            index: 1,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode();

        // Exact-match: badge_low == badge_high == 42.
        setup_typed_regs(
            sender_ptr,
            TypedOperation::FieldSplit as u16,
            field_handle,
            [space_handle, 42, 42, 0],
        );

        let mut core = make_core_state();

        core.current = Some(sender_ptr);

        core.scheduler.enqueue(sender_ptr);
        core.dispatch_typed(TypedOperation::FieldSplit, &ks);

        let x0 = read_typed_result(sender_ptr);

        assert_eq!(x0, 0, "D99: exact-match split must succeed");

        // Source Field must route badge 42 to the new sub-Field.
        let slot1 = crate::frame::capabilities::entry_ref(entries, 16, 1).unwrap();
        let (_, new_field_id) = slot1.object.unwrap();
        let fields = ks.fields.acquire();
        let source = fields.get(source_field_id).unwrap();

        assert_eq!(source.resolve_route(42), Some(new_field_id));
        assert!(source.resolve_route(41).is_none());
        assert!(source.resolve_route(43).is_none());
    }

    // ── D100 fault delivery mechanics ─────────────────────────────────

    /// Helper: create an Observer with registers AND a cap table.
    /// Optionally installs a handler cap at slot 0 pointing to the given Field.
    fn make_observer_with_handler(handler_field_id: Option<(ObjectId, u64)>) -> Observer {
        let mut obs = crate::observer::Observer::test_with_cap_table(8);

        obs.object_id = ObjectId(42);

        if let Some((field_id, field_gen)) = handler_field_id {
            let handler_entry = crate::capability::Entry {
                object: Some((ObjectType::Field, field_id)),
                rights: Rights::SEND,
                badge: Badge(0xFA17),
                slot_tag: crate::capability::SlotTag(0),
                send_once: false,
                stored_generation: field_gen,
            };
            let ptr = NonNull::from(&mut obs);

            crate::frame::cores::observer_write_cap_at(
                ptr,
                crate::capability::SLOT_FAULT_HANDLER,
                handler_entry,
            );
        }

        obs
    }

    /// D100: dispatch_fault with no handler cap → FatalFault.
    #[test]
    fn test_d100_dispatch_fault_no_handler_returns_fatal() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        let mut obs = make_observer_with_handler(None);
        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        core.scheduler.enqueue(ptr);

        let fault = crate::fault::FaultType::HardwareException {
            esr_el1: 0x8200_0000,
            elr_el1: 0x4000,
            far_el1: 0xDEAD,
        };
        let result = core.dispatch_fault(fault, &ks);

        assert!(
            matches!(result, DispatchResult::FatalFault),
            "D100: no handler must produce FatalFault"
        );
    }

    /// D100: dispatch_fault with valid handler → Enqueued, Observer Faulted.
    #[test]
    fn test_d100_dispatch_fault_valid_handler_enqueues() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let mut core = make_core_state();
        let mut obs = make_observer_with_handler(Some((field_id, 0)));
        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        let fault = crate::fault::FaultType::VmFault {
            space_slot: 1,
            byte_offset: 0x1000,
            access: crate::fault::AccessType::Read,
        };
        let result = core.dispatch_fault(fault, &ks);

        // Observer transitions to Faulted.
        assert!(
            matches!(obs.state, crate::observer::PrimaryState::Faulted),
            "D100/D39: Observer must be Faulted after dispatch_fault"
        );

        // Fault message enqueued in handler Field.
        let fields = ks.fields.acquire();
        let handler = fields.get(field_id).unwrap();

        assert_eq!(
            handler.queue_length, 1,
            "D100: fault message must be enqueued"
        );
        // Result must be Idle or Resume (schedule_next with empty run queue).
        assert!(
            matches!(result, DispatchResult::Idle),
            "D100: schedule_next with empty queue must return Idle"
        );
    }

    /// D100: enqueued fault message carries correct label and data words.
    #[test]
    fn test_d100_fault_message_has_correct_content() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let mut core = make_core_state();
        let mut obs = make_observer_with_handler(Some((field_id, 0)));
        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        let fault = crate::fault::FaultType::HardwareException {
            esr_el1: 0x8200_0000,
            elr_el1: 0xFFFF_0000_0040_0000,
            far_el1: 0xDEAD_BEEF,
        };

        core.dispatch_fault(fault, &ks);

        let mut fields = ks.fields.acquire();
        let handler = fields.get_mut(field_id).unwrap();
        let msg = handler.dequeue().unwrap();

        assert_eq!(msg.label, crate::field::LABEL_HARDWARE_EXCEPTION);
        assert_eq!(msg.data[0], 0x8200_0000, "D100: data[0] = ESR_EL1");
        assert_eq!(
            msg.data[1], 0xFFFF_0000_0040_0000,
            "D100: data[1] = ELR_EL1"
        );
        assert_eq!(msg.data[2], 0xDEAD_BEEF, "D100: data[2] = FAR_EL1");
        assert_eq!(msg.data[3], 0, "D100: data[3] = 0");
        assert_eq!(msg.badge, Badge(0xFA17), "D100: badge from handler cap");
        assert!(msg.reply_cap.is_none(), "D100: no reply cap on fault");
    }

    /// D100: fault message carries Observer cap with FAULT_OBSERVER rights.
    #[test]
    fn test_d100_fault_message_carries_observer_cap() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let mut core = make_core_state();
        let mut obs = make_observer_with_handler(Some((field_id, 0)));
        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        let fault = crate::fault::FaultType::CapTableFull;

        core.dispatch_fault(fault, &ks);

        let mut fields = ks.fields.acquire();
        let handler = fields.get_mut(field_id).unwrap();
        let msg = handler.dequeue().unwrap();
        let cap = msg
            .user_cap
            .expect("D100: fault message must carry Observer cap");

        assert_eq!(cap.object_type, ObjectType::Observer);
        assert_eq!(cap.object_id, ObjectId(42), "D100: cap ID matches Observer");
        assert_eq!(cap.rights, Rights::FAULT_OBSERVER, "D100: exactly 5 rights");
        assert_eq!(cap.stored_generation, 0);
    }

    /// D100: stale handler cap (generation mismatch) → FatalFault.
    #[test]
    fn test_d100_stale_handler_cap_returns_fatal() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);

        {
            let mut fields = ks.fields.acquire();

            fields
                .get_mut(field_id)
                .unwrap()
                .generation
                .store(1, core::sync::atomic::Ordering::Release);
        }

        let mut core = make_core_state();
        // handler_field_gen=0 but live gen=1 → stale.
        let mut obs = make_observer_with_handler(Some((field_id, 0)));
        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        core.scheduler.enqueue(ptr);

        let fault = crate::fault::FaultType::CapTableFull;
        let result = core.dispatch_fault(fault, &ks);

        assert!(
            matches!(result, DispatchResult::FatalFault),
            "D100: stale handler must produce FatalFault"
        );
    }

    /// D100: handler Field queue full → Deferred → schedule_next.
    #[test]
    fn test_d100_full_handler_queue_defers() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 1);

        {
            let mut fields = ks.fields.acquire();

            fields
                .get_mut(field_id)
                .unwrap()
                .enqueue(crate::field::Message::timer_fire(Badge(0), 0, 0))
                .unwrap();
        }

        let mut core = make_core_state();
        let mut obs = make_observer_with_handler(Some((field_id, 0)));
        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        let fault = crate::fault::FaultType::ResourceRequest {
            resource: crate::fault::ResourceType::Space,
            quantity: 4,
        };
        let result = core.dispatch_fault(fault, &ks);

        assert!(
            matches!(obs.state, crate::observer::PrimaryState::Faulted),
            "D100: Observer must be Faulted even on deferred delivery"
        );
        assert!(
            matches!(result, DispatchResult::Idle),
            "D100: deferred delivery returns schedule_next result"
        );
    }

    /// D100: all four fault types deliver through dispatch_fault.
    #[test]
    fn test_d100_all_fault_types_dispatch() {
        let faults: [crate::fault::FaultType; 4] = [
            crate::fault::FaultType::VmFault {
                space_slot: 0,
                byte_offset: 0,
                access: crate::fault::AccessType::Read,
            },
            crate::fault::FaultType::ResourceRequest {
                resource: crate::fault::ResourceType::Space,
                quantity: 1,
            },
            crate::fault::FaultType::CapTableFull,
            crate::fault::FaultType::HardwareException {
                esr_el1: 0,
                elr_el1: 0,
                far_el1: 0,
            },
        ];

        for fault in faults {
            let ks = make_kernel_state();
            let field_id = make_field_in_arena(&ks, 8);
            let mut core = make_core_state();
            let mut obs = make_observer_with_handler(Some((field_id, 0)));
            let ptr = NonNull::from(&mut obs);

            core.current = Some(ptr);

            core.dispatch_fault(fault, &ks);

            let fields = ks.fields.acquire();

            assert_eq!(
                fields.get(field_id).unwrap().queue_length,
                1,
                "D100: fault message must be enqueued for all types"
            );
            assert!(matches!(obs.state, crate::observer::PrimaryState::Faulted));
        }
    }

    /// D100: Observer's object_id is used in the fault cap (not hardcoded).
    #[test]
    fn test_d100_fault_cap_uses_observer_object_id() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let mut core = make_core_state();
        let mut obs = make_observer_with_handler(Some((field_id, 0)));

        // Set a distinctive ObjectId.
        obs.object_id = ObjectId(99);

        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        core.dispatch_fault(crate::fault::FaultType::CapTableFull, &ks);

        let mut fields = ks.fields.acquire();
        let handler = fields.get_mut(field_id).unwrap();
        let msg = handler.dequeue().unwrap();
        let cap = msg.user_cap.unwrap();

        assert_eq!(
            cap.object_id,
            ObjectId(99),
            "D100: fault cap must carry the Observer's arena ObjectId"
        );
    }

    /// D100: fault cap generation matches Observer's current generation.
    #[test]
    fn test_d100_fault_cap_uses_observer_generation() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let mut core = make_core_state();
        let mut obs = make_observer_with_handler(Some((field_id, 0)));

        obs.generation = core::sync::atomic::AtomicU64::new(7);

        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        core.dispatch_fault(crate::fault::FaultType::CapTableFull, &ks);

        let mut fields = ks.fields.acquire();
        let handler = fields.get_mut(field_id).unwrap();
        let msg = handler.dequeue().unwrap();
        let cap = msg.user_cap.unwrap();

        assert_eq!(
            cap.stored_generation, 7,
            "D100: fault cap generation must match Observer's current generation"
        );
    }

    /// D100: dispatch_fault with no current Observer returns Idle.
    #[test]
    fn test_d100_dispatch_fault_no_current_returns_idle() {
        let ks = make_kernel_state();
        let mut core = make_core_state();

        core.current = None;

        let fault = crate::fault::FaultType::CapTableFull;
        let result = core.dispatch_fault(fault, &ks);

        assert!(
            matches!(result, DispatchResult::Idle),
            "D100: no current Observer must return Idle"
        );
    }

    /// D100: VmFault data word layout matches D61 table.
    #[test]
    fn test_d100_vm_fault_data_layout_in_dispatch() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let mut core = make_core_state();
        let mut obs = make_observer_with_handler(Some((field_id, 0)));
        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        let fault = crate::fault::FaultType::VmFault {
            space_slot: 3,
            byte_offset: 0x4000,
            access: crate::fault::AccessType::Execute,
        };

        core.dispatch_fault(fault, &ks);

        let mut fields = ks.fields.acquire();
        let handler = fields.get_mut(field_id).unwrap();
        let msg = handler.dequeue().unwrap();

        assert_eq!(msg.label, crate::field::LABEL_VM_FAULT);
        assert_eq!(msg.data[0], 3, "D100: data[0] = space slot");
        assert_eq!(msg.data[1], 0x4000, "D100: data[1] = byte offset");
        assert_eq!(msg.data[2], 2, "D100: data[2] = Execute");
        assert_eq!(msg.data[3], 0, "D100: data[3] = 0");
    }

    // ══════════════════════════════════════════════════════════════════
    // Phase 2: Multi-step integration tests
    // ══════════════════════════════════════════════════════════════════

    // ── Test scenario builder ─────────────────────────────────────────

    struct TestScenario {
        kernel_state: KernelState,
        core: CoreState<RoundRobin>,
    }

    impl TestScenario {
        fn new() -> Self {
            TestScenario {
                kernel_state: make_kernel_state(),
                core: make_core_state(),
            }
        }

        fn create_observer_with_send_cap(
            &self,
            field_id: ObjectId,
            badge: u64,
        ) -> (Observer, NonNull<crate::capability::Entry>) {
            make_sender_with_cap(ObjectType::Field, field_id, Rights::SEND, Badge(badge), 0)
        }

        fn create_observer_with_recv_cap(
            &self,
            field_id: ObjectId,
        ) -> (Observer, NonNull<crate::capability::Entry>) {
            make_sender_with_cap(ObjectType::Field, field_id, Rights::RECEIVE, Badge(0), 0)
        }

        fn create_field(&self, capacity: u32) -> ObjectId {
            make_field_in_arena(&self.kernel_state, capacity)
        }

        fn create_space(&self, va_base: usize, size: usize) -> ObjectId {
            make_space_in_arena(&self.kernel_state, va_base, size)
        }

        fn create_observer_in_arena(&self) -> ObjectId {
            let rs_ptr = crate::frame::cores::alloc_test_register_state();
            let entries = crate::frame::capabilities::alloc_test_entries(16);

            crate::frame::capabilities::init_freelist(
                entries,
                16,
                crate::capability::SLOT_USER_START,
            );

            let mut observers = self.kernel_state.observers.acquire();
            let (id, obs) = observers.allocate().expect("allocate observer");

            obs.object_id = id;
            obs.asid = 0;
            obs.register_state = crate::observer::RegisterStateHandle::new(rs_ptr);
            obs.page_table_root = 0;
            obs.cap_table = entries;
            obs.cap_table_capacity = 16;
            obs.cap_table_free_head = Some(crate::capability::SLOT_USER_START);
            obs.cap_table_count = 0;
            obs.state = crate::observer::PrimaryState::Inert;
            obs.suspended = false;
            obs.compute_aggregate = 100;
            obs.responsiveness = crate::observer::DEFAULT_RESPONSIVENESS;
            obs.throughput = crate::observer::DEFAULT_THROUGHPUT;
            obs.clock_access = false;
            obs.wait_state = crate::observer::WaitState::None;
            obs.backing_va_base = 0;
            obs.backing_size = 0;
            obs.refcount = 1;
            obs.generation = core::sync::atomic::AtomicU64::new(0);

            id
        }

        fn dispatch_typed(
            &mut self,
            sender_ptr: NonNull<Observer>,
            op_code: u16,
            target_handle: u64,
            args: [u64; 4],
        ) -> DispatchResult {
            setup_typed_regs(sender_ptr, op_code, target_handle, args);

            self.core.current = Some(sender_ptr);

            let operation = crate::syscall::TypedOperation::from_code(op_code)
                .expect("valid op_code in dispatch_typed");

            self.core.dispatch_typed(operation, &self.kernel_state)
        }
    }

    fn encode_handle(index: u32) -> u64 {
        crate::capability::Handle {
            index,
            slot_tag: crate::capability::SlotTag(0),
        }
        .encode()
    }

    // ── Scenario 1: Create Field + Send/Receive between two Observers ─

    #[test]
    fn integration_create_field_send_receive() {
        let mut scenario = TestScenario::new();
        let field_id = scenario.create_field(8);
        // Sender with SEND cap.
        let (mut sender, _) = scenario.create_observer_with_send_cap(field_id, 0xBEEF);
        let sender_ptr = NonNull::from(&mut sender);
        // Receiver with RECEIVE cap.
        let (mut receiver, _) = scenario.create_observer_with_recv_cap(field_id);
        let receiver_ptr = NonNull::from(&mut receiver);
        // Send a message.
        let handle = encode_handle(0);

        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0xA, 0xB, 0xC, 0xD],
                label: 0x42,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        scenario.core.current = Some(sender_ptr);

        let result = scenario
            .core
            .dispatch_ipc(IpcOperation::Send, &scenario.kernel_state);

        assert!(matches!(result, DispatchResult::Resume(_)));

        // Receive the message.
        crate::frame::cores::write_test_ipc_registers_via_observer(
            receiver_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        scenario.core.current = Some(receiver_ptr);

        let result = scenario
            .core
            .dispatch_ipc(IpcOperation::Receive, &scenario.kernel_state);

        assert!(matches!(result, DispatchResult::Resume(_)));

        // Verify the receiver got the message.
        let regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_eq!(regs.data[0], 0xA);
        assert_eq!(regs.data[1], 0xB);
        assert_eq!(regs.data[2], 0xC);
        assert_eq!(regs.data[3], 0xD);
    }

    // ── Scenario 2: Fault delivery chain ──────────────────────────────

    #[test]
    fn integration_fault_delivery_to_handler() {
        let mut scenario = TestScenario::new();
        let handler_field_id = scenario.create_field(8);
        // Create a sender Observer with handler cap at slot 0 + self-cap.
        let rs_ptr = crate::frame::cores::alloc_test_register_state();
        let entries = crate::frame::capabilities::alloc_test_entries(16);

        crate::frame::capabilities::init_freelist(entries, 16, crate::capability::SLOT_USER_START);

        // Install handler cap at slot 0.
        let handler_entry = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        *handler_entry = crate::capability::Entry {
            object: Some((ObjectType::Field, handler_field_id)),
            rights: Rights::SEND,
            badge: Badge(0xFA01),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let mut faulting = Observer {
            object_id: ObjectId(99),
            asid: 0,
            asid_generation: 0,
            register_state: crate::observer::RegisterStateHandle::new(rs_ptr),
            page_table_root: 0,
            cap_table: entries,
            cap_table_capacity: 16,
            cap_table_free_head: Some(crate::capability::SLOT_USER_START),
            cap_table_count: 1,
            state: crate::observer::PrimaryState::Runnable,
            suspended: false,
            compute_aggregate: 100,
            responsiveness: crate::observer::DEFAULT_RESPONSIVENESS,
            throughput: crate::observer::DEFAULT_THROUGHPUT,
            clock_access: false,
            wait_state: crate::observer::WaitState::None,
            saved_syscall: crate::observer::SavedSyscallContext::None,
            backing_va_base: 0,
            backing_size: 0,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let faulting_ptr = NonNull::from(&mut faulting);

        scenario.core.current = Some(faulting_ptr);

        // Dispatch a fault.
        let fault = crate::fault::FaultType::HardwareException {
            esr_el1: 0xDEAD,
            elr_el1: 0xBEEF,
            far_el1: 0xCAFE,
        };
        let result = scenario.core.dispatch_fault(fault, &scenario.kernel_state);

        // Fault delivery: observer is faulted, not resumed. Scheduler picks next.
        assert!(
            matches!(result, DispatchResult::Idle),
            "faulted Observer must not be resumed"
        );
        assert!(matches!(
            faulting.state,
            crate::observer::PrimaryState::Faulted
        ));

        // Verify fault message in handler Field.
        let mut fields = scenario.kernel_state.fields.acquire();
        let handler = fields.get_mut(handler_field_id).unwrap();
        let msg = handler.dequeue().unwrap();

        assert_eq!(msg.label, crate::field::LABEL_HARDWARE_EXCEPTION);
        assert_eq!(msg.data[0], 0xDEAD, "ESR_EL1");
        assert_eq!(msg.data[1], 0xBEEF, "ELR_EL1");
        assert_eq!(msg.data[2], 0xCAFE, "FAR_EL1");
    }

    // ── Scenario 3: SpaceSplit + use new Space ────────────────────────

    #[test]
    fn integration_space_split_creates_usable_space() {
        let mut scenario = TestScenario::new();
        let space_id = scenario.create_space(0x10000, 8192);
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Space, space_id, Rights::SPLIT, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        // Split 4096 bytes from the Space.
        let result = scenario.dispatch_typed(
            sender_ptr,
            TypedOperation::SpaceSplit as u16,
            encode_handle(0),
            [4096, 0, 0, 0],
        );

        assert!(matches!(result, DispatchResult::Resume(_)));

        // Verify the result is a valid handle (non-negative = success).
        let result_val = read_typed_result(sender_ptr);

        assert!(
            (result_val as i64) >= 0,
            "SpaceSplit must return a valid handle, got {result_val:#x}"
        );

        // Verify original Space was shrunk.
        let spaces = scenario.kernel_state.spaces.acquire();
        let original = spaces.get(space_id).unwrap();

        assert_eq!(
            original.size, 4096,
            "original Space must shrink by split amount"
        );
    }

    // ── Scenario 4: TimeSplit + verify conservation ───────────────────

    #[test]
    fn integration_time_split_conserves_units() {
        let mut scenario = TestScenario::new();
        let time_id = {
            let mut times = scenario.kernel_state.times.acquire();
            let (id, time) = times.allocate().expect("allocate time");

            time.compute_units = 100;
            time.refcount = 1;
            time.generation = core::sync::atomic::AtomicU64::new(0);

            id
        };
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Time, time_id, Rights::SPLIT, Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let result = scenario.dispatch_typed(
            sender_ptr,
            TypedOperation::TimeSplit as u16,
            encode_handle(0),
            [30, 0, 0, 0],
        );

        assert!(matches!(result, DispatchResult::Resume(_)));

        let result_val = read_typed_result(sender_ptr);

        assert!(
            (result_val as i64) >= 0,
            "TimeSplit must return valid handle"
        );

        let times = scenario.kernel_state.times.acquire();
        let original = times.get(time_id).unwrap();

        assert_eq!(
            original.compute_units, 70,
            "original Time must have 70 units remaining"
        );
    }

    // ── Scenario 5: Timer + Pulsar lifecycle ──────────────────────────

    #[test]
    fn integration_pulsar_fire_and_rearm() {
        let mut scenario = TestScenario::new();
        let delivery_field_id = scenario.create_field(8);
        let space_id = scenario.create_space(0x100000, 4096);
        // Create sender with Space cap at slot 0, Field cap at slot 3.
        let rs_ptr = crate::frame::cores::alloc_test_register_state();
        let entries = crate::frame::capabilities::alloc_test_entries(16);

        crate::frame::capabilities::init_freelist(entries, 16, crate::capability::SLOT_USER_START);

        // Slot 0: Space cap for backing.
        let e = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        *e = crate::capability::Entry {
            object: Some((ObjectType::Space, space_id)),
            rights: Rights::SPLIT,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        // Slot 3: Field cap for delivery.
        let e = crate::frame::capabilities::entry_mut(entries, 16, 3).unwrap();

        *e = crate::capability::Entry {
            object: Some((ObjectType::Field, delivery_field_id)),
            rights: Rights::SEND,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let mut sender = Observer {
            object_id: ObjectId(0),
            asid: 0,
            asid_generation: 0,
            register_state: crate::observer::RegisterStateHandle::new(rs_ptr),
            page_table_root: 0,
            cap_table: entries,
            cap_table_capacity: 16,
            cap_table_free_head: Some(crate::capability::SLOT_USER_START),
            cap_table_count: 2,
            state: crate::observer::PrimaryState::Runnable,
            suspended: false,
            compute_aggregate: 100,
            responsiveness: crate::observer::DEFAULT_RESPONSIVENESS,
            throughput: crate::observer::DEFAULT_THROUGHPUT,
            clock_access: false,
            wait_state: crate::observer::WaitState::None,
            saved_syscall: crate::observer::SavedSyscallContext::None,
            backing_va_base: 0,
            backing_size: 0,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let sender_ptr = NonNull::from(&mut sender);
        // CreatePulsar: Space at handle 0, Field handle at slot 3.
        let field_handle = encode_handle(3);

        // duration=1ms, period=1ms (repeating), badge=0x77.
        setup_typed_regs(
            sender_ptr,
            TypedOperation::CreatePulsar as u16,
            encode_handle(0),
            [field_handle, 0x77, 1_000_000, 1_000_000],
        );

        scenario.core.current = Some(sender_ptr);

        let result = scenario.core.dispatch_typed(
            crate::syscall::TypedOperation::CreatePulsar,
            &scenario.kernel_state,
        );

        assert!(matches!(result, DispatchResult::Resume(_)));
        assert_eq!(
            scenario.core.deadline_count, 1,
            "Pulsar must register a deadline"
        );

        // Advance time past the deadline.
        let deadline = scenario.core.deadlines[0].unwrap().deadline_ticks;

        scenario
            .core
            .handle_timer(deadline + 1, &scenario.kernel_state, TEST_COUNTER_FREQ);

        // Verify fire message in delivery Field.
        let mut fields = scenario.kernel_state.fields.acquire();
        let field = fields.get_mut(delivery_field_id).unwrap();
        let msg = field.dequeue().unwrap();

        assert_eq!(msg.badge.0, 0x77, "fire message must carry Pulsar badge");
        // Repeating Pulsar should still have a deadline.
        assert_eq!(
            scenario.core.deadline_count, 1,
            "repeating Pulsar must rearm"
        );
    }

    // ── Scenario 6: ClockRead returns non-zero + sets clock_access ────

    #[test]
    fn integration_clock_read_sets_access_and_returns_ticks() {
        let mut scenario = TestScenario::new();
        let space_id = scenario.create_space(0x200000, 4096);
        let (mut sender, _entries) =
            make_sender_with_cap(ObjectType::Space, space_id, Rights::empty(), Badge(0), 0);
        let sender_ptr = NonNull::from(&mut sender);

        assert!(!sender.clock_access, "clock_access must start false");

        let result = scenario.dispatch_typed(
            sender_ptr,
            TypedOperation::ClockRead as u16,
            encode_handle(0),
            [0; 4],
        );

        assert!(matches!(result, DispatchResult::Resume(_)));
        assert!(sender.clock_access, "ClockRead must set clock_access");
    }

    // ── Scenario 7: WriteRegisters + ReadRegisters roundtrip ──────────

    #[test]
    fn integration_write_read_registers_roundtrip() {
        let mut scenario = TestScenario::new();
        let target_id = scenario.create_observer_in_arena();
        let (mut sender, _entries) = make_sender_with_cap(
            ObjectType::Observer,
            target_id,
            Rights::WRITE_REGISTERS.union(Rights::READ_REGISTERS),
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        // WriteRegisters: PC=0x4000, SP=0x8000, x0=42, PSTATE=0xF000_0000.
        let result = scenario.dispatch_typed(
            sender_ptr,
            TypedOperation::ObserverWriteRegisters as u16,
            encode_handle(0),
            [0x4000, 0x8000, 42, 0xF000_0000],
        );

        assert!(matches!(result, DispatchResult::Resume(_)));

        let result_val = read_typed_result(sender_ptr);

        assert_eq!(result_val, 0, "WriteRegisters success returns 0");

        // ReadRegisters on the same target.
        let result = scenario.dispatch_typed(
            sender_ptr,
            TypedOperation::ObserverReadRegisters as u16,
            encode_handle(0),
            [0; 4],
        );

        assert!(matches!(result, DispatchResult::Resume(_)));

        // x0 = PC, x1 = SP, x2 = target's x0, x3 = PSTATE.
        let regs = crate::frame::cores::read_typed_registers(sender_ptr);

        assert_eq!(regs.args[0], 0x4000, "PC must match written value");
        assert_eq!(regs.args[1], 0x8000, "SP must match written value");
        assert_eq!(regs.args[2], 42, "x0 must match written value");
        assert_eq!(regs.args[3], 0xF000_0000, "PSTATE must match written NZCV");
    }

    // ── Scenario 7b: PSTATE masking security test ─────────────────────

    #[test]
    fn integration_write_registers_masks_pstate() {
        let mut scenario = TestScenario::new();
        let target_id = scenario.create_observer_in_arena();
        let (mut sender, _entries) = make_sender_with_cap(
            ObjectType::Observer,
            target_id,
            Rights::WRITE_REGISTERS.union(Rights::READ_REGISTERS),
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        // Write PSTATE with M bits set (would escalate to EL1).
        let result = scenario.dispatch_typed(
            sender_ptr,
            TypedOperation::ObserverWriteRegisters as u16,
            encode_handle(0),
            [0x1000, 0x2000, 0, 0xF000_0005],
        );

        assert!(matches!(result, DispatchResult::Resume(_)));

        // Read back — M bits must be stripped.
        let result = scenario.dispatch_typed(
            sender_ptr,
            TypedOperation::ObserverReadRegisters as u16,
            encode_handle(0),
            [0; 4],
        );

        assert!(matches!(result, DispatchResult::Resume(_)));

        let regs = crate::frame::cores::read_typed_registers(sender_ptr);

        assert_eq!(
            regs.args[3], 0xF000_0000,
            "M bits must be masked — only NZCV preserved"
        );
    }

    // ── Scenario 8: WriteRegisters rejected for Runnable target ───────

    #[test]
    fn integration_write_registers_rejects_runnable() {
        let mut scenario = TestScenario::new();
        let target_id = scenario.create_observer_in_arena();

        // Make the target Runnable.
        {
            let mut observers = scenario.kernel_state.observers.acquire();

            observers.get_mut(target_id).unwrap().state = crate::observer::PrimaryState::Runnable;
        }

        let (mut sender, _entries) = make_sender_with_cap(
            ObjectType::Observer,
            target_id,
            Rights::WRITE_REGISTERS,
            Badge(0),
            0,
        );
        let sender_ptr = NonNull::from(&mut sender);
        let result = scenario.dispatch_typed(
            sender_ptr,
            TypedOperation::ObserverWriteRegisters as u16,
            encode_handle(0),
            [0x4000, 0x8000, 0, 0],
        );

        assert!(matches!(result, DispatchResult::Resume(_)));

        let result_val = read_typed_result(sender_ptr) as i64;

        assert!(
            result_val < 0,
            "WriteRegisters on Runnable target must fail"
        );
    }

    // ── Scenario 9: ResourceRequest non-root fault routes to handler ──

    #[test]
    fn integration_resource_request_non_root_delivers_fault() {
        let mut scenario = TestScenario::new();
        let handler_field_id = scenario.create_field(8);
        let rs_ptr = crate::frame::cores::alloc_test_register_state();
        let entries = crate::frame::capabilities::alloc_test_entries(16);

        crate::frame::capabilities::init_freelist(entries, 16, crate::capability::SLOT_USER_START);

        // Handler cap at slot 0.
        let e = crate::frame::capabilities::entry_mut(entries, 16, 0).unwrap();

        *e = crate::capability::Entry {
            object: Some((ObjectType::Field, handler_field_id)),
            rights: Rights::SEND,
            badge: Badge(0xABCD),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        // Self-cap at slot 2.
        let e = crate::frame::capabilities::entry_mut(entries, 16, 2).unwrap();

        *e = crate::capability::Entry {
            object: Some((ObjectType::Observer, ObjectId(0))),
            rights: Rights::OBSERVER_ALL,
            badge: Badge(0),
            slot_tag: crate::capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };

        let mut requester = Observer {
            object_id: ObjectId(0),
            asid: 0,
            asid_generation: 0,
            register_state: crate::observer::RegisterStateHandle::new(rs_ptr),
            page_table_root: 0,
            cap_table: entries,
            cap_table_capacity: 16,
            cap_table_free_head: Some(crate::capability::SLOT_USER_START),
            cap_table_count: 2,
            state: crate::observer::PrimaryState::Runnable,
            suspended: false,
            compute_aggregate: 100,
            responsiveness: crate::observer::DEFAULT_RESPONSIVENESS,
            throughput: crate::observer::DEFAULT_THROUGHPUT,
            clock_access: false,
            wait_state: crate::observer::WaitState::None,
            saved_syscall: crate::observer::SavedSyscallContext::None,
            backing_va_base: 0,
            backing_size: 0,
            refcount: 1,
            generation: core::sync::atomic::AtomicU64::new(0),
        };
        let requester_ptr = NonNull::from(&mut requester);

        scenario.core.current = Some(requester_ptr);

        // ResourceRequest: Space (0), quantity 4.
        setup_typed_regs(
            requester_ptr,
            TypedOperation::ResourceRequest as u16,
            encode_handle(2),
            [0, 4, 0, 0],
        );

        let result = scenario
            .core
            .dispatch_typed(TypedOperation::ResourceRequest, &scenario.kernel_state);

        // Non-root: fault-routed → Observer becomes Faulted.
        assert!(
            matches!(result, DispatchResult::Idle),
            "non-root ResourceRequest should fault-route, not resume"
        );
        assert!(matches!(
            requester.state,
            crate::observer::PrimaryState::Faulted
        ));

        // Verify fault message in handler Field.
        let mut fields = scenario.kernel_state.fields.acquire();
        let handler = fields.get_mut(handler_field_id).unwrap();
        let msg = handler.dequeue().unwrap();

        assert_eq!(msg.label, crate::field::LABEL_RESOURCE_REQUEST);
        assert_eq!(msg.data[0], 0, "resource type = Space");
        assert_eq!(msg.data[1], 4, "quantity = 4");
    }

    // ── Scenario 10: Nested IPC — Call/Reply chain ────────────────────

    #[test]
    fn integration_call_blocks_caller() {
        let mut scenario = TestScenario::new();
        let field_id = scenario.create_field(8);
        let (mut caller, _) = make_sender_with_cap(
            ObjectType::Field,
            field_id,
            Rights::SEND.union(Rights::RECEIVE),
            Badge(0x1234),
            0,
        );
        let caller_ptr = NonNull::from(&mut caller);

        scenario.core.scheduler.enqueue(caller_ptr);

        let handle = encode_handle(0);

        crate::frame::cores::write_test_ipc_registers_via_observer(
            caller_ptr,
            &crate::syscall::IpcRegisters {
                data: [1, 2, 3, 4],
                label: 0x99,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        scenario.core.current = Some(caller_ptr);

        let result = scenario
            .core
            .dispatch_ipc(IpcOperation::Call, &scenario.kernel_state);

        // D16: Call always blocks the caller.
        assert!(
            matches!(result, DispatchResult::Idle),
            "Call must block caller when no receiver waiting"
        );
        assert!(matches!(
            caller.state,
            crate::observer::PrimaryState::Blocked
        ));
    }

    // ── D56 — Cross-core scheduling (IPI, placement, migration) ─────

    #[test]
    fn test_d56_handle_ipi_empty_mailbox_returns_schedule_next() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        // No IPI requests pending — handle_ipi should return Idle (empty queue).
        let result = core.handle_ipi(&ks);

        assert!(
            matches!(result, DispatchResult::Idle),
            "D56: handle_ipi with empty mailbox must return schedule_next (Idle when empty)"
        );
    }

    #[test]
    fn test_d56_handle_ipi_work_steal_schedules_next() {
        let ks = make_kernel_state();
        let mut core = make_core_state();

        // Push a WorkSteal request.
        ks.ipi_mailboxes.mailboxes[0].push(crate::kernel_state::IpiRequest::WorkSteal);

        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.scheduler.enqueue(ptr);

        let result = core.handle_ipi(&ks);

        match result {
            DispatchResult::Resume(resumed) | DispatchResult::ResumeFastPath(resumed) => {
                assert_eq!(
                    resumed, ptr,
                    "D56: after WorkSteal IPI, schedule_next must resume the runnable Observer"
                );
            }
            DispatchResult::Idle => {
                panic!("D56: must resume Observer after WorkSteal IPI");
            }
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    #[test]
    fn test_d56_handle_ipi_drains_multiple_requests() {
        let ks = make_kernel_state();
        let mut core = make_core_state();

        // Push multiple requests.
        ks.ipi_mailboxes.mailboxes[0].push(crate::kernel_state::IpiRequest::WorkSteal);
        ks.ipi_mailboxes.mailboxes[0].push(crate::kernel_state::IpiRequest::TlbInvalidation);
        ks.ipi_mailboxes.mailboxes[0].push(crate::kernel_state::IpiRequest::RoutingEntryCleanup);

        assert_eq!(ks.ipi_mailboxes.mailboxes[0].len(), 3);

        core.handle_ipi(&ks);

        // All requests must be drained.
        assert!(
            ks.ipi_mailboxes.mailboxes[0].is_empty(),
            "D56: handle_ipi must drain all pending requests"
        );
    }

    #[test]
    fn test_d56_handle_ipi_observer_migration() {
        let ks = make_kernel_state();
        let mut core = make_core_state();
        // Allocate an Observer in the global arena.
        let observer_id = {
            let mut observers = ks.observers.acquire();
            let (id, _obs) = observers.allocate().expect("allocate Observer");

            id
        };

        // Push a migration request for that Observer.
        ks.ipi_mailboxes.mailboxes[0].push(crate::kernel_state::IpiRequest::ObserverMigration(
            observer_id,
        ));

        let result = core.handle_ipi(&ks);

        // The Observer should now be in the local scheduler.
        match result {
            DispatchResult::Resume(_resumed) | DispatchResult::ResumeFastPath(_resumed) => {
                // Success: migrated Observer was enqueued and scheduled.
            }
            DispatchResult::Idle => {
                panic!("D56: must resume migrated Observer");
            }
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }
    }

    #[test]
    fn test_d56_handle_ipi_migration_nonexistent_observer_no_crash() {
        let ks = make_kernel_state();
        let mut core = make_core_state();

        // Push a migration request for a non-existent Observer.
        ks.ipi_mailboxes.mailboxes[0].push(crate::kernel_state::IpiRequest::ObserverMigration(
            crate::arena::ObjectId(999),
        ));

        // Must not panic — gracefully ignores non-existent Observer.
        let result = core.handle_ipi(&ks);

        assert!(
            matches!(result, DispatchResult::Idle),
            "D56: migration of non-existent Observer must not crash, returns Idle"
        );
    }

    #[test]
    fn test_d56_send_ipi_pushes_to_target_mailbox() {
        let ks = make_kernel_state();

        send_ipi(&ks, CoreId(1), crate::kernel_state::IpiRequest::WorkSteal);

        // Core 1's mailbox should have the request.
        assert_eq!(
            ks.ipi_mailboxes.mailboxes[1].pop(),
            Some(crate::kernel_state::IpiRequest::WorkSteal),
            "D56: send_ipi must push request to target core's mailbox"
        );
        // Core 0's mailbox should be empty.
        assert!(
            ks.ipi_mailboxes.mailboxes[0].is_empty(),
            "D56: send_ipi must not affect other cores' mailboxes"
        );
    }

    #[test]
    fn test_d56_build_core_snapshot_idle_core() {
        let core = make_core_state();
        let snapshot = core.build_core_snapshot();

        assert_eq!(snapshot.core_id, CoreId(0));
        assert!(
            snapshot.idle,
            "D56: core with no current and empty queue must be idle"
        );
        assert_eq!(snapshot.queue_depth, 0);
        assert_eq!(snapshot.capacity_factor, 100);
    }

    #[test]
    fn test_d56_build_core_snapshot_busy_core() {
        let mut core = make_core_state();
        let mut obs = make_observer();
        let ptr = NonNull::from(&mut obs);

        core.current = Some(ptr);

        core.scheduler.enqueue(ptr);

        let snapshot = core.build_core_snapshot();

        assert!(
            !snapshot.idle,
            "D56: core with current Observer must not be idle"
        );
        assert!(
            snapshot.queue_depth > 0,
            "D56: core with enqueued Observer must have non-zero queue depth"
        );
    }

    #[test]
    fn test_d56_placement_driven_migration() {
        use crate::time_manager::scored_placement::ScoredPlacement;
        use crate::time_manager::{CoreSnapshot, Placement, PlacementDecision};

        let placement = ScoredPlacement::new();
        let obs = make_observer();
        // Local core busy, remote core idle — placement should pick remote.
        let snapshots = [
            CoreSnapshot {
                core_id: CoreId(0),
                idle: false,
                queue_depth: 5,
                capacity_factor: 100,
            },
            CoreSnapshot {
                core_id: CoreId(1),
                idle: true,
                queue_depth: 0,
                capacity_factor: 100,
            },
        ];

        match placement.place(&obs, &snapshots) {
            PlacementDecision::Remote(target) => {
                assert_eq!(target, CoreId(1));

                // Simulate the migration: send IPI to target core.
                let ks = make_kernel_state();

                send_ipi(
                    &ks,
                    target,
                    crate::kernel_state::IpiRequest::ObserverMigration(crate::arena::ObjectId(0)),
                );

                // Verify the request landed in the target's mailbox.
                assert_eq!(
                    ks.ipi_mailboxes.mailboxes[1].len(),
                    1,
                    "D56: migration IPI must be in target core's mailbox"
                );
            }
            PlacementDecision::Local => {
                panic!("D56: idle remote must win over busy local");
            }
        }
    }

    #[test]
    fn test_d56_end_to_end_migration_flow() {
        // Full flow: placement decides Remote, send_ipi enqueues request,
        // target core's handle_ipi picks up the Observer.
        let ks = make_kernel_state();
        // Allocate an Observer in the arena.
        let observer_id = {
            let mut observers = ks.observers.acquire();
            let (id, _obs) = observers.allocate().expect("allocate");

            id
        };

        // 1. Source core decides to migrate (simulated placement result).
        send_ipi(
            &ks,
            CoreId(2),
            crate::kernel_state::IpiRequest::ObserverMigration(observer_id),
        );

        // 2. Target core (core 2) receives the IPI and drains its mailbox.
        let mut target_core = CoreState {
            core_id: CoreId(2),
            current: None,
            scheduler: RoundRobin::new(),
            deadlines: [None; MAX_DEADLINES_PER_CORE],
            deadline_count: 0,
            cascade_continuation: None,
        };
        let result = target_core.handle_ipi(&ks);

        // 3. The migrated Observer should be scheduled on the target core.
        match result {
            DispatchResult::Resume(_) | DispatchResult::ResumeFastPath(_) => {
                // Success: Observer migrated and scheduled.
            }
            DispatchResult::Idle => {
                panic!("D56: target core must schedule the migrated Observer");
            }
            DispatchResult::FatalFault => panic!("unexpected FatalFault"),
        }

        // 4. Mailbox must be empty after draining.
        assert!(
            ks.ipi_mailboxes.mailboxes[2].is_empty(),
            "D56: mailbox must be drained after handle_ipi"
        );
    }

    // ── Integration tests: multi-dispatch scenarios ─────────────────
    //
    // These tests exercise TWO dispatch calls where the second depends
    // on state from the first. This catches regressions in the wiring
    // between dispatch_ipc/dispatch_typed and register writeback that
    // unit tests (single dispatch call) miss.

    /// Integration: Receiver blocks, sender wakes it — verify message in registers.
    #[test]
    fn test_integration_receive_then_send_roundtrip() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let (mut receiver, _re) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::RECEIVE, Badge(0), 0);
        let receiver_ptr = NonNull::from(&mut receiver);
        let (mut sender, _se) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::SEND, Badge(0xBEEF), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let handle = encode_handle(0);

        // Step 1: Receiver does Receive on empty Field → blocks.
        crate::frame::cores::write_test_ipc_registers_via_observer(
            receiver_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(receiver_ptr);

        core.scheduler.enqueue(receiver_ptr);
        core.scheduler.enqueue(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Receive, &ks);

        assert!(matches!(result, DispatchResult::Resume(p) if p == sender_ptr));
        assert!(!core.scheduler.contains(receiver_ptr));

        // Step 2: Sender does Send → wakes receiver.
        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [0x1111, 0x2222, 0x3333, 0x4444],
                label: 0xABCD,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        core.current = Some(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Send, &ks);

        assert!(matches!(result, DispatchResult::Resume(p) if p == sender_ptr));

        // Receiver must have message in registers.
        let regs = crate::frame::cores::read_ipc_registers(receiver_ptr);

        assert_eq!(regs.data, [0x1111, 0x2222, 0x3333, 0x4444]);
        assert_eq!(regs.label, 0xABCD);
        assert_eq!(regs.handle_or_badge, 0xBEEF);

        let (carry, _) = crate::frame::cores::read_ipc_carry_and_x0(receiver_ptr);

        assert!(!carry, "receiver carry must be clear");
        assert!(
            core.scheduler.contains(receiver_ptr),
            "receiver must be re-enqueued"
        );
    }

    /// Integration: Fault delivery to blocked handler — verify fault message in registers.
    #[test]
    fn test_integration_fault_delivery_to_blocked_handler() {
        let ks = make_kernel_state();
        let handler_field_id = make_field_in_arena(&ks, 8);
        let (mut handler_obs, _he) = make_sender_with_cap(
            ObjectType::Field,
            handler_field_id,
            Rights::RECEIVE,
            Badge(0),
            0,
        );
        let handler_ptr = NonNull::from(&mut handler_obs);
        let handle = encode_handle(0);

        // Step 1: Handler blocks on Receive.
        crate::frame::cores::write_test_ipc_registers_via_observer(
            handler_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(handler_ptr);

        core.scheduler.enqueue(handler_ptr);

        let result = core.dispatch_ipc(IpcOperation::Receive, &ks);

        assert!(matches!(result, DispatchResult::Idle));

        // Step 2: Faulting Observer faults → delivered to blocked handler.
        let mut faulting_obs = make_observer_with_handler(Some((handler_field_id, 0)));
        let faulting_ptr = NonNull::from(&mut faulting_obs);

        core.current = Some(faulting_ptr);

        let fault = crate::fault::FaultType::HardwareException {
            esr_el1: 0xDEAD_0001,
            elr_el1: 0x0040_0000,
            far_el1: 0xBAD0_CAFE,
        };
        let result = core.dispatch_fault(fault, &ks);

        assert!(matches!(result, DispatchResult::Resume(p) if p == handler_ptr));

        // Handler's registers must contain fault message.
        let regs = crate::frame::cores::read_ipc_registers(handler_ptr);

        assert_eq!(regs.label, crate::field::LABEL_HARDWARE_EXCEPTION);
        assert_eq!(regs.data[0], 0xDEAD_0001, "ESR");
        assert_eq!(regs.data[1], 0x0040_0000, "ELR");
        assert_eq!(regs.data[2], 0xBAD0_CAFE, "FAR");
        assert!(matches!(
            faulting_obs.state,
            crate::observer::PrimaryState::Faulted
        ));
    }

    /// Integration: Block/Wake scheduler integrity — no duplicate enqueue.
    #[test]
    fn test_integration_block_wake_scheduler_no_duplicates() {
        let ks = make_kernel_state();
        let field_id = make_field_in_arena(&ks, 8);
        let (mut receiver, _re) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::RECEIVE, Badge(0), 0);
        let receiver_ptr = NonNull::from(&mut receiver);
        let (mut sender, _se) =
            make_sender_with_cap(ObjectType::Field, field_id, Rights::SEND, Badge(0xAAAA), 0);
        let sender_ptr = NonNull::from(&mut sender);
        let mut bystander = make_observer();
        let bystander_ptr = NonNull::from(&mut bystander);
        let handle = encode_handle(0);

        // Step 1: Receiver blocks.
        crate::frame::cores::write_test_ipc_registers_via_observer(
            receiver_ptr,
            &crate::syscall::IpcRegisters {
                data: [0; 4],
                label: 0,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        let mut core = make_core_state();

        core.current = Some(receiver_ptr);

        core.scheduler.enqueue(receiver_ptr);
        core.scheduler.enqueue(sender_ptr);
        core.scheduler.enqueue(bystander_ptr);

        let result = core.dispatch_ipc(IpcOperation::Receive, &ks);

        assert!(matches!(result, DispatchResult::Resume(_)));

        // Step 2: Sender wakes receiver.
        crate::frame::cores::write_test_ipc_registers_via_observer(
            sender_ptr,
            &crate::syscall::IpcRegisters {
                data: [1, 2, 3, 4],
                label: 0x55,
                handle_or_badge: handle,
                user_cap: u64::MAX,
                reply_info: 0,
            },
        );

        core.current = Some(sender_ptr);

        let result = core.dispatch_ipc(IpcOperation::Send, &ks);

        assert!(matches!(result, DispatchResult::Resume(_)));
        // Queue must have exactly 3 entries (no duplicate receiver).
        assert_eq!(core.scheduler.queue_depth(), 3);

        core.scheduler.dequeue(receiver_ptr);

        assert!(!core.scheduler.contains(receiver_ptr));
        assert_eq!(core.scheduler.queue_depth(), 2);
    }
}
