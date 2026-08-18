// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function source(path) {
  return readFile(new URL(path, root), "utf8");
}

test("the formal binary exposes the historical session restart smoke", async () => {
  const [main, lib] = await Promise.all([
    source("src-tauri/src/main.rs"),
    source("src-tauri/src/lib.rs"),
  ]);
  assert.ok(main.includes("run_history_session_smoke_cli"));
  assert.ok(lib.includes("--history-session-smoke"));
  assert.ok(lib.includes("--history-session-worker"));
  assert.ok(lib.includes("history_session_smoke::run_parent"));
});

test("required Windows CI runs the cross-process continue, stop and handback oracle", async () => {
  const ci = await source(".github/workflows/ci.yml");
  for (const marker of [
    "Historical session continue/stop/handback cross-process smoke",
    "--history-session-smoke",
    "E2E-002",
    "E2E-003",
    "E2E-007",
    "same_objective",
    "stop_request_was_hard_killed",
    "all_live_objectives_cancelled",
    "second_restart_stayed_cancelled",
    "claimable_remediation_count",
    "handback_survived_two_restarts",
  ]) {
    assert.ok(ci.includes(marker), `required CI is missing ${marker}`);
  }
});

test("nightly repeats the historical session restart fault path", async () => {
  const nightly = await source(".github/workflows/unattended-long-task-nightly.yml");
  for (const marker of [
    "Historical session restart smoke",
    "--history-session-smoke",
    "stop_request_was_hard_killed",
    "second_restart_stayed_cancelled",
    "handback_survived_two_restarts",
    "Upload historical session receipt",
  ]) {
    assert.ok(nightly.includes(marker), `nightly is missing ${marker}`);
  }
});

test("the exact Windows release executable runs the same smoke", async () => {
  const release = await source(".github/workflows/release.yml");
  for (const marker of [
    "Verify Windows release executable historical session restart",
    "--history-session-smoke",
    "build_git_sha",
    "stop_request_was_hard_killed",
    "second_restart_stayed_cancelled",
    "E2E-007",
    "handback_survived_two_restarts",
  ]) {
    assert.ok(release.includes(marker), `release is missing ${marker}`);
  }
});

test("the registry maps all historical E2E cases to the executable without overstating L3", async () => {
  const registry = JSON.parse(await source("docs/testing/scenario-registry.json"));
  for (const id of ["E2E-002", "E2E-003", "E2E-007"]) {
    const scenario = registry.complex_e2e_cases.find((item) => item.id === id);
    assert.ok(scenario, `${id} is missing`);
    assert.equal(scenario.automation_status, "partially_implemented");
    assert.ok(
      scenario.automated_by.includes("binary:--history-session-smoke"),
      `${id} must name the formal executable`,
    );
    assert.ok(
      scenario.remaining_gaps.some((gap) => gap.includes("L3")),
      `${id} must keep its real desktop gap explicit`,
    );
  }
});
