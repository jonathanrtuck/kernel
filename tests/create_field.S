// Proves CreateField typed syscall works from EL0.
//
// CreateField (op_code 13) consumes a Space cap (target) and creates
// a Field object. The root Observer's slot 3 holds the root Space cap.
//
// Typed syscall ABI: SVC #0, x4 = op_code, x5 = target_handle,
// x0-x3 = args. Result in x0 (negative = error).
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    // SpaceSplit first to get a consumable Space for CreateField.
    // We cannot consume the root Space directly — it would leave us
    // with no Space for future operations.
    mov x4, #11             // op_code: SpaceSplit
    mov x5, #3              // target_handle: root Space at slot 3
    mov x0, #4096           // size = 4096 bytes (one page minimum)
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0                  // typed syscall
    tbnz x0, #63, 1f        // fail if SpaceSplit returned error
    mov x19, x0             // save new Space handle

    // CreateField on the split Space.
    mov x4, #13             // op_code: CreateField
    mov x5, x19             // target_handle: the split Space
    mov x0, #8              // capacity hint (ignored — uses Space size)
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0                  // typed syscall
    tbnz x0, #63, 1f        // fail if CreateField returned error
    brk #0x42                // PASS — Field created
1:
    brk #1                   // FAIL
