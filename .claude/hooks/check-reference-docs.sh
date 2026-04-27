#!/bin/bash
# Post-edit hook: reminds Claude to check reference docs when source-of-truth
# files are edited. Reads JSON from stdin (Claude Code PostToolUse protocol).
#
# Mapping derived from the "Source of truth" declarations in each reference doc.

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

[ -z "$FILE_PATH" ] && exit 0

# Strip the repo root to get a relative path for matching.
REL_PATH="${FILE_PATH#*/Sites/kernel/}"

# Map source files to the reference docs they inform.
# Each case accumulates into $DOCS (newline-separated).
DOCS=""

case "$REL_PATH" in
  src/syscall.rs)
    DOCS="design/reference/abi.md (ABI — register conventions, SVC encoding, error signaling)
design/reference/errors.md (error codes — SyscallError enum, error_code() values)
design/reference/syscalls.md (syscall reference — operation codes, arguments, return values)
design/reference/ipc.md (IPC — send/receive/call semantics, error conventions)"
    ;;
  src/frame/arch/aarch64/exception.S)
    DOCS="design/reference/abi.md (ABI — exception entry/exit, register save/restore)"
    ;;
  src/frame/cores.rs)
    DOCS="design/reference/abi.md (ABI — register read/write helpers)
design/reference/faults.md (faults — dispatch integration)"
    ;;
  src/frame/arch/aarch64/register_state.rs)
    DOCS="design/reference/abi.md (ABI — RegisterState layout)"
    ;;
  src/frame/boot.rs)
    DOCS="design/reference/boot.md (boot protocol — kernel init, root Observer creation)"
    ;;
  src/main.rs)
    DOCS="design/reference/boot.md (boot protocol — entry point, BSP init)"
    ;;
  src/frame/arch/aarch64/boot.S)
    DOCS="design/reference/boot.md (boot protocol — assembly entry, EL2→EL1 transition)"
    ;;
  src/fault.rs)
    DOCS="design/reference/faults.md (faults — fault types, message construction, delivery)"
    ;;
  src/core_manager.rs)
    DOCS="design/reference/faults.md (faults — dispatch integration, chain terminus)
design/reference/rights.md (rights — operation→right enforcement)
design/reference/syscalls.md (syscalls — dispatch logic, per-operation handling)
design/reference/ipc.md (IPC — send/receive/call dispatch paths)"
    ;;
  src/field.rs)
    DOCS="design/reference/ipc.md (IPC — Field semantics, queue behavior)
design/reference/objects.md (objects — Field type description, lifecycle)"
    ;;
  src/frame/fields.rs)
    DOCS="design/reference/ipc.md (IPC — Field queue internals, capacity)"
    ;;
  src/communication.rs)
    DOCS="design/reference/ipc.md (IPC — message structure, word layout)"
    ;;
  src/capability.rs)
    DOCS="design/reference/rights.md (rights — right bits, right masks, enforcement)
design/reference/objects.md (objects — CapTable type description, lifecycle)"
    ;;
  src/observer.rs)
    DOCS="design/reference/objects.md (objects — Observer type description, lifecycle)"
    ;;
  src/space.rs | src/space_manager.rs)
    DOCS="design/reference/objects.md (objects — Space type description, lifecycle)"
    ;;
  src/pulsar.rs)
    DOCS="design/reference/objects.md (objects — Pulsar type description, lifecycle)"
    ;;
  src/time.rs)
    DOCS="design/reference/objects.md (objects — Pulsar timer behavior)"
    ;;
  src/arena.rs)
    DOCS="design/reference/objects.md (objects — arena allocation, generation counters)"
    ;;
  src/frame/capabilities.rs)
    DOCS="design/reference/rights.md (rights — capability storage, lookup internals)"
    ;;
  src/frame/mapping.rs)
    DOCS="design/reference/objects.md (objects — Space mapping operations)"
    ;;
esac

[ -z "$DOCS" ] && exit 0

echo "⚠ Reference doc check: you edited $REL_PATH which is a source of truth for:"
echo "$DOCS" | while IFS= read -r line; do
  echo "  • $line"
done
echo "Check whether your changes affect anything described in those docs. If so, update them in the same pass."

exit 0
