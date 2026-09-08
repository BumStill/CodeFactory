// Local feasibility contracts only. No app launch, signal, deletion or gate integration.
import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const SHA = /^[0-9a-f]{64}$/;
const INPUT_DIGEST = createHash("sha256").update("CODEFACTORY_NATIVE_PROBE_ONLY").digest("hex");
const IDENTITY_KEYS = ["run_id", "pid", "start_token", "executable_sha256", "bundle_id"];
const PRIVATE_IDENTITY_KEYS = [...IDENTITY_KEYS, "owner_token"];
const isUUID = (value) => typeof value === "string" && UUID.test(value);
const isSHA = (value) => typeof value === "string" && SHA.test(value);
const isCount = (value) => Number.isSafeInteger(value) && value >= 0;
// Darwin's public SDK defines pid_t as __int32_t. Representability is only
// a necessary condition; a future adapter must also observe the live process.
const isPID = (value) => Number.isSafeInteger(value) && value > 1 && value <= 0x7fffffff;
const plain = (value) => value !== null && typeof value === "object" && !Array.isArray(value);
const keysMatch = (value, keys) => plain(value)
  && Object.keys(value).sort().join("\0") === [...keys].sort().join("\0");
const identifierFor = (runId) => `com.codefactory.scenario.${runId.replaceAll("-", "")}`;

function validIdentity(value, privateOwner = false) {
  return keysMatch(value, privateOwner ? PRIVATE_IDENTITY_KEYS : IDENTITY_KEYS) && isUUID(value.run_id)
    && isPID(value.pid)
    && typeof value.start_token === "string" && /^\d+:\d+$/.test(value.start_token)
    && isSHA(value.executable_sha256)
    && value.bundle_id === identifierFor(value.run_id)
    && (!privateOwner || isUUID(value.owner_token));
}

export function canStopOwnedProcess(lease, observed, { launchRecorded = false } = {}) {
  return launchRecorded === true && validIdentity(lease, true) && validIdentity(observed, true)
    && PRIVATE_IDENTITY_KEYS.every((key) => lease[key] === observed[key]);
}

function assertPathType(target, directory) {
  const stat = fs.lstatSync(target);
  if (fs.realpathSync(target) !== target || stat.isSymbolicLink()
    || (directory ? !stat.isDirectory() : !stat.isFile() || stat.nlink !== 1)
    || (typeof process.getuid === "function" && stat.uid !== process.getuid())
    || (stat.mode & 0o022) !== 0) throw new Error("unsafe_probe_layout");
  return stat;
}

function readOwnedJSON(target) {
  const before = assertPathType(target, false);
  const fd = fs.openSync(target, fs.constants.O_RDONLY | fs.constants.O_NOFOLLOW);
  try {
    const opened = fs.fstatSync(fd);
    if (opened.dev !== before.dev || opened.ino !== before.ino) throw new Error("unsafe_probe_layout");
    const buffer = Buffer.alloc(65537);
    const bytesRead = fs.readSync(fd, buffer);
    const after = assertPathType(target, false);
    if (bytesRead > 65536 || after.dev !== before.dev || after.ino !== before.ino) throw new Error("unsafe_probe_layout");
    return JSON.parse(buffer.subarray(0, bytesRead).toString("utf8"));
  } finally { fs.closeSync(fd); }
}

// Checks current filesystem observations only; this is not a deletion primitive.
// These historical predicates never authorize a destructive operation. Before
// any future signal/deletion, re-observe process identity AND the owned directory
// device/inode, and use an operation-boundary guard against replacement races.
export function assertPreparedLayout(layout, tempRoot = fs.realpathSync("/tmp")) {
  return checkLayout(layout, tempRoot, false);
}
export function assertOwnedLayout(layout, tempRoot = fs.realpathSync("/tmp")) {
  return checkLayout(layout, tempRoot, true);
}
function checkLayout(layout, tempRoot, appRequired) {
  const fail = () => { throw new Error("unsafe_probe_layout"); };
  if (!plain(layout) || !isUUID(layout.runId) || !isUUID(layout.ownerToken)) fail();
  if (tempRoot !== fs.realpathSync("/tmp") || typeof layout.root !== "string") fail();
  if (path.dirname(layout.root) !== tempRoot || !/^codefactory-scenario-.+$/.test(path.basename(layout.root))) fail();
  if (layout.identifier !== identifierFor(layout.runId) || layout.manifest !== path.join(layout.root, "manifest.json")) fail();
  const expected = {
    root: layout.root,
    app: path.join(layout.root, "Probe.app"),
    home: path.join(layout.root, "home"),
    executable: path.join(layout.root, "Probe.app", "Contents", "MacOS", "codefactory"),
  };
  for (const [name, wanted] of Object.entries(expected)) {
    if (layout[name] !== wanted || !path.isAbsolute(wanted)) fail();
    if (!appRequired && (name === "app" || name === "executable")) continue;
    if (fs.realpathSync(wanted) !== wanted) fail();
    assertPathType(wanted, name !== "executable");
  }
  const library = path.join(layout.home, "Library");
  for (const directory of [library, path.join(library, "Application Support"),
    path.join(library, "Caches"), path.join(library, "Application Support", layout.identifier),
    path.join(library, "Caches", layout.identifier), path.join(layout.root, "tmp"),
    ...(appRequired ? [path.join(layout.app, "Contents"), path.join(layout.app, "Contents", "MacOS")] : [])]) assertPathType(directory, true);
  const rootStat = fs.lstatSync(layout.root);
  if ((rootStat.mode & 0o077) !== 0) fail();
  const owner = readOwnedJSON(path.join(layout.root, "owner.json"));
  if (!keysMatch(owner, ["schema_version", "run_id", "owner_token"]) || owner.schema_version !== 1
    || owner.run_id !== layout.runId || owner.owner_token !== layout.ownerToken) fail();
  const manifest = readOwnedJSON(layout.manifest);
  if (!keysMatch(manifest, ["schema_version", "run_id", "owner_token", "identifier", "capabilities"])
    || manifest.schema_version !== 1 || manifest.run_id !== layout.runId || manifest.owner_token !== layout.ownerToken
    || manifest.identifier !== layout.identifier || !Array.isArray(manifest.capabilities)
    || manifest.capabilities.length !== 1 || manifest.capabilities[0] !== "isolated_app_data") fail();
  return true;
}

export function cleanupEligible(layout, tempRoot, observation) {
  try { assertOwnedLayout(layout, tempRoot); } catch { return false; }
  return keysMatch(observation, ["complete", "owned_processes_reaped", "live_under_root"])
    && observation.complete === true && observation.owned_processes_reaped === true
    && isCount(observation.live_under_root) && observation.live_under_root === 0;
}

// Reject missing or surplus evidence without echoing any untrusted value.
// This proposed observation shape is deliberately NOT a trusted case receipt.
export function observationErrors(value) {
  const errors = new Set();
  const check = (ok, code) => { if (!ok) errors.add(code); };
  if (!plain(value)) return ["invalid_observation"];
  check(keysMatch(value, [
    "schema_version", "scope", "run_id", "executable_sha256", "first", "second",
    "accessibility", "screen_capture", "gui_session", "native_ax_press_count",
    "native_text_input_count", "input_digest", "input_readback_digest", "theme_before",
    "theme_after", "theme_reopened", "first_process_reaped", "request_count",
    "credential_access_count", "screenshots", "cleanup",
  ]), "unexpected_or_missing_fields");
  check(value.schema_version === 1 && value.scope === "native-desktop-feasibility-only", "invalid_scope");
  check(isUUID(value.run_id) && isSHA(value.executable_sha256), "invalid_run_identity");
  for (const processIdentity of [value.first, value.second]) {
    check(validIdentity(processIdentity) && processIdentity.run_id === value.run_id
      && processIdentity.executable_sha256 === value.executable_sha256, "process_identity_mismatch");
  }
  check(value.first?.pid !== value.second?.pid && value.first?.start_token !== value.second?.start_token
    && value.first_process_reaped === true, "restart_not_proven");
  check(value.accessibility === true && value.screen_capture === true && value.gui_session === true, "native_permissions_missing");
  check(isCount(value.native_ax_press_count) && value.native_ax_press_count >= 2, "native_clicks_missing");
  check(isCount(value.native_text_input_count) && value.native_text_input_count === 1 && value.input_digest === INPUT_DIGEST
    && value.input_readback_digest === INPUT_DIGEST, "native_input_missing");
  check(value.theme_before === "dark" && value.theme_after === "light"
    && value.theme_reopened === "light", "theme_persistence_missing");
  check(isCount(value.request_count) && value.request_count === 0
    && isCount(value.credential_access_count) && value.credential_access_count === 0, "privacy_boundary_crossed");
  check(Array.isArray(value.screenshots) && value.screenshots.length === 2, "screenshots_missing");
  for (const [index, screenshot] of (Array.isArray(value.screenshots) ? value.screenshots : []).entries()) {
    check(keysMatch(screenshot, ["owner_pid", "run_id", "executable_sha256", "sha256", "width", "height"])
      && screenshot.run_id === value.run_id && screenshot.executable_sha256 === value.executable_sha256
      && screenshot.owner_pid === [value.first?.pid, value.second?.pid][index]
      && isPID(screenshot.owner_pid) && isSHA(screenshot.sha256)
      && isCount(screenshot.width) && screenshot.width >= 800
      && isCount(screenshot.height) && screenshot.height >= 600, "screenshot_identity_mismatch");
  }
  const cleanup = value.cleanup;
  check(keysMatch(cleanup, ["owner_run_id", "observed_complete", "remaining_process_count", "run_root_removed"])
    && cleanup.owner_run_id === value.run_id && cleanup.observed_complete === true
    && isCount(cleanup.remaining_process_count) && cleanup.remaining_process_count === 0
    && cleanup.run_root_removed === true, "cleanup_not_proven");
  return [...errors].sort();
}
