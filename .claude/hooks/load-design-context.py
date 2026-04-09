#!/usr/bin/env python3
"""SessionStart hook: loads kernel design context into the conversation.

Reads design/tree.md (the design tree — source of truth for decisions)
and design/philosophy.md (principles and process).
"""

from pathlib import Path


def load_file(path: Path, label: str) -> list[str]:
    if not path.exists():
        return [f"{label}: {path} not found"]
    content = path.read_text().strip()
    return [
        f"── {label} ──",
        "",
        content,
        "",
    ]


def main():
    root = Path("design")

    lines = ["KERNEL DESIGN CONTEXT — loaded automatically at session start:", ""]

    # Design tree first — it's the working state
    lines.extend(load_file(root / "tree.md", "Design Tree (design/tree.md)"))

    # Philosophy second — principles and process
    lines.extend(load_file(root / "philosophy.md", "Philosophy (design/philosophy.md)"))

    lines.append("── end of design context ──")
    print("\n".join(lines))


if __name__ == "__main__":
    main()
