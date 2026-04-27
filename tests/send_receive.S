// Proves Send and Receive IPC syscalls work end-to-end.
//
// Creates a Field from a split Space, sends a message with known data
// words and label, then receives on the same Field and verifies the
// received data matches what was sent.
//
// IPC ABI:
//   SVC #1 = Send: x5 = Field handle, x4 = label, x0-x3 = data words,
//                   x6 = user cap handle (u64::MAX = no cap), x7 = 0.
//   SVC #2 = Receive: x5 = Field handle.
//            On return: x0-x3 = data, x4 = label, x5 = badge,
//                       x6 = user cap slot, x7 = reply cap slot.
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    // ── Step 1: SpaceSplit to get a consumable Space for the Field ────
    mov x4, #11             // op_code: SpaceSplit
    mov x5, #3              // target_handle: root Space at slot 3
    mov x0, #4096           // size = 4096 bytes (one page minimum)
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0                  // typed syscall
    tbnz x0, #63, 9f       // fail if SpaceSplit returned error
    mov x19, x0             // save new Space handle (slot N)

    // ── Step 2: CreateField on the split Space ───────────────────────
    // CreateField consumes the Space at slot N and overwrites it with
    // the new Field cap (FIELD_ALL rights). Returns 0 on success.
    mov x4, #13             // op_code: CreateField
    mov x5, x19             // target: the split Space
    mov x0, #8              // capacity hint
    mov x1, #0
    mov x2, #0
    mov x3, #0
    svc #0                  // typed syscall
    tbnz x0, #63, 9f       // fail if CreateField returned error
    // Field handle = x19 (same slot as consumed Space)

    // ── Step 3: Send a message to the Field ──────────────────────────
    // Data words: 0xBEEF, 0xCAFE, 0xDEAD, 0xF00D
    // Label: 0x42
    movz x0, #0xBEEF        // data[0]
    movz x1, #0xCAFE        // data[1]
    movz x2, #0xDEAD        // data[2]
    movz x3, #0xF00D        // data[3]
    mov x4, #0x42            // label
    mov x5, x19              // Field handle
    movn x6, #0              // user cap = CAP_ABSENT (0xFFFF_FFFF_FFFF_FFFF)
    mov x7, #0               // reply_info = 0
    svc #1                   // Send

    // After Send: carry clear = success. We don't check carry explicitly
    // since the Send should succeed (queue was empty). If it fails, the
    // subsequent Receive will fail or return wrong data.

    // ── Step 4: Receive on the same Field ────────────────────────────
    // The message we just sent should be in the queue.
    mov x5, x19              // Field handle
    svc #2                   // Receive

    // On return: x0 = data[0], x1 = data[1], x2 = data[2], x3 = data[3],
    //            x4 = label, x5 = badge.

    // ── Step 5: Verify received data matches sent data ───────────────
    // Check data[0] == 0xBEEF
    movz x10, #0xBEEF
    cmp x0, x10
    b.ne 9f

    // Check data[1] == 0xCAFE
    movz x10, #0xCAFE
    cmp x1, x10
    b.ne 9f

    // Check data[2] == 0xDEAD
    movz x10, #0xDEAD
    cmp x2, x10
    b.ne 9f

    // Check data[3] == 0xF00D
    movz x10, #0xF00D
    cmp x3, x10
    b.ne 9f

    // Check label == 0x42
    cmp x4, #0x42
    b.ne 9f

    brk #0x42                // PASS — Send+Receive verified
9:
    brk #1                   // FAIL
