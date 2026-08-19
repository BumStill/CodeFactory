// SPDX-License-Identifier: Apache-2.0
//
// The extension has no bundler and cannot be exercised by vitest as a real
// Chrome extension, so these tests cover the parts that break silently:
// the manifest's permission surface, the build's single-source-of-truth
// guarantee for the shared page script, and the command dispatch in the
// service worker (run against a stubbed `chrome` API).
//
// What these tests deliberately do NOT claim: that the extension works in
// Chrome. That needs a human to load it unpacked and pair it.

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { resolve } from "node:path";
import { beforeAll, describe, expect, it, vi } from "vitest";

const root = process.cwd();
const read = (relative: string) => readFileSync(resolve(root, relative), "utf8");

describe("extension manifest", () => {
  const manifest = JSON.parse(read("extension/manifest.json"));

  it("asks only for what reading open tabs requires", () => {
    expect(manifest.manifest_version).toBe(3);
    expect(manifest.permissions).toEqual(
      expect.arrayContaining(["tabs", "scripting", "storage"]),
    );

    // Nothing that would let the extension act on the user's behalf beyond
    // reading: no cookie access, no downloads, no webRequest, no debugger.
    for (const forbidden of ["cookies", "downloads", "webRequest", "debugger", "history"]) {
      expect(manifest.permissions).not.toContain(forbidden);
    }
  });

  it("scopes host access to web pages, not browser internals", () => {
    expect(manifest.host_permissions).toEqual(["http://*/*", "https://*/*"]);
    // `<all_urls>` would also cover file:// and extension pages; the narrower
    // pair keeps the extension off local files, which the file tools own.
    expect(manifest.host_permissions).not.toContain("<all_urls>");
  });
});

describe("extension build", () => {
  beforeAll(() => {
    execFileSync("node", ["scripts/build-extension.mjs"], { cwd: root, stdio: "pipe" });
  });

  it("ships the same page script the desktop app injects", () => {
    // One source of truth: if these ever diverge, extraction would differ
    // between the two backends and only one of them would be tested.
    expect(existsSync(resolve(root, "extension/dist/content/page.js"))).toBe(true);
    expect(read("extension/dist/content/page.js")).toBe(
      read("src-tauri/src/browser/page.js"),
    );
  });

  it("refuses to build if the shared script stops being standalone", () => {
    const script = read("src-tauri/src/browser/page.js");
    // The guard in the build script only helps if this stays true; assert the
    // property directly so a violation fails here too.
    expect(/^\s*(import|export)\s/m.test(script)).toBe(false);
  });
});

describe("service worker command dispatch", () => {
  /** Load background.js against a stubbed chrome API and return the stubs. */
  async function loadWorker(
    tabs: Record<string, unknown>[],
    options: {
      /** Contents of `pairing.json`, as the app would have written it. */
      packaged?: unknown;
      /** Values a user typed on the options page. */
      stored?: Record<string, unknown>;
    } = {},
  ) {
    const storage: Record<string, unknown> = { ...(options.stored ?? {}) };
    const executeScript = vi.fn(async ({ func, args }: Record<string, unknown>) =>
      func ? [{ result: { called: args } }] : [{ result: undefined }],
    );
    const sockets: { url: string }[] = [];
    class SocketStub {
      static OPEN = 1;
      static CONNECTING = 0;
      readyState = 0;
      constructor(public url: string) {
        sockets.push({ url });
      }
      addEventListener() {}
      send() {}
      close() {}
    }
    // The init argument is part of the signature on purpose: the worker must ask
    // for `cache: "no-store"`, and a stub that dropped it could not assert that.
    const fetchStub = vi.fn(async (url: string, _init?: { cache?: string }) => {
      if (options.packaged === undefined) {
        // What a store install looks like: the file simply is not there.
        return { ok: false, url, json: async () => ({}) };
      }
      return { ok: true, url, json: async () => options.packaged };
    });
    const chromeStub = {
      storage: {
        local: {
          get: vi.fn(async (keys: string[]) =>
            Object.fromEntries(keys.map((key) => [key, storage[key]])),
          ),
          set: vi.fn(async (patch: Record<string, unknown>) => {
            Object.assign(storage, patch);
          }),
        },
        onChanged: { addListener: vi.fn() },
      },
      tabs: { query: vi.fn(async () => tabs) },
      scripting: { executeScript },
      alarms: {
        create: vi.fn(),
        get: vi.fn(async () => undefined),
        onAlarm: { addListener: vi.fn() },
      },
      runtime: {
        onInstalled: { addListener: vi.fn() },
        onStartup: { addListener: vi.fn() },
        getManifest: () => ({ version: "0.2.0" }),
        getURL: (path: string) => `chrome-extension://abcdefghijklmnopabcdefghijklmnop/${path}`,
      },
    };

    const source = read("extension/background.js");
    const module = new Function(
      "chrome",
      "WebSocket",
      "fetch",
      `${source}\nreturn { handle, listTabs, pairing, packagedPairing, addressesFor };`,
    );
    const api = module(chromeStub, SocketStub, fetchStub) as {
      handle: (request: Record<string, unknown>) => Promise<Record<string, unknown>>;
      listTabs: () => Promise<Record<string, unknown>[]>;
      pairing: () => Promise<Record<string, unknown> | null>;
      packagedPairing: () => Promise<Record<string, unknown> | null>;
      addressesFor: (paired: { port: number }) => number[];
    };
    return { api, storage, executeScript, sockets, fetchStub, chromeStub };
  }

  it("pairs from the file the app writes, with nothing typed in", async () => {
    // The manual step this removes: the user copying a port and a token out of
    // Settings, and doing it again after every restart.
    const { api, fetchStub } = await loadWorker([], {
      packaged: { port: 47615, token: "0123456789abcdef0123456789abcdef", protocol_version: 1 },
    });

    const paired = await api.pairing();

    expect(paired).toMatchObject({ port: 47615, token: "0123456789abcdef0123456789abcdef" });
    expect(paired?.source).toBe("packaged");
    // Read straight out of its own package — the one place an extension can read.
    expect(String(fetchStub.mock.calls[0][0])).toContain("pairing.json");
  });

  it("re-reads the pairing file instead of trusting a cached copy", async () => {
    // The port changes when CodeFactory restarts. A cached read would leave the
    // extension dialling a dead socket until a human intervened.
    const { api, fetchStub } = await loadWorker([], {
      packaged: { port: 47615, token: "0123456789abcdef0123456789abcdef" },
    });

    await api.pairing();
    await api.pairing();

    expect(fetchStub.mock.calls.length).toBeGreaterThanOrEqual(2);
    for (const call of fetchStub.mock.calls) {
      expect((call[1] as { cache?: string } | undefined)?.cache).toBe("no-store");
    }
  });

  it("falls back to values typed on the options page when there is no file", async () => {
    // A store install cannot be written into, so manual pairing has to keep
    // working — as a fallback, not as an override.
    const { api } = await loadWorker([], {
      stored: { port: 51789, token: "f".repeat(32) },
    });

    const paired = await api.pairing();

    expect(paired).toMatchObject({ port: 51789, source: "manual" });
  });

  it("prefers the app's live pairing over a stale one typed in earlier", async () => {
    const { api } = await loadWorker([], {
      packaged: { port: 47615, token: "0123456789abcdef0123456789abcdef" },
      stored: { port: 51789, token: "f".repeat(32) },
    });

    expect(await api.pairing()).toMatchObject({ port: 47615, source: "packaged" });
  });

  it("keeps the stable port as a second address so a stale pairing self-heals", async () => {
    // How a working pairing broke: a second CodeFactory took an ephemeral port,
    // stamped it in here, and exited. The extension then dialled a dead socket
    // forever while the app the user runs sat on the stable port. One recorded
    // port must not be the only address it will ever try.
    const { api } = await loadWorker([], {
      packaged: { port: 64530, token: "0123456789abcdef0123456789abcdef" },
    });

    expect(api.addressesFor({ port: 64530 })).toEqual([64530, 47615]);
    // Already on the stable port: no point dialling it twice.
    expect(api.addressesFor({ port: 47615 })).toEqual([47615]);
  });

  it("ignores a pairing file that is not usable rather than dialling a bad port", async () => {
    for (const packaged of [
      { port: 0, token: "0123456789abcdef0123456789abcdef" },
      { port: 70000, token: "0123456789abcdef0123456789abcdef" },
      { port: 47615, token: "short" },
      { port: "not-a-number", token: "0123456789abcdef0123456789abcdef" },
      {},
    ]) {
      const { api } = await loadWorker([], { packaged });
      expect(await api.packagedPairing()).toBeNull();
    }
  });

  it("lists only tabs that can actually be read", async () => {
    const { api } = await loadWorker([
      { id: 1, url: "https://example.com/a", title: "A", active: true },
      { id: 2, url: "chrome://settings", title: "Settings", active: false },
      { id: 3, url: "chrome-extension://abc/page.html", title: "Ext", active: false },
      { id: 4, url: "http://localhost:3000", title: "Dev", active: false },
    ]);

    const listed = await api.listTabs();

    // Browser-internal pages reject injection, so offering them would give the
    // agent tabs that always fail.
    expect(listed.map((tab) => tab.tab_id)).toEqual([1, 4]);
    expect(listed[0].active).toBe(true);
  });

  it("reports an unknown command instead of failing silently", async () => {
    const { api } = await loadWorker([]);
    const reply = await api.handle({ cmd: "delete_everything" });

    expect(reply.ok).toBe(false);
    expect(String(reply.error)).toContain("Unknown command");
  });

  it("turns a thrown page error into a reply rather than dropping the frame", async () => {
    // A dropped frame would leave the app waiting until its timeout; an error
    // reply tells it what happened.
    const { api } = await loadWorker([]);
    const reply = await api.handle({ cmd: "read", tab_id: "not-a-number" });

    expect(reply.ok).toBe(false);
    expect(String(reply.error)).toContain("tab id");
  });

  it("only implements read commands", async () => {
    const { api } = await loadWorker([]);
    // Acting on the browser the user actually uses is not part of this cut, so
    // click/fill must not be silently reachable.
    for (const cmd of ["click", "fill", "press", "navigate"]) {
      const reply = await api.handle({ cmd, tab_id: 1 });
      expect(reply.ok).toBe(false);
    }
  });
});
