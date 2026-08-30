from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


class ReleaseWorkflowTests(unittest.TestCase):
    @staticmethod
    def _workflow_bash() -> str:
        """Use the same Bash implementation selected by Actions' `shell: bash`."""
        if os.name != "nt":
            return "bash"
        git_executable = shutil.which("git")
        if git_executable is None:
            raise RuntimeError("Git for Windows is required for release workflow tests")
        git_bash = Path(git_executable).resolve().parent.parent / "bin" / "bash.exe"
        if not git_bash.is_file():
            raise RuntimeError(f"Git Bash was not found beside {git_executable}")
        return str(git_bash)

    def _git(self, repo: Path, *args: str) -> None:
        subprocess.run(
            ["git", "-C", str(repo), *args],
            check=True,
            capture_output=True,
            text=True,
        )

    def _new_release_history(self) -> Path:
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        repo = Path(temp_dir.name)
        self._git(repo, "init", "-b", "main")
        self._git(repo, "config", "user.name", "Release Test")
        self._git(repo, "config", "user.email", "release-test@example.invalid")
        self._git(repo, "commit", "--allow-empty", "-m", "chore: initial")
        self._git(repo, "tag", "v1.0.0")
        return repo

    def _commit(self, repo: Path, subject: str, body: str = "") -> None:
        args = ["commit", "--allow-empty", "-m", subject]
        if body:
            args.extend(["-m", body])
        self._git(repo, *args)

    def _release_plan(self, repo: Path, *args: str) -> dict[str, object]:
        result = subprocess.run(
            [
                "python3",
                str(REPO_ROOT / "tools/release/plan_release.py"),
                "--repo",
                str(repo),
                "--range",
                "v1.0.0..HEAD",
                *args,
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        return json.loads(result.stdout)

    def test_adaptive_release_contract_is_auditable_and_never_push_triggered(
        self,
    ) -> None:
        auto_release = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(
            encoding="utf-8"
        )
        principle = (REPO_ROOT / "docs/principles/release-cadence.md").read_text(
            encoding="utf-8"
        )
        agents = (REPO_ROOT / "AGENTS.md").read_text(encoding="utf-8")

        trigger = auto_release.split("permissions:", 1)[0]
        self.assertNotIn("push:", trigger)
        self.assertIn("Release-Urgency: immediate", auto_release)
        self.assertIn("Release-Urgency: hold", auto_release)
        self.assertIn("allow_guarded_batch:", auto_release)
        self.assertIn("tools/release/plan_release.py", auto_release)
        self.assertIn("Release-Urgency", agents)
        self.assertIn(
            "A `hold` remains active for the whole unreleased batch",
            principle,
        )

    def test_guarded_version_pr_uses_auto_merge_under_main_ruleset(self) -> None:
        """A green version PR must enter the protected auto-merge path.

        Direct ``gh pr merge --squash`` is still rejected by the active main
        ruleset even after every required check is green. Run 31475705615 hit
        that exact failure on version PR #358 and stranded the release batch.
        """
        workflow = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(
            encoding="utf-8"
        )
        guarded_merge = workflow.split(
            "- name: Wait for guarded version bump merge", 1
        )[1].split("- name: Tag guarded merge", 1)[0]
        self.assertRegex(
            guarded_merge,
            r'gh pr merge "\$PR"[^\n]*--squash[\s\\]*\n\s*--auto\s+--match-head-commit',
        )

    def test_versioned_ruleset_requires_the_ci_jobs_that_exist(self) -> None:
        """The audited ruleset and workflow may not drift by check name."""
        workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        ruleset = json.loads(
            (REPO_ROOT / ".github/rulesets/main.json").read_text(encoding="utf-8")
        )
        required_rule = next(
            rule
            for rule in ruleset["ruleset"]["rules"]
            if rule["type"] == "required_status_checks"
        )
        required = {
            item["context"]
            for item in required_rule["parameters"]["required_status_checks"]
        }
        for job in ("check-frontend", "check-rust"):
            self.assertIn(f"\n  {job}:\n", workflow)
            self.assertIn(job, required)
        self.assertNotIn("check", required)
        auto_release = (
            REPO_ROOT / ".github/workflows/auto-release.yml"
        ).read_text(encoding="utf-8")
        guarded_merge = auto_release.split(
            "- name: Wait for guarded version bump merge", 1
        )[1].split("- name: Tag guarded merge", 1)[0]
        for context in required:
            self.assertIn(context, guarded_merge)
        self.assertIn(f'if [ "$SUCCESSES" -eq {len(required)} ]', guarded_merge)

    def test_force_cannot_bypass_a_guarded_batch(self) -> None:
        repo = self._new_release_history()
        self._commit(
            repo,
            "fix: restore delivery",
            "Release-Urgency: immediate",
        )
        self._commit(
            repo,
            "feat: prepare staged rollout",
            "Release-Urgency: hold",
        )

        default_plan = self._release_plan(repo)
        forced_plan = self._release_plan(repo, "--force", "true")
        reviewed_plan = self._release_plan(
            repo,
            "--allow-guarded-batch",
            "true",
        )

        self.assertEqual(default_plan["slot"], "minor")
        self.assertEqual(default_plan["immediate"], 1)
        self.assertEqual(default_plan["hold"], 1)
        self.assertTrue(default_plan["skip"])
        self.assertTrue(forced_plan["skip"])
        self.assertFalse(reviewed_plan["skip"])

    def test_invalid_urgency_fails_closed_but_prose_does_not(self) -> None:
        repo = self._new_release_history()
        self._commit(
            repo,
            "fix: clarify release notes",
            "This prose mentions Release-Urgency: hold but is not a trailer.",
        )
        prose_plan = self._release_plan(repo)
        self.assertFalse(prose_plan["skip"])
        self.assertEqual(prose_plan["hold"], 0)

        self._commit(
            repo,
            "fix: malformed urgency",
            "Release-Urgency: soon",
        )
        invalid_plan = self._release_plan(repo)
        self.assertTrue(invalid_plan["skip"])
        self.assertEqual(invalid_plan["invalid_urgency"], 1)

    def test_force_only_overrides_the_no_feat_or_fix_rule(self) -> None:
        repo = self._new_release_history()
        self._commit(repo, "docs: explain cadence")

        default_plan = self._release_plan(repo)
        forced_plan = self._release_plan(repo, "--force", "true")

        self.assertTrue(default_plan["skip"])
        self.assertEqual(default_plan["slot"], "none")
        self.assertFalse(forced_plan["skip"])
        self.assertEqual(forced_plan["slot"], "patch")

    def test_breaking_footer_selects_a_major_release(self) -> None:
        repo = self._new_release_history()
        self._commit(
            repo,
            "fix: change persisted format",
            "BREAKING CHANGE: old databases require migration",
        )

        plan = self._release_plan(repo)

        self.assertFalse(plan["skip"])
        self.assertEqual(plan["slot"], "major")

        hyphenated_repo = self._new_release_history()
        self._commit(
            hyphenated_repo,
            "fix: change wire format",
            "BREAKING-CHANGE: old clients require migration",
        )
        hyphenated_plan = self._release_plan(hyphenated_repo)
        self.assertFalse(hyphenated_plan["skip"])
        self.assertEqual(hyphenated_plan["slot"], "major")

    def test_squash_footer_with_breaking_and_urgencies_stays_major(self) -> None:
        repo = self._new_release_history()
        self._commit(
            repo,
            "fix: change persisted format",
            "Reviewed migration.\n\n"
            "BREAKING CHANGE: old databases require migration\n"
            "Release-Urgency: hold\n"
            "Release-Urgency: immediate",
        )

        guarded_plan = self._release_plan(repo)
        reviewed_plan = self._release_plan(
            repo,
            "--allow-guarded-batch",
            "true",
        )

        self.assertEqual(guarded_plan["slot"], "major")
        self.assertEqual(guarded_plan["hold"], 1)
        self.assertEqual(guarded_plan["immediate"], 1)
        self.assertTrue(guarded_plan["skip"])
        self.assertEqual(reviewed_plan["slot"], "major")
        self.assertFalse(reviewed_plan["skip"])

    # 2026-08-04: the reconcile step aborted with "Release v1.77.3 run
    # succeeded but no published release exists" while v1.77.3 was published,
    # non-draft, with six assets. `gh release view` had been rate-limited, and
    # the script treated every non-zero exit as "the release is absent" —
    # stderr was even discarded with 2>/dev/null. A guard must never turn "I
    # could not check" into a claim about the world.
    def test_reconcile_separates_a_missing_release_from_an_unreadable_one(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(encoding="utf-8")

        self.assertIn("read_release_state()", workflow)
        # 404 is the only signal that actually means "not there".
        self.assertRegex(workflow, r"release not found\|HTTP 404")
        # Anything else is unknown, retried, and then reported as unknown.
        self.assertIn("echo unknown", workflow)
        self.assertIn("could not determine the release state", workflow)
        # The old shape must not come back.
        self.assertNotIn(
            "--json isDraft 2>/dev/null",
            workflow,
            "swallowing stderr is what hid the rate-limit error",
        )
        # An unknown state must stop, never silently dispatch a duplicate.
        unknown_block = workflow.split('if [ "$RELEASE_STATE" = unknown ]; then', 1)[1]
        self.assertLess(
            unknown_block.index("exit 1"),
            unknown_block.index("gh workflow run release.yml"),
            "unknown must exit before any duplicate release dispatch",
        )

    def test_failed_unpublished_tag_advances_when_main_contains_a_release_fix(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(
            encoding="utf-8"
        )
        reconcile = workflow.split(
            "- name: Reconcile interrupted version release", 1
        )[1].split("- name: Determine version slot", 1)[0]

        self.assertIn('if [ "$RUN_CONCLUSION" = "failure" ]', reconcile)
        self.assertIn('tools/release/plan_release.py', reconcile)
        self.assertIn('--range "$TAG..HEAD"', reconcile)
        self.assertIn('abandoned_tag=$TAG', reconcile)
        self.assertIn('recovered=false', reconcile)
        self.assertIn('continuing with a new immutable patch tag', reconcile)
        self.assertLess(
            reconcile.index('abandoned_tag=$TAG'),
            reconcile.rindex('gh workflow run release.yml'),
            "a failed immutable tag with a subsequent fix must advance before the retry path",
        )

    def test_release_scenario_gate_uses_trusted_main_policy_and_published_base(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        prepare = workflow.split("\n  prepare-release:\n", 1)[1].split(
            "\n  build-windows:\n", 1
        )[0]

        self.assertIn("path: policy", prepare)
        self.assertIn("persist-credentials: false", prepare)
        self.assertIn("ref: ${{ github.sha }}", prepare)
        self.assertIn(
            "python3 policy/tools/governance/run_scenario_harness_gate.py",
            prepare,
        )
        self.assertIn("--policy-repo policy", prepare)
        self.assertNotIn("--policy-repo .", prepare)
        self.assertIn("gh release list", prepare)
        self.assertIn("isDraft,isPrerelease,publishedAt,tagName", prepare)
        self.assertIn("previous published release tag", prepare)
        self.assertNotIn('git describe --tags --abbrev=0 "${TAG}^"', prepare)

    def test_release_notes_span_the_previous_published_release_not_a_tombstone(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        changelog = workflow.split("\n  changelog:\n", 1)[1].split(
            "\n  prepare-release:\n", 1
        )[0]

        self.assertIn("Resolve previous published release for notes", changelog)
        self.assertIn("gh release list", changelog)
        self.assertIn('git-cliff "$BASE_TAG..$TAG"', changelog)
        self.assertNotIn("git-cliff --latest", changelog)

    # 2026-08-05: a preflight job was added to auto-release.yml demanding five
    # APPLE_* secrets before any version mutation. release.yml references
    # APPLE_* exactly zero times — the build has never done Apple codesigning,
    # only Tauri updater signing — so the gate demanded credentials the pipeline
    # cannot use, and blocked EVERY release including Windows-only fixes. It was
    # a prerequisite invented out of nothing, and the user was nearly sent to
    # buy an Apple developer certificate to satisfy it.
    #
    # The rule this pins: auto-release may only gate on secrets the release
    # build actually consumes, plus the tokens it needs to drive itself.
    def test_release_gates_may_not_require_secrets_the_build_never_uses(self) -> None:
        import re

        def secrets_in(name: str) -> set[str]:
            text = (REPO_ROOT / ".github/workflows" / name).read_text(encoding="utf-8")
            return set(re.findall(r"secrets\.([A-Z0-9_]+)", text))

        # Tokens auto-release needs for its own git/API work, not for building.
        self_drive = {"GITHUB_TOKEN", "RELEASE_PAT"}
        demanded = secrets_in("auto-release.yml") - self_drive
        consumed = secrets_in("release.yml")
        fabricated = demanded - consumed
        self.assertEqual(
            fabricated,
            set(),
            "auto-release gates on secret(s) that release.yml never uses: "
            f"{sorted(fabricated)}. A prerequisite the build cannot consume is a "
            "fabricated blocker — it stops every release and only the user can "
            "clear it. Either make the build use it, or drop the gate.",
        )

    # `cargo check` and `cargo test` ran over the identical manifest with no
    # `-p` filter, so the same code was compiled twice: `cargo test` cannot run
    # without compiling, which makes the separate check step 62s of pure
    # duplication in a 583s job. Measured on run 31354756400.
    def test_ci_does_not_compile_the_rust_workspace_twice(self) -> None:
        ci = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertNotIn(
            "cargo check --manifest-path src-tauri/Cargo.toml",
            ci,
            "`cargo test` over the same manifest already compiles everything; a "
            "separate unscoped `cargo check` only pays the compile cost twice.",
        )
        # The coverage it was standing in for must still be there.
        self.assertIn("cargo test --manifest-path src-tauri/Cargo.toml", ci)

    def test_auto_release_dispatches_tag_from_main_for_shared_cache_scope(self) -> None:
        release = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        auto_release = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(
            encoding="utf-8"
        )

        trigger = release.split("permissions:", 1)[0]
        self.assertIn("workflow_dispatch:", trigger)
        self.assertIn("tag:", trigger)
        self.assertNotIn("push:", trigger)
        self.assertIn("actions: write", auto_release)
        self.assertIn(
            'gh workflow run release.yml --ref main -f tag="$TAG"', auto_release
        )

    def test_release_prepares_one_draft_then_builds_platforms_in_parallel(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("prepare-release:", workflow)
        self.assertEqual(
            workflow.count("needs: [changelog, prepare-release]"),
            2,
            "Windows and macOS must share the same prerequisites",
        )
        self.assertIn(
            "needs: [changelog, prepare-release, build-windows, build-macos]",
            workflow,
        )
        self.assertIn("gh release create", workflow)
        self.assertIn("--draft", workflow)
        self.assertIn("releases?per_page=100", workflow)
        self.assertIn("| .draft", workflow)
        self.assertEqual(workflow.count("includeUpdaterJson: false"), 2)

    def test_release_builds_the_requested_tag_not_the_dispatch_ref(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )

        self.assertEqual(workflow.count("ref: ${{ inputs.tag }}"), 1)
        self.assertGreaterEqual(
            workflow.count("ref: ${{ needs.prepare-release.outputs.tag_sha }}"), 4
        )
        self.assertIn(
            "CODEFACTORY_BUILD_GIT_SHA: "
            "${{ needs.prepare-release.outputs.tag_sha }}",
            workflow,
        )
        self.assertNotIn("github.ref_name", workflow)
        self.assertNotIn("GITHUB_REF_NAME", workflow)

    def test_release_freezes_one_authorized_tag_sha_for_every_build_and_mutation(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )

        expected_input = workflow.split("expected_head_sha:", 1)[1].split(
            "concurrency:", 1
        )[0]
        self.assertIn("required: false", expected_input)
        self.assertIn('default: ""', expected_input)
        self.assertIn(
            "authorized_tag_sha: ${{ steps.authorize.outputs.authorized_tag_sha }}",
            workflow,
        )
        self.assertIn(
            'echo "authorized_tag_sha=$AUTHORIZED_TAG_SHA" >> "$GITHUB_OUTPUT"',
            workflow,
        )
        self.assertGreaterEqual(
            workflow.count("ref: ${{ needs.prepare-release.outputs.tag_sha }}"),
            4,
            "prepare/build/finalize/published verification must checkout the frozen commit, not a mutable tag",
        )
        self.assertNotIn("ref: ${{ inputs.tag }}", workflow.split("prepare-release:", 1)[1])
        self.assertGreaterEqual(
            workflow.count("Require remote release tag to remain authorized"),
            4,
            "draft creation, both build uploads, and final publication must all recheck tag identity",
        )
        # The comparison itself now lives in one executable guard rather than
        # being copy-pasted into every job that mutates the release.
        guard = (REPO_ROOT / "tools/release/require_authorized_tag.sh").read_text(
            encoding="utf-8"
        )
        self.assertIn('REMOTE_TAG_SHA="$(git rev-parse "${TAG}^{commit}")"', guard)
        self.assertIn(
            'if [ "$REMOTE_TAG_SHA" != "$AUTHORIZED_TAG_SHA" ]; then', guard
        )

    def _moved_tag_fixture(self) -> tuple[Path, Path, str, str]:
        """A bare origin whose tag can move behind a runner's back.

        Returns (runner, mover, authorized_sha, moved_sha). `runner` has already
        fetched the tag at `authorized_sha`; `mover` is a second clone used to
        force-push the tag somewhere else, exactly like a human retagging while a
        release build is still compiling.
        """
        temp_dir = tempfile.TemporaryDirectory()
        self.addCleanup(temp_dir.cleanup)
        root = Path(temp_dir.name)

        origin = root / "origin.git"
        subprocess.run(
            ["git", "init", "--bare", "-b", "main", str(origin)],
            check=True,
            capture_output=True,
            text=True,
        )

        runner = root / "runner"
        subprocess.run(
            ["git", "clone", str(origin), str(runner)],
            check=True,
            capture_output=True,
            text=True,
        )
        self._git(runner, "config", "user.name", "Release Test")
        self._git(runner, "config", "user.email", "release-test@example.invalid")
        self._commit(runner, "chore: authorized commit")
        authorized_sha = subprocess.run(
            ["git", "-C", str(runner), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        self._git(runner, "tag", "v9.9.9")
        self._git(runner, "push", "origin", "main", "--tags")

        mover = root / "mover"
        subprocess.run(
            ["git", "clone", str(origin), str(mover)],
            check=True,
            capture_output=True,
            text=True,
        )
        self._git(mover, "config", "user.name", "Release Test")
        self._git(mover, "config", "user.email", "release-test@example.invalid")
        self._commit(mover, "chore: someone retagged mid-build")
        moved_sha = subprocess.run(
            ["git", "-C", str(mover), "rev-parse", "HEAD"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
        self._git(mover, "push", "origin", "main")

        return runner, mover, authorized_sha, moved_sha

    def _guard(self, repo: Path, tag: str, authorized: str, context: str,
               mutation_log: Path | None = None) -> subprocess.CompletedProcess[str]:
        """Run the guard, optionally chaining a stand-in mutation behind it."""
        # The command is executed by Git Bash on Windows. A native
        # ``D:\\...`` path is parsed as a shell command with escape characters,
        # while the forward-slash spelling is accepted by both Git Bash and
        # POSIX shells.
        guard = (REPO_ROOT / "tools/release/require_authorized_tag.sh").as_posix()
        command = 'bash "$1" "$2" "$3" "$4"'
        if mutation_log is not None:
            command += f' && echo mutated >> "$5"'
        args = [
            self._workflow_bash(),
            "-c",
            command,
            "guard",
            guard,
            tag,
            authorized,
            context,
        ]
        if mutation_log is not None:
            args.append(mutation_log.as_posix())
        return subprocess.run(args, cwd=repo, capture_output=True, text=True)

    def test_a_tag_moved_mid_build_blocks_the_mutation_that_follows_it(self) -> None:
        runner, mover, authorized_sha, moved_sha = self._moved_tag_fixture()
        log = runner / "mutations.log"

        allowed = self._guard(runner, "v9.9.9", authorized_sha, "upload assets", log)
        self.assertEqual(
            allowed.returncode,
            0,
            f"stdout:\n{allowed.stdout}\nstderr:\n{allowed.stderr}",
        )
        self.assertEqual(
            log.read_text(encoding="utf-8").count("mutated"),
            1,
            "an unmoved tag must let the release mutation proceed",
        )

        # Someone retags while the build is still compiling.
        self._git(mover, "tag", "-f", "v9.9.9")
        self._git(mover, "push", "--force", "origin", "refs/tags/v9.9.9")

        blocked = self._guard(runner, "v9.9.9", authorized_sha, "upload assets", log)
        self.assertEqual(
            blocked.returncode,
            1,
            "a moved tag must fail closed, not warn",
        )
        self.assertIn("::error::release tag moved", blocked.stderr)
        self.assertIn(moved_sha, blocked.stderr)
        self.assertIn("blocked before: upload assets", blocked.stderr)
        self.assertEqual(
            log.read_text(encoding="utf-8").count("mutated"),
            1,
            "publish/tag/build/upload count after the tag moved must be 0",
        )

    def test_the_guard_never_mutates_the_repository_it_checks(self) -> None:
        runner, mover, authorized_sha, _ = self._moved_tag_fixture()
        self._git(mover, "tag", "-f", "v9.9.9")
        self._git(mover, "push", "--force", "origin", "refs/tags/v9.9.9")

        def remote_state() -> str:
            return subprocess.run(
                ["git", "-C", str(runner), "ls-remote", "origin"],
                check=True,
                capture_output=True,
                text=True,
            ).stdout

        before = remote_state()
        blocked = self._guard(runner, "v9.9.9", authorized_sha, "publish release")
        self.assertEqual(blocked.returncode, 1, blocked.stderr)
        self.assertEqual(
            before,
            remote_state(),
            "the guard must be read-only; a failed check may not rewrite the remote",
        )

    @staticmethod
    def _job_steps(workflow: str, job: str) -> list[str]:
        """Split one job's `steps:` block into individual step texts."""
        body = workflow.split(f"\n  {job}:\n", 1)[1]
        # A job ends where the next top-level job begins.
        body = re.split(r"\n  [a-z][a-z0-9-]*:\n", body, maxsplit=1)[0]
        steps = re.split(r"\n(?=      - (?:name|uses):)", body)
        return [step for step in steps if step.strip().startswith("- ")]

    def test_every_slow_release_mutation_rechecks_the_tag_next_to_it(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        guard = "tools/release/require_authorized_tag.sh"

        self.assertIn(
            guard,
            workflow,
            "the tag guard must be a real executable, not prose duplicated per job",
        )
        self.assertNotIn(
            'REMOTE_TAG_SHA="$(git rev-parse "${TAG}^{commit}")"',
            workflow,
            "the comparison belongs in the guard script, not copy-pasted per job",
        )

        # A check when the job starts is not enough: a full Tauri compile sits
        # between it and tauri-action's upload. Require a guarded step on both
        # sides of every upload.
        for job in ("build-windows", "build-macos"):
            steps = self._job_steps(workflow, job)
            upload = [
                i for i, step in enumerate(steps)
                if "tauri-apps/tauri-action@v0" in step
            ]
            self.assertEqual(
                len(upload), 1, f"{job} must upload through exactly one tauri-action step"
            )
            index = upload[0]
            self.assertIn(
                guard,
                steps[index - 1],
                f"{job} must recheck the tag in the step immediately before uploading",
            )
            self.assertIn(
                guard,
                steps[index + 1],
                f"{job} must recheck the tag immediately after uploading, so a tag "
                f"moved during the compile cannot leave assets on the wrong tag",
            )

        # Publication is the one mutation that cannot be undone, and the
        # latest.json assembly ahead of it is itself slow.
        publish_step = [
            step for step in self._job_steps(workflow, "finalize")
            if 'gh release edit   "$TAG"' in step
        ]
        self.assertEqual(len(publish_step), 1)
        head = publish_step[0].split("gh release upload", 1)[0]
        self.assertIn(
            guard,
            head.rsplit("cat latest.json", 1)[-1],
            "the final upload and publish must be guarded adjacently, not only at "
            "the top of finalize",
        )
        self.assertNotIn(
            guard,
            publish_step[0].split("gh release upload", 1)[1],
            "the guard belongs ahead of the irreversible mutations, not between them",
        )
        self.assertIn(
            "AUTHORIZED_TAG_SHA: ${{ needs.prepare-release.outputs.tag_sha }}",
            publish_step[0],
            "the publish step must carry the frozen SHA the guard compares against",
        )

        self.assertIn(
            "needs: [changelog, prepare-release, build-windows, build-macos]",
            workflow,
            "a failed post-upload recheck must keep the draft unpublished: only "
            "finalize publishes, and it may not run unless both builds passed",
        )

    def test_latest_manifest_carries_the_binary_build_identity(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )

        build_identity = (
            "CODEFACTORY_BUILD_GIT_SHA: "
            "${{ needs.prepare-release.outputs.tag_sha }}"
        )
        self.assertEqual(
            workflow.count(build_identity),
            2,
            "Windows and macOS binaries must embed the exact tag commit",
        )

        finalize = workflow.split("\n  finalize:\n", 1)[1].split(
            "\n  verify-published-macos:\n", 1
        )[0]
        self.assertIn(
            "needs: [changelog, prepare-release, build-windows, build-macos]",
            finalize,
        )
        self.assertIn(
            "BUILD_GIT_SHA: ${{ needs.prepare-release.outputs.tag_sha }}",
            finalize,
        )
        self.assertIn('--arg build_git_sha "$BUILD_GIT_SHA"', finalize)
        self.assertIn("build_git_sha:$build_git_sha", finalize)

    def test_published_macos_verifier_matches_manifest_to_both_app_binaries(
        self,
    ) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        published_job = workflow.split("\n  verify-published-macos:\n", 1)[1]
        artifact_smoke = (
            REPO_ROOT / "scripts/verify-macos-release-artifact.sh"
        ).read_text(encoding="utf-8")

        self.assertIn('LATEST_MANIFEST="$RELEASE_DIR/latest.json"', published_job)
        self.assertIn('"$PUBLIC_BASE/latest.json"', published_job)
        self.assertIn('"$UPDATER_ARCHIVE" "$LATEST_MANIFEST"', published_job)

        self.assertIn("[CodeFactory.app.tar.gz] [latest.json]", artifact_smoke)
        self.assertIn(
            'MANIFEST_BUILD_SHA="$(/usr/bin/plutil -extract build_git_sha raw',
            artifact_smoke,
        )
        self.assertIn(
            'MANIFEST_VERSION="$(/usr/bin/plutil -extract version raw',
            artifact_smoke,
        )
        self.assertIn(
            '[[ "$MANIFEST_BUILD_SHA" != "$EXPECTED_BUILD_SHA" ]]', artifact_smoke
        )
        self.assertIn("DMG_EXECUTABLE_SHA256", artifact_smoke)
        self.assertIn("UPDATER_EXECUTABLE_SHA256", artifact_smoke)
        self.assertIn("release build identity matched", artifact_smoke)

        manual_smoke = (
            REPO_ROOT / ".github/workflows/macos-release-smoke.yml"
        ).read_text(encoding="utf-8")
        self.assertIn("ref: ${{ inputs.tag }}", manual_smoke)
        self.assertIn("CodeFactory_aarch64.app.tar.gz", manual_smoke)
        self.assertIn("latest.json", manual_smoke)
        self.assertIn('EXPECTED_BUILD_SHA="$(git rev-parse HEAD)"', manual_smoke)
        self.assertIn(
            '"$UPDATER_ARCHIVE" "$DMG_DIR/latest.json"', manual_smoke
        )

    def test_release_automatically_reverifies_the_published_macos_asset(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("verify-published-macos:", workflow)
        job = workflow.split("verify-published-macos:", 1)[1]
        self.assertIn("needs: [finalize, prepare-release]", job)
        self.assertIn("runs-on: macos-latest", job)
        self.assertNotIn("GH_TOKEN:", job)
        self.assertIn("env -u GH_TOKEN curl", job)
        self.assertIn("--retry 4", job)
        self.assertIn("--retry-all-errors", job)
        self.assertIn("scripts/verify-macos-release-artifact.sh", job)
        self.assertIn('tee "$CODEFACTORY_RELEASE_EVIDENCE_DIR/verification.log"', job)
        self.assertIn("if: always()", job)
        self.assertIn("actions/upload-artifact@v4", job)

        ci_workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn("python -m unittest tests.test_release_workflow", ci_workflow)

    def test_missing_apple_credentials_never_block_the_existing_release_path(self) -> None:
        release = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        auto_release = (REPO_ROOT / ".github/workflows/auto-release.yml").read_text(
            encoding="utf-8"
        )

        self.assertNotIn("macos-signing-preflight:", auto_release)
        self.assertNotIn("needs: [release-token-preflight, macos-signing-preflight]", auto_release)
        self.assertNotIn("macos-signing-preflight:", release)
        self.assertNotIn("needs: macos-signing-preflight", release)

        macos_job = release.split("\n  build-macos:\n", 1)[1].split(
            "\n  finalize:\n", 1
        )[0]
        self.assertIn("TAURI_SIGNING_PRIVATE_KEY", macos_job)
        self.assertIn("TAURI_SIGNING_PRIVATE_KEY_PASSWORD", macos_job)
        self.assertNotIn("APPLE_CERTIFICATE", macos_job)
        self.assertNotIn("APPLE_API_", macos_job)
        self.assertNotIn("scripts/import-apple-signing-identity.sh", macos_job)

    def test_macos_compatibility_channel_uses_full_adhoc_bundle_signing(self) -> None:
        tauri_config = json.loads(
            (REPO_ROOT / "src-tauri/tauri.conf.json").read_text(encoding="utf-8")
        )

        self.assertEqual(
            tauri_config["bundle"]["macOS"].get("signingIdentity"),
            "-",
            "the no-Apple-credential compatibility channel still needs a complete "
            "ad-hoc app-bundle signature, not the linker-only Mach-O signature",
        )

    def test_macos_artifact_verifier_fences_execution_with_signatures_and_sha(
        self,
    ) -> None:
        artifact_smoke = (
            REPO_ROOT / "scripts/verify-macos-release-artifact.sh"
        ).read_text(encoding="utf-8")

        strict_verification = (
            '/usr/bin/codesign --verify --deep --strict --verbose=4 "$app_path"'
        )
        self.assertIn(strict_verification, artifact_smoke)
        self.assertIn('verify_app_bundle "$INSTALLED_APP" "DMG"', artifact_smoke)
        self.assertIn('verify_app_bundle "$UPDATER_APP" "updater"', artifact_smoke)
        self.assertLess(
            artifact_smoke.index('verify_app_bundle "$INSTALLED_APP" "DMG"'),
            artifact_smoke.index('"$EXECUTABLE_PATH" --evolution-smoke'),
            "the downloaded app must pass strict signature verification before it executes",
        )
        self.assertLess(
            artifact_smoke.index('verify_app_bundle "$UPDATER_APP" "updater"'),
            artifact_smoke.index('"$EXECUTABLE_PATH" --evolution-smoke'),
            "the public updater app must also be verified before any release app executes",
        )
        self.assertIn("build_git_sha", artifact_smoke)
        self.assertIn("EXPECTED_BUILD_SHA", artifact_smoke)
        self.assertNotIn("Developer ID Application:", artifact_smoke)
        self.assertNotIn("xcrun stapler validate", artifact_smoke)
        self.assertNotIn("spctl --assess", artifact_smoke)

    def test_macos_updater_archive_is_link_safe_and_matches_the_dmg_binary(
        self,
    ) -> None:
        artifact_smoke = (
            REPO_ROOT / "scripts/verify-macos-release-artifact.sh"
        ).read_text(encoding="utf-8")

        self.assertIn('tar -tvzf "$UPDATER_ARCHIVE"', artifact_smoke)
        self.assertIn("only regular files and directories", artifact_smoke)
        self.assertIn("DMG_EXECUTABLE_SHA256", artifact_smoke)
        self.assertIn("UPDATER_EXECUTABLE_SHA256", artifact_smoke)
        self.assertIn(
            '[[ "$DMG_EXECUTABLE_SHA256" != "$UPDATER_EXECUTABLE_SHA256" ]]',
            artifact_smoke,
        )

    def test_macos_release_keeps_real_app_and_public_updater_verification(self) -> None:
        release = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        artifact_smoke = (
            REPO_ROOT / "scripts/verify-macos-release-artifact.sh"
        ).read_text(encoding="utf-8")

        self.assertIn("--evolution-smoke", artifact_smoke)
        self.assertIn("CODEFACTORY_GUI_PROOF_TIER", release)
        build_job = release.split("\n  build-macos:\n", 1)[1].split(
            "\n  finalize:\n", 1
        )[0]
        self.assertIn("UPDATER_ARCHIVE", build_job)
        self.assertIn("needs.prepare-release.outputs.tag_sha", build_job)
        published_job = release.split("\n  verify-published-macos:\n", 1)[1]
        self.assertIn("env -u GH_TOKEN curl", published_job)
        self.assertIn("CodeFactory_aarch64.app.tar.gz", published_job)
        self.assertIn('EXPECTED_BUILD_SHA="$(git rev-parse HEAD)"', published_job)
        self.assertIn("UPDATER_ARCHIVE", published_job)
        self.assertIn("scripts/verify-macos-release-artifact.sh", published_job)
        self.assertIn("macos-published-release-gui-evidence", published_job)

    def test_macos_chrome_attach_gate_uses_the_exact_installed_artifact(self) -> None:
        """RTE-003 must run before the temporary DMG install is removed.

        Run 32843403103 verified a temporary install, removed it, and then a
        separate workflow step searched /Applications for an app that had
        never been copied there.  That shape can never exercise the candidate
        artifact and must not return.
        """
        release = (REPO_ROOT / ".github/workflows/release.yml").read_text(
            encoding="utf-8"
        )
        artifact_smoke = (
            REPO_ROOT / "scripts/verify-macos-release-artifact.sh"
        ).read_text(encoding="utf-8")
        attach_cli = (REPO_ROOT / "src-tauri/src/lib.rs").read_text(encoding="utf-8")

        build_job = release.split("\n  build-macos:\n", 1)[1].split(
            "\n  finalize:\n", 1
        )[0]
        self.assertNotIn("find /Applications -path '*/CodeFactory.app", build_job)
        self.assertNotIn(
            "- name: Verify installed macOS release can attach to existing Chrome",
            build_job,
        )
        self.assertIn("CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT", build_job)

        exact_binary_call = (
            '"$EXECUTABLE_PATH" --browser-chrome-attach-smoke '
            '"$CODEFACTORY_BROWSER_CHROME_ATTACH_RECEIPT"'
        )
        self.assertIn(exact_binary_call, artifact_smoke)
        self.assertGreater(
            artifact_smoke.index(exact_binary_call),
            artifact_smoke.index('verify_app_bundle "$INSTALLED_APP" "DMG"'),
        )
        self.assertIn("CODEFACTORY_BROWSER_CHROME_FIXTURE", artifact_smoke)
        self.assertIn('CODEFACTORY_BROWSER_CHROME_FIXTURE="managed"', artifact_smoke)
        self.assertIn("LocalNetworkAccessAllowedForUrls", artifact_smoke)
        self.assertIn("LoopbackNetworkAllowedForUrls", artifact_smoke)
        # Chromium's policy contract uses the bare wildcard to grant every
        # origin, including extension and opaque worker origins.  The narrower
        # looking `chrome-extension://*` pattern left the MV3 worker unable to
        # reach loopback on Chrome 142+ and kept releases in draft.
        self.assertIn("-json '[\"*\"]'", artifact_smoke)
        self.assertNotIn('"chrome-extension://*"', artifact_smoke)
        self.assertIn("RTE003_POLICY_INSTALLED", artifact_smoke)
        self.assertIn('sudo -n /bin/test -e "$policy_file"', artifact_smoke)
        self.assertNotIn("sudo -n /usr/bin/test", artifact_smoke)
        self.assertIn(
            'sudo -n /usr/bin/plutil -create xml1 "$policy_file"',
            artifact_smoke,
        )
        self.assertIn(
            'sudo -n /usr/bin/plutil -insert LocalNetworkAccessAllowedForUrls',
            artifact_smoke,
        )
        self.assertIn(
            'sudo -n /usr/bin/plutil -insert LoopbackNetworkAllowedForUrls',
            artifact_smoke,
        )
        self.assertIn(
            'sudo -n /usr/bin/plutil -extract "$policy_key" raw -expect array',
            artifact_smoke,
        )
        self.assertIn(
            'sudo -n /usr/bin/plutil -extract "$policy_key.0" raw -expect string',
            artifact_smoke,
        )
        self.assertNotIn(
            'sudo -n /usr/bin/plutil -extract "$policy_key" json',
            artifact_smoke,
        )
        self.assertNotIn("/usr/bin/defaults write", artifact_smoke)
        self.assertIn('"phase":"policy_setup"', artifact_smoke)
        self.assertLess(
            artifact_smoke.index('"phase":"policy_setup"'),
            artifact_smoke.index("if ! sudo -n true"),
        )
        self.assertIn('sudo -n /bin/rm -f "$policy_file"', artifact_smoke)
        self.assertNotIn("/Applications/Google Chrome.app", artifact_smoke)
        for field in (
            "connection_kind",
            "tab_observation_ok",
            "detached_without_managed_close",
            "lease_reclaimed_after_detach",
            "browser_process_alive_after_detach",
        ):
            self.assertIn(field, artifact_smoke)

        # The exact binary owns the native bridge and a synthetic, paired
        # browser fixture.  Merely moving the old invocation into the script
        # would still fail because the CLI previously started no bridge.
        self.assertIn("let pairing = bridge.start().await", attach_cli)
        self.assertIn("browser::download::ensure_installed", attach_cli)
        self.assertIn("CODEFACTORY_BROWSER_CHROME_FIXTURE", attach_cli)
        self.assertIn("browser_process_alive_after_detach", attach_cli)
        # The exact-artifact smoke must not rewrite or reuse the user's stable
        # extension package. A fresh package and Chrome profile isolate this
        # bridge from any installed CodeFactory/Chrome instance on the runner.
        self.assertNotIn("extension_package::prepare", attach_cli)
        self.assertIn('prefix("browser-extension-fixture-")', attach_cli)
        self.assertIn("extension_package::materialize", attach_cli)
        self.assertIn("extension_package::write_pairing", attach_cli)
        self.assertNotIn('format!("--load-extension=', attach_cli)
        self.assertIn('"--enable-unsafe-extension-debugging"', attach_cli)
        self.assertIn('"--remote-debugging-port=0"', attach_cli)
        self.assertIn("Extensions.loadUnpacked", attach_cli)
        self.assertIn('"browser_fixture_version"', attach_cli)
        self.assertIn('"scenario_id": "RTE-003"', attach_cli)
        self.assertIn('"status": "failed"', attach_cli)
        self.assertNotIn('.arg("--headless=new")', attach_cli)

    def test_ci_runs_agent_bridge_and_evaluation_tests_on_linux(self) -> None:
        workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(
            encoding="utf-8"
        )

        self.assertIn("agent-bridge-linux:", workflow)
        job = workflow.split("agent-bridge-linux:", 1)[1]
        self.assertIn("runs-on: ubuntu-latest", job)
        self.assertIn("python-version: '3.12'", job)
        self.assertIn("harbor==0.15.0", job)
        self.assertIn("python -m unittest discover -s tests -p 'test_*.py'", job)

    def _check_job_steps(self, job: str = "check-rust") -> dict[str, str]:
        """Map every `- name:` step in one ci.yml job to its own block.

        PyYAML is not installed in the jobs that run this module, so the steps
        are split on the `      - name:` indentation ci.yml uses rather than
        parsed. Steps are keyed by name; the value is that step's text up to
        the next step.
        """
        workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(
            encoding="utf-8"
        )
        self.assertIn(f"\n  {job}:\n", workflow)
        job = workflow.split(f"\n  {job}:\n", 1)[1]

        steps: dict[str, str] = {}
        current: str | None = None
        for line in job.splitlines():
            if line.startswith("      - name: "):
                current = line[len("      - name: ") :].strip()
                steps[current] = ""
            elif line.startswith("  ") and not line.startswith("    "):
                break  # next top-level job
            elif current is not None:
                steps[current] += line + "\n"
        return steps

    def test_a_frontend_failure_does_not_hide_the_rust_suite(self) -> None:
        """A frontend failure must not hide the Rust suite.

        Run 30795455214 lost a timing-sensitive Vitest case on a loaded Windows
        runner and skipped the entire Rust suite — on a PR whose only change was
        Rust. That was patched with `!cancelled()` on every Rust step; the real
        fix is that they no longer share a job at all, so a frontend failure
        cannot reach them. Both properties are asserted: separate jobs, and no
        residual coupling inside the Rust job.
        """
        workflow = (REPO_ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8")
        self.assertIn("\n  check-frontend:\n", workflow)
        self.assertIn("\n  check-rust:\n", workflow)
        self.assertNotIn(
            "\n  check:\n",
            workflow,
            "the combined job is what made one Vitest flake skip every Rust gate",
        )
        # Parallel, not chained: a `needs:` on the frontend job would rebuild
        # the very coupling this split removes.
        rust_job = workflow.split("\n  check-rust:\n", 1)[1].split("\n  ", 1)[0]
        self.assertNotIn("needs:", rust_job, "check-rust must not wait on the frontend")

        steps = self._check_job_steps()
        front = self._check_job_steps("check-frontend")

        rust_steps = [
            # `Cargo check` intentionally absent: `Cargo test` compiles the same
            # manifest, so a separate check step only doubled the compile cost.
            "Cargo test",
            "Cargo test (agent-loop crate)",
            "Cargo test (agent-headless crate)",
            "Evolution executable closed-loop smoke",
            "Headless AgentLoop construction smoke",
            "Browser session lifecycle smoke",
        ]
        for name in rust_steps:
            self.assertIn(name, steps, f"`check-rust` lost its {name!r} step")
            block = steps[name]
            self.assertIn(
                "!cancelled()",
                block,
                f"{name!r} runs only when every earlier step passed, so one "
                "unrelated frontend failure hides it",
            )
            # `always()` would keep a 2x-billed Windows runner busy after a
            # human cancels the run.
            self.assertNotIn("if: always()", block, f"{name!r} ignores cancellation")
            # Gating a Rust step on the frontend is the exact coupling this
            # test exists to prevent.
            self.assertNotIn(
                "steps.deps.outcome",
                block,
                f"{name!r} is gated on the frontend install",
            )

        self.assertIn("Vitest", front)
        self.assertIn("id: deps", front["Install frontend deps"])

    def test_viewport_evidence_is_required_only_when_its_gate_ran(self) -> None:
        """Skipped gates must not manufacture a second failure.

        Both uploads are `if-no-files-found: error` because the evidence is
        mandatory. Combined with `if: always()` that turned one skipped gate
        into an extra red step in run 30795455214.
        """
        steps = self._check_job_steps("check-frontend")

        for upload, gate in (
            ("Upload evolution viewport evidence", "evolution_gate"),
            ("Upload resume journal viewport evidence", "resume_gate"),
        ):
            self.assertIn(upload, steps)
            block = steps[upload]
            self.assertIn("if-no-files-found: error", block)
            self.assertIn(f"steps.{gate}.outcome != 'skipped'", block)
            self.assertNotIn("if: always()", block)


if __name__ == "__main__":
    unittest.main()
