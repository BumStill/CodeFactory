"""M131d / R1+R2: web_fetch 属于被登记的产品面，且 PR 与发版阶段对覆盖缺口判定一致。

回归背景（release_artifact 假红 + PR 假绿）：
- 真实 web_fetch 变更（#793）在 pull_request 阶段因为同一批次里有全局产品文件
  （Cargo.toml / Cargo.lock）而被 expand_global_files 展开到全部场景，
  于是"未映射的产品文件"从不报错（假绿）；
- 同一个批次在 release_artifact 阶段关闭展开后被判成覆盖缺口（假红），
  且缺口信息里混进了版本清单文件，看不出真正的未映射文件是哪一个。
"""

import importlib.util
import json
import subprocess
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
GOVERNANCE = REPO_ROOT / "tools" / "governance" / "validate_scenario_test_governance.py"
REGISTRY_PATH = REPO_ROOT / "docs" / "testing" / "scenario-registry.json"

WEB_FETCH_FILES = [
    "src-tauri/src/tools/web_fetch.rs",
    "src-tauri/src/tools/fixtures/web-fetch-wiki.html",
]

UNMAPPED_PROBE = "src-tauri/src/tools/scenario_gap_probe_not_registered.rs"
GLOBAL_MANIFEST = "src-tauri/Cargo.lock"


def _load_governance():
    spec = importlib.util.spec_from_file_location("scenario_governance", GOVERNANCE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _manifest_text(path, version):
    """One-file-per-line manifest shapes the exemption predicate fullmatches."""

    if path in ("package.json", "src-tauri/tauri.conf.json"):
        return '{\n  "version": "%s"\n}\n' % version
    return '[package]\nname = "codefactory"\nversion = "%s"\n' % version


def _registry():
    return json.loads(REGISTRY_PATH.read_text(encoding="utf-8"))


class WebFetchScenarioRegistrationTest(unittest.TestCase):
    def setUp(self):
        self.gov = _load_governance()
        self.registry = _registry()

    def test_every_web_fetch_product_file_maps_to_a_real_pull_request_scenario(self):
        scenarios = self.gov._impacted_scenarios(
            WEB_FETCH_FILES, self.registry, expand_global_files=False
        )
        self.assertTrue(
            scenarios,
            "web_fetch 产品文件没有任何场景映射："
            + ", ".join(sorted(WEB_FETCH_FILES)),
        )
        evidence = 0
        for scenario_id in scenarios:
            scenario = next(
                s for s in self.registry["scenarios"] if s["id"] == scenario_id
            )
            self.assertIn("pull_request", scenario["gates"], scenario_id)
            for target in scenario["automated_by"]:
                self.assertTrue(
                    self.gov._automation_exists(target, REPO_ROOT),
                    f"{scenario_id} 绑定的自动化不存在：{target}",
                )
                stages = self.gov._automation_gate_stages(
                    target, self.registry["gate_policy"], REPO_ROOT
                )
                if "pull_request" in stages:
                    evidence += 1
        self.assertGreater(
            evidence, 0, "web_fetch 场景必须在 pull_request 阶段由真实测试承接"
        )

    def test_web_fetch_fixture_and_module_share_the_same_scenario(self):
        module_hits = self.gov._impacted_scenarios(
            ["src-tauri/src/tools/web_fetch.rs"], self.registry, expand_global_files=False
        )
        fixture_hits = self.gov._impacted_scenarios(
            ["src-tauri/src/tools/fixtures/web-fetch-wiki.html"],
            self.registry,
            expand_global_files=False,
        )
        self.assertEqual(module_hits, fixture_hits)


class CoverageGapStageParityTest(unittest.TestCase):
    def setUp(self):
        self.gov = _load_governance()
        self.registry = _registry()

    def test_pull_request_stage_reports_the_unmapped_product_file(self):
        # #793 的真实形状：全局清单文件 + 一个未登记的产品文件。
        errors = self.gov.validate_impacted_execution(
            self.registry,
            [GLOBAL_MANIFEST, UNMAPPED_PROBE],
            "pull_request",
            REPO_ROOT,
            expand_global_files=True,
            fail_on_unmapped=True,
        )
        self.assertTrue(errors, "PR 阶段必须报出未映射的产品文件")
        joined = "\n".join(errors)
        self.assertIn(UNMAPPED_PROBE, joined)

    def test_both_stages_report_the_same_unmapped_file(self):
        files = [GLOBAL_MANIFEST, UNMAPPED_PROBE]
        pr_errors = self.gov.validate_impacted_execution(
            self.registry, files, "pull_request", REPO_ROOT,
            expand_global_files=True, fail_on_unmapped=True,
        )
        release_errors = self.gov.validate_impacted_execution(
            self.registry, files, "release_artifact", REPO_ROOT,
            expand_global_files=False, fail_on_unmapped=True,
        )
        self.assertTrue(pr_errors)
        self.assertTrue(release_errors)
        for errors in (pr_errors, release_errors):
            self.assertIn(UNMAPPED_PROBE, "\n".join(errors))

    def test_registered_web_fetch_change_is_not_a_gap(self):
        for stage, expand in (("pull_request", True), ("release_artifact", False)):
            errors = self.gov.validate_impacted_execution(
                self.registry, WEB_FETCH_FILES, stage, REPO_ROOT,
                expand_global_files=expand, fail_on_unmapped=True,
            )
            self.assertEqual(errors, [], f"{stage}: {errors}")

    def test_global_manifest_alone_is_expanded_at_pr_stage(self):
        errors = self.gov.validate_impacted_execution(
            self.registry, [GLOBAL_MANIFEST], "pull_request", REPO_ROOT,
            expand_global_files=True, fail_on_unmapped=True,
        )
        self.assertEqual(errors, [], errors)


class VersionManifestExemptionTest(unittest.TestCase):
    def setUp(self):
        self.gov = _load_governance()
        self.registry = _registry()

    def _repo_with_version_only_patch(self):
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        root = Path(tmp.name)
        files = sorted(self.gov.VERSION_MANIFEST_FILES)
        for path in files:
            target = root / path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(_manifest_text(path, "1.0.0"), encoding="utf-8")
        env = {
            "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com",
            "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.com",
        }
        subprocess.run(["git", "init", "-q"], cwd=root, check=True, env=env)
        subprocess.run(["git", "add", "-A"], cwd=root, check=True, env=env)
        subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@example.com",
             "commit", "-qm", "base"],
            cwd=root, check=True, env=env,
        )
        for path in files:
            target = root / path
            target.write_text(_manifest_text(path, "1.0.1"), encoding="utf-8")
        subprocess.run(["git", "add", "-A"], cwd=root, check=True, env=env)
        subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@example.com",
             "commit", "-qm", "bump"],
            cwd=root, check=True, env=env,
        )
        return root, files

    def test_synchronized_version_bump_stays_exempt_at_release(self):
        root, files = self._repo_with_version_only_patch()
        impact = self.gov.scenario_impact_files(root, "HEAD~1", files)
        self.assertEqual(impact, [])
        errors = self.gov.validate_impacted_execution(
            self.registry, impact, "release_artifact", root,
            expand_global_files=False, fail_on_unmapped=True,
        )
        self.assertEqual(errors, [], errors)

    def test_m176_bump_with_other_files_keeps_only_real_impact(self):
        for extra in ("README.md", "src/m176_unmapped.ts", WEB_FETCH_FILES[0]):
            with self.subTest(extra=extra):
                root, files = self._repo_with_version_only_patch()
                target = root / extra
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text("synthetic change\n", encoding="utf-8")
                subprocess.run(["git", "add", "-A"], cwd=root, check=True)
                subprocess.run(
                    ["git", "-c", "user.name=t", "-c", "user.email=t@example.com",
                     "commit", "-qm", "extra"], cwd=root, check=True,
                )
                impact = self.gov.scenario_impact_files(root, "HEAD~2", files + [extra])
                self.assertEqual(impact, [extra])
                errors = self.gov.validate_impacted_execution(
                    self.registry, impact, "release_artifact", root,
                    expand_global_files=False, fail_on_unmapped=True,
                )
                if extra == "src/m176_unmapped.ts":
                    self.assertTrue(errors)
                    self.assertIn(extra, "\n".join(errors))
                elif extra == "README.md":
                    self.assertEqual(errors, [])

    def test_m176_mixed_batch_configuration_edit_is_not_exempt(self):
        root, files = self._repo_with_version_only_patch()
        manifest = root / "src-tauri/Cargo.toml"
        manifest.write_text(manifest.read_text() + 'edition = "2021"\n')
        (root / "README.md").write_text("synthetic docs\n")
        subprocess.run(["git", "add", "-A"], cwd=root, check=True)
        subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@example.com",
             "commit", "-qm", "configuration"], cwd=root, check=True,
        )
        changed = files + ["README.md"]
        self.assertEqual(self.gov.scenario_impact_files(root, "HEAD~2", changed), changed)
        self.assertEqual(self.gov.scenario_impact_files(root, "missing-ref", changed), changed)

    def test_version_bump_with_a_product_change_is_still_a_gap(self):
        root, files = self._repo_with_version_only_patch()
        probe = root / UNMAPPED_PROBE
        probe.parent.mkdir(parents=True, exist_ok=True)
        probe.write_text("// probe\n", encoding="utf-8")
        subprocess.run(["git", "add", "-A"], cwd=root, check=True,
                       env={
                "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com",
                "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.com",
            })
        subprocess.run(
            ["git", "-c", "user.name=t", "-c", "user.email=t@example.com",
             "commit", "-qm", "probe"],
            cwd=root,
            check=True,
            env={
                "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@example.com",
                "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@example.com",
            },
        )
        impact = self.gov.scenario_impact_files(root, "HEAD~2", files + [UNMAPPED_PROBE])
        errors = self.gov.validate_impacted_execution(
            self.registry, impact, "release_artifact", root,
            expand_global_files=False, fail_on_unmapped=True,
        )
        self.assertTrue(errors)
        self.assertIn(UNMAPPED_PROBE, "\n".join(errors))
        self.assertNotIn(
            "Cargo.toml", "\n".join(errors),
            "已同步豁免的版本清单文件不应出现在缺口列表里",
        )


if __name__ == "__main__":
    unittest.main()
