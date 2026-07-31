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
  async function loadWorker(tabs: Record<string, unknown>[]) {
    const storage: Record<string, unknown> = {};
    const executeScript = vi.fn(async ({ func, args }: Record<string, unknown>) =>
      func ? [{ result: { called: args } }] : [{ result: undefined }],
    );
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
      alarms: { create: vi.fn(), onAlarm: { addListener: vi.fn() } },
      runtime: {
        onInstalled: { addListener: vi.fn() },
        onStartup: { addListener: vi.fn() },
        getManifest: () => ({ version: "0.1.0" }),
      },
    };

    // The worker calls connect() at load; without pairing it just sets a status.
    const source = read("extension/background.js");
    const module = new Function("chrome", "WebSocket", `${source}\nreturn { handle, listTabs };`);
    return {
      api: module(chromeStub, class {} as unknown) as {
        handle: (request: Record<string, unknown>) => Promise<Record<string, unknown>>;
        listTabs: () => Promise<Record<string, unknown>[]>;
      },
      storage,
      executeScript,
    };
  }

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
