#!/usr/bin/env python3
"""M223: separate transient GitHub infrastructure faults from real CI failures.

A release must never die because GitHub's own plumbing hiccupped. On the night
of 2026-11-03 three consecutive Auto Release runs failed while waiting on the
guarded version PR: the required checks had ended in a harness-level state
(runner setup, action download, a cancelled run) and the wait loop treated
every non-``SUCCESS`` conclusion as a red test, aborting the whole release.

The wait loop now asks this classifier instead of guessing, and this script
owns *all* of the interpretation: it normalises the PR's ``statusCheckRollup``
(superseded attempts included), digs the failing steps out of ``gh run view
--json jobs`` payloads, and returns one decision for the round.

Fail-closed is the default. A check is only retryable when its failed step is
positively recognisable as harness-level; an unknown step name, no evidence at
all, or a mixed signal is reported as a real failure, so a genuine regression
is never masked by a rerun.

Usage: ``classify_ci_infra.py < payload.json`` (or pass a path) prints one JSON
object:

    {"decision": "wait" | "merge" | "retry_infra" | "fail",
     "reason": "<one line>",
     "failed_checks": ["<check>", ...],
     "retry_run_ids": [<int>, ...],
     "checks": {"<check>": "success" | "pending" | "infra" | "real" | "unknown"}}

Payload::

    {"checks": [{"name": "check-rust", "conclusion": "FAILURE",
                 "startedAt": "2026-11-03T22:00:00Z"}, ...],
     "runs": [{"run_id": 41642171366, "jobs": {"jobs": [...]}}, ...],
     "retry_run_ids": [41642171366],
     "infra_retries_used": 0,
     "max_infra_retries": 2,
     "required": ["check-rust", ...]}

The script exits 0 whenever it produced a decision (including one that reports
a real failure); it exits 1 only when the input document itself is unusable.
"""
from __future__ import annotations

import json
import re
import sys

DEFAULT_REQUIRED = (
    "governance-baseline",
    "agent-bridge-linux",
    "check-frontend",
    "check-rust",
    "check-rust-macos-security",
    "check-rust-unix",
    "docker-sandbox",
    "remote-real-app-gui",
    "scenario-gate-pr",
)

SUCCESS_CONCLUSIONS = {"SUCCESS"}
FAILURE_CONCLUSIONS = {
    "FAILURE",
    "CANCELLED",
    "TIMED_OUT",
    "ACTION_REQUIRED",
    "STARTUP_FAILURE",
    "STALE",
}
# Job-level conclusions that mean "this job produced no test result at all".
RESULTLESS_CONCLUSIONS = {"STARTUP_FAILURE", "CANCELLED"}
STEP_FAILURE_CONCLUSIONS = {"failure", "cancelled", "timed_out"}

# Harness-level steps: checked before the failure vocabulary below, because
# ``Run pnpm/action-setup`` contains "pnpm" while installing the package
# manager is infrastructure, not a test result.
INFRA_OVERRIDE_PATTERNS = (
    r"^set up job$",
    r"^complete job$",
    r"^post ",
    r"^run actions/checkout",
    r"^run actions/cache",
    r"^run actions/upload-artifact",
    r"^run actions/download-artifact",
    r"^run actions/setup-",
    r"^setup-node",
    r"^setup-python",
    r"^run pnpm/action-setup",
    r"^run dtolnay/rust-toolchain",
    r"^run swatinem/rust-cache",
    r"^run actions-rust-lang/",
    r"^start proxy",
    r"^stop proxy",
)

REAL_STEP_PATTERNS = (
    r"\btest(s|ing|er)?\b",
    r"\bcargo\b",
    r"\bpnpm\b",
    r"\bnpm\b",
    r"\bvitest\b",
    r"\bunittest\b",
    r"\bassert",
    r"\blint\b",
    r"\btypecheck\b",
    r"\btsc\b",
    r"\bbuild\b",
    r"\bcompile",
    r"\bvalidate",
    r"\bscenario",
    r"\bgovernance",
    r"\btauri\b",
    r"\bsmoke\b",
    r"\bacceptance\b",
    r"\bbench",
)

INFRA_STEP_PATTERNS = (
    r"\bcheckout\b",
    r"\bcache\b",
    r"\bartifact",
    r"\brustup\b",
    r"\btoolchain\b",
    r"\bworkflow command\b",
    r"\brunner\b",
    r"\bhostedtoolcache\b",
)

INFRA_LOG_PATTERNS = (
    r"\bHTTP 5\d\d\b",
    r"\bHTTP 429\b",
    r"\b(primary|secondary) rate limit\b",
    r"could not resolve host",
    r"temporary failure in name resolution",
    r"connection reset",
    r"\bECONNRESET\b",
    r"\bETIMEDOUT\b",
    r"\bEAI_AGAIN\b",
    r"i/o timeout",
    r"tls handshake timeout",
    r"no space left on device",
    r"the runner has received a shutdown signal",
    # GitHub's own words when it could not place a job on a machine.
    r"not acquired by runner",
    r"no runner matching the specified labels",
    r"all eligible runners",
    r"capacity constraints",
    r"service unavailable",
    r"bad gateway",
    r"gateway timeout",
    r"socket timeout",
    r"lost connection",
    r"remote end hung up unexpectedly",
    r"unable to download",
    r"could not be found in the cache",
)

REAL_LOG_PATTERNS = (
    r"AssertionError",
    r"assertion failed",
    r"test result: FAILED",
    r"^error\[E\d+\]",
    r"\berror TS\d+",
    r"\bpanicked at\b",
    r"\b[1-9]\d* (tests? )?failed\b",
)

_INFRA_OVERRIDE = tuple(re.compile(p, re.IGNORECASE) for p in INFRA_OVERRIDE_PATTERNS)
_REAL_STEP = tuple(re.compile(p, re.IGNORECASE) for p in REAL_STEP_PATTERNS)
_INFRA_STEP = tuple(re.compile(p, re.IGNORECASE) for p in INFRA_STEP_PATTERNS)
_INFRA_LOG = tuple(re.compile(p, re.IGNORECASE) for p in INFRA_LOG_PATTERNS)
_REAL_LOG = tuple(re.compile(p) for p in REAL_LOG_PATTERNS)


def _matches(patterns: tuple[re.Pattern[str], ...], text: str) -> bool:
    return any(pattern.search(text) for pattern in patterns)


def classify_step(step: str) -> str:
    """Return ``infra``, ``real`` or ``unknown`` for one failed step name."""

    name = (step or "").strip()
    if not name:
        return "unknown"
    if _matches(_INFRA_OVERRIDE, name):
        return "infra"
    if _matches(_REAL_STEP, name):
        return "real"
    if _matches(_INFRA_STEP, name):
        return "infra"
    return "unknown"


def classify_log(lines: list[str]) -> str:
    """Classify a log excerpt: a real result always outranks an infra signal."""

    text = "\n".join(lines or [])
    if not text.strip():
        return "unknown"
    if _matches(_REAL_LOG, text):
        return "real"
    if _matches(_INFRA_LOG, text):
        return "infra"
    return "unknown"


def latest_attempts(rollup: list[dict]) -> dict[str, tuple[str, str]]:
    """Collapse the rollup to the newest attempt per check name.

    A rerun appends a fresh check run for the same head SHA, so the rollup can
    carry a superseded ``FAILURE`` beside the rerun that replaced it. Comparing
    ``startedAt`` keeps the fresh attempt authoritative.
    """

    latest: dict[str, tuple[str, str]] = {}
    for entry in rollup or []:
        if not isinstance(entry, dict):
            continue
        name = entry.get("name") or entry.get("context") or ""
        if not name:
            continue
        started = str(entry.get("startedAt") or "")
        conclusion = str(entry.get("conclusion") or "")
        previous = latest.get(name)
        if previous is None or started >= previous[0]:
            latest[name] = (started, conclusion)
    return latest


def collect_failed_steps(
    runs: list[dict], red_checks: set[str]
) -> tuple[list[dict], list[int]]:
    """Extract failing steps for the red checks from ``gh run view --json jobs``.

    Returns ``(entries, run_ids)`` where each entry is
    ``{"check", "step", "run_id"}`` and ``run_ids`` are runs that actually
    touched a red check (i.e. the runs a ``--failed`` rerun makes sense for).
    """

    entries: list[dict] = []
    run_ids: list[int] = []
    for run in runs or []:
        if not isinstance(run, dict):
            continue
        run_id = run.get("run_id")
        payload = run.get("jobs") or {}
        jobs = payload.get("jobs") if isinstance(payload, dict) else None
        touched = False
        for job in jobs or []:
            if not isinstance(job, dict):
                continue
            name = job.get("name") or ""
            if name not in red_checks:
                continue
            if str(job.get("conclusion") or "") in ("success", "skipped", "neutral"):
                continue
            touched = True
            for step in job.get("steps") or []:
                if not isinstance(step, dict):
                    continue
                if str(step.get("conclusion") or "") in STEP_FAILURE_CONCLUSIONS:
                    entries.append(
                        {
                            "check": name,
                            "step": str(step.get("name") or ""),
                            "run_id": run_id,
                        }
                    )
        if touched and isinstance(run_id, int) and run_id not in run_ids:
            run_ids.append(run_id)
    return entries, run_ids


def _check_verdict(
    check: str,
    conclusion: str,
    evidence: list[dict],
    log_lines: list[str],
    annotations: list[str],
) -> str:
    verdicts = [
        classify_step(str(entry.get("step") or ""))
        for entry in evidence
        if (entry.get("check") or "") == check
    ]
    for excerpt in (log_lines, annotations):
        verdict = classify_log(excerpt)
        if verdict != "unknown":
            verdicts.append(verdict)

    if "real" in verdicts:
        return "real"
    if verdicts and all(verdict == "infra" for verdict in verdicts):
        return "infra"
    if conclusion in RESULTLESS_CONCLUSIONS:
        # The job never started, or the harness tore the run down: no test
        # result exists to honour, so a rerun is the honest next step (bounded
        # by the caller's retry budget).
        return "infra"
    return "unknown"


def classify(payload: dict) -> dict:
    required = list(payload.get("required") or DEFAULT_REQUIRED)
    latest = latest_attempts(payload.get("checks") or [])
    max_retries = int(payload.get("max_infra_retries", 2))
    used_retries = int(payload.get("infra_retries_used", 0))
    log_lines = payload.get("log_lines") or {}

    checks: dict[str, str] = {}
    red: dict[str, str] = {}
    pending: list[str] = []

    for check in required:
        attempt = latest.get(check)
        if attempt is None:
            checks[check] = "pending"
            pending.append(check)
            continue
        conclusion = attempt[1].upper()
        if conclusion in FAILURE_CONCLUSIONS:
            red[check] = conclusion
        elif conclusion in SUCCESS_CONCLUSIONS:
            checks[check] = "success"
        else:
            checks[check] = "pending"
            pending.append(check)

    if not red:
        if pending:
            return {
                "decision": "wait",
                "reason": "waiting on " + ", ".join(sorted(set(pending))),
                "failed_checks": [],
                "retry_run_ids": [],
                "checks": checks,
            }
        return {
            "decision": "merge",
            "reason": "all required checks passed",
            "failed_checks": [],
            "retry_run_ids": [],
            "checks": checks,
        }

    derived, run_ids = collect_failed_steps(payload.get("runs") or [], set(red))
    evidence = list(derived) + [
        entry for entry in (payload.get("failed_steps") or []) if isinstance(entry, dict)
    ]
    for run_id in payload.get("retry_run_ids") or []:
        if isinstance(run_id, int) and run_id not in run_ids:
            run_ids.append(run_id)

    for check, conclusion in red.items():
        checks[check] = _check_verdict(
            check,
            conclusion,
            evidence,
            log_lines.get(check) or [],
            (payload.get("annotations") or {}).get(check) or [],
        )

    real = sorted(check for check in red if checks[check] != "infra")
    infra = sorted(check for check in red if checks[check] == "infra")

    if real:
        unknown = [check for check in real if checks[check] == "unknown"]
        if unknown:
            return {
                "decision": "fail",
                "reason": (
                    "required check(s) "
                    + ", ".join(unknown)
                    + " failed with no recognisable infrastructure signal; "
                    "treating as a real failure rather than retrying blindly"
                ),
                "failed_checks": real,
                "retry_run_ids": [],
                "checks": checks,
            }
        return {
            "decision": "fail",
            "reason": "required check(s) " + ", ".join(real) + " failed with a real result",
            "failed_checks": real,
            "retry_run_ids": [],
            "checks": checks,
        }

    if not run_ids:
        return {
            "decision": "fail",
            "reason": (
                "transient infrastructure failure on "
                + ", ".join(infra)
                + " but no rerunnable workflow run id was found; "
                "rerun Auto Release to replan"
            ),
            "failed_checks": infra,
            "retry_run_ids": [],
            "checks": checks,
        }

    if used_retries >= max_retries:
        return {
            "decision": "fail",
            "reason": (
                "transient infrastructure failure on "
                + ", ".join(infra)
                + " persisted after "
                + str(used_retries)
                + " automatic retries; GitHub infrastructure is not healthy"
            ),
            "failed_checks": infra,
            "retry_run_ids": run_ids,
            "checks": checks,
        }

    return {
        "decision": "retry_infra",
        "reason": (
            "transient infrastructure failure on "
            + ", ".join(infra)
            + " (retry "
            + str(used_retries + 1)
            + "/"
            + str(max_retries)
            + ")"
        ),
        "failed_checks": infra,
        "retry_run_ids": run_ids,
        "checks": checks,
    }


def main(argv: list[str]) -> int:
    if len(argv) > 1:
        try:
            with open(argv[1], encoding="utf-8") as handle:
                raw = handle.read()
        except OSError as error:
            print(f"classify_ci_infra: cannot read {argv[1]}: {error}", file=sys.stderr)
            return 1
    else:
        raw = sys.stdin.read()

    try:
        payload = json.loads(raw or "{}")
    except json.JSONDecodeError as error:
        print(f"classify_ci_infra: payload is not JSON: {error}", file=sys.stderr)
        return 1

    if not isinstance(payload, dict):
        print("classify_ci_infra: payload must be a JSON object", file=sys.stderr)
        return 1

    print(json.dumps(classify(payload), ensure_ascii=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
