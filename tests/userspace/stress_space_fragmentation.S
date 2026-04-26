// Stress test: Space split/close fragmentation.
//
// Repeatedly splits the root Space into small pieces and closes them.
// Catches Space accounting bugs, va_base drift, and SpaceManager
// page return issues. 100 iterations.
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    mov x20, #100            // iteration count
1:
    // SpaceSplit: carve off 16 KiB.
    mov x4, #11              // SpaceSplit
    mov x5, #3               // root Space handle
    mov x0, #16384           // 16 KiB
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 2f        // fail if split failed
    mov x21, x0              // save new Space handle

    // Close the split-off Space (returns pages to pool).
    mov x4, #9               // Close
    mov x5, x21              // target = split Space
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
