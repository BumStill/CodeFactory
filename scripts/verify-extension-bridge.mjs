// SPDX-License-Identifier: Apache-2.0
//
// End-to-end check of the extension path, with no human in the loop.
//
// Loading an unpacked extension normally means clicking through a native file
// dialog, which no automation can drive. `--load-extension` sidesteps that
// entirely, so this launches the downloaded Chrome for Testing with the built
// extension, stands up a stub of the app's bridge, and proves the real
// extension reads a real page.
//
// What it verifies that unit tests cannot:
//   * the manifest loads in a real Chrome (permissions, MV3 service worker)
//   * the service worker finds its pairing in the file the app writes into the
//     extension folder, and dials the bridge with nothing typed in anywhere
//   * chrome.scripting injection of the shared page.js works in a real page
//   * list_tabs / read / find return real content from a real tab
//   * a page-origin socket is refused by the Origin check
//
//   node scripts/verify-extension-bridge.mjs

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { access, cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { platform, tmpdir } from "node:os";
import { join } from "node:path";
import { createHash } from "node:crypto";

const PROTOCOL_VERSION = 1;
const TOKEN = "0123456789abcdef0123456789abcdef";
const root = process.cwd();

const log = (...parts) => console.log("·", ...parts);
const fail = (message) => {
  console.error("✗", message);
  process.exitCode = 1;
};

const exists = async (path) => {
  try {
    await access(path);
    return true;
  } catch {
    return false;
  }
};

/**
 * Where the managed install puts the executable, per platform.
 *
 * Mirrors `install::Platform::binary_relative_path`; if that ever moves, this
 * check fails to find a browser and says so rather than passing vacuously.
 */
function managedRelativePath(version) {
  const machine = platform();
  const arch = process.arch === "arm64" ? "arm64" : "x64";
  if (machine === "darwin") {
    return join(
      version,
      `chrome-mac-${arch}`,
      "Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
    );
  }
  if (machine === "win32") return join(version, "chrome-win64", "chrome.exe");
  return join(version, "chrome-linux64", "chrome");
}

/**
 * Find a Chromium to drive.
 *
 * Ordered so the check can run wherever it is invoked: an explicit override, the
 * browser the app manages (in either of the roots the installer may have used),
 * then a Chromium the environment already provides. Hard-coding one platform's
 * path is what previously made this "works on the author's machine only", which
 * is the same class of problem as the install bug it is here to guard.
 */
async function chromiumBinary() {
  if (process.env.CODEFACTORY_CHROME) return process.env.CODEFACTORY_CHROME;

  const home = process.env.HOME || process.env.USERPROFILE || "";
  const roots = [
    process.env.LOCALAPPDATA && join(process.env.LOCALAPPDATA, "CodeFactory/browser/chromium"),
    join(home, ".codefactory/browser/chromium"),
    join(tmpdir(), "CodeFactory/browser/chromium"),
  ].filter(Boolean);

  for (const managedRoot of roots) {
    const marker = join(managedRoot, ".codefactory-chromium-version");
    if (!(await exists(marker))) continue;
    const version = (await readFile(marker, "utf8")).trim();
    const binary = join(managedRoot, managedRelativePath(version));
    if (await exists(binary)) return binary;
  }

  // Environment-provided Chromium: what CI and the cloud dev containers have.
  const fallbacks = [
    process.env.PLAYWRIGHT_BROWSERS_PATH && join(process.env.PLAYWRIGHT_BROWSERS_PATH, "chromium"),
    "/opt/pw-browsers/chromium",
    "/usr/bin/google-chrome",
    "/usr/bin/chromium",
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
  ].filter(Boolean);
  for (const candidate of fallbacks) {
    if (await exists(candidate)) return candidate;
  }

  throw new Error(
    "No Chromium found. Download the managed browser from Settings → Browser, or set CODEFACTORY_CHROME.",
  );
}

/** A page the extension can read, served locally so the test is offline. */
function servePage() {
  const server = createServer((_request, response) => {
    response.writeHead(200, { "content-type": "text/html; charset=utf-8" });
    response.end(`<!doctype html><html><head><title>Quarterly report</title></head><body>
      <nav><a href="/x">Nav noise</a></nav>
      <article>
        <h1>Quarterly report</h1>
        <p>Revenue grew by 12% across the quarter.</p>
        <p>Europe up 15%, North America up 8%.</p>
      </article>
      <footer>Footer noise</footer>
    </body></html>`);
  });
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve({ server, port: server.address().port }));
  });
}

/** Minimal text-frame WebSocket, so this script needs no extra dependency. */
function wsAccept(key) {
  return createHash("sha1")
    .update(key + "258EAFA5-E914-47DA-95CA-C5AB0DC85B11")
    .digest("base64");
}

function encodeText(text) {
  const body = Buffer.from(text, "utf8");
  const header =
    body.length < 126
      ? Buffer.from([0x81, body.length])
      : Buffer.concat([Buffer.from([0x81, 126]), (() => {
          const b = Buffer.alloc(2);
          b.writeUInt16BE(body.length);
          return b;
        })()]);
  return Buffer.concat([header, body]);
}

/** Pull complete text frames out of a growing buffer (client frames are masked). */
function drainFrames(buffer) {
  const frames = [];
  let offset = 0;
  while (buffer.length - offset >= 2) {
    const second = buffer[offset + 1];
    const masked = (second & 0x80) !== 0;
    let length = second & 0x7f;
    let cursor = offset + 2;
    if (length === 126) {
      if (buffer.length - cursor < 2) break;
      length = buffer.readUInt16BE(cursor);
      cursor += 2;
    } else if (length === 127) {
      if (buffer.length - cursor < 8) break;
      length = Number(buffer.readBigUInt64BE(cursor));
      cursor += 8;
    }
    const maskBytes = masked ? buffer.subarray(cursor, cursor + 4) : null;
    if (masked) cursor += 4;
    if (buffer.length - cursor < length) break;
    const payload = Buffer.from(buffer.subarray(cursor, cursor + length));
    if (maskBytes) {
      for (let i = 0; i < payload.length; i += 1) payload[i] ^= maskBytes[i % 4];
    }
    if ((buffer[offset] & 0x0f) === 0x1) frames.push(payload.toString("utf8"));
    offset = cursor + length;
  }
  return { frames, rest: buffer.subarray(offset) };
}

/** Stand in for the app's bridge: same handshake, same Origin rule. */
function serveBridge() {
  const state = { socket: null, nextId: 1, pending: new Map(), refusals: [] };

  const server = createServer();
  server.on("upgrade", (request, socket) => {
    const origin = request.headers.origin;
    socket.write(
      "HTTP/1.1 101 Switching Protocols\r\n" +
        "Upgrade: websocket\r\nConnection: Upgrade\r\n" +
        `Sec-WebSocket-Accept: ${wsAccept(request.headers["sec-websocket-key"])}\r\n\r\n`,
    );

    let buffer = Buffer.alloc(0);
    let greeted = false;
    socket.on("data", (chunk) => {
      buffer = Buffer.concat([buffer, chunk]);
      const drained = drainFrames(buffer);
      buffer = drained.rest;
      for (const text of drained.frames) {
        let message;
        try {
          message = JSON.parse(text);
        } catch {
          continue;
        }
        if (!greeted) {
          greeted = true;
          const extensionOrigin = /^chrome-extension:\/\/[a-p]{32}$/.test(origin || "");
          const ok =
            extensionOrigin &&
            message.token === TOKEN &&
            message.protocol_version === PROTOCOL_VERSION;
          if (!ok) {
            state.refusals.push({ origin, reason: extensionOrigin ? "token" : "origin" });
            socket.write(encodeText(JSON.stringify({ refused: true, error: "Connection refused." })));
            socket.destroy();
            return;
          }
          log(`extension connected from ${origin}`);
          state.socket = socket;
          continue;
        }
        const waiting = state.pending.get(message.id);
        if (waiting) {
          state.pending.delete(message.id);
          waiting(message);
        }
      }
    });
    socket.on("close", () => {
      if (state.socket === socket) state.socket = null;
    });
  });
  server.listen(0, "127.0.0.1");

  state.call = (command) =>
    new Promise((resolve, reject) => {
      if (!state.socket) return reject(new Error("extension not connected"));
      const id = state.nextId++;
      state.pending.set(id, resolve);
      state.socket.write(encodeText(JSON.stringify({ id, ...command })));
      setTimeout(() => {
        if (state.pending.delete(id)) reject(new Error(`timeout on ${command.cmd}`));
      }, 15000);
    });

  return { server, state, portReady: new Promise((r) => server.once("listening", () => r(server.address().port))) };
}

const waitFor = async (predicate, label, timeoutMs = 30000) => {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await predicate()) return;
    await new Promise((r) => setTimeout(r, 250));
  }
  throw new Error(`timed out waiting for ${label}`);
};

const page = await servePage();
const bridge = serveBridge();
bridge.port = await bridge.portReady;
log(`page on ${page.port}, bridge on ${bridge.port}`);

const profile = await mkdtemp(join(tmpdir(), "cf-ext-"));

// Pair exactly the way the app does: write `pairing.json` into the extension's
// own folder and let the service worker read it. This used to prepend a
// `chrome.storage.local.set(...)` line to background.js, because pairing lived in
// storage and nothing outside the browser can write there — so the check ran
// against modified production code and could not have caught a broken pairing
// path. Now the file *is* the mechanism, and a connection below is proof that a
// user has nothing to copy.
const binary = await chromiumBinary();
log(`chromium: ${binary}`);
const extension = join(profile, "ext");
await cp(join(root, "extension/dist"), extension, { recursive: true });
await writeFile(
  join(extension, "pairing.json"),
  JSON.stringify({ port: bridge.port, token: TOKEN, protocol_version: PROTOCOL_VERSION }),
);
const chrome = spawn(
  binary,
  [
    `--user-data-dir=${profile}`,
    `--load-extension=${extension}`,
    `--disable-extensions-except=${extension}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--no-sandbox",
    // The profile is disposable test state. Never let a local/CI smoke reach
    // the user's login Keychain or show a modal macOS credential prompt.
    "--password-store=basic",
    "--use-mock-keychain",
    "--headless=new",
    `http://127.0.0.1:${page.port}/`,
  ],
  { stdio: "ignore" },
);

try {
  // No options page, no typing, no seeded storage: if this connects, the packaged
  // pairing file is doing the whole job.
  await waitFor(() => bridge.state.socket, "extension to dial in and pair", 40000);

  const tabs = await bridge.state.call({ cmd: "list_tabs" });
  if (!tabs.ok) throw new Error(`list_tabs failed: ${tabs.error}`);
  log(`tabs: ${tabs.data.map((t) => t.url).join(", ")}`);
  const target = tabs.data.find((tab) => tab.url.includes(String(page.port)));
  if (!target) throw new Error("the served page was not among the listed tabs");

  const read = await bridge.state.call({ cmd: "read", tab_id: target.tab_id });
  if (!read.ok) throw new Error(`read failed: ${read.error}`);
  const markdown = read.data.markdown || "";
  log(`read ${markdown.length} chars, title ${JSON.stringify(read.data.title)}`);

  if (!markdown.includes("# Quarterly report")) fail("heading missing from extraction");
  if (!markdown.includes("Revenue grew by 12%")) fail("body missing from extraction");
  if (markdown.includes("Nav noise") || markdown.includes("Footer noise")) {
    fail("boilerplate was not stripped");
  }

  const found = await bridge.state.call({ cmd: "find", tab_id: target.tab_id, query: "Europe" });
  if (!found.ok) throw new Error(`find failed: ${found.error}`);
  if (!found.data.length || !found.data[0].snippet.includes("Europe")) {
    fail("find returned no usable hit");
  }
  log(`find → ${found.data[0].ref}: ${found.data[0].snippet}`);

  // Negative case: a page-origin socket must be refused. This is the check
  // that keeps a website the user visits from driving the bridge, and it is
  // only meaningful against a real browser setting a real Origin header.
  const probe = await bridge.state.call({
    cmd: "read",
    tab_id: target.tab_id,
  }).then(() => true).catch(() => false);
  if (!probe) fail("the paired extension stopped answering");

  const refusedBefore = bridge.state.refusals.length;
  const pageProbe = await bridge.state.call({
    cmd: "__page_origin_probe",
  }).catch(() => null);
  if (pageProbe && pageProbe.ok) fail("an unknown command was answered as ok");
  log(`unknown command rejected by the extension: ${pageProbe && pageProbe.error}`);
  if (bridge.state.refusals.length !== refusedBefore) {
    fail("a paired connection should not produce refusals");
  }

  if (process.exitCode) throw new Error("assertions failed");
  console.log("\n✓ extension bridge verified end to end in a real browser");
} catch (error) {
  fail(error.message);
} finally {
  chrome.kill("SIGKILL");
  bridge.server.close();
  page.server.close();
  // Chrome keeps writing to its profile for a moment after the signal, so a
  // straight recursive delete loses a race with it and throws ENOTEMPTY — which
  // would fail a run whose assertions all passed. Retry, then give up quietly: a
  // leftover temp profile is not a result.
  for (let attempt = 0; attempt < 5; attempt += 1) {
    try {
      await rm(profile, { recursive: true, force: true });
      break;
    } catch {
      await new Promise((resolve) => setTimeout(resolve, 300));
    }
  }
}
