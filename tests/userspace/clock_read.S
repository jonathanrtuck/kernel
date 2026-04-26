// Proves EL0 can read CNTVCT_EL0 (clock_access=true after boot).
//
// The root Observer has clock_access=true, so CNTKCTL_EL1.EL0VCTEN=1.
// Read the virtual counter and verify it is non-zero (the counter runs
// from reset and is always non-zero by the time EL0 code executes).
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL.

    mrs x0, CNTVCT_EL0      // read virtual counter
    cbz x0, 1f              // if zero, fail (should never happen)
    brk #0x42                // PASS
1:
    brk #1                   // FAIL — counter was zero
