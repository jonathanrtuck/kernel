// Stress test: heavy register and ALU pressure across a single Yield.
//
// Loads all 29 general-purpose registers (x0-x28) with distinct
// non-trivial values, executes a Yield syscall, then verifies every
// register survived the kernel round-trip. This exercises the full
// register save/restore path under maximum register pressure — any
// missing save or restore in the exception vector or context switch
// code will corrupt at least one value.
//
// Note: only one Yield per test because the kernel's round-robin
// scheduler does not yet dequeue-before-re-enqueue for the single-
// observer case (double-enqueue panic). The value of this test is
// width (all registers) rather than depth (many iterations).
//
// Protocol: BRK #0x42 = PASS, BRK #1 = FAIL (register corruption).

    // -- Load all GPRs with distinct values --
    // Pattern: x(N) = 0x100 + N, making corruption easy to identify.
    mov x0, #0x100
    mov x1, #0x101
    mov x2, #0x102
    mov x3, #0x103
    mov x4, #0x104
    mov x5, #0x105
    mov x6, #0x106
    mov x7, #0x107
    mov x8, #0x108
    mov x9, #0x109
    mov x10, #0x10A
    mov x11, #0x10B
    mov x12, #0x10C
    mov x13, #0x10D
    mov x14, #0x10E
    mov x15, #0x10F
    mov x16, #0x110
    mov x17, #0x111
    mov x18, #0x112
    mov x19, #0x113
    mov x20, #0x114
    mov x21, #0x115
    mov x22, #0x116
    mov x23, #0x117
    mov x24, #0x118
    mov x25, #0x119
    mov x26, #0x11A
    mov x27, #0x11B
    mov x28, #0x11C

    // -- Yield: kernel must save and restore all of x0-x28 --
    svc #5

    // -- Verify every register --
    cmp x0, #0x100
    b.ne 1f
    cmp x1, #0x101
    b.ne 1f
    cmp x2, #0x102
    b.ne 1f
    cmp x3, #0x103
    b.ne 1f
    cmp x4, #0x104
    b.ne 1f
    cmp x5, #0x105
    b.ne 1f
    cmp x6, #0x106
    b.ne 1f
    cmp x7, #0x107
    b.ne 1f
    cmp x8, #0x108
    b.ne 1f
    cmp x9, #0x109
    b.ne 1f
    cmp x10, #0x10A
    b.ne 1f
    cmp x11, #0x10B
    b.ne 1f
    cmp x12, #0x10C
    b.ne 1f
    cmp x13, #0x10D
    b.ne 1f
    cmp x14, #0x10E
    b.ne 1f
    cmp x15, #0x10F
    b.ne 1f
    cmp x16, #0x110
    b.ne 1f
    cmp x17, #0x111
    b.ne 1f
    cmp x18, #0x112
    b.ne 1f
    cmp x19, #0x113
    b.ne 1f
    cmp x20, #0x114
    b.ne 1f
    cmp x21, #0x115
    b.ne 1f
    cmp x22, #0x116
    b.ne 1f
    cmp x23, #0x117
    b.ne 1f
    cmp x24, #0x118
    b.ne 1f
    cmp x25, #0x119
    b.ne 1f
    cmp x26, #0x11A
    b.ne 1f
    cmp x27, #0x11B
    b.ne 1f
    cmp x28, #0x11C
    b.ne 1f

    brk #0x42               // PASS — all 29 GPRs preserved across Yield

1:
    brk #1                  // FAIL — register corrupted across Yield
