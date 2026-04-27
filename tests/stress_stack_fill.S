// Stress test: fill and verify a pattern across 128 stack slots.
//
// Writes a known pattern (slot index XOR 0xDEAD) to 128 consecutive
// 16-byte stack slots (2048 bytes, well within the 16K stack page).
// Then reads every slot back and verifies the pattern matches. This
// exercises user memory mapping integrity under heavy access and
// catches any corruption or mapping faults.
//
// Stack layout: SP starts at 0xC000 (16-byte aligned). Each push
// decrements by 16 to maintain AArch64 SP alignment requirements.
// 128 slots * 16 bytes = 2048 bytes, bringing SP down to 0xB800 —
// safely above the page base at 0x8000.
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL (pattern mismatch).

    mov x4, #0xDEAD         // XOR mask for pattern generation
    mov x0, #0              // slot index (0..127)
    mov x2, #128            // total slot count

    // -- Phase 1: Fill stack with pattern --
    // Each slot gets (index XOR 0xDEAD) stored via STP (16-byte push).
1:
    eor x1, x0, x4          // pattern = index XOR 0xDEAD
    stp x1, xzr, [sp, #-16]!  // push pattern + zero pad (16-byte aligned)
    add x0, x0, #1          // next index
    cmp x0, x2              // done?
    b.lt 1b                 // loop until 128 slots filled

    // -- Phase 2: Verify stack pattern --
    // Read slots back in reverse order (127 down to 0) since we
    // pushed 0 first (deepest) and 127 last (at current SP).
    sub x0, x2, #1          // start verifying from index 127
2:
    ldp x1, x3, [sp], #16   // pop value + pad from stack
    eor x3, x0, x4          // recompute expected pattern
    cmp x1, x3              // actual == expected?
    b.ne 3f                 // mismatch -> FAIL
    sub x0, x0, #1          // next index (counting down)
    cmp x0, #0              // check if we still have slots
    b.ge 2b                 // loop until index goes below 0
    brk #0x42               // PASS — all 128 slots verified

3:
    brk #1                  // FAIL — stack pattern mismatch
