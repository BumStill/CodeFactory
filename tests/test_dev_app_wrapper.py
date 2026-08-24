import json
import subprocess
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
WRAPPER_PATH = REPO_ROOT / "scripts" / "install-dev-app-wrapper.sh"


class DevAppWrapperTests(unittest.TestCase):
    def setUp(self) -> None:
        self.wrapper = WRAPPER_PATH.read_text(encoding="utf-8")

    def test_wrapper_uses_a_separate_tauri_identifier(self) -> None:
        config_path = REPO_ROOT / "src-tauri" / "tauri.dev.conf.json"
        config = json.loads(config_path.read_text(encoding="utf-8"))
        self.assertEqual(config["identifier"], "com.codefactory.dev")

        self.assertIn(
            'TAURI_CONFIG="${CODEFACTORY_DEV_TAURI_CONFIG:-src-tauri/tauri.dev.conf.json}"',
            self.wrapper,
        )
        self.assertIn('--config "$CF_TAURI_CONFIG"', self.wrapper)

    def test_script_is_valid_bash(self) -> None:
        result = subprocess.run(
            ["bash", "-n", str(WRAPPER_PATH)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_shim_resolves_the_checkout_at_launch_time(self) -> None:
        """The checkout must NOT be baked in at install time.

        A shim that hardcodes its checkout can only ever launch the one it
        was installed from, so an agent working in a git worktree cannot
        live-verify without reinstalling the shared wrapper and breaking it
        for everyone else.
        """
        candidates_lines = [
            line
            for line in self.wrapper.splitlines()
            if line.startswith("CF_CANDIDATES=(")
        ]
        self.assertEqual(
            len(candidates_lines), 1, "expected exactly one candidate list in the shim"
        )
        candidates = candidates_lines[0]

        positions = []
        for token in ('"${CODEFACTORY_DEV_TARGET:-}"', '"$(read_pointer)"', '"$CF_FALLBACK_ROOT"'):
            self.assertIn(token, candidates)
            positions.append(candidates.index(token))
        self.assertEqual(
            positions,
            sorted(positions),
            "resolution order must be env override, then pointer file, then install root",
        )

        # The install-time path is only the last-resort fallback, so the shim
        # must never cd straight into it.
        self.assertIn('cd "$CF_TARGET"', self.wrapper)
        self.assertNotIn('cd "$CF_FALLBACK_ROOT"', self.wrapper)

    def test_shim_skips_a_checkout_that_no_longer_exists(self) -> None:
        """A closed-out worktree must degrade to the fallback, not launch nothing."""
        self.assertIn("looks_like_checkout", self.wrapper)
        self.assertIn('[ -f "$1/src-tauri/tauri.conf.json" ]', self.wrapper)
        self.assertIn("warn: ignoring", self.wrapper)

    def test_installer_exposes_retarget_subcommands(self) -> None:
        for option in ("--target", "--clear-target", "--show"):
            self.assertIn(option, self.wrapper)
        # --target must only rewrite the pointer; rebuilding the bundle from a
        # worktree is what breaks the user's wrapper in the first place.
        self.assertIn("write_pointer", self.wrapper)

    def test_installer_refuses_to_retarget_a_bundle_that_ignores_the_pointer(self) -> None:
        """Silently retargeting a pre-pointer bundle yields evidence from the wrong code."""
        self.assertIn("shim_reads_pointer", self.wrapper)
        self.assertIn("grep -q '^CF_POINTER_FILE=' \"$shim\"", self.wrapper)

    def test_shim_pins_the_window_to_the_primary_display(self) -> None:
        """Screen capture of a window on a secondary display can fail outright."""
        self.assertIn(
            'WINDOW_ORIGIN="${CODEFACTORY_DEV_WINDOW_ORIGIN:-60,60}"', self.wrapper
        )
        # The patch is generated from the target checkout's own window config at
        # launch, so it cannot drift from tauri.conf.json.
        self.assertIn("CF_BASE_CONF", self.wrapper)
        self.assertIn('[ "$CF_WINDOW_ORIGIN" != "off" ] || return 1', self.wrapper)

    def test_shim_wires_the_bundle_identity_runner(self) -> None:
        """Identity has to survive `cargo run` restarting the app after a rebuild.

        `tauri dev` runs the GUI through `cargo run`, and only the first of
        those processes can absorb the LaunchServices record that launching
        this bundle creates.  Wiring a cargo runner makes every build exec
        from inside a .app instead, which needs no such record.
        """
        self.assertIn("CARGO_TARGET_", self.wrapper)
        self.assertIn("_RUNNER", self.wrapper)
        self.assertIn("dev-app-bundle-runner.sh", self.wrapper)
        self.assertTrue(
            (REPO_ROOT / "scripts" / "dev-app-bundle-runner.sh").is_file(),
            "the shim points at a runner that must exist in the repo",
        )

    def test_runner_is_read_from_the_target_checkout(self) -> None:
        """It must track the code under verification, like the checkout does.

        Baking the runner path at install time would keep running the main
        checkout's copy while the agent verifies a worktree.
        """
        self.assertIn('CF_RUNNER="$CF_TARGET/scripts/dev-app-bundle-runner.sh"', self.wrapper)
        self.assertNotIn('CF_RUNNER="$CF_FALLBACK_ROOT', self.wrapper)

    def test_missing_runner_degrades_instead_of_breaking_the_launch(self) -> None:
        """Pointing at a checkout that predates the runner must still boot.

        An unconditional export would hand cargo a runner path that does not
        exist, and `cargo run` would fail outright — turning a lost-automation
        problem into an app that will not start at all.
        """
        self.assertIn('if [ -x "$CF_RUNNER" ]; then', self.wrapper)

    def test_show_reports_whether_the_running_app_kept_its_identity(self) -> None:
        """An anonymous process is indistinguishable from a bad click coordinate.

        Screenshots keep working while every click is refused, so --show has to
        answer this directly rather than leaving an agent to guess.
        """
        self.assertIn("show_running_identity", self.wrapper)
        self.assertIn("lsappinfo", self.wrapper)
        self.assertIn("ANONYMOUS", self.wrapper)
        # show_state is the one entry point every mode prints, so the report
        # cannot be missed.
        show_state = self.wrapper.split("show_state() {", 1)[1]
        self.assertIn("show_running_identity", show_state.split("\n}", 1)[0])


if __name__ == "__main__":
    unittest.main()
