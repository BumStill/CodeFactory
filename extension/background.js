// SPDX-License-Identifier: Apache-2.0
//
// Bridge client. Connects out to CodeFactory and serves read commands against
// the tabs the user already has open.
//
// Direction matters: an extension cannot listen, so this side dials out. That
// also means the app never has to reach into the browser — nothing is exposed
// to the network, and closing CodeFactory ends the connection.
//
// Two constraints from Manifest V3 shape the code:
//
//   * A service worker is killed when idle. The socket cannot be assumed alive,
//     so every command path tolerates a cold start and an alarm re-opens the
//     connection after the worker is evicted.
//   * Page work happens through `chrome.scripting`, in the isolated world. The
//     injected file is the same `page.js` the desktop app injects over CDP, so
//     extraction behaves identically on both paths.
//
// Pairing needs no typing. An extension cannot read the user's disk, which is
// why the port and token used to be copied in by hand — but it can always read
// its *own* package, and CodeFactory is what writes that package out. So the app
// drops the live values into `pairing.json` beside this file and refreshes them
// whenever the bridge restarts; this worker just re-reads the file on every
// connection attempt. The manually entered values stay supported as a fallback,
// for an install this app did not write (a store build, or a copy of the folder).

const PROTOCOL_VERSION = 1;
const PAGE_SCRIPT = "content/page.js";
const PAIRING_FILE = "pairing.json";
const KEEPALIVE_ALARM = "codefactory-bridge-keepalive";
const RECONNECT_CEILING_MS = 30_000;

let socket = null;
let reconnectDelayMs = 1000;

/**
 * Pairing written into this extension's folder by the running app.
 *
 * Read fresh every time, with the HTTP cache bypassed: the port can change when
 * CodeFactory restarts, and a cached answer would leave the extension dialling a
 * socket nobody is listening on until the user intervened — the exact manual
 * step this file exists to remove.
 */
async function packagedPairing() {
  try {
    const response = await fetch(chrome.runtime.getURL(PAIRING_FILE), { cache: "no-store" });
    if (!response.ok) return null;
    const paired = await response.json();
    const port = Number(paired.port);
    const token = String(paired.token || "");
    if (!Number.isInteger(port) || port < 1 || port > 65535 || token.length < 16) return null;
    return { port, token, source: "packaged" };
  } catch {
    // No file at all is the normal case for a store install, not an error.
    return null;
  }
}

/** Pairing details the user entered on the options page, if any. */
async function storedPairing() {
  const stored = await chrome.storage.local.get(["port", "token"]);
  if (!stored.port || !stored.token) return null;
  return { port: Number(stored.port), token: String(stored.token), source: "manual" };
}

/**
 * Which pairing to dial.
 *
 * The packaged file wins: it is written by the app that is running right now, so
 * it is the only one that can be relied on to be current. Manual values are the
 * fallback rather than the override — otherwise a stale value typed in months
 * ago would permanently shadow the live one.
 */
async function pairing() {
  return (await packagedPairing()) || (await storedPairing());
}

async function setStatus(status, extra = {}) {
  await chrome.storage.local.set({ status, statusAt: Date.now(), ...extra });
}

function connect() {
  pairing().then((paired) => {
    if (!paired) {
      setStatus("not_paired");
      return;
    }
    // Recorded so the options page can say "paired automatically" instead of
    // showing an empty form that looks like nothing has been set up.
    setStatus("connecting", { pairingSource: paired.source, activePort: paired.port });
    if (socket && (socket.readyState === WebSocket.OPEN || socket.readyState === WebSocket.CONNECTING)) {
      return;
    }

    // Loopback only. The app refuses any origin that is not this extension, so
    // a page cannot stand in for us even though it could open the same socket.
    socket = new WebSocket(`ws://127.0.0.1:${paired.port}`);

    socket.addEventListener("open", () => {
      reconnectDelayMs = 1000;
      socket.send(
        JSON.stringify({
          protocol_version: PROTOCOL_VERSION,
          token: paired.token,
          extension_version: chrome.runtime.getManifest().version,
        }),
      );
      setStatus("connected");
    });

    socket.addEventListener("message", async (event) => {
      let request;
      try {
        request = JSON.parse(event.data);
      } catch {
        return;
      }
      // A refusal arrives as a message with no command; surface it so the
      // options page can tell the user to re-pair instead of looping silently.
      if (request.refused) {
        setStatus("refused");
        socket.close();
        return;
      }
      const reply = await handle(request);
      if (socket && socket.readyState === WebSocket.OPEN) {
        socket.send(JSON.stringify({ id: request.id, ...reply }));
      }
    });

    socket.addEventListener("close", () => {
      socket = null;
      setStatus("disconnected");
      // Backoff: CodeFactory being closed is the normal case, not an error, so
      // retries must not spin.
      reconnectDelayMs = Math.min(reconnectDelayMs * 2, RECONNECT_CEILING_MS);
      setTimeout(connect, reconnectDelayMs);
    });

    socket.addEventListener("error", () => setStatus("error"));
  });
}

/** Run one command and produce a reply. Never throws. */
async function handle(request) {
  try {
    switch (request.cmd) {
      case "list_tabs":
        return { ok: true, data: await listTabs() };
      case "read":
        return { ok: true, data: await inPage(request.tab_id, "readable") };
      case "find":
        return {
          ok: true,
          data: await inPage(request.tab_id, "find", [request.query, 20]),
        };
      default:
        return { ok: false, error: `Unknown command: ${request.cmd}` };
    }
  } catch (error) {
    return { ok: false, error: String(error && error.message ? error.message : error) };
  }
}

/**
 * Tabs the user has open, filtered to pages that can actually be read.
 *
 * Browser-internal pages (chrome://, the Web Store) reject injection, so
 * listing them would offer the agent tabs that always fail.
 */
async function listTabs() {
  const tabs = await chrome.tabs.query({});
  return tabs
    .filter((tab) => typeof tab.url === "string" && /^https?:/.test(tab.url))
    .map((tab) => ({
      tab_id: tab.id,
      title: tab.title || "",
      url: tab.url,
      active: Boolean(tab.active),
    }));
}

/** Inject the shared page script into a tab and call one of its functions. */
async function inPage(tabId, fn, args = []) {
  if (!Number.isInteger(tabId)) throw new Error("A tab id is required");

  await chrome.scripting.executeScript({
    target: { tabId },
    files: [PAGE_SCRIPT],
  });

  const [result] = await chrome.scripting.executeScript({
    target: { tabId },
    // Runs in the same isolated world as the injected file, so the namespace
    // it installed is visible here.
    func: (name, callArgs) => {
      const api = window.__codefactory_page;
      if (!api) throw new Error("The page script did not load");
      return api[name](...callArgs);
    },
    args: [fn, args],
  });

  if (!result || result.result === undefined) {
    throw new Error("The page returned nothing — it may have navigated away");
  }
  return result.result;
}

/**
 * Make sure the alarm that revives this worker exists.
 *
 * Creating it only on install was a single point of failure: the worker is
 * evicted when idle, and without an alarm nothing would ever wake it to notice
 * that CodeFactory had come back — leaving the user to click the extension to
 * "fix" a bridge that was only asleep.
 */
async function ensureKeepalive() {
  const existing = await chrome.alarms.get(KEEPALIVE_ALARM);
  if (!existing) chrome.alarms.create(KEEPALIVE_ALARM, { periodInMinutes: 1 });
}

chrome.runtime.onInstalled.addListener(() => {
  void ensureKeepalive();
  connect();
});
chrome.runtime.onStartup.addListener(() => {
  void ensureKeepalive();
  connect();
});
// The worker is evicted when idle; the alarm is what brings it back so the
// bridge reconnects without the user touching anything.
chrome.alarms.onAlarm.addListener((alarm) => {
  if (alarm.name === KEEPALIVE_ALARM) connect();
});
chrome.storage.onChanged.addListener((changes) => {
  if (changes.port || changes.token) connect();
});

// Every cold start of the worker, however it was triggered.
void ensureKeepalive();
connect();
