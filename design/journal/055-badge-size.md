# 055 — Badge size: u64

Date: 2026-04-24

## Starting point

D17 settled badge representation (minter-assigned), assignment (at clone time),
and lifecycle (closure notifications). Badge size was explicitly deferred as an
"implementation detail" with a "64-bit default." This entry settles it.

## Exploration

The "64-bit default" is not a soft default — it is forced by the ABI.

### The forcing chain

**Step 1: D47/D49 place badge in a 64-bit register.** The IPC receive return
layout assigns x5 = badge. x5 is a 64-bit ARM64 general-purpose register. The
delivered badge width is 64 bits, full stop. The only way to change this would
be to revise D47 and redesign the IPC ABI.

**Step 2: The cap-table entry should store what the ABI delivers.** D8 defines
the cap-table entry as `(object pointer, rights mask, badge, slot tag)`. If the
entry stored a 32-bit badge, the kernel would zero-extend to 64 bits before
writing x5 — creating two widths for the same value with no structural benefit.
Every other word-sized field (data words, handles) is 64 bits in storage and 64
bits in transit. Badge should be the same.

**Step 3: Prior art is unanimous.** From `design/research/badge-semantics.md`:
every 64-bit capability system (seL4/64, L4/Fiasco.OC, Zircon) uses the full
machine word for badges/identifiers. The only narrower examples are seL4 on
32-bit (28 bits — CTE packing pressure that does not exist here) and Coyotos's
two-field design (rejected by D17).

**Step 4: No benefit from narrowing.** No cap-table entry size budget exists. No
value-space ceiling has been identified. The fast-path cost difference between a
32-bit and 64-bit load/store is immeasurable on ARM64 against the ~400-cycle
budget.

### Foreclosed alternatives

- **32-bit badge:** requires zero-extend invariant with no benefit.
- **Variable-width:** D47/D49 assign a fixed register (x5).
- **No badge:** foreclosed by D17.
- **Two-field:** foreclosed by D17.

### Adjacent open question

Badge value zero: seL4 reserves zero as "unbadged." This kernel has not derived
whether zero is reserved. D17 notes badge = 0 in kernel-created send-once caps
as a special case. Whether zero is valid as a minter-chosen badge is a
downstream convention question — not a size question. It should be settled
alongside M10 (badge on kernel-created send-once caps).

## Status

**Settled.** Badge is u64 in the cap-table entry and in the delivered message.
No masking, no zero-extension, no conventions about which bits are meaningful.
The minter provides a u64 at clone time; the receiver reads a u64 in x5.
