// Benchmark: measure Send+Receive IPC round-trip latency.
//
// Creates a Field, then performs 100 iterations of Send followed by
// Receive. Brackets the loop with CNTVCT_EL0 reads to measure total
// elapsed counter ticks. Verifies elapsed > 0 and that the final
// received data matches the last sent value.
//
// This measures the kernel's IPC hot path: message construction,
// queue enqueue, queue dequeue, register save/restore overhead.
//
// Requires clock_access=true on the root Observer (D66).
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    // ── Step 1: SpaceSplit for the Field ─────────────────────────────
    mov x4, #11             // op_code: SpaceSplit
    mov x5, #3              // target_handle: root Space at slot 3
    mov x0, #4096           // size = 4096 bytes
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 9f
    mov x19, x0             // Space handle

    // ── Step 2: CreateField ──────────────────────────────────────────
    mov x4, #13             // op_code: CreateField
    mov x5, x19             // target: Space
    mov x0, #8
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 9f
    // Field handle = x19 (same slot)

    // ── Step 3: Read counter before IPC loop ─────────────────────────
    mrs x20, CNTVCT_EL0     // start timestamp

    // ── Step 4: IPC loop — 100 Send+Receive pairs ────────────────────
    mov x21, #100           // iteration count

1:
    // Send: data[0] = iteration count (x21), label = 0x99
    mov x0, x21             // data[0] = current iteration
    mov x1, #0              // data[1]
    mov x2, #0              // data[2]
    mov x3, #0              // data[3]
    mov x4, #0x99           // label
    mov x5, x19             // Field handle
    movn x6, #0             // user cap = CAP_ABSENT
    mov x7, #0              // reply_info
    svc #1                  // Send

    // Receive: dequeue the message we just sent.
    mov x5, x19             // Field handle
    svc #2                  // Receive

    // x0 now holds data[0] from the received message.
    // Verify it matches what we sent (current iteration count).
    cmp x0, x21
    b.ne 9f                 // data mismatch — fail

    sub x21, x21, #1
    cbnz x21, 1b

    // ── Step 5: Read counter after IPC loop ──────────────────────────
    mrs x22, CNTVCT_EL0     // end timestamp

    // ── Step 6: Compute and verify elapsed ticks ─────────────────────
    sub x0, x22, x20        // elapsed = end - start
    cbz x0, 9f              // fail if elapsed was zero (impossible)

    // Elapsed ticks are in x0. The test passes if the counter advanced
    // (which it must have, given 100 IPC round-trips).

    brk #0x42                // PASS — IPC benchmark completed
9:
    brk #1                   // FAIL
