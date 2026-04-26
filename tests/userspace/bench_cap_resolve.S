// Benchmark: cap resolution cost.
//
// Measures the cost of 200 typed syscalls that each resolve a cap handle,
// check rights, and perform an operation. Uses SpaceSplit + Close in a
// loop. Each iteration: split 16 KiB from root Space, close the result.
//
// Protocol: BRK #0x42 = PASS (all iterations complete without error).

    mrs x19, CNTVCT_EL0     // start timestamp
    mov x20, #200            // iteration count
1:
    // SpaceSplit: resolve Space cap at slot 3, check SPLIT right, split.
    mov x4, #11              // SpaceSplit
    mov x5, #3               // root Space handle
    mov x0, #16384           // 16 KiB
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 2f        // fail if split failed
    mov x21, x0              // save new Space handle in x21

    // Close the new Space cap to free the slot for next iteration.
    mov x4, #9               // Close
    mov x5, x21              // target = new Space handle
    mov x0, #0
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0

    sub x20, x20, #1
    cbnz x20, 1b

    mrs x21, CNTVCT_EL0      // end timestamp
    sub x0, x21, x19         // elapsed cycles
    cbz x0, 2f               // fail if zero elapsed (shouldn't happen)
    brk #0x42                // PASS
2:
    brk #1                   // FAIL
