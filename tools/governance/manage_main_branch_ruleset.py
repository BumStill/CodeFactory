#!/usr/bin/env python3
"""Validate, reconcile, and verify CodeFactory's GitHub main-branch gate.

The desired GitHub state lives in `.github/rulesets/main.json`. Read-only
commands are the default; changing GitHub requires the explicit `apply`
subcommand. The safety invariants below intentionally reject a human/admin
bypass or a reduced required-check set.
"""
from __future__ import annotations

import argparse
import copy
import json
import subprocess
import sys
from pathlib import Path
from typing import Any


REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_POLICY_PATH = REPO_ROOT / ".github" / "rulesets" / "main.json"
EXPECTED_REPOSITORY = "BumStill/CodeFactory"
GITHUB_ACTIONS_APP_ID = 15368
EXPECTED_CHECKS = {
    "agent-bridge-linux",
    "check-frontend",
    "check-rust",
    "governance-baseline",
    "remote-real-app-gui",
    "scenario-gate-pr",
}


def load_policy(path: Path = DEFAULT_POLICY_PATH) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def build_ruleset_payload(policy: dict[str, Any]) -> dict[str, Any]:
    """Return only fields accepted by the repository rulesets API."""
    return copy.deepcopy(policy["ruleset"])


def validate_policy(policy: dict[str, Any]) -> list[str]:
    errors: list[str] = []
    if policy.get("repository") != EXPECTED_REPOSITORY:
        errors.append(f"repository must be {EXPECTED_REPOSITORY}")
    if policy.get("repository_settings", {}).get("allow_auto_merge") is not True:
        errors.append("allow_auto_merge must be true")
    if policy.get("cleanup", {}).get("remove_classic_review_requirement") is not True:
        errors.append("the legacy one-review requirement must be removed")

    ruleset = policy.get("ruleset", {})
    if ruleset.get("target") != "branch":
        errors.append("ruleset target must be branch")
    if ruleset.get("enforcement") != "active":
        errors.append("ruleset enforcement must be active")
    include = ruleset.get("conditions", {}).get("ref_name", {}).get("include")
    if include != ["~DEFAULT_BRANCH"]:
        errors.append("ruleset must target only ~DEFAULT_BRANCH")
    exclude = ruleset.get("conditions", {}).get("ref_name", {}).get("exclude")
    if exclude != []:
        errors.append("ruleset must not exclude any ref")

    if ruleset.get("bypass_actors") != []:
        errors.append("ruleset bypass actors must be empty")

    rules = ruleset.get("rules", [])
    if not isinstance(rules, list):
        return [*errors, "rules must be a list"]
    expected_rule_types = {
        "deletion",
        "non_fast_forward",
        "pull_request",
        "required_status_checks",
    }
    rule_types = [
        rule.get("type") for rule in rules if isinstance(rule, dict)
    ]
    if len(rule_types) != len(expected_rule_types) or set(rule_types) != expected_rule_types:
        errors.append(
            "rules must be exactly deletion, non_fast_forward, pull_request, required_status_checks"
        )
    by_type = {rule.get("type"): rule for rule in rules if isinstance(rule, dict)}
    for rule_type in ("deletion", "non_fast_forward", "pull_request", "required_status_checks"):
        if rule_type not in by_type:
            errors.append(f"missing rule: {rule_type}")

    pull_request = by_type.get("pull_request", {}).get("parameters", {})
    if pull_request.get("required_approving_review_count") != 0:
        errors.append("solo-maintainer policy requires zero approving reviews")
    if pull_request.get("required_review_thread_resolution") is not True:
        errors.append("review conversations must be resolved")
    if pull_request.get("allowed_merge_methods") != ["squash"]:
        errors.append("only squash merge may be enabled")

    status = by_type.get("required_status_checks", {}).get("parameters", {})
    if status.get("strict_required_status_checks_policy") is not True:
        errors.append("required checks must use strict/up-to-date mode")
    checks = status.get("required_status_checks", [])
    contexts = {item.get("context") for item in checks if isinstance(item, dict)}
    if contexts != EXPECTED_CHECKS:
        errors.append(f"required checks must be exactly {sorted(EXPECTED_CHECKS)}")
    integrations = {
        item.get("integration_id") for item in checks if isinstance(item, dict)
    }
    if integrations != {GITHUB_ACTIONS_APP_ID}:
        errors.append("every required check must be bound to GitHub Actions App 15368")
    return errors


def gh_api(
    repo: str,
    endpoint: str,
    *,
    method: str = "GET",
    payload: dict[str, Any] | None = None,
    allow_not_found: bool = False,
) -> Any:
    api_path = f"repos/{repo}"
    if endpoint:
        api_path = f"{api_path}/{endpoint}"
    command = ["gh", "api", "--method", method, api_path]
    encoded = None
    if payload is not None:
        command.extend(["--input", "-"])
        encoded = json.dumps(payload)
    result = subprocess.run(
        command,
        input=encoded,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        if allow_not_found and "HTTP 404" in result.stderr:
            return None
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"gh api {method} {endpoint} failed: {detail}")
    if not result.stdout.strip():
        return None
    return json.loads(result.stdout)


def gh_graphql(query: str, variables: dict[str, str]) -> dict[str, Any]:
    command = ["gh", "api", "graphql", "-f", f"query={query}"]
    for key, value in sorted(variables.items()):
        command.extend(["-F", f"{key}={value}"])
    result = subprocess.run(
        command,
        capture_output=True,
        text=True,
        check=False,
    )
    if result.returncode != 0:
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(f"gh api graphql failed: {detail}")
    return json.loads(result.stdout)


def classic_review_requirement_present(repo: str) -> bool:
    """Read effective classic review state without the stale REST subresource.

    GitHub can keep returning the pre-delete review payload from the dedicated
    REST endpoint even after a successful 204 cleanup. GraphQL reflects the
    branch protection rule's current review fields and avoids false drift.
    """
    owner, name = repo.split("/", 1)
    query = """
      query($owner: String!, $name: String!) {
        repository(owner: $owner, name: $name) {
          ref(qualifiedName: "refs/heads/main") {
            branchProtectionRule {
              pattern
              requiresApprovingReviews
              requiredApprovingReviewCount
            }
          }
        }
      }
    """
    payload = gh_graphql(query, {"owner": owner, "name": name})
    try:
        repository = payload["data"]["repository"]
        ref = repository["ref"]
        if ref is None or "branchProtectionRule" not in ref:
            raise KeyError("main ref or effective branch protection rule is missing")
        rule = ref["branchProtectionRule"]
    except (KeyError, TypeError) as exc:
        raise RuntimeError("GraphQL branch protection response is incomplete") from exc
    if rule is None:
        return False
    return (
        rule.get("requiresApprovingReviews") is True
        or (rule.get("requiredApprovingReviewCount") or 0) > 0
    )


def find_ruleset(repo: str, name: str) -> dict[str, Any] | None:
    summaries = gh_api(repo, "rulesets")
    match = next((item for item in summaries if item.get("name") == name), None)
    if match is None:
        return None
    return gh_api(repo, f"rulesets/{match['id']}")


def contains(actual: Any, desired: Any) -> bool:
    """Compare desired API fields while ignoring server-added response fields."""
    if isinstance(desired, dict):
        return isinstance(actual, dict) and all(
            key in actual and contains(actual[key], value)
            for key, value in desired.items()
        )
    if isinstance(desired, list):
        if not isinstance(actual, list) or len(actual) != len(desired):
            return False
        return all(
            any(contains(actual_item, desired_item) for actual_item in actual)
            for desired_item in desired
        )
    return actual == desired


def inspect_live(policy: dict[str, Any]) -> dict[str, Any]:
    repo = policy["repository"]
    desired = build_ruleset_payload(policy)
    repository = gh_api(repo, "")
    ruleset = find_ruleset(repo, desired["name"])
    return {
        "repository": repo,
        "allow_auto_merge": repository.get("allow_auto_merge"),
        "ruleset_id": ruleset.get("id") if ruleset else None,
        "ruleset_matches": contains(ruleset, desired) if ruleset else False,
        "classic_review_requirement_present": classic_review_requirement_present(repo),
    }


def apply_policy(policy: dict[str, Any]) -> dict[str, Any]:
    repo = policy["repository"]
    desired = build_ruleset_payload(policy)
    existing = find_ruleset(repo, desired["name"])

    # Install the active PR/check gate before removing any legacy constraint.
    if existing:
        gh_api(repo, f"rulesets/{existing['id']}", method="PUT", payload=desired)
    else:
        gh_api(repo, "rulesets", method="POST", payload=desired)

    installed = find_ruleset(repo, desired["name"])
    if installed is None or not contains(installed, desired):
        raise RuntimeError(
            "ruleset read-back did not match desired active protection; "
            "refusing to enable auto-merge or remove the legacy review gate"
        )

    gh_api(
        repo,
        "",
        method="PATCH",
        payload={"allow_auto_merge": True},
    )
    if policy["cleanup"]["remove_classic_review_requirement"]:
        gh_api(
            repo,
            "branches/main/protection/required_pull_request_reviews",
            method="DELETE",
            allow_not_found=True,
        )
    return inspect_live(policy)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("validate", "plan", "apply", "verify"))
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY_PATH)
    args = parser.parse_args()

    policy = load_policy(args.policy)
    errors = validate_policy(policy)
    if errors:
        print(json.dumps({"status": "invalid", "errors": errors}, indent=2))
        return 1
    if args.command == "validate":
        print(json.dumps({"status": "valid", "policy": str(args.policy)}, indent=2))
        return 0

    state = apply_policy(policy) if args.command == "apply" else inspect_live(policy)
    converged = (
        state["allow_auto_merge"] is True
        and state["ruleset_matches"] is True
        and state["classic_review_requirement_present"] is False
    )
    state["status"] = "converged" if converged else "drift"
    print(json.dumps(state, indent=2))
    if args.command in {"apply", "verify"} and not converged:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
