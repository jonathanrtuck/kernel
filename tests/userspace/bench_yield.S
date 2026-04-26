// Benchmark: measure Yield syscall round-trip in counter ticks.
//
// Reads CNTVCT_EL0 before and after 100 Yield syscalls. Verifies that
// elapsed ticks > 0 (the counter advanced). The actual cycle count is
// not checked — this proves the measurement infrastructure works.
//
// Requires clock_access=true on the root Observer (D66).
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    // Read counter before yield loop.
    mrs x19, CNTVCT_EL0

    // Perform 100 yields.
    mov x20, #100
1:
    svc #5                  // Yield
    sub x20, x20, #1
    cbnz x20, 1b

    // Read counter after yield loop.
    mrs x21, CNTVCT_EL0

    // Compute elapsed ticks.
    sub x0, x21, x19

    // Verify elapsed > 0 (counter must have advanced).
    cbz x0, 2f
    brk #0x42                // PASS — benchmark completed
2:
    brk #1                   // FAIL — elapsed was zero
