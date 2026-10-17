#!/usr/bin/env python3
"""Run a test command, retrying once when a quarantined flaky test fails.

Req E1-E4 of docs/specs/feature-specs/ci-workflow-lint-and-flaky-quarantine.md.

Rules:
  * The command always runs at least once. The quarantine list can never skip
    or deselect a test.
  * Only a failure whose output actually contains the name of an entry scoped
    to the current job is retried, and only once, in this same job.
  * A retry that passes makes the step pass and is recorded in the job summary
    as "flaky, retried once".
  * Any other failure propagates the original exit code.
  * An expired or malformed entry aborts before the command runs (fail closed).
"""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

REQUIRED_FIELDS = ("test", "job", "evidence", "expires_on")
DEFAULT_QUARANTINE = "docs/testing/flaky-quarantine.json"


def load_entries(path: Path, today: dt.date) -> tuple[list[dict], list[str]]:
    """Return (entries, errors). Expired/malformed entries are errors."""
    errors: list[str] = []
    try:
        payload = json.loads(path.read_text(encoding="utf-8"))
    except FileNotFoundError:
        return [], [f"{path}: quarantine file not found"]
    except json.JSONDecodeError as exc:
        return [], [f"{path}: JSON parse error: {exc}"]

    if not isinstance(payload, dict):
        return [], [f"{path}: expected a JSON object"]
    raw_entries = payload.get("entries")
    if not isinstance(raw_entries, list):
        return [], [f"{path}: 'entries' must be a list"]

    entries: list[dict] = []
    for index, entry in enumerate(raw_entries):
        where = f"{path}: entries[{index}]"
        if not isinstance(entry, dict):
            errors.append(f"{where}: expected an object")
            continue
        missing = [field for field in REQUIRED_FIELDS if not str(entry.get(field, "")).strip()]
        if missing:
            errors.append(f"{where}: missing required field(s): {', '.join(missing)}")
            continue
        try:
            expires_on = dt.date.fromisoformat(entry["expires_on"])
        except (TypeError, ValueError):
            errors.append(f"{where}: expires_on must be an ISO date (YYYY-MM-DD)")
            continue
        if expires_on < today:
            errors.append(
                f"{where}: expired on {expires_on.isoformat()} (test "
                f"{entry['test']!r}) - fix the test or remove/refresh the entry with fresh evidence"
            )
            continue
        entries.append(entry)
    return entries, errors


def resolve_command(command: list[str]) -> list[str]:
    """On Windows, .cmd/.bat shims (pnpm, npm) need cmd.exe to be executable."""
    executable = shutil.which(command[0])
    if executable is None:
        return command
    if os.name == "nt" and executable.lower().endswith((".cmd", ".bat")):
        return ["cmd", "/c", executable, *command[1:]]
    return [executable, *command[1:]]


def run_streaming(command: list[str], label: str) -> tuple[int, str]:
    print(f"[flaky-retry] {label}: {' '.join(command)}", flush=True)
    process = subprocess.Popen(
        resolve_command(command),
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        bufsize=1,
    )
    chunks: list[str] = []
    assert process.stdout is not None
    for line in process.stdout:
        sys.stdout.write(line)
        chunks.append(line)
    process.wait()
    return process.returncode, "".join(chunks)


def append_summary(path: Path | None, text: str) -> None:
    if path is None:
        return
    try:
        with path.open("a", encoding="utf-8") as handle:
            handle.write(text.rstrip("\n") + "\n")
    except OSError as exc:  # a broken summary must not mask the test result
        print(f"[flaky-retry] WARNING: could not write job summary: {exc}", file=sys.stderr)


def split_command(raw: list[str]) -> tuple[list[str], list[str]]:
    if "--" in raw:
        index = raw.index("--")
        return raw[:index], raw[index + 1 :]
    return raw, []


def main(argv: list[str] | None = None) -> int:
    raw = list(sys.argv[1:] if argv is None else argv)
    options, command = split_command(raw)

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--quarantine", default=DEFAULT_QUARANTINE)
    parser.add_argument("--job", default=None, help="CI job name used to scope entries")
    parser.add_argument(
        "--summary-file",
        default=os.environ.get("GITHUB_STEP_SUMMARY") or None,
        help="markdown file for the job summary (defaults to $GITHUB_STEP_SUMMARY)",
    )
    parser.add_argument(
        "--check-only",
        action="store_true",
        help="validate the quarantine file and exit without running tests",
    )
    args = parser.parse_args(options)

    today = dt.datetime.now(dt.timezone.utc).date()
    entries, errors = load_entries(Path(args.quarantine), today)
    if errors:
        for message in errors:
            print(f"ERROR: {message}", file=sys.stderr)
        print("FAILED: invalid flaky quarantine list", file=sys.stderr)
        return 2

    if args.check_only:
        print(f"OK: {len(entries)} flaky quarantine entr{'y' if len(entries) == 1 else 'ies'} valid")
        return 0

    if not command:
        print("ERROR: no test command given after '--'", file=sys.stderr)
        return 2

    summary_path = Path(args.summary_file) if args.summary_file else None
    scoped = [entry for entry in entries if args.job is None or entry["job"] == args.job]

    code, output = run_streaming(command, "run")
    if code == 0:
        return 0

    matched = [entry for entry in scoped if entry["test"] in output]
    if not matched:
        print(
            "[flaky-retry] no quarantined flaky test matched this failure; "
            f"propagating exit code {code}",
            file=sys.stderr,
        )
        return code if code > 0 else 1

    names = ", ".join(entry["test"] for entry in matched)
    print(
        f"::warning title=Flaky test quarantined::Retrying once because the failure matched: {names}",
        flush=True,
    )

    retry_code, _ = run_streaming(command, "retry 1/1 (quarantined flaky)")
    if retry_code == 0:
        for entry in matched:
            append_summary(
                summary_path,
                "- Flaky, retried once, passed in job "
                f"`{entry['job']}`: `{entry['test']}` "
                f"(expires {entry['expires_on']}; evidence: {entry['evidence']})",
            )
        print(f"[flaky-retry] PASSED after one retry (flaky, quarantined): {names}")
        return 0

    for entry in matched:
        append_summary(
            summary_path,
            "- Flaky entry still failing after one retry in job "
            f"`{entry['job']}`: `{entry['test']}` (expires {entry['expires_on']})",
        )
    print(
        f"[flaky-retry] FAILED: quarantined test still failed on the retry: {names}",
        file=sys.stderr,
    )
    return retry_code if retry_code > 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
