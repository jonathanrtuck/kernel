// Proves CreateObserver typed syscall works from EL0.
//
// CreateObserver (op_code 18) consumes a Space cap and creates a new
// Observer. It requires args: x0 = handler Field handle, x1 = handler
// badge. We need to create a Field first (for the handler), then create
// the Observer using a second Space.
//
// Typed syscall ABI: SVC #0, x4 = op_code, x5 = target_handle,
// x0-x3 = args. Result in x0 (negative = error, >= 0 = new handle).
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    // Step 1: SpaceSplit for the handler Field.
    mov x4, #11             // op_code: SpaceSplit
    mov x5, #3              // target_handle: root Space at slot 3
    mov x0, #4096           // size = one page
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 1f
    mov x19, x0             // save Space handle for Field

    // Step 2: CreateField for the handler.
    // CreateField consumes the Space and overwrites the same cap slot
    // with the new Field cap. So the Field handle = the Space handle (x19).
    // CreateField returns 0 on success (void), not a new handle.
    mov x4, #13             // op_code: CreateField
    mov x5, x19             // target: Space for handler Field
    mov x0, #8
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 1f
    mov x20, x19            // Field handle = same slot as consumed Space

    // Step 3: SpaceSplit for the Observer's structural backing.
    // CreateObserver needs: RegisterState (816 bytes) + L1 table (16384)
    // + cap table entries. We need at least ~32 KiB.
    mov x4, #11             // op_code: SpaceSplit
    mov x5, #3              // target_handle: root Space
    movz x0, #0x0001, lsl #16  // x0 = 0x10000 = 65536 bytes (4 pages of 16 KiB)
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 1f
    mov x21, x0             // save Space handle for Observer

    // Step 4: CreateObserver.
    mov x4, #18             // op_code: CreateObserver
    mov x5, x21             // target: Space for structural backing
    mov x0, x20             // arg0 = handler Field handle
    mov x1, #0              // arg1 = handler badge = 0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 1f
    brk #0x42                // PASS — Observer created
1:
    brk #1                   // FAIL
