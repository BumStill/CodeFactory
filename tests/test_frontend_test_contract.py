from __future__ import annotations

import unittest

from tools.governance.validate_frontend_test_contract import validate


UI = "src/pages/Workspace/WorkspacePage.tsx"
UI_TEST = "src/pages/Workspace/WorkspacePage.test.tsx"


class FrontendTestContractTests(unittest.TestCase):
    # The four real offenders from the week to 2026-08-05. Every backend fix
    # that week carried tests; every frontend fix carried none, because nothing
    # on that path ever asked.
    def test_a_user_visible_frontend_change_without_a_test_is_rejected(self) -> None:
        for title in (
            "fix(ui): compact delivery status and complete settings navigation",
            "feat(ui): preview conversation documents in shared right-side tabs",
            "fix(workspace): show task activity as status indicator",
            "fix(ui): settings nav group labels vs items typography",
        ):
            errors = validate(title, "no declaration here", [UI])
            self.assertTrue(errors, f"{title!r} must be rejected")
            self.assertIn("Add a test", errors[0])

    def test_shipping_a_test_alongside_satisfies_it(self) -> None:
        self.assertEqual(validate("fix(ui): x", "", [UI, UI_TEST]), [])
        # A test anywhere counts — a Rust or Python test can cover UI wiring.
        self.assertEqual(validate("fix(ui): x", "", [UI, "tests/test_x.py"]), [])

    def test_types_that_change_nothing_user_visible_are_untouched(self) -> None:
        for title in (
            "chore: bump version to 1.78.5",
            "docs: update readme",
            "refactor(ui): extract a hook",
            "style(ui): reformat",
        ):
            self.assertEqual(validate(title, "", [UI]), [], title)

    def test_backend_only_changes_are_untouched(self) -> None:
        self.assertEqual(validate("fix: delivery guard", "", ["src-tauri/src/x.rs"]), [])

    def test_the_escape_hatch_exists_but_demands_a_real_reason(self) -> None:
        # A gate with no way through would be a dead end, which this product
        # forbids — but the reason has to say something.
        self.assertEqual(
            validate(
                "fix(ui): x",
                "UI-Test: jsdom cannot compute the sticky offset this fixes; verified in the real app",
                [UI],
            ),
            [],
        )
        for placeholder in ("UI-Test: TBD", "UI-Test: n/a", "UI-Test: <reason>", "UI-Test:   "):
            self.assertTrue(
                validate("fix(ui): x", placeholder, [UI]),
                f"{placeholder!r} must not pass",
            )

    def test_a_declaration_inside_a_code_block_does_not_count(self) -> None:
        # Otherwise pasting this very contract into a PR body would satisfy it.
        body = "see the docs:\n```\nUI-Test: some example\n```"
        self.assertTrue(validate("fix(ui): x", body, [UI]))


if __name__ == "__main__":
    unittest.main()
