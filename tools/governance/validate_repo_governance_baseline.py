from __future__ import annotations

import argparse
import importlib.util
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


def _acceptance_ledger_problems(repo_root: Path, body_file: str | None, base_ref: str | None) -> list[str]:
    """M162 R2：PR 引用的规格（或已存在的账本）必须逐条有状态与证据。

    与交付预检共用 `validate_acceptance_ledger.py` 的唯一实现——基线侧不另写一套判断。
    """
    module_path = Path(__file__).resolve().parent / "validate_acceptance_ledger.py"
    if not module_path.exists():
        return [f"缺少验收账本校验器：{module_path}"]
    spec = importlib.util.spec_from_file_location("validate_acceptance_ledger", module_path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    report = module.build_report(repo_root, module.read_body(body_file), base_ref)
    return list(report["problems"])


def main(argv: list[str] | None = None) -> int:
    # M162: 账本校验可以带 --body-file / --base-ref；缺省时从 GITHUB_EVENT_PATH 取 PR 正文，
    # 两者都没有（push 到 main）则退化为"已存在账本的自洽检查"。
    parser = argparse.ArgumentParser(add_help=True)
    parser.add_argument("--body-file", default=None)
    parser.add_argument("--base-ref", default=None)
    arguments, _ = parser.parse_known_args(argv)

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

    ledger_problems = _acceptance_ledger_problems(repo_root, arguments.body_file, arguments.base_ref)
    if ledger_problems:
        failures.append(
            make_failure(
                "ACCEPTANCE_LEDGER_INCOMPLETE",
                "规格里的需求项没有在验收账本中逐条给出状态与证据。",
                repo_root / "docs" / "acceptance",
                "; ".join(ledger_problems[:10]),
                "在 docs/acceptance/<task-id>.md 为每个 R 项写一行状态与证据；"
                "deferred 必须写原因与后续任务。",
            )
        )
    else:
        checked_items.append("acceptance_ledger")

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
