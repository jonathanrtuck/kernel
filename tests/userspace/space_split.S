// Proves SpaceSplit typed syscall works from EL0.
//
// SpaceSplit (op_code 11) splits the target Space, extracting a new
// Space of the requested size. The root Observer's slot 3 holds the
// root Space cap with SPLIT rights.
//
// Typed syscall ABI: SVC #0, x4 = op_code, x5 = target_handle,
// x0-x3 = args. Result in x0 (negative = error, >= 0 = new handle).
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    mov x4, #11             // op_code: SpaceSplit
    mov x5, #3              // target_handle: root Space at slot 3
    mov x0, #4096           // size to split = 4096 bytes (one page)
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0                  // typed syscall
    tbnz x0, #63, 1f        // if negative (bit 63 set), fail
    brk #0x42                // PASS — Space split succeeded
1:
    brk #1                   // FAIL
