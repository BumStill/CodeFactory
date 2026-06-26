#!/usr/bin/env python3
"""Block commits whose branch does not include the latest default branch."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args],
        check=check,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )


def out(*args: str, check: bool = True) -> str:
    return git(*args, check=check).stdout.strip()


def fail(message: str) -> int:
    print(f"sync-gate: {message}", file=sys.stderr)
    return 1


def default_branch(remote: str) -> str:
    symbolic = out("symbolic-ref", "--quiet", "--short", f"refs/remotes/{remote}/HEAD", check=False)
    if symbolic.startswith(f"{remote}/"):
        return symbolic.split("/", 1)[1]
    for candidate in ("main", "master"):
        if out("rev-parse", "--verify", "--quiet", f"{remote}/{candidate}", check=False):
            return candidate
    return "main"


def main() -> int:
    if os.environ.get("CODEFACTORY_SKIP_SYNC_GATE") == "1":
        print("sync-gate: skipped via CODEFACTORY_SKIP_SYNC_GATE=1", file=sys.stderr)
        return 0

    try:
        repo_root = Path(out("rev-parse", "--show-toplevel"))
    except subprocess.CalledProcessError:
        return fail("not inside a git repository")

    os.chdir(repo_root)

    branch = out("symbolic-ref", "--quiet", "--short", "HEAD", check=False)
    if not branch:
        return fail("detached HEAD; create or switch to a branch before committing")

    remotes = out("remote", check=False).splitlines()
    remote = "origin" if "origin" in remotes else (remotes[0] if remotes else "")
    if not remote:
        return fail("no git remote configured; cannot verify latest default branch")

    base_branch = os.environ.get("CODEFACTORY_SYNC_BASE_BRANCH") or default_branch(remote)
    base_ref = f"{remote}/{base_branch}"

    fetch = git("fetch", "--prune", remote, base_branch, check=False)
    if fetch.returncode != 0:
        print(fetch.stderr.strip(), file=sys.stderr)
        return fail(f"could not fetch {base_ref}; sync before committing")

    if not out("rev-parse", "--verify", "--quiet", base_ref, check=False):
        return fail(f"{base_ref} does not exist after fetch")

    contains_base = git("merge-base", "--is-ancestor", base_ref, "HEAD", check=False)
    if contains_base.returncode == 0:
        print(f"sync-gate: OK; {branch} includes latest {base_ref}")
        return 0

    print(
        "\n".join(
            [
                f"sync-gate: {branch} does not include latest {base_ref}.",
                "",
                "Commit blocked. Sync and merge the default branch first, then rerun validation:",
                f"  git fetch --prune {remote} {base_branch}",
                f"  git merge {base_ref}",
                "",
                "If local changes prevent the merge, stash them, save a patch, or move the",
                "slice into a fresh worktree; then merge, resolve conflicts, rerun tests, and commit.",
                "",
                "Emergency/offline bypass, only when explicitly called out as hotfix bypass:",
                "  CODEFACTORY_SKIP_SYNC_GATE=1 git commit ...",
            ]
        ),
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
