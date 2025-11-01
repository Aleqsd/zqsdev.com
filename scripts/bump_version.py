#!/usr/bin/env python3
"""
Utility to bump the project version across workspace manifests.

Usage:
    python3 scripts/bump_version.py [patch|minor|major]

Defaults to a patch bump when no level is provided.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


VERSION_PATTERN = re.compile(r'^(version\s*=\s*")(?P<version>\d+\.\d+\.\d+)(")', re.MULTILINE)


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Bump workspace version.")
    parser.add_argument(
        "level",
        nargs="?",
        default="patch",
        choices=("patch", "minor", "major"),
        help="Version component to increment (default: patch).",
    )
    return parser.parse_args()


def read_current_version(version_file: Path, manifest: Path) -> str:
    if version_file.exists():
        current = version_file.read_text(encoding="utf-8").strip()
        if _is_valid_version(current):
            return current
        raise ValueError(f"Invalid version in {version_file}: {current!r}")

    manifest_text = manifest.read_text(encoding="utf-8")
    match = VERSION_PATTERN.search(manifest_text)
    if not match:
        raise ValueError(f"Unable to find version in {manifest}")
    return match.group("version")


def bump(version: str, level: str) -> str:
    parts = [int(part) for part in version.split(".")]
    if len(parts) != 3:
        raise ValueError(f"Unsupported version format: {version}")

    major, minor, patch = parts
    if level == "major":
        major += 1
        minor = 0
        patch = 0
    elif level == "minor":
        minor += 1
        patch = 0
    else:
        patch += 1

    return f"{major}.{minor}.{patch}"


def update_manifest(manifest: Path, new_version: str) -> None:
    original = manifest.read_text(encoding="utf-8")

    def replacer(match: re.Match[str]) -> str:
        return f"{match.group(1)}{new_version}{match.group(3)}"

    updated, count = VERSION_PATTERN.subn(replacer, original, count=1)
    if count == 0:
        raise ValueError(f"Failed to update version in {manifest}")
    manifest.write_text(updated, encoding="utf-8")


def write_version_file(version_file: Path, version: str) -> None:
    version_file.write_text(f"{version}\n", encoding="utf-8")


def _is_valid_version(candidate: str) -> bool:
    return bool(re.fullmatch(r"\d+\.\d+\.\d+", candidate))


def main() -> int:
    args = parse_args()
    repo_root = Path(__file__).resolve().parents[1]
    version_file = repo_root / "VERSION"
    workspace_manifest = repo_root / "Cargo.toml"
    server_manifest = repo_root / "server" / "Cargo.toml"

    try:
        current_version = read_current_version(version_file, workspace_manifest)
        new_version = bump(current_version, args.level)
        write_version_file(version_file, new_version)
        update_manifest(workspace_manifest, new_version)
        update_manifest(server_manifest, new_version)
    except Exception as exc:  # noqa: BLE001
        print(f"[bump-version] error: {exc}", file=sys.stderr)
        return 1

    print(f"[bump-version] {current_version} -> {new_version}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
