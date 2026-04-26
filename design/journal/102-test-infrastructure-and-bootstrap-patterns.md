# D102 — Test infrastructure and bootstrap patterns

**Question:** How does the kernel load and run test binaries, and how does a
root Observer bootstrap child Observers for multi-Observer tests?

**Rests on:** D24 (cap-mapping invariant — InstallCap IS mapping), D26
(kernel-managed VA), D31 (boot architecture — DTB discovery), D32 (type
conversion — Space becomes object), D35 (composable Observer setup — 5-step
sequence), A3 (generic kernel — workload-independent), A5 (kernel absorbs
complexity — no format parsing in kernel).

**Status:** settled.

---

## Settles

### Test binary format: flat binary

Test binaries are flat binary — raw ARM64 code with entry point at offset 0. No
ELF header, no format parsing in the kernel. The kernel maps the binary's
physical pages into the root Observer's address space and sets PC to the VA
base.

This is consistent with the microkernel pattern. No surveyed microkernel parses
ELF inside the kernel proper:

| System | Where ELF is parsed                                   |
| ------ | ----------------------------------------------------- |
| seL4   | Pre-kernel `elfloader` (separate binary, runs at EL2) |
| Zircon | `userboot` shim (first userspace process)             |
| L4Re   | `Moe` (first userspace component)                     |
| Genode | `Core` (first userspace component)                    |
| EROS   | Checkpoint image loader (pre-kernel)                  |

An ELF parser would add ~500-1000 LOC to the kernel's trusted computing base. A5
says the kernel absorbs complexity for its users — but ELF parsing is
workload-specific complexity, not essential kernel complexity. The kernel
provides the mechanism (memory mapping, register setup); the format is a
userspace concern.

### Test binary packaging: hypervisor + DTB

The hypervisor loads test binaries into guest RAM and describes them via a DTB
module node (standard ARM64 `/chosen` convention). The kernel discovers the
binary's physical address and size by parsing the DTB module entry.

```text
/chosen {
    module@<addr> {
        compatible = "multiboot,module";
        reg = <addr size>;
    };
};
```

Different tests = different binaries loaded by the hypervisor. The kernel binary
is unchanged across tests. This separation is important:

- **No per-test kernel rebuild.** The hypervisor loads a different test binary
  into guest RAM; the same kernel image discovers it via DTB.
- **Self-describing.** The DTB module node carries address and size — no
  hard-coded assumptions in the kernel about where the binary lives.
- **Standard convention.** ARM64 firmware already uses `/chosen` for passing
  boot-time module information. The kernel's firmware layer (`frame/firmware/`)
  already parses DTB.

### Multi-Observer test bootstrap: 5-step composable sequence

Confirmed from D35. The root Observer creates child Observers using the standard
capability operations — no special kernel support needed beyond the existing
typed operations:

1. **SpaceSplit** — root Observer splits its Space to create backing for child
2. **CreateObserver(space_cap, handler_field_cap, badge)** — child created inert
3. **ObserverInstallCap(child_cap, code_space_cap, slot)** — maps code into
   child's address space (D24: installing a Space cap IS creating the mapping)
4. **ObserverWriteRegisters(child_cap, register_state)** — sets PC, SP
5. **ObserverResume(child_cap)** — child transitions Inert to Runnable

For IPC between root and child: root creates a Field (`CreateField`), installs
the Field's send cap into the child (`ObserverInstallCap`), keeps the receive
cap. Or creates two Fields for bidirectional communication.

This is the standard capability-based bootstrap pattern — the same mechanism
that a real init process would use. The test infrastructure validates the
production bootstrap path.

---

## Rejected alternatives

**ELF format:** Adds ~500-1000 LOC parser to the kernel TCB. Every surveyed
microkernel avoids this. The parser would need to handle section headers,
program headers, relocations, and validation — all complexity that belongs
outside the kernel. A3 (generic) means the kernel does not assume any particular
executable format.

**Embedded binary (`include_bytes!`):** Couples the kernel image to a specific
test workload. Every test change requires a kernel rebuild. Violates A3 (generic
kernel — workload-independent). The kernel binary should be stable across
different test scenarios.

**Hypervisor-injected binaries without DTB:** Not self-describing. The kernel
would need hard-coded assumptions about where the binary lives in RAM. DTB
module nodes are the ARM64 standard for passing this information and are already
parsed by the kernel's firmware layer.

**Per-test kernel rebuild:** Wasteful and slow. Embedding the test binary (via
linker script or `include_bytes!`) means recompiling the kernel for every test
change. The hypervisor-loads-binary approach keeps kernel compilation and test
compilation independent.

**Kernel-internal ELF loader with "just headers":** Even a minimal ELF header
parser (PT_LOAD segments only) is ~200 LOC with error handling and validation
against malformed input. The flat binary approach achieves zero parsing LOC in
the kernel. The simplicity gap is not worth closing — `objcopy -O binary`
produces flat binaries from ELF trivially.

---

## Does NOT settle

- DTB module node format details (exact property names, multiple module support)
- Root Observer's initial Space size (how much memory for splitting to children)
- Test binary toolchain (how flat binaries are produced — `objcopy`, custom
  linker script, or `cargo` configuration)
- Multi-binary test packaging (multiple test binaries in a single hypervisor
  run)
- Test result reporting mechanism (how test pass/fail status reaches the host)
