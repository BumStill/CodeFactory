from __future__ import annotations

import argparse
import json
from pathlib import Path

REQUIRED_SNIPPETS = [
    "## Basics",
    "## Completion Standard",
    "## Current State",
    "## Completed Items",
    "## Remaining Items",
    "## Blockers",
    "## Evidence",
    "## Stop Boundary",
]


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Validate a long-task record.")
    parser.add_argument("--task-record-path", required=True)
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    path = Path(args.task_record_path).resolve()
    text = path.read_text(encoding="utf-8")
    missing = [snippet for snippet in REQUIRED_SNIPPETS if snippet not in text]
    result = {"status": "fail" if missing else "pass", "path": str(path), "missing": missing}
    print(json.dumps(result, ensure_ascii=False, indent=2))
    return 1 if missing else 0


if __name__ == "__main__":
    raise SystemExit(main())
