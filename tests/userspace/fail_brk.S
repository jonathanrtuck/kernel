// Negative test: proves the runner detects failures.
//
// Protocol: BRK #N (N != 0x42) triggers a fault that the root Observer
// cannot handle (no handler cap), so the kernel prints "FATAL FAULT"
// and exits via PSCI SYSTEM_OFF. The runner must detect this as FAIL.

    brk #1               // deliberate FAIL
