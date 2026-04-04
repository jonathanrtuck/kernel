/*
 * Minimal kernel linker script for QEMU virt (aarch64).
 *
 * Physical memory starts at 0x4000_0000. The kernel loads at 0x4008_0000
 * (512 KiB in, leaving room for DTB at RAM base).
 *
 * Everything runs at physical addresses for now. Upper-VA mapping, KASLR,
 * and W^X section permissions come later when the MMU setup lands.
 */

ENTRY(_start)

PHYS_BASE = 0x40080000;

SECTIONS
{
    . = PHYS_BASE;

    /* Boot code — entered first, must be at the load address. */
    .text.boot : {
        KEEP(*(.text.boot))
    }

    /* Kernel code. */
    . = ALIGN(16);
    __text_start = .;
    .text : {
        *(.text .text.*)
    }
    __text_end = .;

    /* Read-only data. */
    . = ALIGN(16);
    .rodata : {
        *(.rodata .rodata.*)
    }

    /* Read-write data. */
    . = ALIGN(16);
    .data : {
        *(.data .data.*)
    }

    /* Zero-initialized data. */
    . = ALIGN(16);
    __bss_start = .;
    .bss : {
        *(.bss .bss.*)
        *(COMMON)
    }
    __bss_end = .;

    /* Kernel stack (64 KiB). */
    . = ALIGN(16);
    .bss.stack (NOLOAD) : {
        __stack_bottom = .;
        . = . + 0x10000;
        __stack_top = .;
    }

    /* Discard unneeded sections. */
    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
        *(.comment .note.*)
    }

    . = ALIGN(16);
    __kernel_end = .;
}
