from __future__ import annotations

import copy
import unittest
from pathlib import Path

from tools.governance.scenario_catalog_docs import (
    load_registry,
    render_cases,
    render_categories,
    render_summary,
    validate_catalog_docs,
)


REPO_ROOT = Path(__file__).resolve().parents[1]
REGISTRY_PATH = REPO_ROOT / "docs" / "testing" / "scenario-registry.json"


class ScenarioCatalogDocsTests(unittest.TestCase):
    def test_repository_catalog_docs_match_registry(self) -> None:
        registry = load_registry(REGISTRY_PATH)
        self.assertEqual(validate_catalog_docs(REPO_ROOT, registry), [])

    def test_registry_change_invalidates_derived_blocks(self) -> None:
        registry = load_registry(REGISTRY_PATH)
        changed = copy.deepcopy(registry)
        changed["scenarios"].append(
            {
                "id": "TEST-ONLY",
                "category": changed["categories"][0]["id"],
                "priority": "P2",
            }
        )
        errors = validate_catalog_docs(REPO_ROOT, changed)
        self.assertTrue(
            any("registry-derived block is stale" in error for error in errors),
            errors,
        )

    def test_renderers_include_every_registry_row(self) -> None:
        registry = load_registry(REGISTRY_PATH)
        summary = render_summary(registry)
        categories = render_categories(registry)
        cases = render_cases(registry)
        for category in registry["categories"]:
            self.assertIn(f"`{category['id']}`", categories)
        for case in registry["complex_e2e_cases"]:
            self.assertIn(f"| {case['id']} |", cases)
        self.assertIn(f"逻辑 Scenario：`{len(registry['scenarios'])}`", summary)


if __name__ == "__main__":
    unittest.main()
