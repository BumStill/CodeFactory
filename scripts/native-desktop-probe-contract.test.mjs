import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import test from "node:test";

import {
  assertOwnedLayout,
  canStopOwnedProcess,
  cleanupEligible,
  observationErrors,
} from "./native-desktop-probe-contract.mjs";

const runId = "00000000-0000-4000-8000-000000000001";
const ownerToken = "00000000-0000-4000-8000-000000000002";
const identifier = `com.codefactory.scenario.${runId.replaceAll("-", "")}`;
const sha = "a".repeat(64);
const inputDigest = createHash("sha256").update("CODEFACTORY_NATIVE_PROBE_ONLY").digest("hex");
const identity = (pid, startToken) => ({
  run_id: runId, pid, start_token: startToken, executable_sha256: sha,
  bundle_id: identifier,
});
const ownedIdentity = (pid, startToken) => ({ ...identity(pid, startToken), owner_token: ownerToken });

function layoutFixture() {
  const tempRoot = fs.realpathSync("/tmp");
  const root = fs.mkdtempSync(path.join(tempRoot, "codefactory-scenario-"));
  const app = path.join(root, "Probe.app");
  const home = path.join(root, "home");
  const executable = path.join(app, "Contents", "MacOS", "codefactory");
  for (const directory of [path.dirname(executable),
    path.join(home, "Library", "Application Support", identifier),
    path.join(home, "Library", "Caches", identifier), path.join(root, "tmp")]) {
    fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  }
  fs.writeFileSync(executable, "synthetic executable fixture");
  const manifest = path.join(root, "manifest.json");
  fs.writeFileSync(manifest, JSON.stringify({ schema_version: 1, run_id: runId,
    owner_token: ownerToken, identifier, capabilities: ["isolated_app_data"] }), { mode: 0o600 });
  fs.writeFileSync(path.join(root, "owner.json"), JSON.stringify({ schema_version: 1,
    run_id: runId, owner_token: ownerToken }), { mode: 0o600 });
  return { tempRoot, layout: { runId, ownerToken, identifier, root, manifest, app, home, executable } };
}

function observation() {
  return {
    schema_version: 1, scope: "native-desktop-feasibility-only", run_id: runId,
    executable_sha256: sha, first: identity(101, "100:1"), second: identity(102, "102:1"),
    accessibility: true, screen_capture: true, gui_session: true,
    native_ax_press_count: 2, native_text_input_count: 1,
    input_digest: inputDigest, input_readback_digest: inputDigest,
    theme_before: "dark", theme_after: "light", theme_reopened: "light",
    first_process_reaped: true, request_count: 0, credential_access_count: 0,
    screenshots: [101, 102].map((pid) => ({
      owner_pid: pid, run_id: runId, executable_sha256: sha,
      sha256: "b".repeat(64), width: 800, height: 600,
    })),
    cleanup: { owner_run_id: runId, observed_complete: true, remaining_process_count: 0, run_root_removed: true },
  };
}

test("stop authority needs launch ownership plus exact birth, executable and run identity", () => {
  const owned = ownedIdentity(101, "100:1");
  assert.equal(canStopOwnedProcess(owned, owned, { launchRecorded: true }), true);
  for (const replacement of [
    { pid: 999 }, { start_token: "200:1" }, { executable_sha256: "b".repeat(64) },
    { bundle_id: "com.codefactory.app" }, { run_id: "other-run" }, { owner_token: runId },
  ]) {
    assert.equal(canStopOwnedProcess(owned, { ...owned, ...replacement }, { launchRecorded: true }), false);
  }
  assert.equal(canStopOwnedProcess(owned, owned, { launchRecorded: false }), false);
  assert.equal(canStopOwnedProcess(owned, null, { launchRecorded: true }), false);
});

test("non-owned or PID-reused processes produce no termination authority", () => {
  const owned = ownedIdentity(101, "100:1");
  let signals = 0;
  for (const observed of [ownedIdentity(101, "200:1"), { ...owned, bundle_id: "com.codefactory.app" }, null]) {
    if (canStopOwnedProcess(owned, observed, { launchRecorded: true })) signals++;
  }
  assert.equal(signals, 0);
});

test("identity rejects coerced UUID or digest values and PIDs outside the Darwin pid_t range", () => {
  for (const changed of [
    { run_id: [runId] }, { run_id: new String(runId) }, { run_id: Symbol("run") },
    { executable_sha256: [sha] }, { executable_sha256: new String(sha) },
    { pid: 2 ** 31 }, { pid: Number.MAX_SAFE_INTEGER + 1 }, { pid: 1.5 }, { pid: "101" },
  ]) {
    const invalid = { ...ownedIdentity(101, "100:1"), ...changed };
    assert.equal(canStopOwnedProcess(invalid, invalid, { launchRecorded: true }), false);
  }
  const lastRepresentablePID = ownedIdentity(2 ** 31 - 1, "100:1");
  assert.equal(canStopOwnedProcess(lastRepresentablePID, lastRepresentablePID, { launchRecorded: true }), true);
});

test("layout requires one owned temporary run and rejects broad roots, escapes and wrong markers", () => {
  const { tempRoot, layout } = layoutFixture();
  try {
    assert.doesNotThrow(() => assertOwnedLayout(layout, tempRoot));
    assert.doesNotThrow(() => assertOwnedLayout(layout));
    assert.ok(!path.basename(layout.root).includes(runId));
    assert.throws(() => assertOwnedLayout(layout, layout.root));
    for (const changed of [{ root: tempRoot }, { home: tempRoot }, { executable: process.execPath }]) {
      assert.throws(() => assertOwnedLayout({ ...layout, ...changed }, tempRoot));
    }
    fs.writeFileSync(path.join(layout.root, "owner.json"), JSON.stringify({ run_id: "other" }));
    assert.throws(() => assertOwnedLayout(layout, tempRoot));
  } finally { fs.rmSync(layout.root, { recursive: true }); }
});

test("product manifest and owner schema must agree with the private runtime lease", () => {
  for (const [file, mutate] of [
    ["manifest.json", (v) => { v.schema_version = 2; }],
    ["manifest.json", (v) => { v.run_id = ownerToken; }],
    ["manifest.json", (v) => { v.owner_token = runId; }],
    ["manifest.json", (v) => { v.identifier = "com.codefactory.app"; }],
    ["manifest.json", (v) => { v.capabilities.push("external_network"); }],
    ["manifest.json", (v) => { v.extra = true; }],
    ["owner.json", (v) => { v.schema_version = 2; }],
    ["owner.json", (v) => { v.owner_token = runId; }],
    ["owner.json", (v) => { v.extra = true; }],
  ]) {
    const { tempRoot, layout } = layoutFixture();
    try {
      const target = path.join(layout.root, file);
      const value = JSON.parse(fs.readFileSync(target, "utf8"));
      mutate(value);
      fs.writeFileSync(target, JSON.stringify(value));
      assert.throws(() => assertOwnedLayout(layout, tempRoot));
    } finally { fs.rmSync(layout.root, { recursive: true }); }
  }
});

test("every product-owned storage directory rejects symlink replacement", () => {
  for (const member of ["home/Library", "home/Library/Application Support", "home/Library/Caches",
    `home/Library/Application Support/${identifier}`, `home/Library/Caches/${identifier}`, "tmp"]) {
    const { tempRoot, layout } = layoutFixture();
    try {
      const target = path.join(layout.root, member);
      const held = path.join(layout.root, "held-directory");
      fs.renameSync(target, held);
      fs.symlinkSync(held, target, "dir");
      assert.throws(() => assertOwnedLayout(layout, tempRoot));
    } finally { fs.rmSync(layout.root, { recursive: true }); }
  }
});

test("a symlink in the home or executable path prevents cleanup authority", () => {
  const { tempRoot, layout } = layoutFixture();
  try {
    fs.renameSync(layout.home, path.join(layout.root, "original-home"));
    fs.symlinkSync(path.join(layout.root, "original-home"), layout.home, "dir");
    assert.throws(() => assertOwnedLayout(layout, tempRoot));
    assert.equal(cleanupEligible(layout, tempRoot, { complete: true, owned_processes_reaped: true, live_under_root: 0 }), false);
  } finally { fs.rmSync(layout.root, { recursive: true }); }
});

for (const member of ["owner.json", "manifest.json", "Probe.app/Contents/MacOS/codefactory"]) {
  test(`hardlinked ${member} cannot authorize run cleanup`, () => {
    const { tempRoot, layout } = layoutFixture();
    const outside = fs.mkdtempSync(path.join(tempRoot, "native-probe-hardlink-"));
    try {
      fs.linkSync(path.join(layout.root, member), path.join(outside, "external-hardlink"));
      assert.throws(() => assertOwnedLayout(layout, tempRoot));
      assert.equal(cleanupEligible(layout, tempRoot, { complete: true, owned_processes_reaped: true, live_under_root: 0 }), false);
    } finally {
      fs.rmSync(layout.root, { recursive: true });
      fs.rmSync(outside, { recursive: true });
    }
  });
  test(`directory in place of ${member} cannot authorize run cleanup`, () => {
    const { tempRoot, layout } = layoutFixture();
    try {
      const target = path.join(layout.root, member);
      fs.unlinkSync(target);
      fs.mkdirSync(target);
      assert.throws(() => assertOwnedLayout(layout, tempRoot));
    } finally { fs.rmSync(layout.root, { recursive: true }); }
  });
}

test("cleanup requires complete process observation and reap evidence even with no visible window", () => {
  const { tempRoot, layout } = layoutFixture();
  try {
    const complete = { complete: true, owned_processes_reaped: true, live_under_root: 0 };
    assert.equal(cleanupEligible(layout, tempRoot, complete), true);
    for (const changed of [{ complete: false }, { owned_processes_reaped: false }, { live_under_root: 1 }]) {
      assert.equal(cleanupEligible(layout, tempRoot, { ...complete, ...changed }), false);
    }
  } finally { fs.rmSync(layout.root, { recursive: true }); }
});

test("window-only, missing click/input, same-PID restart, and theme loss cannot satisfy the probe", () => {
  assert.deepEqual(observationErrors(observation()), []);
  for (const changed of [
    { native_ax_press_count: 0 }, { native_text_input_count: 0 },
    { input_readback_digest: "c".repeat(64) }, { theme_reopened: "dark" },
    { first_process_reaped: false }, { second: identity(101, "100:1") },
    { screenshots: [] }, { accessibility: false }, { screen_capture: false }, { gui_session: false },
  ]) assert.ok(observationErrors({ ...observation(), ...changed }).length > 0);
});

test("screenshots, executable and cleanup must bind to the same run", () => {
  for (const mutate of [
    (v) => { v.second.executable_sha256 = "c".repeat(64); },
    (v) => { v.screenshots[0].owner_pid = 999; },
    (v) => { v.screenshots[1].run_id = "other"; },
    (v) => { v.cleanup.owner_run_id = "other"; },
    (v) => { v.cleanup.run_root_removed = false; },
    (v) => { v.cleanup.remaining_process_count = 1; },
  ]) {
    const value = observation(); mutate(value);
    assert.ok(observationErrors(value).length > 0);
  }
});

test("observation rejects JS coercion and unsafe integer counts or image dimensions", () => {
  for (const mutate of [
    (v) => {
      const coercedRun = [runId];
      v.run_id = v.first.run_id = v.second.run_id = v.cleanup.owner_run_id = coercedRun;
      v.screenshots.forEach((s) => { s.run_id = coercedRun; });
    },
    (v) => {
      const coercedDigest = [sha];
      v.executable_sha256 = v.first.executable_sha256 = v.second.executable_sha256 = coercedDigest;
      v.screenshots.forEach((s) => { s.executable_sha256 = coercedDigest; });
    },
    (v) => { v.screenshots[0].sha256 = ["b".repeat(64)]; },
    (v) => { v.native_ax_press_count = Number.MAX_SAFE_INTEGER + 1; },
    (v) => { v.screenshots[0].width = Number.MAX_SAFE_INTEGER + 1; },
    (v) => { v.screenshots[0].height = Number.MAX_SAFE_INTEGER + 1; },
  ]) {
    const value = observation(); mutate(value);
    assert.ok(observationErrors(value).length > 0);
  }
});

test("raw text, paths, endpoint/key references and unexpected nested fields are rejected without echoing values", () => {
  for (const mutate of [
    (v) => { v.raw_text = "private fixture text"; },
    (v) => { v.first.executable_path = "/private/fixture-path"; },
    (v) => { v.key_ref = "sensitive-reference"; },
    (v) => { v.first.owner_token = ownerToken; },
    (v) => { v.endpoint = "https://invalid.example"; },
    (v) => { v.request_count = 1; },
    (v) => { v.credential_access_count = 1; },
  ]) {
    const value = observation(); mutate(value);
    const errors = observationErrors(value);
    assert.ok(errors.length > 0);
    assert.ok(!JSON.stringify(errors).includes("private") && !JSON.stringify(errors).includes("sensitive-reference"));
  }
});

test("permission preflight has no permission request, app launch, AX action or kill capability", () => {
  const source = fs.readFileSync(new URL("./probe-macos-native-preflight.swift", import.meta.url), "utf8");
  assert.match(source, /AXIsProcessTrusted\(\)/);
  assert.match(source, /CGPreflightScreenCaptureAccess\(\)/);
  assert.doesNotMatch(source, /AXIsProcessTrustedWithOptions|CGRequestScreenCaptureAccess|NSWorkspace|Process\(|AXUIElementPerformAction|\bkill\(|\.terminate\(|\.forceTerminate\(/);
  assert.match(source, /"status": "blocked"/);
  assert.match(source, /exit\(3\)/);
});
