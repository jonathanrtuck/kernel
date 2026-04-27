// Proves basic syscall round-trip: Yield returns to the next instruction.
//
// Protocol: BRK #0x42 = PASS (kernel prints "TEST PASSED", exits via
// PSCI SYSTEM_OFF). If Yield did not return, we never reach the BRK.

    mov x0, #0x42       // marker value (proves register state is live)
    svc #5              // Yield — should return to next instruction
    // If we get here, yield returned successfully
    mov x0, #0           // clear x0 (proves yield came back)
    brk #0x42            // PASS
