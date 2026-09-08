#!/usr/bin/env python3
"""Validate and publish only the narrow observer slice; never claim full E2E."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import stat
import tempfile

MISSING = frozenset({"native_theme_click", "native_text_input", "restart_theme_persistence",
                     "owned_window_screenshots", "descendant_process_cleanup", "world_directory_cleanup",
                     "request_observation", "credential_observation", "embedded_build_identity"})
ROOT_KEYS = frozenset({"schema_version", "scope", "run_id", "identifier", "expected_build_sha",
                       "build_identity_source", "source_executable_sha256", "copy_executable_sha256",
                       "driver_sha256", "process", "observer_slice", "cleanup", "full_probe",
                       "request_count", "credential_access_count"})


def keys(value, expected):
    return type(value) is dict and set(value) == set(expected)


def matches(value, pattern):
    return type(value) is str and re.fullmatch(pattern, value) is not None


def validate_receipt(value, expected_build_sha):
    errors = set()
    def check(condition, code):
        if not condition:
            errors.add(code)
    if not keys(value, ROOT_KEYS):
        return ["invalid_receipt_fields"]
    check(type(value["schema_version"]) is int and value["schema_version"] == 1
          and value["scope"] == "native-desktop-observer-slice", "invalid_scope")
    run = value["run_id"]
    check(matches(run, r"[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}"), "invalid_run")
    identifier = "com.codefactory.scenario." + (run.replace("-", "") if type(run) is str else "")
    check(value["identifier"] == identifier, "invalid_identifier")
    check(matches(expected_build_sha, r"[0-9a-f]{40}") and value["expected_build_sha"] == expected_build_sha,
          "wrong_expected_build")
    check(value["build_identity_source"] == "ci_input_unverified_in_binary", "build_provenance_overclaim")
    for field in ["source_executable_sha256", "copy_executable_sha256", "driver_sha256"]:
        check(matches(value[field], r"[0-9a-f]{64}"), "invalid_digest")
    check(value["source_executable_sha256"] == value["copy_executable_sha256"], "copied_binary_mismatch")
    process = value["process"]
    if not keys(process, {"run_id", "pid", "start_token", "executable_sha256", "bundle_id"}):
        errors.add("invalid_process_fields")
    else:
        check(process["run_id"] == run and process["bundle_id"] == identifier
              and process["executable_sha256"] == value["copy_executable_sha256"], "process_identity_mismatch")
        check(type(process["pid"]) is int and 1 < process["pid"] <= 2**31 - 1, "invalid_pid")
        token = process["start_token"]
        check(matches(token, r"[1-9][0-9]{0,18}:(?:0|[1-9][0-9]{0,5})"), "invalid_birth_token")
    observer = value["observer_slice"]
    if not keys(observer, {"status", "reason_codes", "ax_window_seen", "settings_control"}):
        errors.add("invalid_observer_fields")
    else:
        check(observer["status"] == "passed" and observer["reason_codes"] == [], "observer_not_passed")
        check(observer["ax_window_seen"] is True and observer["settings_control"] is True, "native_controls_unproven")
    cleanup = value["cleanup"]
    if not keys(cleanup, {"child_reaped", "signal_attempts", "world_directory"}):
        errors.add("invalid_cleanup_fields")
    else:
        check(cleanup["child_reaped"] is True, "owned_child_not_reaped")
        check(type(cleanup["signal_attempts"]) is int and 0 <= cleanup["signal_attempts"] <= 2,
              "invalid_signal_count")
        check(cleanup["world_directory"] == "retained", "directory_cleanup_overclaim")
    full = value["full_probe"]
    if not keys(full, {"status", "missing"}):
        errors.add("invalid_full_probe_fields")
    else:
        missing = full["missing"]
        check(full["status"] == "blocked", "full_probe_overclaim")
        check(type(missing) is list and all(type(item) is str for item in missing)
              and len(missing) == len(MISSING) and set(missing) == MISSING, "missing_scope_limits")
    check(value["request_count"] is None and value["credential_access_count"] is None,
          "unobserved_counts_forged")
    return sorted(errors)


def unique_object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError("duplicate JSON field")
        value[key] = item
    return value


def publish_validated_receipt(raw: Path, public: Path, expected_build_sha: str) -> bool:
    accepted = False
    result = {"schema_version": 1, "scope": "native-desktop-observer-validation",
              "status": "failed", "reason_codes": ["receipt_not_accepted"]}
    try:
        info = raw.lstat()
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1 or info.st_size > 65536:
            raise ValueError("invalid input file")
        descriptor = os.open(raw, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
        with os.fdopen(descriptor, "rb") as stream:
            opened = os.fstat(stream.fileno())
            if (opened.st_dev, opened.st_ino) != (info.st_dev, info.st_ino):
                raise ValueError("input identity changed")
            content = stream.read(65537)
        if len(content) > 65536:
            raise ValueError("oversized input")
        value = json.loads(content.decode("utf-8"), object_pairs_hook=unique_object)
        if not validate_receipt(value, expected_build_sha):
            accepted, result = True, value
    except (OSError, ValueError, TypeError, UnicodeError, RecursionError):
        pass  # Never echo a rejected value or exception containing a private path.
    public.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    if public.parent.is_symlink() or public.is_symlink() or raw.absolute() == public.absolute():
        raise ValueError("unsafe publication destination")
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", encoding="utf-8", dir=public.parent,
                                         prefix=".native-observer-", delete=False) as stream:
            temporary = Path(stream.name)
            json.dump(result, stream, sort_keys=True, separators=(",", ":"))
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, public)
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)
    return accepted


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--receipt", required=True, type=Path)
    parser.add_argument("--public-output", required=True, type=Path)
    parser.add_argument("--expected-build-sha", required=True)
    args = parser.parse_args()
    try:
        accepted = publish_validated_receipt(args.receipt, args.public_output, args.expected_build_sha)
        if output := os.environ.get("GITHUB_OUTPUT"):
            with Path(output).open("a", encoding="utf-8") as stream:
                stream.write("public_receipt_ready=true\n")
    except (OSError, ValueError):
        print("native-observer-publication: failed")
        return 1
    print("native-observer-validation: " + ("observer-only passed; full probe blocked" if accepted else "failed"))
    return 0 if accepted else 1


if __name__ == "__main__":
    raise SystemExit(main())
