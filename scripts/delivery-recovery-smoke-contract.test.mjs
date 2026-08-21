// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const root = new URL("../", import.meta.url);

async function source(path) {
  return readFile(new URL(path, root), "utf8");
}

test("the formal executable exposes DeliveryRun crash recovery", async () => {
  const [main, lib, smoke] = await Promise.all([
    source("src-tauri/src/main.rs"),
    source("src-tauri/src/lib.rs"),
    source("src-tauri/src/agent/delivery_recovery_smoke.rs"),
  ]);
  assert.ok(main.includes("run_delivery_recovery_smoke_cli"));
  assert.ok(lib.includes("--delivery-recovery-smoke"));
  for (const required of [
    "post_commit_owner_hard_killed",
    "pre_ref_owner_hard_killed",
    "post_intent_target_index_lock_pre_ref_fault_injected",
    "post_rebind_owner_hard_killed",
    "four_process_owners_observed",
    "post_push_owner_hard_killed",
    "post_push_pre_outcome_receipt_reconciled",
    "exact_receipted_head_reconciled",
    "canonical_parent_reconciled",
    "canonical_parent_mutation_count",
    "foreign_identity_parked",
    "claim_epoch_plateau",
    "duplicate_remote_write_count",
    "production_resume_path",
    "completion_arbiter_converged",
    "single_push_receipt_count",
    "canonical_pr_number",
    "user_message_count",
    "human_prompt_count",
    "cleanup_ok",
  ]) {
    assert.ok(smoke.includes(required), `executable smoke is missing ${required}`);
  }
});

test("PR, nightly, and exact release executable run E2E-011", async () => {
  const [ci, nightly, release] = await Promise.all([
    source(".github/workflows/ci.yml"),
    source(".github/workflows/unattended-long-task-nightly.yml"),
    source(".github/workflows/release.yml"),
  ]);
  for (const [surface, workflow] of [
    ["PR", ci],
    ["nightly", nightly],
    ["release", release],
  ]) {
    assert.ok(
      workflow.includes("--delivery-recovery-smoke"),
      `${surface} does not invoke the DeliveryRun smoke`,
    );
    assert.ok(
      workflow.includes("claim_epoch_plateau"),
      `${surface} does not assert the bounded-recovery oracle`,
    );
    assert.ok(
      workflow.includes("duplicate_remote_write_count"),
      `${surface} does not assert remote idempotency`,
    );
    assert.ok(
      workflow.includes("canonical_parent_reconciled"),
      `${surface} does not assert the pre-push canonical PR boundary`,
    );
    assert.ok(
      workflow.includes("pre_ref_owner_hard_killed"),
      `${surface} does not assert the intent-before-ref crash boundary`,
    );
  }
});

test("the unified scenario registry maps actual DeliveryRun source paths", async () => {
  const catalog = JSON.parse(await source("docs/testing/scenario-registry.json"));
  const e2e = catalog.complex_e2e_cases.find((item) => item.id === "E2E-011");
  assert.ok(e2e, "E2E-011 must be a governed complex E2E");
  assert.equal(e2e.automation_status, "partially_implemented");
  assert.ok(e2e.covers.includes("CXD-002"));

  for (const id of ["HLT-001", "HLT-002", "HLT-005", "CXD-002"]) {
    const scenario = catalog.scenarios.find((item) => item.id === id);
    assert.ok(scenario, `missing ${id}`);
    assert.ok(
      scenario.change_patterns.includes("src-tauri/src/agent/delivery_run.rs"),
      `${id} does not match the actual DeliveryRun store`,
    );
    assert.ok(
      scenario.automated_by.includes("binary:--delivery-recovery-smoke"),
      `${id} does not point at the executable recovery oracle`,
    );
  }
});
