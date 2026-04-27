// Proves user stack memory is accessible from EL0.
//
// The kernel maps a stack page at VA 0x8000 (USER_STACK_VA) with RW+EL0.
// SP starts at 0xC000 (USER_STACK_TOP = USER_STACK_VA + PAGE_SIZE).
// This test writes to the stack and reads it back, verifying user-mode
// memory access works end-to-end.
//
// Protocol: BRK #0x42 = PASS, fault = FAIL.

    mov x0, #0xBEEF         // marker value
    str x0, [sp, #-16]!     // push to stack (pre-decrement)
    ldr x1, [sp], #16       // pop from stack
    cmp x0, x1              // verify round-trip
    b.ne 1f                 // branch to fail if mismatch
    brk #0x42               // PASS
1:
    brk #1                  // FAIL — stack read/write mismatch
