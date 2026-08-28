#!/usr/bin/env python3
"""Plan, execute, and verify only the scenarios affected by a candidate diff.

The planner and receipt verifier are loaded from the trusted base checkout. The
candidate checkout supplies only the product and test code that is executed.
"""

from __future__ import annotations

import argparse
import fnmatch
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path
from typing import Any


SCRIPT_REPO_ROOT = Path(__file__).resolve().parents[2]
if str(SCRIPT_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(SCRIPT_REPO_ROOT))

from tools.governance.validate_scenario_test_governance import scenario_impact_files


SCHEMA_VERSION = 1
SHA_PATTERN = re.compile(r"^[0-9a-f]{40}$")
SUPPORTED_RUNNERS = {"windows-latest", "macos-14"}
GLOBAL_PRODUCT_FILES = {
    "package.json",
    "pnpm-lock.yaml",
    "src-tauri/Cargo.lock",
    "src-tauri/Cargo.toml",
    "src-tauri/tauri.conf.json",
}


def _matches(files: list[str], patterns: list[str]) -> bool:
    return any(fnmatch.fnmatch(path, pattern) for path in files for pattern in patterns)


def _execution_policy(registry: dict[str, Any]) -> dict[str, Any]:
    policy = registry.get("execution_policy")
    return policy if isinstance(policy, dict) else {}


def _target_runner(target: str, policy: dict[str, Any]) -> str:
    overrides = policy.get("target_runners") or {}
    runner = overrides.get(target, policy.get("default_runner", "windows-latest"))
    return runner if isinstance(runner, str) else ""


def _concrete_target(target: str, policy: dict[str, Any]) -> str:
    if target in (policy.get("workflow_commands") or {}):
        return "node-command:workflow"
    alias = (policy.get("workflow_aliases") or {}).get(target, target)
    return alias if isinstance(alias, str) else ""


def build_execution_plan(
    registry: dict[str, Any],
    changed_files: list[str],
    *,
    base_sha: str,
    head_sha: str,
) -> dict[str, Any]:
    """Return the exact PR target set for the directly affected scenarios."""

    scenario_ids: list[str] = []
    e2e_ids: list[str] = []
    targets: set[str] = set()
    blockers: list[str] = []
    if not SHA_PATTERN.fullmatch(base_sha):
        blockers.append("execution plan base SHA must be a full lowercase commit SHA")
    if not SHA_PATTERN.fullmatch(head_sha):
        blockers.append("execution plan head SHA must be a full lowercase commit SHA")
    policy = _execution_policy(registry)
    excluded = set(policy.get("pull_request_excluded_targets") or [])

    for scenario in registry.get("scenarios") or []:
        if "pull_request" not in set(scenario.get("gates") or []):
            continue
        if not (
            _matches(changed_files, scenario.get("change_patterns") or [])
            or any(path in GLOBAL_PRODUCT_FILES for path in changed_files)
        ):
            continue
        scenario_id = str(scenario.get("id", "unknown"))
        scenario_ids.append(scenario_id)
        required = scenario.get("pull_request_required_targets")
        if required is None:
            required = [
                target
                for target in scenario.get("automated_by") or []
                if target not in excluded
            ]
        if not required:
            blockers.append(f"{scenario_id} has no pull_request execution targets")
        targets.update(str(target) for target in required)

    for case in registry.get("complex_e2e_cases") or []:
        if "pull_request" not in (case.get("execution") or {}):
            continue
        if not _matches(changed_files, case.get("change_patterns") or []):
            continue
        case_id = str(case.get("id", "unknown"))
        e2e_ids.append(case_id)
        gate = case.get("pull_request_gate") or {}
        gaps = gate.get("remaining_gaps") or []
        if gate.get("status") != "implemented" or gaps:
            blockers.append(
                f"{case_id} affected E2E is not automated for pull_request "
                f"(status={gate.get('status')}, remaining_gaps={len(gaps)})"
            )
            continue
        required = gate.get("required_targets") or []
        if not required:
            blockers.append(f"{case_id} has no pull_request execution targets")
        targets.update(str(target) for target in required)

    runners: dict[str, list[str]] = {}
    requirements: dict[str, dict[str, bool]] = {}
    for target in sorted(targets):
        runner = _target_runner(target, policy)
        if runner not in SUPPORTED_RUNNERS:
            blockers.append(f"{target} has no supported affected-scenario runner")
            continue
        runners.setdefault(runner, []).append(target)
        kind = _concrete_target(target, policy).partition(":")[0]
        item = requirements.setdefault(runner, {"node": False, "rust": False})
        item["node"] = item["node"] or kind in {"path", "pnpm", "node-command"}
        item["rust"] = item["rust"] or kind in {"rust", "binary"}

    return {
        "schema_version": SCHEMA_VERSION,
        "base_sha": base_sha,
        "head_sha": head_sha,
        "changed_files": sorted(set(changed_files)),
        "scenario_ids": sorted(scenario_ids),
        "e2e_ids": sorted(e2e_ids),
        "required_targets": sorted(targets),
        "runners": runners,
        "requirements": requirements,
        "blockers": sorted(set(blockers)),
    }


def validate_aggregate_receipt(
    plan: dict[str, Any], receipt: dict[str, Any]
) -> list[str]:
    errors: list[str] = []
    if receipt.get("schema_version") != SCHEMA_VERSION:
        errors.append("scenario execution receipt has an unsupported schema version")
    if receipt.get("base_sha") != plan.get("base_sha"):
        errors.append("scenario execution receipt base SHA does not match the plan")
    if receipt.get("head_sha") != plan.get("head_sha"):
        errors.append("scenario execution receipt head SHA does not match the plan")
    if plan.get("blockers"):
        errors.append("scenario execution plan contains blockers")

    passed = {
        item.get("target")
        for item in receipt.get("targets") or []
        if isinstance(item, dict) and item.get("outcome") == "passed"
    }
    for target in plan.get("required_targets") or []:
        if target not in passed:
            errors.append(f"missing successful execution receipt for {target}")
    return errors


def _git_changed_files(repo: Path, base_sha: str, head_sha: str) -> list[str]:
    result = subprocess.run(
        ["git", "-C", str(repo), "diff", "--name-only", f"{base_sha}...{head_sha}"],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(result.stderr.strip() or "git diff failed")
    return [line.strip() for line in result.stdout.splitlines() if line.strip()]


def _write_json(path: Path, value: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def _command_digest(command: list[str]) -> str:
    encoded = json.dumps(command, separators=(",", ":"), ensure_ascii=True).encode()
    return hashlib.sha256(encoded).hexdigest()


def _run_command(
    command: list[str], repo: Path, *, env: dict[str, str] | None = None
) -> tuple[int, str]:
    merged_env = os.environ.copy()
    if env:
        merged_env.update(env)
    resolved = shutil.which(command[0], path=merged_env.get("PATH"))
    process_command = [resolved or command[0], *command[1:]]
    try:
        result = subprocess.run(
            process_command,
            cwd=repo,
            env=merged_env,
            capture_output=True,
            text=True,
            timeout=45 * 60,
        )
    except OSError as error:
        output = f"unable to start {command[0]}: {error}\n"
        print(output, end="")
        return 127, output
    output = (result.stdout or "") + (result.stderr or "")
    if output:
        print(output, end="" if output.endswith("\n") else "\n")
    return result.returncode, output


def _smoke_passed(flag: str, receipt_path: Path, policy: dict[str, Any]) -> bool:
    try:
        payload = json.loads(receipt_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return False
    oracle = (policy.get("binary_receipt_oracles") or {}).get(flag) or {}
    field = oracle.get("field")
    return isinstance(field, str) and payload.get(field) == oracle.get("equals")


def _execute_concrete_target(
    target: str, repo: Path, receipt_dir: Path, policy: dict[str, Any]
) -> tuple[bool, str]:
    if ":" not in target:
        return False, "target has no kind prefix"
    kind, marker = target.split(":", 1)
    env: dict[str, str] = {}
    smoke_receipt: Path | None = None
    if kind == "rust":
        command = [
            "cargo",
            "test",
            "--manifest-path",
            "src-tauri/Cargo.toml",
            "--workspace",
            marker,
            "--",
            "--nocapture",
        ]
    elif kind == "path":
        command = ["pnpm", "exec", "vitest", "run", marker]
    elif kind == "pnpm":
        command = ["pnpm", marker]
    elif kind == "binary":
        smoke_receipt = receipt_dir / (marker.removeprefix("--") + ".json")
        command = [
            "cargo",
            "run",
            "--manifest-path",
            "src-tauri/Cargo.toml",
            "--",
            marker,
            str(smoke_receipt),
        ]
        if marker == "--browser-session-smoke":
            env["CODEFACTORY_BROWSER_HEADLESS"] = "1"
        if marker == "--browser-chrome-attach-smoke":
            env["CODEFACTORY_BROWSER_CHROME_FIXTURE"] = "managed"
    elif kind == "workflow":
        configured = (policy.get("workflow_commands") or {}).get(target)
        if not isinstance(configured, list) or not all(
            isinstance(item, str) and item for item in configured
        ):
            return False, "workflow target has no executable command"
        command = configured
    else:
        return False, f"unsupported target kind: {kind}"

    try:
        returncode, output = _run_command(command, repo, env=env)
    except subprocess.TimeoutExpired:
        return False, "target exceeded the 45 minute timeout"
    if returncode != 0:
        return False, f"command exited {returncode}"
    if kind == "rust" and not re.search(
        rf"test\s+[^\n]*{re.escape(marker)}[^\n]*\.\.\.\s+ok", output
    ):
        return False, "cargo returned success without running the named test"
    if smoke_receipt is not None and not _smoke_passed(marker, smoke_receipt, policy):
        return False, "binary exited successfully without a passing smoke receipt"
    return True, _command_digest(command)


def execute_plan(plan: dict[str, Any], repo: Path, runner: str) -> dict[str, Any]:
    policy_path = Path(os.environ.get("SCENARIO_EXECUTION_REGISTRY", ""))
    if policy_path.is_file():
        registry = json.loads(policy_path.read_text(encoding="utf-8"))
    else:
        registry = json.loads(
            (repo / "docs/testing/scenario-registry.json").read_text(encoding="utf-8")
        )
    policy = _execution_policy(registry)
    aliases = policy.get("workflow_aliases") or {}
    selected = list((plan.get("runners") or {}).get(runner) or [])
    results: dict[str, dict[str, Any]] = {}
    receipt_dir = Path(tempfile.mkdtemp(prefix="scenario-target-receipts-"))

    def execute(target: str) -> dict[str, Any]:
        if target in results:
            return results[target]
        concrete = aliases.get(target, target)
        if not isinstance(concrete, str):
            result = {"target": target, "outcome": "failed", "detail": "invalid alias"}
        elif concrete != target:
            underlying = execute(concrete)
            result = {
                "target": target,
                "outcome": underlying["outcome"],
                "alias_of": concrete,
                "command_sha256": underlying.get("command_sha256"),
            }
        else:
            passed, detail = _execute_concrete_target(target, repo, receipt_dir, policy)
            result = {
                "target": target,
                "outcome": "passed" if passed else "failed",
                ("command_sha256" if passed else "detail"): detail,
            }
        results[target] = result
        return result

    for target in selected:
        execute(target)
    return {
        "schema_version": SCHEMA_VERSION,
        "base_sha": plan.get("base_sha"),
        "head_sha": plan.get("head_sha"),
        "runner": runner,
        "targets": [results[target] for target in selected],
    }


def aggregate_receipts(
    plan: dict[str, Any], receipt_paths: list[Path]
) -> dict[str, Any]:
    targets: list[dict[str, Any]] = []
    for path in receipt_paths:
        try:
            receipt = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if (
            receipt.get("schema_version") == SCHEMA_VERSION
            and receipt.get("base_sha") == plan.get("base_sha")
            and receipt.get("head_sha") == plan.get("head_sha")
        ):
            targets.extend(receipt.get("targets") or [])
    return {
        "schema_version": SCHEMA_VERSION,
        "base_sha": plan.get("base_sha"),
        "head_sha": plan.get("head_sha"),
        "scenario_ids": plan.get("scenario_ids") or [],
        "e2e_ids": plan.get("e2e_ids") or [],
        "targets": targets,
    }


def _github_json(url: str, token: str) -> dict[str, Any]:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "codefactory-scenario-gate",
        },
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


class _CrossOriginSafeRedirectHandler(urllib.request.HTTPRedirectHandler):
    """Keep API auth on same-origin redirects, never on signed artifact hosts."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected is None:
            return None
        source = urllib.parse.urlsplit(req.full_url).netloc.lower()
        destination = urllib.parse.urlsplit(newurl).netloc.lower()
        if source != destination:
            redirected.remove_header("Authorization")
        return redirected


def _github_bytes(url: str, token: str) -> bytes:
    request = urllib.request.Request(
        url,
        headers={
            "Accept": "application/vnd.github+json",
            "Authorization": f"Bearer {token}",
            "X-GitHub-Api-Version": "2022-11-28",
            "User-Agent": "codefactory-scenario-gate",
        },
    )
    opener = urllib.request.build_opener(_CrossOriginSafeRedirectHandler())
    with opener.open(request, timeout=60) as response:
        return response.read()


def await_github_receipt(
    plan: dict[str, Any],
    *,
    repository: str,
    workflow: str,
    token: str,
    timeout_seconds: int = 2700,
) -> list[str]:
    """Wait for the unprivileged execution workflow and validate its artifact."""

    if plan.get("blockers"):
        return list(plan["blockers"])
    if not plan.get("required_targets"):
        print("affected-scenario execution: no impacted targets; execution runners skipped")
        return []
    if not token:
        return ["scenario execution receipt lookup is missing GITHUB_TOKEN"]

    encoded_workflow = urllib.parse.quote(workflow, safe="")
    api_root = f"https://api.github.com/repos/{repository}"
    runs_url = (
        f"{api_root}/actions/workflows/{encoded_workflow}/runs"
        "?event=pull_request&per_page=100"
    )
    deadline = time.monotonic() + timeout_seconds
    last_state = "not-started"
    while time.monotonic() < deadline:
        try:
            payload = _github_json(runs_url, token)
        except (urllib.error.URLError, json.JSONDecodeError) as exc:
            last_state = f"GitHub API read failed: {exc}"
            time.sleep(10)
            continue
        candidates = [
            run
            for run in payload.get("workflow_runs") or []
            if run.get("head_sha") == plan.get("head_sha")
            and run.get("event") == "pull_request"
        ]
        if not candidates:
            last_state = "execution workflow has not started for this head SHA"
            time.sleep(10)
            continue
        run = max(candidates, key=lambda item: int(item.get("run_attempt") or 0) * 10**18 + int(item.get("id") or 0))
        if run.get("status") != "completed":
            last_state = f"execution workflow is {run.get('status')}"
            time.sleep(15)
            continue
        if run.get("conclusion") != "success":
            return [
                "affected scenario execution workflow did not succeed for the exact head SHA: "
                f"{run.get('conclusion')}"
            ]
        artifacts = _github_json(f"{api_root}/actions/runs/{run['id']}/artifacts", token)
        expected_name = f"affected-scenario-receipt-{plan.get('head_sha')}"
        artifact = next(
            (
                item
                for item in artifacts.get("artifacts") or []
                if item.get("name") == expected_name and not item.get("expired")
            ),
            None,
        )
        if artifact is None:
            return ["successful scenario execution run has no exact-head receipt artifact"]
        archive = _github_bytes(artifact["archive_download_url"], token)
        with tempfile.TemporaryDirectory(prefix="scenario-receipt-") as directory:
            archive_path = Path(directory) / "receipt.zip"
            archive_path.write_bytes(archive)
            try:
                with zipfile.ZipFile(archive_path) as bundle:
                    names = bundle.namelist()
                    if names != ["scenario-execution-receipt.json"]:
                        return ["scenario execution artifact has an unexpected file set"]
                    receipt = json.loads(bundle.read(names[0]))
            except (zipfile.BadZipFile, json.JSONDecodeError) as exc:
                return [f"scenario execution receipt artifact is invalid: {exc}"]
        return validate_aggregate_receipt(plan, receipt)
    return [f"timed out waiting for affected scenario execution: {last_state}"]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)

    plan_parser = subparsers.add_parser("plan")
    plan_parser.add_argument("--registry", required=True)
    plan_parser.add_argument("--repo", required=True)
    plan_parser.add_argument("--base-sha", required=True)
    plan_parser.add_argument("--head-sha", required=True)
    plan_parser.add_argument("--output", required=True)
    plan_parser.add_argument("--github-output")

    execute_parser = subparsers.add_parser("execute")
    execute_parser.add_argument("--plan", required=True)
    execute_parser.add_argument("--repo", required=True)
    execute_parser.add_argument("--runner", required=True, choices=sorted(SUPPORTED_RUNNERS))
    execute_parser.add_argument("--output", required=True)

    aggregate_parser = subparsers.add_parser("aggregate")
    aggregate_parser.add_argument("--plan", required=True)
    aggregate_parser.add_argument("--receipts", required=True)
    aggregate_parser.add_argument("--output", required=True)

    validate_parser = subparsers.add_parser("validate")
    validate_parser.add_argument("--plan", required=True)
    validate_parser.add_argument("--receipt", required=True)

    await_parser = subparsers.add_parser("await-github")
    await_parser.add_argument("--registry", required=True)
    await_parser.add_argument("--repo", required=True)
    await_parser.add_argument("--base-sha", required=True)
    await_parser.add_argument("--head-sha", required=True)
    await_parser.add_argument("--repository", required=True)
    await_parser.add_argument("--workflow", default="scenario-execution.yml")
    await_parser.add_argument("--token-env", default="GITHUB_TOKEN")

    args = parser.parse_args()
    if args.command == "plan":
        registry = json.loads(Path(args.registry).read_text(encoding="utf-8"))
        repo = Path(args.repo)
        changed = _git_changed_files(repo, args.base_sha, args.head_sha)
        changed = scenario_impact_files(repo, args.base_sha, changed)
        plan = build_execution_plan(
            registry, changed, base_sha=args.base_sha, head_sha=args.head_sha
        )
        _write_json(Path(args.output), plan)
        if args.github_output:
            with Path(args.github_output).open("a", encoding="utf-8") as handle:
                for runner in sorted(SUPPORTED_RUNNERS):
                    key = runner.replace("-latest", "").replace("-14", "").replace("-", "_")
                    handle.write(f"run_{key}={'true' if runner in plan['runners'] else 'false'}\n")
                    requirements = plan.get("requirements", {}).get(runner, {})
                    handle.write(f"{key}_node={'true' if requirements.get('node') else 'false'}\n")
                    handle.write(f"{key}_rust={'true' if requirements.get('rust') else 'false'}\n")
        if plan["blockers"]:
            for blocker in plan["blockers"]:
                print(f"::error::{blocker}")
            return 1
        return 0
    if args.command == "execute":
        plan = json.loads(Path(args.plan).read_text(encoding="utf-8"))
        os.environ["SCENARIO_EXECUTION_REGISTRY"] = str(
            Path(__file__).resolve().parents[2] / "docs/testing/scenario-registry.json"
        )
        receipt = execute_plan(plan, Path(args.repo), args.runner)
        _write_json(Path(args.output), receipt)
        return 0 if all(item.get("outcome") == "passed" for item in receipt["targets"]) else 1
    if args.command == "aggregate":
        plan = json.loads(Path(args.plan).read_text(encoding="utf-8"))
        paths = sorted(Path(args.receipts).rglob("runner-receipt.json"))
        receipt = aggregate_receipts(plan, paths)
        _write_json(Path(args.output), receipt)
        errors = validate_aggregate_receipt(plan, receipt)
        for error in errors:
            print(f"::error::{error}")
        return 1 if errors else 0
    if args.command == "await-github":
        registry = json.loads(Path(args.registry).read_text(encoding="utf-8"))
        repo = Path(args.repo)
        changed = _git_changed_files(repo, args.base_sha, args.head_sha)
        changed = scenario_impact_files(repo, args.base_sha, changed)
        plan = build_execution_plan(
            registry, changed, base_sha=args.base_sha, head_sha=args.head_sha
        )
        errors = await_github_receipt(
            plan,
            repository=args.repository,
            workflow=args.workflow,
            token=os.environ.get(args.token_env, ""),
        )
    else:
        plan = json.loads(Path(args.plan).read_text(encoding="utf-8"))
        receipt = json.loads(Path(args.receipt).read_text(encoding="utf-8"))
        errors = validate_aggregate_receipt(plan, receipt)
    for error in errors:
        print(f"::error::{error}")
    return 1 if errors else 0


if __name__ == "__main__":
    sys.exit(main())
