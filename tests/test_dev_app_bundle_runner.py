"""Behavioural tests for the dev-app bundle runner.

`pnpm tauri dev` starts the GUI through `cargo run`, and cargo re-runs it
from scratch after every rebuild.  A bare `target/debug/codefactory` has no
`CFBundleIdentifier` of its own (Tauri only embeds CFBundleName/version), so
macOS can only give it an identity by way of the pending LaunchServices
launch record that `open -a CodeFactoryDev` creates -- and that record is
claimed exactly once, by the first GUI child.  Every hot restart after that
runs an anonymous process, which makes the computer-use frontmost-app gate
reject every click while screenshots keep working.

The runner removes the race entirely: it re-exec's the freshly built binary
from inside a real `.app`, which confers identity unconditionally.
"""

import os
import plistlib
import stat
import subprocess
import sys
import unittest
from pathlib import Path
from tempfile import TemporaryDirectory


REPO_ROOT = Path(__file__).resolve().parents[1]
RUNNER_PATH = REPO_ROOT / "scripts" / "dev-app-bundle-runner.sh"

BUNDLE_NAME = "CodeFactoryDev.app"
BUNDLE_ID = "com.codefactory.dev"


def _fake_binary(path: Path, marker: str) -> None:
    """A stand-in for the cargo-built GUI binary that reports how it was run."""
    path.write_text(
        "#!/bin/bash\n"
        f'echo "MARKER={marker}"\n'
        'echo "EXEC_PATH=$0"\n'
        'echo "ARGS=$*"\n',
        encoding="utf-8",
    )
    path.chmod(path.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)


@unittest.skipUnless(sys.platform == "darwin", "macOS bundle identity only")
class DevAppBundleRunnerTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = TemporaryDirectory()
        self.target_dir = Path(self._tmp.name) / "target" / "debug"
        self.target_dir.mkdir(parents=True)
        # `resource_dir()` in dev keys off a .cargo-lock next to the binary.
        (self.target_dir / ".cargo-lock").write_text("", encoding="utf-8")
        (self.target_dir / "resources" / "skills").mkdir(parents=True)
        self.binary = self.target_dir / "codefactory"
        _fake_binary(self.binary, "v1")
        self.addCleanup(self._tmp.cleanup)

    def run_runner(self, *args: str) -> subprocess.CompletedProcess:
        return subprocess.run(
            ["bash", str(RUNNER_PATH), *args],
            capture_output=True,
            text=True,
            timeout=60,
        )

    @property
    def bundle(self) -> Path:
        return self.target_dir / BUNDLE_NAME

    def test_script_is_valid_bash(self) -> None:
        result = subprocess.run(
            ["bash", "-n", str(RUNNER_PATH)], capture_output=True, text=True
        )
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_execs_the_binary_from_inside_an_app_bundle(self) -> None:
        """The whole point: the running process must live in `*.app/Contents/MacOS`.

        That is the only path that confers a CFBundleIdentifier on every
        launch, as opposed to only on the one that wins the LaunchServices
        launch-record race.
        """
        result = self.run_runner(str(self.binary), "--flag", "value")
        self.assertEqual(result.returncode, 0, result.stderr)
        expected = self.bundle / "Contents" / "MacOS" / "CodeFactoryDev"
        self.assertIn(f"EXEC_PATH={expected}", result.stdout)
        self.assertIn("ARGS=--flag value", result.stdout)

    def test_bundle_declares_the_dev_identity(self) -> None:
        self.run_runner(str(self.binary))
        info = plistlib.loads((self.bundle / "Contents" / "Info.plist").read_bytes())
        self.assertEqual(info["CFBundleIdentifier"], BUNDLE_ID)
        self.assertEqual(info["CFBundleExecutable"], "CodeFactoryDev")
        # request_access resolves the app by this name.
        self.assertEqual(info["CFBundleName"], "CodeFactoryDev")

    def test_hot_restart_picks_up_the_rebuilt_binary(self) -> None:
        """A stale hardlink would silently run the previous build forever.

        cargo replaces the binary in place, so the runner has to relink on
        every launch or the agent would collect evidence against old code --
        which is exactly the failure the wrapper's pointer file guards against
        at the checkout level.
        """
        first = self.run_runner(str(self.binary))
        self.assertIn("MARKER=v1", first.stdout)

        _fake_binary(self.binary, "v2")  # <- the rebuild
        second = self.run_runner(str(self.binary))
        self.assertIn("MARKER=v2", second.stdout, "runner served a stale binary")
        self.assertNotIn("MARKER=v1", second.stdout)

    def test_resources_still_resolve_to_the_cargo_output_directory(self) -> None:
        """Moving the exe into a bundle flips Tauri's resource_dir() lookup.

        In dev, tauri-utils returns the exe's own directory (it sees
        `target/<profile>` plus a .cargo-lock).  From inside `Contents/MacOS`
        that test fails and it falls through to `../Resources`, so the bundle
        has to point that back at the cargo output dir or built-in skills
        disappear from the dev app.
        """
        self.run_runner(str(self.binary))
        resources = self.bundle / "Contents" / "Resources"
        self.assertTrue(resources.exists(), "Contents/Resources missing")
        self.assertEqual(resources.resolve(), self.target_dir.resolve())
        self.assertTrue((resources / "resources" / "skills").is_dir())

    def test_other_binaries_pass_straight_through(self) -> None:
        """The runner is wired in through CARGO_TARGET_<triple>_RUNNER, which
        cargo also applies to `cargo test`.  Test binaries must run untouched.
        """
        deps = self.target_dir / "deps"
        deps.mkdir()
        test_bin = deps / "codefactory-1a2b3c4d"
        _fake_binary(test_bin, "test-binary")
        result = self.run_runner(str(test_bin), "--nocapture")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"EXEC_PATH={test_bin}", result.stdout)
        self.assertIn("ARGS=--nocapture", result.stdout)
        self.assertFalse(self.bundle.exists(), "must not bundle a test binary")

    def test_can_be_disabled(self) -> None:
        """An escape hatch, so a broken bundle can never block `tauri dev`."""
        env = dict(os.environ, CODEFACTORY_DEV_BUNDLE_IDENTITY="0")
        result = subprocess.run(
            ["bash", str(RUNNER_PATH), str(self.binary)],
            capture_output=True,
            text=True,
            env=env,
            timeout=60,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn(f"EXEC_PATH={self.binary}", result.stdout)
        self.assertFalse(self.bundle.exists())


if __name__ == "__main__":
    unittest.main()
