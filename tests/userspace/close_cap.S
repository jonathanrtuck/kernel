// Proves Close typed syscall works: create a Field, then close its cap.
//
// Close (op_code 9) releases a capability slot. For a Field cap, it
// decrements the Field's refcount. The returned value in x0 is the
// encoded handle of the backing Space cap (>= 0) or error (< 0).
//
// Typed syscall ABI: SVC #0, x4 = op_code, x5 = target_handle,
// x0-x3 = args. Result in x0 (negative = error).
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    // SpaceSplit to get a consumable Space.
    mov x4, #11             // op_code: SpaceSplit
    mov x5, #3              // target_handle: root Space at slot 3
    mov x0, #4096           // size = 4096 bytes
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 1f
    mov x19, x0             // save Space handle

    // CreateField on the split Space.
    // CreateField consumes the Space and overwrites the same cap slot
    // with the new Field cap. The Field handle = the original Space handle.
    // CreateField returns 0 on success (void), not a new handle.
    mov x4, #13             // op_code: CreateField
    mov x5, x19             // target_handle: the split Space
    mov x0, #8
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 1f
    // x19 already holds the correct Field handle (same slot as consumed Space)

    // Close the Field cap.
    mov x4, #9              // op_code: Close
    mov x5, x19             // target: the Field handle
    mov x0, #0
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0
    // Close returns 0 on success or error (< 0).
    tbnz x0, #63, 1f
    brk #0x42                // PASS — cap closed successfully
1:
    brk #1                   // FAIL
