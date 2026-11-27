#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Release-supply and host-git guards.

Fails when a workflow uses a floating action tag, when the signing key appears
outside the sign-updater sign step, or when production Rust constructs a git
process outside the host_git builder.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
WORKFLOWS = ROOT / ".github" / "workflows"
SHA = re.compile(r"^[0-9a-f]{40}$")
USES = re.compile(r"^\s*-?\s*uses:\s+(\S+)")
SIGNING_NAMES = ("TAURI_SIGNING_PRIVATE_KEY", "TAURI_SIGNING_PRIVATE_KEY_PASSWORD")
GIT_SPAWN = re.compile(r"""Command::new\(\s*"git"\s*\)|Command::new\(\s*resolve_developer_command\(\s*"git"\s*\)""")


def workflow_errors(text: str, path: str) -> list[str]:
    errors = []
    for line_no, line in enumerate(text.splitlines(), 1):
        match = USES.search(line.split("#", 1)[0])
        if not match:
            continue
        uses = match.group(1)
        if uses.startswith("./") or uses.startswith("docker://"):
            continue
        ref = uses.rsplit("@", 1)[-1] if "@" in uses else ""
        if not SHA.fullmatch(ref):
            errors.append(f"{path}:{line_no}: action ref is not a full SHA: {uses}")
    return errors


def signing_errors(release_text: str) -> list[str]:
    errors = []
    jobs = re.split(r"\n  (?=[A-Za-z0-9_-]+:\n)", "\n" + release_text)
    for job in jobs:
        header = job.splitlines()[0].strip() if job.strip() else ""
        name = header[:-1]
        if name in {"build-windows", "build-macos"}:
            if "contents: read" not in job:
                errors.append(f"{name} must set permissions contents: read")
            for secret in SIGNING_NAMES:
                if secret in job:
                    errors.append(f"{name} must not reference {secret}")
        if name != "sign-updater":
            continue
        steps = re.split(r"\n(?=      - )", job)
        for step in steps:
            if not any(secret in step for secret in SIGNING_NAMES):
                continue
            if "name: Sign updater artifacts" not in step:
                errors.append("signing secrets may appear only in the Sign updater artifacts step")
    return errors


def strip_test_modules(text: str) -> str:
    lines = text.splitlines()
    out = []
    depth = 0
    skipping = 0
    pending_test = False
    for line in lines:
        stripped = line.strip()
        if stripped.startswith("#[cfg(test)]") or stripped == "#[cfg(test)]":
            pending_test = True
            continue
        if pending_test and stripped.startswith("mod ") and "{" in stripped:
            pending_test = False
            skipping = 1
            depth = stripped.count("{") - stripped.count("}")
            if depth <= 0:
                skipping = 0
            continue
        pending_test = False
        if skipping:
            depth += stripped.count("{") - stripped.count("}")
            if depth <= 0:
                skipping = 0
            continue
        out.append(line)
    return "\n".join(out)


def git_spawn_errors() -> list[str]:
    errors = []
    root = ROOT / "src-tauri" / "src"
    for path in root.rglob("*.rs"):
        if path.name == "host_git.rs":
            continue
        production = strip_test_modules(path.read_text(encoding="utf-8"))
        for line_no, line in enumerate(production.splitlines(), 1):
            if GIT_SPAWN.search(line):
                errors.append(f"{path.relative_to(ROOT)}:{line_no}: direct git process construction")
    return errors


def main() -> int:
    errors: list[str] = []
    for path in sorted(WORKFLOWS.glob("*.yml")):
        errors.extend(workflow_errors(path.read_text(encoding="utf-8"), str(path.relative_to(ROOT))))
    release = (WORKFLOWS / "release.yml").read_text(encoding="utf-8")
    errors.extend(signing_errors(release))
    errors.extend(git_spawn_errors())
    if errors:
        print("\n".join(errors))
        return 1
    print("supply-chain guards passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
