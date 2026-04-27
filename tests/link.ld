/* Minimal linker script for userspace test binaries.
 *
 * The kernel loads the flat binary at USER_CODE_VA and sets PC to offset 0.
 * Stack is pre-allocated by the kernel at USER_STACK_TOP. No runtime init
 * needed — _start is the first instruction executed.
 */

ENTRY(_start)

SECTIONS {
    . = 0;
    .text : { *(.text.start) *(.text .text.*) }
    .rodata : { *(.rodata .rodata.*) }
    .data : { *(.data .data.*) }
    .bss : { *(.bss .bss.* COMMON) }
    /DISCARD/ : { *(.eh_frame) *(.eh_frame_hdr) *(.comment) *(.note*) *(.ARM.exidx*) }
}
