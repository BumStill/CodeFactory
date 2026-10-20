"""Bounded local governance checks for thin delivery. No remote side effects."""
from __future__ import annotations
import json
from pathlib import Path
import re
import subprocess
import sys
import time


def check(root: Path, base: str, timeout: float = 10) -> dict:
    started = time.monotonic()
    warnings = []
    checks = [
        ('validate_repo_governance_baseline.py', []),
        ('validate_objective_non_interruption_contract.py', ['--base', base]),
    ]
    for name, args in checks:
        script = root / 'tools/governance' / name
        if not script.is_file():
            warnings.append(f'{name}: script unavailable')
            continue
        try:
            result = subprocess.run([sys.executable, str(script), *args], cwd=root,
                                    capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout)
        except (OSError, subprocess.TimeoutExpired) as error:
            warnings.append(f'{name}: timeout or unavailable: {error}')
            continue
        output = result.stdout + '\n' + result.stderr
        if result.returncode == 0:
            continue
        if name == 'validate_repo_governance_baseline.py':
            try:
                payload = json.loads(result.stdout)
            except ValueError:
                payload = {}
            rule_failure = payload.get('status') == 'fail' and bool(payload.get('failures'))
        else:
            rule_failure = 'OBJECTIVE_MANUAL_TECHNICAL_HANDOFF:' in output
            output = re.sub(r'::error file=([^,]+),line=(\d+)::', r'\1:\2: ', output)
        if rule_failure:
            return {'ok': False, 'error': f'{name}: {output.strip()}',
                    'warnings': warnings, 'elapsed_ms': round((time.monotonic()-started)*1000)}
        warnings.append(f'{name}: validator unavailable (exit {result.returncode}): {output.strip()}')
    return {'ok': True, 'warnings': warnings, 'elapsed_ms': round((time.monotonic()-started)*1000)}


if __name__ == '__main__':
    print(json.dumps(check(Path(sys.argv[1]), sys.argv[2]), ensure_ascii=False))
