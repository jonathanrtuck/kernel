// Proves ResourceRequest typed syscall works for the root Observer.
//
// ResourceRequest (op_code 19) root path: kernel splits from the
// target Space when slot 0 (handler) is empty.
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    // ResourceRequest: allocate 1 page of Space.
    mov x4, #19             // op_code: ResourceRequest
    mov x5, #3              // target: root Space (authorization + source)
    mov x0, #0              // resource type = Space
    mov x1, #1              // quantity = 1 page
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 1f       // fail if error
    mov x19, x0             // save first handle

    // Second ResourceRequest: verify repeatable.
    mov x4, #19
    mov x5, #3
    mov x0, #0
    mov x1, #1
    mov x2, #0
    mov x3, #0
    svc #0
    tbnz x0, #63, 1f
    mov x20, x0

    // Handles must be different.
    cmp x19, x20
    b.eq 1f

    brk #0x42               // PASS
1:
    brk #1                  // FAIL
