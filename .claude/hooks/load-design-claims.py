#!/usr/bin/env python3
"""SessionStart hook: loads kernel design claims into the conversation context.

Reads design/claims.toml and formats them identically to the remote Company OS
claims hook, so both org-level and project-level claims appear in the same style.
"""

import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:
    print("kernel-claims: Python 3.11+ required for tomllib (have {}.{})".format(*sys.version_info[:2]),
          file=sys.stderr)
    sys.exit(0)


def main():
    # Find claims.toml relative to the repo root (CWD when Claude invokes hooks)
    claims_path = Path("design/claims.toml")
    if not claims_path.exists():
        # Try relative to this script's location as fallback
        claims_path = Path(__file__).resolve().parent.parent.parent / "design" / "claims.toml"

    if not claims_path.exists():
        print("kernel-claims: design/claims.toml not found", file=sys.stderr)
        return

    with open(claims_path, "rb") as f:
        data = tomllib.load(f)

    claims = data.get("claim", [])
    if not claims:
        print("Kernel design claims: none found.")
        return

    # Sort: canonical first, then signal, then working; within each, certain > high > medium
    status_order = {"canonical": 0, "signal": 1, "working": 2}
    confidence_order = {"certain": 0, "high": 1, "medium": 2}
    claims.sort(key=lambda c: (
        status_order.get(c.get("status", ""), 3),
        confidence_order.get(c.get("confidence", ""), 3),
    ))

    lines = [
        "KERNEL DESIGN CLAIMS (local Company OS) — loaded automatically at session start:",
        "",
    ]

    for c in claims:
        scope = ", ".join(c.get("scope", []))
        lines.append(f"[{c['status']}/{c['confidence']}] {c['statement']}")
        lines.append(f"  scope: {scope}  |  id: {c['id']}")
        lines.append("")

    lines.append(f"Total: {len(claims)} kernel design claims. Apply these throughout the session.")
    lines.append("Rationale for each claim is in design/claims.toml.")

    print("\n".join(lines))


if __name__ == "__main__":
    main()
