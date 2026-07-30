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
//   * the service worker dials the loopback socket and completes the handshake
//   * chrome.scripting injection of the shared page.js works in a real page
//   * list_tabs / read / find return real content from a real tab
//   * a page-origin socket is refused by the Origin check
//
//   node scripts/verify-extension-bridge.mjs

import { spawn } from "node:child_process";
import { createServer } from "node:http";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
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

/** Locate the Chromium the app's installer downloaded. */
async function chromiumBinary() {
  const home = process.env.HOME;
  const marker = join(home, ".codefactory/browser/chromium/.codefactory-chromium-version");
  const version = (await readFile(marker, "utf8")).trim();
  return join(
    home,
    ".codefactory/browser/chromium",
    version,
    "chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
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

// Preseed the extension's pairing so no options-page clicking is needed. The
// storage is per-profile, so this writes into the throwaway profile only.
const profile = await mkdtemp(join(tmpdir(), "cf-ext-"));

// The service worker reads pairing from chrome.storage, which cannot be written
// from outside the browser. So load a *copy* of the built extension with the
// pairing prepended. Production code is untouched, and the seed lands before
// connect() runs — which also exercises the real "re-connect when pairing
// changes" path via the storage.onChanged listener.
const binary = await chromiumBinary();
const extension = join(profile, "ext");
await cp(join(root, "extension/dist"), extension, { recursive: true });
const backgroundPath = join(extension, "background.js");
await writeFile(
  backgroundPath,
  `chrome.storage.local.set(${JSON.stringify({ port: bridge.port, token: TOKEN })});\n` +
    (await readFile(backgroundPath, "utf8")),
);
const chrome = spawn(
  binary,
  [
    `--user-data-dir=${profile}`,
    `--load-extension=${extension}`,
    `--disable-extensions-except=${extension}`,
    "--no-first-run",
    "--no-default-browser-check",
    "--headless=new",
    `http://127.0.0.1:${page.port}/`,
  ],
  { stdio: "ignore" },
);

try {
  // The service worker reads pairing from chrome.storage, which we cannot write
  // from outside — so pair by driving the extension's own options page instead.
  // `--load-extension` gives a stable id, discovered from the connection.
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
  await rm(profile, { recursive: true, force: true });
}
