import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { observeOwnedChild } from "./run-macos-native-observer.mjs";
import * as supervisor from "./run-macos-native-observer.mjs";
import { assertPreparedLayout } from "./native-desktop-probe-contract.mjs";

const run = "00000000-0000-4000-8000-000000000001";
const owner = "00000000-0000-4000-8000-000000000002";
const expected = { run_id: run, owner_token: owner, executable_sha256: "a".repeat(64),
  bundle_id: `com.codefactory.scenario.${run.replaceAll("-", "")}` };

function fixture(overrides = {}) {
  const calls = [];
  const child = { pid: 123, exitCode: null, signalCode: null };
  const identity = { ...expected, pid: child.pid, start_token: "123:456" };
  const adapters = {
    checkWorld: async () => { calls.push("check"); },
    launch: async () => { calls.push("launch"); return child; },
    inspect: async () => { calls.push("inspect"); return identity; },
    observe: async () => { calls.push("observe"); return { ax_window_seen: true, settings_control: true }; },
    stop: async (_lease, force) => { calls.push(force ? "kill" : "term"); child.signalCode = "SIGTERM"; },
    waitForExit: async () => child.exitCode !== null || child.signalCode !== null,
    ...overrides,
  };
  return { calls, child, identity, adapters };
}

test("real observer slice and full feasibility keep separate outcomes without invented access counts", async () => {
  const { calls, adapters } = fixture();
  const result = await observeOwnedChild(expected, adapters);
  assert.equal(result.observer_slice.status, "passed");
  assert.equal(result.full_probe.status, "blocked");
  assert.equal(result.request_count, null);
  assert.equal(result.credential_access_count, null);
  assert.equal(result.cleanup.child_reaped, true);
  assert.equal(result.cleanup.world_directory, "retained");
  assert.ok(calls.indexOf("check") < calls.indexOf("launch"));
  assert.ok(calls.lastIndexOf("inspect") < calls.indexOf("term"));
  assert.ok(!JSON.stringify(result).includes(owner));
});

test("wrong bundle never acquires signal authority", async () => {
  const base = fixture();
  base.adapters.inspect = async () => ({ ...base.identity, bundle_id: "com.codefactory.app" });
  const result = await observeOwnedChild(expected, base.adapters);
  assert.equal(result.observer_slice.status, "failed");
  assert.equal(result.cleanup.child_reaped, false);
  assert.ok(!base.calls.includes("term") && !base.calls.includes("kill"));
});

test("PID reuse before cleanup blocks signal and retains world", async () => {
  const base = fixture();
  let count = 0;
  base.adapters.inspect = async () => ({ ...base.identity, start_token: ++count === 1 ? "123:456" : "999:1" });
  const result = await observeOwnedChild(expected, base.adapters);
  assert.equal(result.observer_slice.status, "failed");
  assert.ok(!base.calls.includes("term") && !base.calls.includes("kill"));
  assert.equal(result.cleanup.world_directory, "retained");
});

test("AX failure still reclaims the exactly owned main child without exporting diagnostic text", async () => {
  const base = fixture({ observe: async () => { throw new Error("private text and /private/path"); } });
  const result = await observeOwnedChild(expected, base.adapters);
  assert.equal(result.cleanup.child_reaped, true);
  assert.equal(result.observer_slice.status, "blocked");
  assert.ok(base.calls.includes("term"));
  assert.ok(!JSON.stringify(result).includes("private"));
});

test("a changed root before cleanup blocks signal", async () => {
  const base = fixture();
  let checks = 0;
  base.adapters.checkWorld = async () => { if (++checks >= 3) throw new Error("replaced root"); };
  const result = await observeOwnedChild(expected, base.adapters);
  assert.equal(result.observer_slice.status, "failed");
  assert.ok(!base.calls.includes("term"));
});

test("a hanging AX adapter is bounded and cleanup still runs", async () => {
  const base = fixture({ observe: () => new Promise(() => {}) });
  const result = await observeOwnedChild(expected, base.adapters, { operationTimeoutMs: 20 });
  assert.equal(result.observer_slice.status, "blocked");
  assert.equal(result.cleanup.child_reaped, true);
});

test("world validation failure prevents launch", async () => {
  const base = fixture({ checkWorld: async () => { throw new Error("unsafe root"); } });
  const result = await observeOwnedChild(expected, base.adapters);
  assert.ok(!base.calls.includes("launch"));
  assert.equal(result.observer_slice.status, "failed");
});

test("unknown AX fields are never forwarded as public evidence", async () => {
  const base = fixture({ observe: async () => ({ ax_window_seen: true, settings_control: true, raw_ax_tree: "secret" }) });
  const result = await observeOwnedChild(expected, base.adapters);
  assert.ok(!JSON.stringify(result).includes("secret"));
  assert.ok(!JSON.stringify(result).includes("raw_ax_tree"));
});

test("preflight stdout projection is strict booleans with no arbitrary helper fields", () => {
  assert.deepEqual(supervisor.preflightProjection({ accessibility: true, screen_capture: "true", gui_session: 1,
    session_present: true, on_console: 1, login_done: "true", same_uid: true, lock_state: false,
    owner_token: owner, raw_ax_tree: "private" }), { accessibility: true, screen_capture: false, gui_session: false,
    session_present: true, on_console: false, login_done: false, same_uid: true, lock_state: "unknown" });
  assert.deepEqual(supervisor.preflightProjection(null), { accessibility: false, screen_capture: false, gui_session: false,
    session_present: false, on_console: false, login_done: false, same_uid: false, lock_state: "unknown" });
  for (const lock_state of ["locked", "unlocked", "unknown"]) {
    assert.equal(supervisor.preflightProjection({ lock_state }).lock_state, lock_state);
  }
});

test("read-only preflight requires explicit unlock independently of GUI presence", async () => {
  const value = { accessibility: true, screen_capture: true, gui_session: true,
    session_present: true, on_console: true, login_done: true, same_uid: true, lock_state: "unlocked" };
  const options = { platform: "darwin", environment: { GITHUB_ACTIONS: "true", RUNNER_OS: "macOS" } };
  let calls = 0;
  const call = async (driver, request) => {
    assert.equal(driver, "/synthetic-driver");
    assert.deepEqual(request, { operation: "preflight" });
    calls++;
    return value;
  };
  const result = await supervisor.readOnlyPreflight("/synthetic-driver", { ...options, call });
  assert.equal(calls, 1);
  assert.equal(result.scope, "native-desktop-preflight");
  assert.equal(result.status, "ready");
  for (const field of ["accessibility", "screen_capture", "gui_session", "session_present", "on_console", "login_done"]) {
    const blocked = await supervisor.readOnlyPreflight("/synthetic-driver", {
      ...options, call: async () => ({ ...value, [field]: false }),
    });
    assert.equal(blocked.status, "blocked");
  }
  for (const lock_state of ["locked", "unknown", null, false, "false", undefined]) {
    const blocked = await supervisor.readOnlyPreflight("/synthetic-driver", {
      ...options, call: async () => ({ ...value, lock_state }),
    });
    assert.equal(blocked.status, "blocked", `lock state ${lock_state} must not launch`);
    assert.equal(blocked.preflight.gui_session, true);
    assert.ok(blocked.reason_codes.includes(lock_state === "locked" ? "screen_locked" : "unlock_state_unproven"));
  }
  for (const same_uid of [false, undefined, "true", 0]) {
    const blocked = await supervisor.readOnlyPreflight("/synthetic-driver", {
      ...options, call: async () => ({ ...value, same_uid }),
    });
    assert.equal(blocked.status, "blocked", "another or unproven console user cannot authorize a launch");
    assert.deepEqual(blocked.reason_codes, ["session_uid_unproven"]);
  }
  const unavailable = await supervisor.readOnlyPreflight("/synthetic-driver", {
    ...options, call: async () => { throw new Error("private /path owner_token"); },
  });
  assert.equal(unavailable.status, "blocked");
  assert.equal(unavailable.preflight.lock_state, "unknown");
  assert.deepEqual(unavailable.reason_codes, ["preflight_observation_unavailable"]);
  assert.ok(!JSON.stringify(unavailable).includes("private"));
});

test("preflight distinguishes an observed absent session from unavailable diagnostics", async () => {
  const options = { platform: "darwin", environment: { GITHUB_ACTIONS: "true", RUNNER_OS: "macOS" } };
  const absent = await supervisor.readOnlyPreflight("/synthetic-driver", { ...options,
    call: async () => ({ accessibility: true, screen_capture: true, session_present: false,
      gui_session: false, on_console: false, login_done: false, same_uid: false, lock_state: "unknown" }),
  });
  assert.equal(absent.status, "blocked");
  assert.deepEqual(absent.reason_codes, ["no_session_observed"]);
  for (const raw of [null, {}, [], "private", { session_present: "false" }]) {
    const unavailable = await supervisor.readOnlyPreflight("/synthetic-driver", { ...options, call: async () => raw });
    assert.equal(unavailable.status, "blocked");
    assert.deepEqual(unavailable.reason_codes, ["preflight_observation_unavailable"]);
  }
});

test("read-only preflight rejects non-CI or relative driver before any helper invocation", async () => {
  let calls = 0;
  const call = async () => { calls++; };
  for (const overrides of [
    { platform: "linux", environment: { GITHUB_ACTIONS: "true", RUNNER_OS: "macOS" } },
    { platform: "darwin", environment: { GITHUB_ACTIONS: "false", RUNNER_OS: "macOS" } },
  ]) await assert.rejects(supervisor.readOnlyPreflight("/synthetic-driver", { call, ...overrides }), /macos_ci_only/);
  await assert.rejects(supervisor.readOnlyPreflight("relative-driver", { call, platform: "darwin",
    environment: { GITHUB_ACTIONS: "true", RUNNER_OS: "macOS" } }), /absolute_driver_required/);
  assert.equal(calls, 0);
});

test("pure native preflight fixture diagnoses missing lock and CLI exits blocked without an App", { skip: process.platform !== "darwin" }, () => {
  const outer = fs.mkdtempSync(path.join(fs.realpathSync("/tmp"), "native-observer-preflight-test-"));
  try {
    const driver = path.join(outer, "contract-driver");
    execFileSync("xcrun", ["swiftc", "-D", "NATIVE_PREFLIGHT_CONTRACT_TESTS",
      fileURLToPath(new URL("./macos-native-observer.swift", import.meta.url)), "-o", driver],
    { timeout: 15000, stdio: "pipe" });
    assert.equal(execFileSync(driver, [], { input: "{}", encoding: "utf8", timeout: 5000, stdio: "pipe" }).trim(),
      "native_preflight_contracts_passed");
    let failure;
    try {
      execFileSync(process.execPath, [fileURLToPath(new URL("./run-macos-native-observer.mjs", import.meta.url)),
        "preflight", "--driver", driver], { encoding: "utf8", stdio: "pipe", timeout: 5000,
        env: { ...process.env, GITHUB_ACTIONS: "true", RUNNER_OS: "macOS" } });
    } catch (error) { failure = error; }
    assert.equal(failure?.status, 3);
    const result = JSON.parse(failure.stdout);
    assert.equal(result.scope, "native-desktop-preflight");
    assert.equal(result.status, "blocked");
    assert.deepEqual(result.preflight, { accessibility: true, screen_capture: true, gui_session: true,
      session_present: true, on_console: true, login_done: true, same_uid: true, lock_state: "unknown" });
    assert.deepEqual(result.reason_codes, ["unlock_state_unproven"]);
    assert.ok(!failure.stdout.includes(outer) && !failure.stdout.includes("synthetic-private"));
    assert.deepEqual(fs.readdirSync(outer), ["contract-driver"]);
  } finally { fs.rmSync(outer, { recursive: true }); }
});

test("standalone preflight reports session and lock independently without accessing the desktop", { skip: process.platform !== "darwin" }, () => {
  const outer = fs.mkdtempSync(path.join(fs.realpathSync("/tmp"), "native-preflight-standalone-test-"));
  try {
    const source = fileURLToPath(new URL("./probe-macos-native-preflight.swift", import.meta.url));
    // Refuse to execute the binary unless its explicitly synthetic build mode
    // exists; a missing implementation must not fall through to real OS APIs.
    assert.match(fs.readFileSync(source, "utf8"), /#if NATIVE_PREFLIGHT_CONTRACT_TESTS/);
    const driver = path.join(outer, "contract-driver");
    execFileSync("xcrun", ["swiftc", "-D", "NATIVE_PREFLIGHT_CONTRACT_TESTS", source, "-o", driver],
      { timeout: 15000, stdio: "pipe" });
    for (const lock of ["unknown", "locked", "unlocked", "absent"]) {
      let failure;
      try {
        execFileSync(driver, ["--preflight"], { input: JSON.stringify({ fixture: lock }),
          encoding: "utf8", timeout: 5000, stdio: "pipe" });
      } catch (error) { failure = error; }
      assert.equal(failure?.status, 3);
      const value = JSON.parse(failure.stdout);
      assert.equal(value.status, "blocked");
      assert.equal(value.session_present, lock !== "absent");
      assert.equal(value.gui_session, lock !== "absent");
      assert.equal(value.lock_state, lock === "absent" ? "unknown" : lock);
      assert.equal(value.app_launch_count, 0);
      if (lock === "absent") assert.ok(value.reason_codes.includes("no_session_observed"));
      if (lock === "unknown") assert.ok(value.reason_codes.includes("unlock_state_unproven"));
      if (lock === "locked") assert.ok(value.reason_codes.includes("screen_locked"));
    }
  } finally { fs.rmSync(outer, { recursive: true }); }
});

test("unreaped child permits at most two identity-checked signal attempts and never passes", async () => {
  const base = fixture({ stop: async () => {}, waitForExit: async () => false });
  const result = await observeOwnedChild(expected, base.adapters);
  assert.equal(result.cleanup.signal_attempts, 2);
  assert.equal(result.cleanup.child_reaped, false);
  assert.equal(result.observer_slice.status, "failed");
  assert.equal(base.calls.filter((value) => value === "inspect").length, 3);
});

test("an initially empty AX tree retries until one later snapshot proves both controls", async () => {
  let attempts = 0;
  const result = await supervisor.observeAXUntilReachable(async (remaining) => {
    assert.ok(remaining > 0 && remaining <= 100);
    attempts++;
    return attempts === 1 ? { ax_window_seen: false, settings_control: false }
      : { ax_window_seen: true, settings_control: true };
  }, { timeoutMs: 100, pollMs: 1 });
  assert.equal(attempts, 2);
  assert.deepEqual(result, { ax_window_seen: true, settings_control: true });
});

test("unreachable or split AX snapshots cannot pass and the total wait is bounded", async () => {
  let attempts = 0;
  const start = Date.now();
  const result = await supervisor.observeAXUntilReachable(async () => {
    attempts++;
    return { ax_window_seen: attempts % 2 === 0, settings_control: attempts % 2 !== 0 };
  }, { timeoutMs: 25, pollMs: 1 });
  assert.ok(attempts > 1);
  assert.ok(result.ax_window_seen !== true || result.settings_control !== true);
  assert.ok(Date.now() - start < 500);
  await assert.rejects(supervisor.observeAXUntilReachable(() => new Promise(() => {}),
    { timeoutMs: 15, pollMs: 1 }), /operation_timeout/);
});

test("AX retry propagates identity failure without taking another snapshot", async () => {
  let attempts = 0;
  await assert.rejects(supervisor.observeAXUntilReachable(async () => {
    attempts++;
    throw new Error("process_birth_mismatch");
  }, { timeoutMs: 100, pollMs: 1 }), /process_birth_mismatch/);
  assert.equal(attempts, 1);
});

test("native signal boundary rejects identity changes during expensive validation", { skip: process.platform !== "darwin" }, () => {
  const outer = fs.mkdtempSync(path.join(fs.realpathSync("/tmp"), "native-observer-signal-test-"));
  try {
    const driver = path.join(outer, "contract-driver");
    execFileSync("xcrun", ["swiftc", "-D", "NATIVE_OBSERVER_CONTRACT_TESTS",
      fileURLToPath(new URL("./macos-native-observer.swift", import.meta.url)), "-o", driver],
    { timeout: 15000, stdio: "pipe" });
    // A compile-time branch executes only injected pure closures, never an App,
    // OS process observer, AX query, permission API or real signal.
    // macOS may spend over a second loading system frameworks for a fresh
    // executable while Cargo is compiling. Bound startup without weakening any
    // of the injected identity/signal assertions inside the fixture.
    const output = execFileSync(driver, [], { input: "{}", encoding: "utf8", timeout: 5000, stdio: "pipe" });
    assert.equal(output.trim(), "native_signal_contracts_passed");
  } finally { fs.rmSync(outer, { recursive: true }); }
});

test("prepare creates private parent directories and compatible product world without launching an app", () => {
  const outer = fs.mkdtempSync(path.join(fs.realpathSync("/tmp"), "native-observer-cli-test-"));
  const stateFile = path.join(outer, "private", "state.json");
  let state;
  try {
    const output = execFileSync(process.execPath, [fileURLToPath(new URL("./run-macos-native-observer.mjs", import.meta.url)),
      "prepare", "--state", stateFile, "--build-config", path.join(outer, "build.json"),
      "--expected-build-sha", "a".repeat(40)], { encoding: "utf8" });
    state = JSON.parse(fs.readFileSync(stateFile, "utf8"));
    assertPreparedLayout(state.layout);
    assert.equal(fs.statSync(stateFile).mode & 0o077, 0);
    assert.equal(fs.statSync(path.dirname(stateFile)).mode & 0o077, 0);
    assert.ok(!fs.existsSync(state.layout.app));
    assert.ok(!output.includes(state.layout.ownerToken) && !output.includes(state.layout.root));
    assert.equal(JSON.parse(fs.readFileSync(path.join(outer, "build.json"), "utf8")).build.devUrl, null);
  } finally {
    if (state) fs.rmSync(state.layout.root, { recursive: true });
    fs.rmSync(outer, { recursive: true });
  }
});

test("observe outside macOS CI creates anonymous failure receipt and never launches an app", () => {
  const outer = fs.mkdtempSync(path.join(fs.realpathSync("/tmp"), "native-observer-cli-test-"));
  const receipt = path.join(outer, "private", "receipt.json");
  try {
    assert.throws(() => execFileSync(process.execPath, [fileURLToPath(new URL("./run-macos-native-observer.mjs", import.meta.url)),
      "observe", "--state", "/not-opened", "--candidate-app", "/not-opened", "--driver", "/not-opened", "--receipt", receipt],
    { env: { ...process.env, GITHUB_ACTIONS: "false" }, stdio: "pipe" }));
    const result = JSON.parse(fs.readFileSync(receipt, "utf8"));
    assert.equal(result.observer_slice.status, "failed");
    assert.equal(result.process, null);
    assert.equal(result.cleanup.signal_attempts, 0);
    assert.ok(!JSON.stringify(result).includes("/not-opened"));
  } finally { fs.rmSync(outer, { recursive: true }); }
});

test("prepare rejects symlink output parents and existing state without replacing anything", () => {
  const outer = fs.mkdtempSync(path.join(fs.realpathSync("/tmp"), "native-observer-cli-test-"));
  const runPrepare = (state) => execFileSync(process.execPath,
    [fileURLToPath(new URL("./run-macos-native-observer.mjs", import.meta.url)), "prepare",
      "--state", state, "--build-config", path.join(outer, "build.json"), "--expected-build-sha", "a".repeat(40)],
    { stdio: "pipe" });
  const worlds = () => fs.readdirSync(fs.realpathSync("/tmp")).filter((name) => name.startsWith("codefactory-scenario-")).sort();
  try {
    fs.mkdirSync(path.join(outer, "private"), { mode: 0o700 });
    fs.symlinkSync(path.join(outer, "private"), path.join(outer, "alias"));
    const before = worlds();
    assert.throws(() => runPrepare(path.join(outer, "alias", "state.json")));
    assert.deepEqual(worlds(), before);
    assert.ok(!fs.existsSync(path.join(outer, "private", "state.json")));
    fs.writeFileSync(path.join(outer, "private", "state.json"), "existing", { mode: 0o600 });
    assert.throws(() => runPrepare(path.join(outer, "private", "state.json")));
    assert.equal(fs.readFileSync(path.join(outer, "private", "state.json"), "utf8"), "existing");
    assert.deepEqual(worlds(), before);
  } finally { fs.rmSync(outer, { recursive: true }); }
});
