// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function source(path) {
  return readFile(new URL(path, root), "utf8");
}

test("the formal binary exposes the unattended long-task smoke", async () => {
  const [main, lib] = await Promise.all([
    source("src-tauri/src/main.rs"),
    source("src-tauri/src/lib.rs"),
  ]);
  assert.ok(
    main.includes("run_unattended_long_task_smoke_cli"),
    "main must route the formal executable to the unattended smoke",
  );
  assert.ok(
    lib.includes("--unattended-long-task-smoke"),
    "the library must parse the public smoke flag",
  );
  assert.ok(
    lib.includes("run_unattended_long_task_smoke"),
    "the library must run the cross-process smoke implementation",
  );
});

test("required Windows CI executes the cross-process contract", async () => {
  const ci = await source(".github/workflows/ci.yml");
  for (const required of [
    "Cargo test (agent-core crate)",
    "Unattended long-task cross-process smoke",
    "--unattended-long-task-smoke",
    "user_message_count",
    "human_prompt_count",
    "process_restart_observed",
    "side_effect_receipt_count",
  ]) {
    assert.ok(ci.includes(required), `required CI is missing ${required}`);
  }
});

test("nightly repeats every historical scenario on a real Windows process", async () => {
  const nightly = await source(
    ".github/workflows/unattended-long-task-nightly.yml",
  );
  for (const required of [
    "schedule:",
    "windows-latest",
    "cargo test --manifest-path src-tauri/Cargo.toml --workspace",
    "--unattended-long-task-smoke",
    "HLT-001",
    "HLT-002",
    "HLT-003",
    "HLT-004",
    "CXD-001",
    "CXD-002",
    "Upload unattended receipt",
  ]) {
    assert.ok(nightly.includes(required), `nightly is missing ${required}`);
  }
});

test("the exact Windows release executable repeats the same contract", async () => {
  const release = await source(".github/workflows/release.yml");
  assert.ok(
    release.includes("Verify Windows release executable unattended long task"),
    "release must repeat the contract against the exact executable",
  );
  assert.ok(
    release.includes("--unattended-long-task-smoke"),
    "release must invoke the unattended smoke flag",
  );
});

test("history-derived scenarios use the unified synthetic registry", async () => {
  const [catalog, redirect, agent, delivery, deliveryRun, deliveryTool, objective, toolBackend, chat, trajectory, skills, loop, ci, nightly, release] = await Promise.all([
    source("docs/testing/scenario-registry.json"),
    source("docs/testing/history-derived-long-task-scenarios.json"),
    source("src-tauri/src/agent/mod.rs"),
    source("src-tauri/src/agent/delivery.rs"),
    source("src-tauri/src/agent/delivery_run.rs"),
    source("src-tauri/src/tools/delivery.rs"),
    source("src-tauri/src/agent/objective.rs"),
    source("src-tauri/src/agent/tool_backend.rs"),
    source("src-tauri/src/commands/chat.rs"),
    source("src-tauri/src/trajectory.rs"),
    source("src-tauri/src/commands/skills.rs"),
    source("src-tauri/crates/agent-loop/src/run.rs"),
    source(".github/workflows/ci.yml"),
    source(".github/workflows/unattended-long-task-nightly.yml"),
    source(".github/workflows/release.yml"),
  ]);
  const parsed = JSON.parse(catalog);
  const legacy = JSON.parse(redirect);
  assert.equal(parsed.schema_version, 1);
  assert.equal(parsed.source_policy, "aggregate-shapes-only");
  assert.equal(legacy.status, "redirect");
  assert.equal(legacy.canonical_registry, "scenario-registry.json");
  assert.ok(parsed.scenarios.some((item) => item.id === "HLT-001"));
  assert.ok(parsed.scenarios.some((item) => item.id === "HLT-002"));
  assert.ok(parsed.scenarios.some((item) => item.id === "CXD-001"));
  assert.ok(parsed.scenarios.some((item) => item.id === "CXD-002"));
  assert.ok(
    parsed.scenarios.every(
      (item) =>
        !Object.hasOwn(item, "session_id") &&
        !Object.hasOwn(item, "content") &&
        !Object.hasOwn(item, "local_user_path"),
    ),
  );
  assert.ok(
    parsed.scenarios.every(
      (item) => Array.isArray(item.automated_by) && item.automated_by.length > 0,
    ),
    "every scenario must name an executable automation target",
  );
  const implementation = `${agent}\n${delivery}\n${deliveryRun}\n${deliveryTool}\n${objective}\n${toolBackend}\n${chat}\n${trajectory}\n${skills}\n${loop}\n${ci}\n${nightly}\n${release}`;
  for (const scenario of parsed.scenarios.filter((item) => legacy.scenario_ids.includes(item.id))) {
    for (const automation of scenario.automated_by) {
      const target = automation.slice(automation.indexOf(":") + 1);
      assert.ok(
        implementation.includes(target),
        `${scenario.id} points at missing automation ${automation}`,
      );
    }
  }
});
