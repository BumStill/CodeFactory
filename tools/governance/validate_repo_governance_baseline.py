from __future__ import annotations

import json
import sys
from pathlib import Path

BASELINE_VERSION = "2026-08-25"
EXPECTED_BASELINE_ENTRIES = ['docs/long-tasks/README.md', 'docs/long-tasks/task-record-template.md', '.github/workflows/governed-delivery.yml', 'tools/governance/diagnose_release_latency.py', 'tools/governance/validate_long_task_record.py']
DEFAULT_REPO_PROFILE_MARKERS = ['Adopted Governance Version', 'Global Governance Sources', 'Propagation Policy', 'Surface ID', 'Path ID', 'Role Gate']


def make_failure(code: str, message: str, file: Path, missing_or_invalid: str, recommended_action: str) -> dict:
    return {
        "code": code,
        "severity": "blocker",
        "message": message,
        "file": str(file),
        "missing_or_invalid": missing_or_invalid,
        "recommended_action": recommended_action,
    }


def main() -> int:
    repo_root = Path(__file__).resolve().parents[2]
    manifest_path = repo_root / ".codex" / "governance" / "baseline-manifest.json"
    failures: list[dict] = []
    checked_items: list[str] = []

    if not manifest_path.exists():
        failures.append(
            make_failure(
                "BASELINE_MANIFEST_MISSING",
                "The governance baseline manifest is missing.",
                manifest_path,
                "baseline-manifest.json",
                "Restore .codex/governance/baseline-manifest.json.",
            )
        )
    else:
        manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
        checked_items.append("baseline_manifest")

        manifest_version = manifest.get("governance_version") or manifest.get("version")
        if manifest_version != BASELINE_VERSION:
            failures.append(
                make_failure(
                    "BASELINE_MANIFEST_VERSION_DRIFT",
                    "The governance baseline manifest is not on the active baseline version.",
                    manifest_path,
                    str(manifest_version),
                    f"Update the baseline manifest to governance version `{BASELINE_VERSION}`.",
                )
            )

        missing_manifest_entries = [entry for entry in EXPECTED_BASELINE_ENTRIES if entry not in manifest.get("required_files", [])]
        if missing_manifest_entries:
            failures.append(
                make_failure(
                    "BASELINE_MANIFEST_OUTDATED",
                    "The governance baseline manifest is missing governed-delivery assets.",
                    manifest_path,
                    ", ".join(missing_manifest_entries),
                    "Update the baseline manifest with the missing governed assets.",
                )
            )

        missing = []
        for relative_path in manifest.get("required_files", []):
            if not (repo_root / relative_path).exists():
                missing.append(relative_path)
        if missing:
            failures.append(
                make_failure(
                    "BASELINE_REQUIRED_FILES_MISSING",
                    "The repository is missing required governance baseline files.",
                    repo_root / "AGENTS.md",
                    ", ".join(missing),
                    "Restore the missing governance baseline files.",
                )
            )
        else:
            checked_items.append("required_file_presence")

        required_markers = dict(manifest.get("required_markers", {}))
        repo_profile_markers = manifest.get("repo_profile_markers", DEFAULT_REPO_PROFILE_MARKERS)
        if repo_profile_markers:
            required_markers.setdefault("docs/repo-governance-profile.md", repo_profile_markers)

        for relative_path, markers in required_markers.items():
            target = repo_root / relative_path
            if not target.exists():
                continue
            text = target.read_text(encoding="utf-8")
            missing_markers = [marker for marker in markers if marker not in text]
            if missing_markers:
                failures.append(
                    make_failure(
                        "BASELINE_MARKER_DRIFT",
                        f"Required governance markers are missing from {relative_path}.",
                        target,
                        ", ".join(missing_markers),
                        f"Restore the required governance markers in {relative_path}.",
                    )
                )
        checked_items.append("marker_checks")

    result = {
        "status": "fail" if failures else "pass",
        "governance_version": BASELINE_VERSION,
        "repo_root": str(repo_root),
        "failures": failures,
        "checked_items": checked_items,
    }
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
