//! Fault types and delivery mechanism.
//!
//! D12: fault delegation to userspace pager Observers.
//! D40: per-fault-type resolution via typed kernel syscalls.
//! D61: four fault types with specific data word assignments.
//!      Faults ARE IPC — kernel-as-sender to handler Field (D7).
//! D80: error and fault delivery protocol — how syscall errors reach
//!      userspace and how hardware faults become IPC messages to handler
//!      Fields.

use crate::capability::{self, Badge, ObjectType, Rights, TransferredCap};
use crate::field::{self, Field, FieldError};
use crate::observer::Observer;
use core::ptr::NonNull;

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

// ── Fault delivery protocol (D80) ────────────────────────────────

/// Outcome of attempting to deliver a fault message to the handler Field.
///
/// D80: the kernel acts as IPC sender for fault messages (D12). The
/// protocol composes existing primitives: FaultType → Message → enqueue
/// to handler Field. This enum reports the outcome so the caller can
/// decide the scheduling action.
///
/// D78: message ownership is explicit through return types. WokeReceiver
/// carries the message for dispatch to deliver (same pattern as
/// communication::SendOutcome::WokeReceiver).
pub enum FaultDeliveryOutcome {
    /// Fault message enqueued into the handler Field's queue.
    /// Ownership transferred: the Message now lives in the queue.
    Enqueued,

    /// A waiting receiver was found on the handler Field. The message
    /// bypassed the queue (D13 direct delivery). The dispatch layer
    /// must deliver it to the receiver's saved registers via
    /// `write_message_to_registers` and enqueue the receiver.
    WokeReceiver(NonNull<Observer>, field::Message),

    /// Handler Field queue is full (D18). The faulting Observer should
    /// be linked into the handler Field's pending list. The next
    /// receive() that frees a slot will drain the pending entry and
    /// deliver the deferred fault message.
    Deferred,

    /// The fault handler cap at slot 0 is invalid: empty slot, wrong
    /// type, or stale generation. D68 pager unavailability — the caller
    /// must initiate the supervision notification or destroy chain.
    HandlerUnavailable,
}

/// Construct the TransferredCap for the faulting Observer in a fault message.
///
/// D61: the fault message carries an Observer handle cap with exactly
/// FAULT_OBSERVER rights (5 of 9: resume, destroy, install_cap,
/// write_registers, read_registers). The kernel constructs this directly
/// from the arena — no minting from the self-cap needed.
///
/// D80: the kernel has direct knowledge of the observer's identity and
/// generation. It constructs a TransferredCap with the 5-right subset.
/// This is NOT an attenuation of an existing cap — the kernel is the
/// sender and creates authority directly. This is the same pattern as
/// D16 reply cap construction.
pub fn make_observer_fault_cap(
    observer_id: crate::arena::ObjectId,
    observer_generation: u64,
) -> TransferredCap {
    TransferredCap {
        object_type: ObjectType::Observer,
        object_id: observer_id,
        rights: Rights::FAULT_OBSERVER,
        badge: Badge(0),
        send_once: false,
        stored_generation: observer_generation,
    }
}

/// Deliver a fault to the handler Field at cap-table slot 0 (D12, D21, D80).
///
/// Full protocol:
/// 1. Read the handler cap entry at reserved slot 0 (D21).
/// 2. Validate: occupied, correct type (Field), correct generation.
/// 3. Transition the faulting Observer to Faulted state (D39).
/// 4. Construct the fault message (D61): badge from handler cap,
///    fault-type label, data words, Observer handle cap with 5 rights.
/// 5. Attempt enqueue to the handler Field:
///    - If a receiver is waiting: direct delivery (D13).
///    - If queue has space: enqueue.
///    - If queue full: return Deferred for D18 pending-list linkage.
///
/// The caller handles the scheduling decision (DispatchResult) and,
/// for Deferred, links the Observer into the handler Field's pending list.
///
/// D80: this function does NOT call frame/ write helpers — those are for
/// syscall error paths. Fault delivery constructs a Message and enqueues
/// it. The Observer is NOT resumed; it stays in Faulted state until the
/// handler calls resume().
pub fn deliver_fault(
    fault: FaultType,
    handler_field: &mut Field,
    handler_badge: Badge,
    observer_id: crate::arena::ObjectId,
    observer_generation: u64,
) -> FaultDeliveryOutcome {
    // Step 1: Construct the Observer handle cap for the fault message.
    let observer_cap = make_observer_fault_cap(observer_id, observer_generation);
    // Step 2: Construct the fault message (D61).
    let message = fault.to_message(handler_badge, observer_cap);

    // Step 3: Attempt delivery — same path as user Send with cap (D12).
    // Check for a waiting receiver first (D13 direct delivery).
    if let Some(waiter_ptr) = handler_field.pop_waiter() {
        let observer = crate::frame::fields::waiter_observer(waiter_ptr);

        // D78: message ownership passes to the caller via WokeReceiver.
        // The dispatch layer delivers it to the receiver's saved registers.
        return FaultDeliveryOutcome::WokeReceiver(observer, message);
    }

    // No waiter — try to enqueue into the handler Field's queue.
    match handler_field.enqueue(message) {
        Ok(()) => FaultDeliveryOutcome::Enqueued,
        Err(FieldError::QueueFull) => {
            // D18: handler Field full. The faulting Observer will be linked
            // into the pending list by the caller.
            FaultDeliveryOutcome::Deferred
        }
        Err(_) => {
            // No other error variants from enqueue currently exist.
            // Defensive: treat as deferred.
            FaultDeliveryOutcome::Deferred
        }
    }
}

/// Validate the fault handler cap entry at slot 0 (D21, D80).
///
/// Returns `Some((handler_field_id, handler_badge))` if the handler cap
/// is valid, or `None` if the handler is unavailable (D68).
///
/// The caller uses the returned ObjectId to look up the handler Field
/// in the arena, then calls `deliver_fault`.
pub fn validate_handler_cap(
    handler_entry: &capability::Entry,
    handler_field_generation: u64,
) -> Option<(crate::arena::ObjectId, Badge)> {
    // Must be occupied.
    let (object_type, object_id) = handler_entry.object?;

    // Must be a Field cap.
    if object_type != ObjectType::Field {
        return None;
    }
    // Must have SEND right (D21: handler cap carries send rights).
    if !handler_entry.check_rights(Rights::SEND) {
        return None;
    }
    // D67: generation check — stale cap means the handler Field was destroyed.
    if !handler_entry.check_generation(handler_field_generation) {
        return None;
    }

    Some((object_id, handler_entry.badge))
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

    // ── D80: Error and fault delivery protocol ────────────────────────

    // ── Helpers ────────────────────────────────────────────────────────

    use crate::arena::ObjectId;
    use core::ptr::NonNull;
    use core::sync::atomic::AtomicU64;

    /// Construct a Field with a real queue allocation for test use.
    fn test_field(capacity: u32) -> field::Field {
        field::Field {
            queue: crate::frame::fields::alloc_test_queue(capacity),
            queue_capacity: capacity,
            queue_length: 0,
            queue_head: 0,
            waiters_head: None,
            waiters_tail: None,
            routing_table: None,
            pending_head: None,
            badge_tracking: false,
            back_pointer_head: None,
            refcount: 1,
            generation: AtomicU64::new(0),
            backing_va_base: 0,
            backing_size: 0,
        }
    }

    /// Construct a WaitEntry with dangling pointers for test use.
    fn make_wait_entry() -> crate::observer::WaitEntry {
        crate::observer::WaitEntry {
            observer: NonNull::dangling(),
            field: NonNull::dangling(),
            prev: None,
            next: None,
        }
    }

    // ── D80: make_observer_fault_cap ──────────────────────────────────

    /// D80: fault cap carries exactly FAULT_OBSERVER rights (5 of 9).
    #[test]
    fn test_d80_fault_cap_has_exact_rights() {
        let cap = make_observer_fault_cap(ObjectId(42), 7);

        assert_eq!(cap.rights, Rights::FAULT_OBSERVER);
        assert_eq!(cap.object_type, capability::ObjectType::Observer);
        assert_eq!(cap.object_id, ObjectId(42));
        assert_eq!(cap.stored_generation, 7);
        assert!(!cap.send_once, "D80: fault cap must not be send-once");
        assert_eq!(
            cap.badge,
            Badge(0),
            "D80: fault cap badge is zero (handler badge is on the message)"
        );
    }

    /// D80: fault cap includes resume right (needed for fault resolution).
    #[test]
    fn test_d80_fault_cap_includes_resume() {
        let cap = make_observer_fault_cap(ObjectId(0), 0);

        assert!(cap.rights.contains(Rights::RESUME));
    }

    /// D80: fault cap includes destroy right.
    #[test]
    fn test_d80_fault_cap_includes_destroy() {
        let cap = make_observer_fault_cap(ObjectId(0), 0);

        assert!(cap.rights.contains(Rights::DESTROY));
    }

    /// D80: fault cap includes install_cap right (needed for D40 resolution).
    #[test]
    fn test_d80_fault_cap_includes_install_cap() {
        let cap = make_observer_fault_cap(ObjectId(0), 0);

        assert!(cap.rights.contains(Rights::INSTALL_CAP));
    }

    /// D80: fault cap includes write_registers right.
    #[test]
    fn test_d80_fault_cap_includes_write_registers() {
        let cap = make_observer_fault_cap(ObjectId(0), 0);

        assert!(cap.rights.contains(Rights::WRITE_REGISTERS));
    }

    /// D80: fault cap includes read_registers right.
    #[test]
    fn test_d80_fault_cap_includes_read_registers() {
        let cap = make_observer_fault_cap(ObjectId(0), 0);

        assert!(cap.rights.contains(Rights::READ_REGISTERS));
    }

    /// D80: fault cap excludes suspend (not needed for resolution).
    #[test]
    fn test_d80_fault_cap_excludes_suspend() {
        let cap = make_observer_fault_cap(ObjectId(0), 0);

        assert!(!cap.rights.contains(Rights::SUSPEND));
    }

    /// D80: fault cap excludes change_handler (would escalate privilege).
    #[test]
    fn test_d80_fault_cap_excludes_change_handler() {
        let cap = make_observer_fault_cap(ObjectId(0), 0);

        assert!(!cap.rights.contains(Rights::CHANGE_HANDLER));
    }

    /// D80: fault cap excludes modify_scheduling (not resolution).
    #[test]
    fn test_d80_fault_cap_excludes_modify_scheduling() {
        let cap = make_observer_fault_cap(ObjectId(0), 0);

        assert!(!cap.rights.contains(Rights::MODIFY_SCHEDULING));
    }

    /// D80: fault cap excludes clone (handler already has the cap).
    #[test]
    fn test_d80_fault_cap_excludes_clone() {
        let cap = make_observer_fault_cap(ObjectId(0), 0);

        assert!(!cap.rights.contains(Rights::CLONE));
    }

    // ── D80: deliver_fault — successful delivery ─────────────────────

    /// D80: deliver_fault to an empty handler Field succeeds.
    #[test]
    fn test_d80_deliver_fault_to_empty_field_succeeds() {
        let mut handler_field = test_field(4);
        let fault = FaultType::VmFault {
            space_slot: 1,
            byte_offset: 0x1000,
            access: AccessType::Read,
        };
        let result = deliver_fault(fault, &mut handler_field, Badge(100), ObjectId(5), 0);

        assert!(
            matches!(result, FaultDeliveryOutcome::Enqueued),
            "D80: fault to empty field must enqueue"
        );
        assert_eq!(
            handler_field.queue_length, 1,
            "D80: fault message must be enqueued"
        );
    }

    /// D80: fault message in queue has correct label for VM fault.
    #[test]
    fn test_d80_delivered_fault_has_correct_label() {
        let mut handler_field = test_field(4);
        let fault = FaultType::VmFault {
            space_slot: 2,
            byte_offset: 0x200,
            access: AccessType::Write,
        };

        deliver_fault(fault, &mut handler_field, Badge(10), ObjectId(3), 0);

        let msg = handler_field.dequeue().unwrap();

        assert_eq!(msg.label, field::LABEL_VM_FAULT, "D80/D61: VM fault label");
    }

    /// D80: fault message carries handler badge (D21).
    #[test]
    fn test_d80_delivered_fault_has_handler_badge() {
        let mut handler_field = test_field(4);
        let fault = FaultType::CapTableFull;

        deliver_fault(fault, &mut handler_field, Badge(0xDEAD), ObjectId(1), 0);

        let msg = handler_field.dequeue().unwrap();

        assert_eq!(
            msg.badge,
            Badge(0xDEAD),
            "D80/D21: message carries handler badge"
        );
    }

    /// D80: fault message carries Observer handle cap.
    #[test]
    fn test_d80_delivered_fault_has_observer_cap() {
        let mut handler_field = test_field(4);
        let fault = FaultType::ResourceRequest {
            resource: ResourceType::Space,
            quantity: 4,
        };

        deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(7), 42);

        let msg = handler_field.dequeue().unwrap();
        let cap = msg
            .user_cap
            .expect("D80/D61: fault message must carry Observer cap");

        assert_eq!(cap.object_type, capability::ObjectType::Observer);
        assert_eq!(cap.object_id, ObjectId(7));
        assert_eq!(cap.rights, Rights::FAULT_OBSERVER);
        assert_eq!(cap.stored_generation, 42);
    }

    /// D80: fault message has no reply cap (kernel deposit, not Call).
    #[test]
    fn test_d80_delivered_fault_has_no_reply_cap() {
        let mut handler_field = test_field(4);
        let fault = FaultType::CapTableFull;

        deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        let msg = handler_field.dequeue().unwrap();

        assert!(
            msg.reply_cap.is_none(),
            "D80/D16: kernel-as-sender fault message has no reply cap"
        );
    }

    /// D80: VM fault data words match D61 table.
    #[test]
    fn test_d80_vm_fault_data_words_in_delivered_message() {
        let mut handler_field = test_field(4);
        let fault = FaultType::VmFault {
            space_slot: 3,
            byte_offset: 0x4000,
            access: AccessType::Execute,
        };

        deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        let msg = handler_field.dequeue().unwrap();

        assert_eq!(msg.data[0], 3, "D61: data[0] = space slot index");
        assert_eq!(msg.data[1], 0x4000, "D61: data[1] = byte offset");
        assert_eq!(msg.data[2], 2, "D61: data[2] = access type (Execute=2)");
        assert_eq!(msg.data[3], 0, "D61: data[3] = reserved zero");
    }

    /// D80: hardware exception data words match D61 table.
    #[test]
    fn test_d80_hardware_exception_data_words() {
        let mut handler_field = test_field(4);
        let fault = FaultType::HardwareException {
            esr_el1: 0x8200_0000,
            elr_el1: 0xFFFF_0000_0040_0000,
            far_el1: 0xDEAD_BEEF,
        };

        deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        let msg = handler_field.dequeue().unwrap();

        assert_eq!(msg.data[0], 0x8200_0000, "D61: data[0] = ESR_EL1");
        assert_eq!(msg.data[1], 0xFFFF_0000_0040_0000, "D61: data[1] = ELR_EL1");
        assert_eq!(msg.data[2], 0xDEAD_BEEF, "D61: data[2] = FAR_EL1");
        assert_eq!(msg.data[3], 0, "D61: data[3] = reserved zero");
    }

    // ── D80: deliver_fault — direct delivery to waiting receiver ─────

    /// D80: deliver_fault with a waiter on the handler Field uses direct delivery.
    /// D78: the WokeReceiver variant carries the message for the dispatch layer.
    #[test]
    fn test_d80_deliver_fault_direct_delivery_to_waiter() {
        let mut handler_field = test_field(4);
        let mut entry = make_wait_entry();

        handler_field.add_waiter(&mut entry);

        let fault = FaultType::CapTableFull;
        let result = deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        assert!(
            matches!(result, FaultDeliveryOutcome::WokeReceiver(_, _)),
            "D80/D78: direct delivery must return WokeReceiver with message"
        );
        assert_eq!(
            handler_field.queue_length, 0,
            "D80: direct delivery must not enqueue"
        );
        assert!(
            handler_field.waiters_head.is_none(),
            "D80: waiter must be popped"
        );
    }

    // ── D80: deliver_fault — deferred delivery (D18) ─────────────────

    /// D80/D18: deliver_fault to a full handler Field returns Deferred.
    #[test]
    fn test_d80_deliver_fault_full_queue_returns_deferred() {
        let mut handler_field = test_field(2);

        // Fill the handler Field queue.
        handler_field
            .enqueue(field::Message::timer_fire(Badge(0), 0, 0))
            .unwrap();
        handler_field
            .enqueue(field::Message::timer_fire(Badge(0), 0, 0))
            .unwrap();

        assert!(handler_field.is_full());

        let fault = FaultType::VmFault {
            space_slot: 0,
            byte_offset: 0,
            access: AccessType::Read,
        };
        let result = deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        assert!(
            matches!(result, FaultDeliveryOutcome::Deferred),
            "D80/D18: full queue must defer"
        );
        assert_eq!(
            handler_field.queue_length, 2,
            "D80: queue must not change on deferred"
        );
    }

    /// D80/D18: full queue with waiter still uses direct delivery (waiter bypasses queue).
    #[test]
    fn test_d80_deliver_fault_full_queue_with_waiter_uses_direct_delivery() {
        let mut handler_field = test_field(1);

        handler_field
            .enqueue(field::Message::timer_fire(Badge(0), 0, 0))
            .unwrap();

        assert!(handler_field.is_full());

        let mut entry = make_wait_entry();

        handler_field.add_waiter(&mut entry);

        let fault = FaultType::CapTableFull;
        let result = deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        assert!(
            matches!(result, FaultDeliveryOutcome::WokeReceiver(_, _)),
            "D80/D78: waiter bypasses full queue via WokeReceiver"
        );
    }

    // ── D80: validate_handler_cap ─────────────────────────────────────

    /// D80: valid handler cap returns Some with field id and badge.
    #[test]
    fn test_d80_validate_handler_cap_valid() {
        let entry = capability::Entry {
            object: Some((capability::ObjectType::Field, ObjectId(7))),
            rights: Rights::SEND,
            badge: Badge(42),
            slot_tag: capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };
        let result = validate_handler_cap(&entry, 0);

        assert_eq!(result, Some((ObjectId(7), Badge(42))));
    }

    /// D80: empty handler cap returns None (HandlerUnavailable).
    #[test]
    fn test_d80_validate_handler_cap_empty_slot() {
        let entry = capability::Entry::empty(capability::SlotTag(0));
        let result = validate_handler_cap(&entry, 0);

        assert!(
            result.is_none(),
            "D80: empty handler cap must be unavailable"
        );
    }

    /// D80: handler cap with wrong type returns None.
    #[test]
    fn test_d80_validate_handler_cap_wrong_type() {
        let entry = capability::Entry {
            object: Some((capability::ObjectType::Observer, ObjectId(1))),
            rights: Rights::SEND,
            badge: Badge(0),
            slot_tag: capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };
        let result = validate_handler_cap(&entry, 0);

        assert!(
            result.is_none(),
            "D80: non-Field handler cap must be unavailable"
        );
    }

    /// D80: handler cap without SEND right returns None.
    #[test]
    fn test_d80_validate_handler_cap_no_send_right() {
        let entry = capability::Entry {
            object: Some((capability::ObjectType::Field, ObjectId(1))),
            rights: Rights::RECEIVE,
            badge: Badge(0),
            slot_tag: capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };
        let result = validate_handler_cap(&entry, 0);

        assert!(
            result.is_none(),
            "D80: handler cap without SEND must be unavailable"
        );
    }

    /// D80/D67: handler cap with stale generation returns None.
    #[test]
    fn test_d80_validate_handler_cap_stale_generation() {
        let entry = capability::Entry {
            object: Some((capability::ObjectType::Field, ObjectId(1))),
            rights: Rights::SEND,
            badge: Badge(0),
            slot_tag: capability::SlotTag(0),
            send_once: false,
            stored_generation: 0,
        };
        // Live generation is 1, stored is 0 — stale.
        let result = validate_handler_cap(&entry, 1);

        assert!(
            result.is_none(),
            "D80/D67: stale handler cap must be unavailable"
        );
    }

    /// D80: handler cap with full rights still valid (SEND is present in FIELD_ALL).
    #[test]
    fn test_d80_validate_handler_cap_full_rights() {
        let entry = capability::Entry {
            object: Some((capability::ObjectType::Field, ObjectId(99))),
            rights: Rights::FIELD_ALL,
            badge: Badge(0xBEEF),
            slot_tag: capability::SlotTag(0),
            send_once: false,
            stored_generation: 5,
        };
        let result = validate_handler_cap(&entry, 5);

        assert_eq!(result, Some((ObjectId(99), Badge(0xBEEF))));
    }

    // ── D80: all fault types deliver correctly ────────────────────────

    /// D80: all four fault types can be delivered to a handler Field.
    #[test]
    fn test_d80_all_fault_types_deliver() {
        let faults: [FaultType; 4] = [
            FaultType::VmFault {
                space_slot: 0,
                byte_offset: 0,
                access: AccessType::Read,
            },
            FaultType::ResourceRequest {
                resource: ResourceType::Space,
                quantity: 1,
            },
            FaultType::CapTableFull,
            FaultType::HardwareException {
                esr_el1: 0,
                elr_el1: 0,
                far_el1: 0,
            },
        ];

        for fault in faults {
            let mut handler_field = test_field(4);
            let result = deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

            assert!(matches!(result, FaultDeliveryOutcome::Enqueued));

            let msg = handler_field.dequeue().unwrap();

            assert!(
                msg.user_cap.is_some(),
                "all fault types must carry Observer cap"
            );
            assert!(msg.reply_cap.is_none(), "no reply cap on kernel-as-sender");
        }
    }

    /// D80: each fault type's label is preserved in the delivered message.
    #[test]
    fn test_d80_fault_labels_preserved_in_delivery() {
        let expected: [(FaultType, u64); 4] = [
            (
                FaultType::VmFault {
                    space_slot: 0,
                    byte_offset: 0,
                    access: AccessType::Read,
                },
                field::LABEL_VM_FAULT,
            ),
            (
                FaultType::ResourceRequest {
                    resource: ResourceType::Space,
                    quantity: 0,
                },
                field::LABEL_RESOURCE_REQUEST,
            ),
            (FaultType::CapTableFull, field::LABEL_CAP_TABLE_FULL),
            (
                FaultType::HardwareException {
                    esr_el1: 0,
                    elr_el1: 0,
                    far_el1: 0,
                },
                field::LABEL_HARDWARE_EXCEPTION,
            ),
        ];

        for (fault, label) in expected {
            let mut handler_field = test_field(4);

            deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

            let msg = handler_field.dequeue().unwrap();

            assert_eq!(msg.label, label);
        }
    }

    // ── D80: resource request data words ──────────────────────────────

    /// D80/D61: resource request data words match spec table.
    #[test]
    fn test_d80_resource_request_data_words() {
        let mut handler_field = test_field(4);
        let fault = FaultType::ResourceRequest {
            resource: ResourceType::Time,
            quantity: 100,
        };

        deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        let msg = handler_field.dequeue().unwrap();

        assert_eq!(msg.data[0], 1, "D61: data[0] = resource type (Time=1)");
        assert_eq!(msg.data[1], 100, "D61: data[1] = quantity");
        assert_eq!(msg.data[2], 0, "D61: data[2] = reserved zero");
        assert_eq!(msg.data[3], 0, "D61: data[3] = reserved zero");
    }

    /// D80/D61: cap-table-full data words are all zeros.
    #[test]
    fn test_d80_cap_table_full_data_words() {
        let mut handler_field = test_field(4);
        let fault = FaultType::CapTableFull;

        deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        let msg = handler_field.dequeue().unwrap();

        assert_eq!(
            msg.data,
            [0, 0, 0, 0],
            "D61: cap-table-full data words are all zero"
        );
    }

    // ── D80: zero-capacity handler Field ──────────────────────────────

    /// D80: deliver_fault to a zero-capacity handler Field with no waiter defers.
    #[test]
    fn test_d80_deliver_fault_zero_capacity_no_waiter_defers() {
        let mut handler_field = test_field(0);
        let fault = FaultType::CapTableFull;
        let result = deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        assert!(matches!(result, FaultDeliveryOutcome::Deferred));
    }

    /// D80: zero-capacity handler Field with waiter still delivers directly.
    #[test]
    fn test_d80_deliver_fault_zero_capacity_with_waiter_delivers() {
        let mut handler_field = test_field(0);
        let mut entry = make_wait_entry();

        handler_field.add_waiter(&mut entry);

        let fault = FaultType::CapTableFull;
        let result = deliver_fault(fault, &mut handler_field, Badge(0), ObjectId(0), 0);

        assert!(
            matches!(result, FaultDeliveryOutcome::WokeReceiver(_, _)),
            "D80/D78: waiter delivers directly via WokeReceiver"
        );
    }
}
