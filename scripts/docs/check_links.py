#!/usr/bin/env python3
"""Check relative links and anchors in repository Markdown files.

This is intentionally dependency-free: it runs in CI without a network fetch
and does not check external URLs. It validates that every local link resolves
to an existing file or directory and, for Markdown targets, that the fragment
matches a heading anchor.

Usage:
    python3 scripts/docs/check_links.py [--root PATH]
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import urllib.parse
from pathlib import Path

SKIP_SCHEMES = ("http://", "https://", "mailto:", "tel:", "ssh:", "data:")
INLINE_CODE_RE = re.compile(r"`[^`\n]*`")
LINK_RE = re.compile(
    r"!?\[[^\]\n]*\]\(\s*(<[^>\n]+>|[^)\s]+)(?:\s+[\"'][^\"']*[\"'])?\s*\)"
)
HEADING_RE = re.compile(r"^#{1,6}\s+(.*?)\s*#*\s*$", re.MULTILINE)


def repo_markdown(root: Path) -> list[Path]:
    """Return tracked Markdown files, falling back to a filesystem walk."""
    try:
        output = subprocess.check_output(
            [
                "git",
                "-C",
                str(root),
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
                "*.md",
            ],
            stderr=subprocess.DEVNULL,
        )
        files = [root / name for name in output.decode().split("\0") if name]
        if files:
            return sorted(files)
    except (OSError, subprocess.CalledProcessError):
        pass

    ignored = {".git", "target", "node_modules", ".lynx"}
    return sorted(
        path
        for path in root.rglob("*.md")
        if not any(part in ignored for part in path.relative_to(root).parts)
    )


def strip_code(text: str) -> str:
    """Remove fenced and inline code so examples are not treated as links."""
    kept: list[str] = []
    fence: str | None = None
    for line in text.splitlines():
        stripped = line.lstrip()
        if fence is None and (stripped.startswith("```") or stripped.startswith("~~~")):
            fence = stripped[:3]
            kept.append("")
            continue
        if fence is not None:
            if stripped.startswith(fence):
                fence = None
            kept.append("")
            continue
        kept.append(INLINE_CODE_RE.sub("", line))
    return "\n".join(kept)


def slugify(heading: str) -> str:
    """Approximate GitHub's heading-anchor algorithm."""
    text = re.sub(r"<[^>]+>", "", heading)
    text = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", text)
    text = text.replace("`", "").replace("*", "").replace("~", "")
    text = text.lower().strip()
    text = re.sub(r"[^\w\s-]", "", text, flags=re.UNICODE)
    text = re.sub(r"\s+", "-", text)
    return text


def anchors_for(path: Path) -> set[str]:
    headings = HEADING_RE.findall(strip_code(path.read_text(encoding="utf-8")))
    seen: dict[str, int] = {}
    anchors: set[str] = set()
    for heading in headings:
        base = slugify(heading)
        count = seen.get(base, 0)
        anchors.add(base if count == 0 else f"{base}-{count}")
        seen[base] = count + 1
    return anchors


def check_file(path: Path, root: Path) -> list[str]:
    errors: list[str] = []
    text = strip_code(path.read_text(encoding="utf-8"))
    for raw_target in LINK_RE.findall(text):
        target = raw_target.strip("<>").strip()
        if not target or target.startswith("#"):
            target_path, fragment = "", target[1:] if target.startswith("#") else ""
        elif target.lower().startswith(SKIP_SCHEMES):
            continue
        else:
            target_path, _, fragment = target.partition("#")

        if target_path:
            decoded = urllib.parse.unquote(target_path)
            if decoded.startswith("/"):
                # Site-root links are not repository-relative; skip them.
                continue
            resolved = (path.parent / decoded).resolve()
            try:
                resolved.relative_to(root.resolve())
            except ValueError:
                errors.append(
                    f"{path.relative_to(root)}: link escapes the repository: {target}"
                )
                continue
            if not resolved.exists():
                errors.append(f"{path.relative_to(root)}: missing target: {target}")
                continue
        else:
            resolved = path

        if not fragment or resolved.suffix.lower() != ".md" or not resolved.is_file():
            continue
        wanted = urllib.parse.unquote(fragment).lower()
        if wanted.startswith("user-content-"):
            wanted = wanted[len("user-content-") :]
        if wanted not in anchors_for(resolved):
            errors.append(
                f"{path.relative_to(root)}: missing anchor #{fragment} in "
                f"{resolved.relative_to(root)}"
            )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        default=Path(__file__).resolve().parents[2],
        type=Path,
        help="repository root (default: two levels above this script)",
    )
    args = parser.parse_args()
    root = args.root.resolve()

    files = repo_markdown(root)
    if not files:
        print("link check: no Markdown files found", file=sys.stderr)
        return 1

    errors: list[str] = []
    for path in files:
        errors.extend(check_file(path, root))

    if errors:
        print(f"link check: {len(errors)} problem(s) in {len(files)} file(s)")
        for error in errors:
            print(f"  {error}")
        return 1

    print(f"link check: OK ({len(files)} Markdown files)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
