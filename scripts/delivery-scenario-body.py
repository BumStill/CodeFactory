#!/usr/bin/env python3
"""Delivery adapter: call the repository validator, never duplicate its mapping."""
import importlib.util
import json
from pathlib import Path
import sys


def reconcile(validator_path, root, base, body, validate_registry=False):
    spec = importlib.util.spec_from_file_location('delivery_scenario_validator', validator_path)
    validator = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(validator)
    files, error = validator._changed_files(root, base)
    if error:
        raise ValueError(error)
    # Load after the diff so an unresolvable base keeps reporting "cannot diff",
    # while a readable base still enforces the append-only mapping contract.
    registry = validator.load_registry(root / 'docs/testing/scenario-registry.json', base_ref=base)
    if validate_registry:
        errors = validator.validate_registry(registry, root)
        if errors:
            raise ValueError('; '.join(errors))
    files = validator.scenario_impact_files(root, base, files)
    product = [path for path in files if validator._is_product_file(path)]
    impacted = sorted(validator._impacted_scenarios(product, registry))
    old = [line.strip() for line in body.splitlines()
           if line.strip().startswith('Scenario-Test:')]
    cleaned = [line for line in body.splitlines()
               if not line.strip().startswith('Scenario-Test:')]
    declaration = 'Scenario-Test: ' + ', '.join(impacted) if impacted else None
    if declaration:
        cleaned.insert(0, declaration)  # Preserve release trailers as the final paragraph.
    changed = old != ([declaration] if declaration else [])
    result = '\n'.join(cleaned).rstrip() + '\n' if changed else body
    errors = validator.validate_change_contract('', result, files, registry)
    if errors:
        raise ValueError('; '.join(errors))
    return {'body': result, 'changed': changed,
            'detail': f"Scenario-Test: {old or ['<missing>']} -> {declaration or '<not impacted>'}"}


if __name__ == '__main__':
    try:
        print(json.dumps(reconcile(Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3], sys.stdin.read(),
                                   validate_registry='--validate-registry' in sys.argv[4:])))
    except Exception as error:
        print(f'scenario validator failed: {error}', file=sys.stderr)
        sys.exit(1)
