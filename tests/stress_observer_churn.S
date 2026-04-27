// Stress test: Observer create/close churn.
//
// Each iteration: split Space for a handler Field, create the Field,
// split Space for the Observer, create the Observer (with handler),
// close the Observer, close the Field. Tests arena allocation, cap
// table slot reuse, and freelist integrity. 20 iterations.
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    mov x20, #20             // iteration count
1:
    // Step 1: SpaceSplit for handler Field.
    mov x4, #11              // SpaceSplit
    mov x5, #3               // root Space
    mov x0, #16384
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 2f
    mov x21, x0              // x21 = new Space handle (for Field)

    // Step 2: CreateField from the Space.
    mov x4, #13              // CreateField
    mov x5, x21              // target = Space (overwrites with Field)
    mov x0, #8               // capacity = 8
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 2f
    // x21 now holds the Field cap.

    // Step 3: SpaceSplit for Observer (needs 16 KiB L1 + 816B regs + cap table).
    mov x4, #11              // SpaceSplit
    mov x5, #3               // root Space
    mov x0, #32768           // 32 KiB (2 pages)
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 2f
    mov x22, x0              // x22 = new Space handle (for Observer)

    // Step 4: CreateObserver with handler Field.
    mov x4, #18              // CreateObserver
    mov x5, x22              // target = Space (overwrites with Observer)
    mov x0, x21              // args[0] = handler Field handle
    mov x1, #0               // args[1] = handler badge
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 2f
    // x22 now holds the Observer cap.

    // Step 5: Close the Observer cap.
    mov x4, #9               // Close
    mov x5, x22
    mov x0, #0
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0

    // Step 6: Close the Field cap.
    mov x4, #9               // Close
    mov x5, x21
    mov x0, #0
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0

    sub x20, x20, #1
    cbnz x20, 1b

    brk #0x42                // PASS
2:
    brk #1                   // FAIL
