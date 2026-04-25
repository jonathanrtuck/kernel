# frame/firmware/

Boot-time data parsing. Untrusted input from firmware or the bootloader.

## Threat model

Firmware data (DTB blobs, future ACPI tables, UEFI memory maps) is **untrusted
external input**. It may be malformed, truncated, or adversarial. Every field
read must be bounds-checked. Every pointer or offset derived from firmware data
must be validated before use.

This is a fundamentally different threat model from the rest of frame/ — `arch/`
trusts the hardware (it programs it), but `firmware/` does not trust the data
(it parses it). Treat this code like a format parser, not like a driver.

## Current modules

- `dtb.rs` — Device Tree Blob parser. Extracts memory layout, core topology,
  device base addresses, timer frequency.

## Future additions

- ACPI table parser (if/when ACPI targets are added)
- UEFI memory map parser
- Any other boot-time data source

## Rules

- **Validate all offsets and sizes** before using them to index into the blob.
  An out-of-bounds read in DTB parsing is a kernel vulnerability — the DTB is
  provided by the hypervisor/bootloader and could be crafted.
- **No assumptions about field presence.** Missing DTB nodes should produce
  clear errors, not panics or undefined behavior.
- **Architecture-independent.** DTB parsing is the same regardless of the CPU.
  Device base addresses and topology are arch-specific _data_ extracted from an
  arch-independent _format_. The parser lives here; the interpretation lives in
  `arch/`.
