// Non-required feasibility slice. Importing this module never launches a process.
import { execFile, execFileSync, spawn } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { assertPreparedLayout, assertOwnedLayout, canStopOwnedProcess } from "./native-desktop-probe-contract.mjs";

const digest = (file) => createHash("sha256").update(fs.readFileSync(file)).digest("hex");
const fileIdentity = (file) => {
  const stat = fs.lstatSync(file, { bigint: true });
  return { device: stat.dev.toString(), inode: stat.ino.toString() };
};
const reaped = (child) => child && (child.exitCode !== null || child.signalCode !== null);
const missing = ["native_theme_click", "native_text_input", "restart_theme_persistence",
  "owned_window_screenshots", "descendant_process_cleanup", "world_directory_cleanup",
  "request_observation", "credential_observation", "embedded_build_identity"];
const anonymousIdentity = (value) => value && Object.fromEntries(
  ["run_id", "pid", "start_token", "executable_sha256", "bundle_id"].map((key) => [key, value[key]]));

export const preflightProjection = (value) => ({ accessibility: value?.accessibility === true,
  screen_capture: value?.screen_capture === true, gui_session: value?.gui_session === true,
  session_present: value?.session_present === true, on_console: value?.on_console === true,
  login_done: value?.login_done === true, same_uid: value?.same_uid === true,
  lock_state: ["locked", "unlocked", "unknown"].includes(value?.lock_state) ? value.lock_state : "unknown" });

// Session observation and permission to launch are different facts. In
// particular, a missing private lock key must never erase an observed session
// or be treated as evidence that the screen is unlocked.
export function preflightDecision(raw) {
  const preflight = preflightProjection(raw);
  const reason_codes = [];
  if (!raw || typeof raw !== "object" || Array.isArray(raw) || typeof raw.session_present !== "boolean") {
    reason_codes.push("preflight_observation_unavailable");
  } else if (!preflight.session_present) {
    // This means the caller's CGSession query returned no session, not that no
    // user anywhere on the machine has a WindowServer session.
    reason_codes.push("no_session_observed");
  } else {
    if (!preflight.accessibility) reason_codes.push("accessibility_unavailable");
    if (!preflight.screen_capture) reason_codes.push("screen_capture_unavailable");
    if (!preflight.on_console) reason_codes.push("session_not_on_console");
    if (!preflight.login_done) reason_codes.push("session_login_unproven");
    if (!preflight.same_uid) reason_codes.push("session_uid_unproven");
    if (!preflight.gui_session) reason_codes.push("gui_session_unproven");
    if (preflight.lock_state === "locked") reason_codes.push("screen_locked");
    if (preflight.lock_state === "unknown") reason_codes.push("unlock_state_unproven");
  }
  return { scope: "native-desktop-preflight", status: reason_codes.length === 0 ? "ready" : "blocked",
    preflight, reason_codes: reason_codes.sort() };
}

export async function readOnlyPreflight(driver, { platform = process.platform, environment = process.env, call = native } = {}) {
  if (platform !== "darwin" || environment.GITHUB_ACTIONS !== "true" || environment.RUNNER_OS !== "macOS") {
    throw new Error("macos_ci_only");
  }
  if (typeof driver !== "string" || !path.isAbsolute(driver)) throw new Error("absolute_driver_required");
  let raw = null;
  try { raw = await call(driver, { operation: "preflight" }); } catch { /* No raw helper errors leave this boundary. */ }
  return preflightDecision(raw);
}

async function bounded(operation, milliseconds) {
  let timer;
  try {
    return await Promise.race([Promise.resolve().then(operation), new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("operation_timeout")), milliseconds);
    })]);
  } finally { clearTimeout(timer); }
}

// Retry whole read-only snapshots, never combine partial successes across rounds.
// The real sampler revalidates the same PID/birth lease before AND after each
// native AX traversal. Identity failures propagate immediately, without retry.
export async function observeAXUntilReachable(sample, { timeoutMs = 8000, pollMs = 150 } = {}) {
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 8000
    || !Number.isSafeInteger(pollMs) || pollMs < 1 || pollMs > 1000) throw new Error("invalid_observation_budget");
  const deadline = performance.now() + timeoutMs;
  let last = { ax_window_seen: false, settings_control: false };
  while (performance.now() < deadline) {
    const remaining = Math.max(1, Math.ceil(deadline - performance.now()));
    const snapshot = await bounded(() => sample(remaining), remaining);
    last = { ax_window_seen: snapshot?.ax_window_seen === true, settings_control: snapshot?.settings_control === true };
    if (last.ax_window_seen && last.settings_control) return last;
    const pause = Math.min(pollMs, Math.max(0, deadline - performance.now()));
    if (pause > 0) await new Promise((resolve) => setTimeout(resolve, pause));
  }
  return last;
}

export async function observeOwnedChild(expected, adapters, { operationTimeoutMs = 15000 } = {}) {
  let child = null, lease = null, phase = "world", signals = 0;
  let status = "blocked", reason = "ax_unavailable", observed = {};
  try {
    await bounded(adapters.checkWorld, operationTimeoutMs);
    child = await bounded(adapters.launch, operationTimeoutMs);
    phase = "identity";
    const actual = await bounded(() => adapters.inspect(child.pid), operationTimeoutMs);
    const proposed = { ...expected, pid: child.pid, start_token: actual.start_token };
    if (!canStopOwnedProcess(proposed, actual, { launchRecorded: true })) throw new Error("identity_mismatch");
    lease = actual;
    phase = "world";
    await bounded(adapters.checkWorld, operationTimeoutMs);
    phase = "observe";
    observed = await bounded(() => adapters.observe(lease), operationTimeoutMs);
    if (observed.ax_window_seen === true && observed.settings_control === true) {
      status = "passed"; reason = null;
    }
  } catch {
    status = phase === "observe" ? "blocked" : "failed";
    reason = phase === "observe" ? "ax_observation_unavailable" : `${phase}_validation_failed`;
  } finally {
    if (child && !reaped(child)) {
      if (!lease) { status = "failed"; reason = "cleanup_identity_unavailable"; }
      else {
        try {
          for (const force of [false, true]) {
            if (reaped(child)) break;
            await bounded(adapters.checkWorld, operationTimeoutMs);
            const fresh = await bounded(() => adapters.inspect(child.pid), operationTimeoutMs);
            if (!canStopOwnedProcess(lease, fresh, { launchRecorded: true })) throw new Error("identity_changed");
            signals++;
            await bounded(() => adapters.stop(lease, force), operationTimeoutMs);
            await bounded(() => adapters.waitForExit(child, 2000), 2500);
          }
          if (!reaped(child)) throw new Error("child_not_reaped");
        } catch { status = "failed"; reason = "cleanup_identity_or_reap_failed"; }
      }
    }
  }
  return {
    observer_slice: { status, reason_codes: reason ? [reason] : [],
      ax_window_seen: observed.ax_window_seen === true, settings_control: observed.settings_control === true },
    process: anonymousIdentity(lease),
    cleanup: { child_reaped: Boolean(reaped(child)), signal_attempts: signals, world_directory: "retained" },
    full_probe: { status: "blocked", missing: [...missing] }, request_count: null, credential_access_count: null,
  };
}

function prepareOutput(file) {
  if (typeof file !== "string" || !path.isAbsolute(file) || path.resolve(file) !== file) {
    throw new Error("absolute_output_required");
  }
  let directory = path.parse(file).root;
  const observed = [];
  for (const component of path.dirname(file).slice(directory.length).split(path.sep).filter(Boolean)) {
    directory = path.join(directory, component);
    try { fs.mkdirSync(directory, { mode: 0o700 }); }
    catch (error) { if (error.code !== "EEXIST") throw error; }
    const stat = fs.lstatSync(directory);
    if (!stat.isDirectory() || stat.isSymbolicLink() || fs.realpathSync(directory) !== directory) {
      throw new Error("unsafe_output_parent");
    }
    observed.push([directory, fileIdentity(directory)]);
  }
  const parent = fs.lstatSync(path.dirname(file));
  if (parent.uid !== process.getuid() || (parent.mode & 0o022) !== 0) throw new Error("unsafe_output_parent");
  // O_EXCL below remains the no-overwrite authority. This precheck lets prepare
  // reject bad output destinations before allocating a private Scenario World.
  try { fs.lstatSync(file); throw new Error("output_exists"); }
  catch (error) { if (error.code !== "ENOENT") throw error; }
  return () => {
    for (const [name, before] of observed) {
      const stat = fs.lstatSync(name);
      if (!stat.isDirectory() || stat.isSymbolicLink() || fs.realpathSync(name) !== name
        || JSON.stringify(fileIdentity(name)) !== JSON.stringify(before)) throw new Error("output_parent_replaced");
    }
  };
}

function writeNewJSON(file, value) {
  const check = prepareOutput(file);
  check();
  const fd = fs.openSync(file, fs.constants.O_WRONLY | fs.constants.O_CREAT
    | fs.constants.O_EXCL | fs.constants.O_NOFOLLOW, 0o600);
  try {
    check();
    fs.writeFileSync(fd, JSON.stringify(value, null, 2) + "\n");
  } finally { fs.closeSync(fd); }
}

function prepare(args) {
  if (typeof args["expected-build-sha"] !== "string" || !/^[0-9a-f]{40}$/.test(args["expected-build-sha"])) throw new Error("build_sha_required");
  if (args.state === args["build-config"]) throw new Error("outputs_must_differ");
  prepareOutput(args.state);
  prepareOutput(args["build-config"]);
  const runId = randomUUID(), ownerToken = randomUUID();
  const identifier = `com.codefactory.scenario.${runId.replaceAll("-", "")}`;
  const root = fs.mkdtempSync(path.join(fs.realpathSync("/tmp"), "codefactory-scenario-"));
  const home = path.join(root, "home"), app = path.join(root, "Probe.app");
  for (const directory of [path.join(home, "Library", "Application Support", identifier),
    path.join(home, "Library", "Caches", identifier), path.join(root, "tmp")]) {
    fs.mkdirSync(directory, { recursive: true, mode: 0o700 });
  }
  const manifest = path.join(root, "manifest.json");
  writeNewJSON(manifest, { schema_version: 1, run_id: runId, owner_token: ownerToken,
    identifier, capabilities: ["isolated_app_data"] });
  writeNewJSON(path.join(root, "owner.json"), { schema_version: 1, run_id: runId, owner_token: ownerToken });
  writeNewJSON(args.state, { schema_version: 1, expected_build_sha: args["expected-build-sha"],
    layout: { runId, ownerToken, identifier, root, home, app, manifest,
      executable: path.join(app, "Contents", "MacOS", "codefactory") },
    root_identity: fileIdentity(root), manifest_sha256: digest(manifest),
    owner_sha256: digest(path.join(root, "owner.json")) });
  writeNewJSON(args["build-config"], { identifier, build: { devUrl: null },
    bundle: { createUpdaterArtifacts: false } });
  return { status: "prepared", run_id: runId, identifier };
}

function native(driver, request, timeout = 1500) {
  return new Promise((resolve, reject) => {
    const child = execFile(driver, [], { timeout, maxBuffer: 65536, killSignal: "SIGKILL" }, (error, stdout) => {
      try {
        const value = JSON.parse(stdout);
        if (error || value.error_code) reject(new Error(value.error_code ?? "native_failed"));
        else resolve(value);
      } catch { reject(new Error("native_unavailable")); }
    });
    child.stdin.end(JSON.stringify(request));
  });
}

async function observe(args, progress) {
  const state = JSON.parse(fs.readFileSync(args.state, "utf8"));
  if (state.schema_version !== 1 || typeof state.expected_build_sha !== "string"
    || !/^[0-9a-f]{40}$/.test(state.expected_build_sha)) throw new Error("state_schema_invalid");
  const layout = state.layout;
  assertPreparedLayout(layout);
  const source = fs.realpathSync(args["candidate-app"]);
  const checkout = fs.realpathSync(process.cwd());
  if (!source.startsWith(checkout + path.sep) || !source.endsWith(".app")) throw new Error("candidate_outside_checkout");
  if (fs.existsSync(layout.app)) throw new Error("world_app_already_exists");
  if (JSON.stringify(fileIdentity(layout.root)) !== JSON.stringify(state.root_identity)
    || digest(layout.manifest) !== state.manifest_sha256
    || digest(path.join(layout.root, "owner.json")) !== state.owner_sha256) throw new Error("world_identity_changed");
  fs.cpSync(source, layout.app, { recursive: true, errorOnExist: true, force: false,
    filter: (item) => {
      const stat = fs.lstatSync(item);
      if (stat.isSymbolicLink() || (!stat.isFile() && !stat.isDirectory())) throw new Error("candidate_link_or_type");
      return true;
    } });
  assertOwnedLayout(layout);
  const sourceSHA = digest(path.join(source, "Contents", "MacOS", "codefactory"));
  const copySHA = digest(layout.executable);
  if (sourceSHA !== copySHA) throw new Error("copied_executable_mismatch");
  const bundleID = execFileSync("/usr/bin/plutil", ["-extract", "CFBundleIdentifier", "raw", "-o", "-",
    path.join(layout.app, "Contents", "Info.plist")], { encoding: "utf8", timeout: 5000, stdio: ["ignore", "pipe", "pipe"] }).trim();
  if (bundleID !== layout.identifier) throw new Error("candidate_identifier_mismatch");
  const expected = { run_id: layout.runId, owner_token: layout.ownerToken,
    executable_sha256: copySHA, bundle_id: layout.identifier };
  const request = { ...expected, root: layout.root, executable: layout.executable,
    root_identity: state.root_identity, manifest_sha256: state.manifest_sha256, owner_sha256: state.owner_sha256 };
  const result = { schema_version: 1, scope: "native-desktop-observer-slice", run_id: layout.runId,
    identifier: layout.identifier, expected_build_sha: state.expected_build_sha,
    build_identity_source: "ci_input_unverified_in_binary", source_executable_sha256: sourceSHA,
    copy_executable_sha256: copySHA, driver_sha256: digest(args.driver) };
  const decision = preflightDecision(await native(args.driver, { operation: "preflight" }));
  console.log(JSON.stringify(decision));
  if (decision.status !== "ready") {
    return { ...result, process: null, observer_slice: { status: "blocked", reason_codes: ["native_preflight_unproven"],
      ax_window_seen: false, settings_control: false }, cleanup: { child_reaped: false, signal_attempts: 0, world_directory: "retained" },
    full_probe: { status: "blocked", missing: [...missing] }, request_count: null, credential_access_count: null };
  }
  let child;
  const call = (operation, extra = {}, timeout) => native(args.driver, { ...request, operation, ...extra }, timeout);
  const adapters = {
    checkWorld: async () => {
      assertOwnedLayout(layout);
      if (JSON.stringify(fileIdentity(layout.root)) !== JSON.stringify(state.root_identity)
        || digest(layout.manifest) !== state.manifest_sha256 || digest(path.join(layout.root, "owner.json")) !== state.owner_sha256
        || digest(layout.executable) !== copySHA) throw new Error("world_identity_changed");
    },
    launch: () => {
      progress.launchAttempted = true;
      child = spawn(layout.executable, [], { cwd: layout.root, stdio: "ignore", env: {
        PATH: "/usr/bin:/bin:/usr/sbin:/sbin", HOME: layout.home, TMPDIR: path.join(layout.root, "tmp"),
        CODEFACTORY_SCENARIO_MANIFEST: layout.manifest, CODEFACTORY_SCENARIO_RUN_ID: layout.runId,
        CODEFACTORY_SCENARIO_OWNER_TOKEN: layout.ownerToken,
      } });
      child.on("error", () => {}); // Failure is represented by missing/reaped process identity.
      return child;
    },
    inspect: async (pid) => {
      let last;
      for (let attempt = 0; attempt < 3; attempt++) {
        try { return await call("identity", { pid }); } catch (error) {
          last = error;
          if (!["process_unavailable", "bundle_unavailable"].includes(error.message)) throw error;
          await new Promise((resolve) => setTimeout(resolve, 100));
        }
      }
      throw last;
    },
    observe: (lease) => observeAXUntilReachable((remaining) =>
      call("observe", { pid: lease.pid, start_token: lease.start_token }, Math.min(4500, remaining))),
    stop: (lease, force) => call(force ? "force-stop" : "stop", { pid: lease.pid, start_token: lease.start_token }),
    waitForExit: async (_child, milliseconds) => {
      const deadline = Date.now() + milliseconds;
      while (!reaped(child) && Date.now() < deadline) await new Promise((resolve) => setTimeout(resolve, 25));
      return Boolean(reaped(child));
    },
  };
  return { ...result, ...await observeOwnedChild(expected, adapters) };
}

async function main() {
  const [command, ...rest] = process.argv.slice(2);
  const allowed = command === "prepare" ? ["state", "build-config", "expected-build-sha"]
    : command === "observe" ? ["state", "candidate-app", "driver", "receipt"]
      : command === "preflight" ? ["driver"] : [];
  const args = {};
  for (let i = 0; i < rest.length; i += 2) {
    const key = rest[i]?.replace(/^--/, "");
    if (!allowed.includes(key) || key in args || !rest[i + 1]) throw new Error("invalid_arguments");
    args[key] = rest[i + 1];
  }
  if (allowed.length === 0 || allowed.some((key) => !args[key])) throw new Error("invalid_arguments");
  if (command === "prepare") { console.log(JSON.stringify(prepare(args))); return; }
  if (command === "preflight") {
    const result = await readOnlyPreflight(args.driver);
    console.log(JSON.stringify(result));
    process.exitCode = result.status === "ready" ? 0 : 3;
    return;
  }
  // Refuse unsafe receipt paths before any possible app launch. Only this file
  // is raw private evidence; stdout is a fixed summary, never the receipt.
  prepareOutput(args.receipt);
  const progress = { launchAttempted: false };
  let result;
  try {
    if (process.platform !== "darwin" || process.env.GITHUB_ACTIONS !== "true" || process.env.RUNNER_OS !== "macOS") {
      throw new Error("macos_ci_only");
    }
    result = await observe(args, progress);
  } catch {
    result = { schema_version: 1, scope: "native-desktop-observer-slice", run_id: null, identifier: null,
      expected_build_sha: null, build_identity_source: "ci_input_unverified_in_binary",
      source_executable_sha256: null, copy_executable_sha256: null, driver_sha256: null, process: null,
      observer_slice: { status: "failed", reason_codes: ["safety_or_execution_unproven"],
        ax_window_seen: false, settings_control: false },
      cleanup: { child_reaped: false, signal_attempts: progress.launchAttempted ? null : 0, world_directory: "retained" },
      full_probe: { status: "blocked", missing: [...missing] }, request_count: null, credential_access_count: null };
  }
  writeNewJSON(args.receipt, result);
  console.log(JSON.stringify({ observer_slice: result.observer_slice.status, full_probe: "blocked" }));
  process.exitCode = result.observer_slice.status === "passed" ? 0 : result.observer_slice.status === "blocked" ? 3 : 2;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  main().catch(() => { console.error("native_observer_safety_or_execution_failed; private world retained"); process.exitCode = 2; });
}
