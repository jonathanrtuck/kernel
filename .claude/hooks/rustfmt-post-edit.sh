#!/bin/bash
# Post-edit hook: runs rustfmt on .rs files after Edit or Write.
# Reads JSON from stdin (Claude Code PostToolUse protocol).

INPUT=$(cat)
FILE_PATH=$(echo "$INPUT" | jq -r '.tool_input.file_path // empty')

# Only act on Rust files
if [[ "$FILE_PATH" == *.rs ]]; then
    rustfmt --edition 2024 "$FILE_PATH" 2>/dev/null
fi

exit 0
