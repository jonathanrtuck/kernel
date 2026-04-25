# 058 — Fault message content and delivery mechanism

Date: 2026-04-24

## Starting point

D12 settled that faults are delegated via IPC. D18 settled deferred delivery.
D20/D21 settled fault handler registration. D28 settled the message format. D39
settled Observer rights. D40 settled fault resolution. The message content and
delivery mechanism were implicit across these but never stated as a single
derivation.

## Delivery mechanism

Fully constrained by D13 + D20/D21 + D18 + D28. Fault delivery is standard
queued-Field IPC with the kernel as sender:

1. Kernel saves full register state, classifies fault (ESR_EL1/FAR_EL1)
2. Observer transitions to "faulted" state (D39), descheduled
3. Kernel reads handler cap at reserved slot 0 (D21)
4. Kernel constructs D28-format message with Observer handle cap
5. Kernel enqueues to handler Field — same code path as user Send() with cap
6. If handler waiting + same core: direct-switch. Note: fault messages always
   carry a cap, so they do NOT qualify for D50's 0-cap fast-path gate
7. If Field full (D18): Observer goes on pending list via D43 wait-state linkage

No separate mechanism needed. D7's open question about fault traffic
classification is answered: faults ARE IPC (kernel-as-sender).

## Four fault types

| Type               | Trigger                       | data[0]                        | data[1]     | data[2]                   | data[3] |
| ------------------ | ----------------------------- | ------------------------------ | ----------- | ------------------------- | ------- |
| VM_FAULT           | VA outside all Space caps     | Space slot index               | byte offset | access type (0=R,1=W,2=X) | 0       |
| RESOURCE_REQUEST   | resource_request() syscall    | resource type (0=Space,1=Time) | quantity    | 0                         | 0       |
| CAP_TABLE_FULL     | cap table full during install | 0                              | 0           | 0                         | 0       |
| HARDWARE_EXCEPTION | unhandled ARM64 exception     | ESR_EL1                        | ELR_EL1     | FAR_EL1                   | 0       |

All carry: badge from D21 handler cap, fault-type label, Observer handle cap
with 5 rights (resume + destroy + install_cap + write_registers +
read_registers).

### VM_FAULT: divergence from all prior art

Every surveyed system (seL4, L4, Mach, Zircon) carries the raw faulting VA. This
kernel cannot — D26 makes VA kernel-internal. Instead: Space slot index + byte
offset within that Space. The handler knows which Space the slot corresponds to
(it set up the Observer) and can grow it via space_merge (D41).

### Observer handle rights

Five of nine D39 rights. Excluded: suspend (not needed for resolution),
change_handler (would escalate privilege), modify_scheduling (not resolution),
clone (handler already has the cap). The five included cover all resolution
actions across all fault types.

### Faulted state

Distinct from blocked (D39). Cannot simultaneously be blocked and faulted
(faulted reached only from runnable). Can be simultaneously suspended
(suspended-while-faulted requires two resumes). D18 pending list linkage reuses
D43's wait-state field.

## Status

**Settled.** Four fault types, delivery via standard Field IPC, 5-right Observer
handle, faulted as distinct state.

Does NOT settle: hardware exception label taxonomy (one vs many labels — minor
choice), debug fault delivery, pager unavailability (G04), label numeric values,
lazy vs eager PTE population policy.
