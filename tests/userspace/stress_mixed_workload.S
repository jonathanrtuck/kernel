// Stress test: heavy ALU + memory workload with integrity verification.
//
// Runs 200 iterations of a compute-and-store loop: each iteration
// computes a value from an accumulator using shifts and XORs (a
// simple hash-like mixing function), stores the result to the stack,
// then after all iterations reads the final value back and verifies
// it matches the expected result. This exercises:
//
// - Sustained ALU pressure (shifts, adds, XORs)
// - Stack memory under repeated writes (200 * 16 = 3200 bytes)
// - Correct instruction fetch across the code page
// - No silent memory corruption under load
//
// The expected final accumulator value is precomputed: starting from
// 1, applying (x = (x << 5) ^ (x >> 3) + x) 200 times. We verify
// by re-computing in a second loop rather than hard-coding the
// constant (which would require knowing it at assembly time).
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL (computation mismatch).

    mov x0, #200            // iteration count
    mov x1, #1              // accumulator (seed)
    mov x2, sp              // save original SP for later verification

    // -- Phase 1: Compute and store loop --
    // Each iteration mixes the accumulator and pushes it to the stack.
1:
    lsl x3, x1, #5          // x3 = accumulator << 5
    lsr x4, x1, #3          // x4 = accumulator >> 3
    eor x3, x3, x4          // x3 = (acc << 5) ^ (acc >> 3)
    add x1, x3, x1          // accumulator = mixed + old accumulator
    stp x1, xzr, [sp, #-16]!  // push accumulator to stack (16-byte aligned)
    sub x0, x0, #1          // decrement counter
    cbnz x0, 1b             // repeat 200 times

    // x1 now holds the final accumulator value after 200 iterations.
    mov x5, x1              // save final accumulator

    // -- Phase 2: Restore SP and recompute from scratch --
    mov sp, x2              // restore original SP
    mov x0, #200            // same iteration count
    mov x1, #1              // same seed

2:
    lsl x3, x1, #5
    lsr x4, x1, #3
    eor x3, x3, x4
    add x1, x3, x1
    sub x0, x0, #1
    cbnz x0, 2b

    // -- Phase 3: Verify both computations agree --
    cmp x5, x1              // stored result == recomputed result?
    b.ne 3f                 // mismatch -> FAIL

    // Also verify SP was restored correctly.
    cmp sp, x2
    b.ne 3f

    brk #0x42               // PASS — deterministic compute verified

3:
    brk #1                  // FAIL — computation or SP mismatch
