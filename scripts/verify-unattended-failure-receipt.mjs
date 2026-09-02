// SPDX-License-Identifier: Apache-2.0
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";

const binary = process.argv[2];
if (!binary) {
  console.error(
    "usage: node scripts/verify-unattended-failure-receipt.mjs <CodeFactory-binary>",
  );
  process.exit(2);
}

const root = mkdtempSync(join(tmpdir(), "codefactory-unattended-failure-"));
const invalidTempRoot = join(root, "not-a-directory");
const receiptPath = join(root, "raw-receipt.json");
writeFileSync(invalidTempRoot, "synthetic fixture boundary\n", "utf8");

try {
  const result = spawnSync(
    resolve(binary),
    ["--unattended-long-task-smoke", receiptPath],
    {
      encoding: "utf8",
      env: {
        ...process.env,
        TMPDIR: invalidTempRoot,
        TEMP: invalidTempRoot,
        TMP: invalidTempRoot,
      },
    },
  );
  assert.equal(result.status, 1, `expected exit 1, stderr: ${result.stderr}`);

  const receipt = JSON.parse(readFileSync(receiptPath, "utf8"));
  assert.deepEqual(
    {
      ok: receipt.ok,
      error: receipt.error,
      observation_schema_version: receipt.observation_schema_version,
      case_id: receipt.case_id,
      descendant_process_count: receipt.descendant_process_count,
      cleanup_attempted: receipt.cleanup_attempted,
      orphan_sweep_performed: receipt.orphan_sweep_performed,
      leaked_resource_count: receipt.leaked_resource_count,
      cleanup_ok: receipt.cleanup_ok,
    },
    {
      ok: false,
      error: "unattended_smoke_failed",
      observation_schema_version: 1,
      case_id: "E2E-001",
      descendant_process_count: 0,
      cleanup_attempted: false,
      orphan_sweep_performed: false,
      leaked_resource_count: 1,
      cleanup_ok: false,
    },
  );
  const rendered = JSON.stringify(receipt);
  for (const forbidden of [root, invalidTempRoot, "token=", "TMPDIR"]) {
    assert.ok(!rendered.includes(forbidden), `failure receipt leaked ${forbidden}`);
  }
  console.log(
    JSON.stringify(
      {
        status: "pass",
        exit_code: result.status,
        diagnostic: receipt.error,
        cleanup_ok: receipt.cleanup_ok,
        leaked_resource_count: receipt.leaked_resource_count,
      },
      null,
      2,
    ),
  );
} finally {
  rmSync(receiptPath, { force: true });
  rmSync(invalidTempRoot, { force: true });
  rmSync(root, { recursive: true, force: true });
}
