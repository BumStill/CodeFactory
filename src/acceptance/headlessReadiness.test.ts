// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

// The acceptance helpers are plain ESM scripts, not TS; vitest resolves them
// from the repo root and the shape we use is asserted below.
// @ts-ignore - no type declarations for the acceptance support module
const support = await import("../../scripts/headless-acceptance-support.mjs");
const { APP_BOOT_BUDGET_MS, ASSERTION_BUDGET_MS, gotoScenario, observePage, waitForAppBoot } =
  support as {
    APP_BOOT_BUDGET_MS: number;
    ASSERTION_BUDGET_MS: number;
    gotoScenario: (page: unknown, url: string, options?: Record<string, unknown>) => Promise<void>;
    observePage: (page: unknown) => { pageErrors: string[] };
    waitForAppBoot: (
      page: unknown,
      locator: unknown,
      options?: Record<string, unknown>,
    ) => Promise<number>;
  };

type FakePage = {
  calls: { goto: Array<{ url: string; options: Record<string, unknown> }>; loadState: Array<Record<string, unknown>>; evaluate: unknown[] };
  url: () => string;
  on: (event: string, handler: (payload: unknown) => void) => void;
  emit: (event: string, payload: unknown) => void;
  goto: (url: string, options: Record<string, unknown>) => Promise<void>;
  waitForLoadState: (state: string, options: Record<string, unknown>) => Promise<void>;
  evaluate: (fn: () => unknown) => Promise<unknown>;
};

function fakePage({ body = "acceptance body", failEvaluate = false } = {}): FakePage {
  const listeners = new Map<string, Array<(payload: unknown) => void>>();
  const calls: FakePage["calls"] = { goto: [], loadState: [], evaluate: [] };
  return {
    calls,
    url: () => "http://127.0.0.1:4517/?scenario=fixture",
    on: (event, handler) => {
      listeners.set(event, [...(listeners.get(event) ?? []), handler]);
    },
    emit: (event, payload) => {
      for (const handler of listeners.get(event) ?? []) handler(payload);
    },
    goto: async (url, options) => {
      calls.goto.push({ url, options });
    },
    waitForLoadState: async (state, options) => {
      calls.loadState.push({ state, options });
    },
    evaluate: async (fn) => {
      calls.evaluate.push(fn);
      if (failEvaluate) throw new Error("page crashed");
      // describePageState reads document.*; serve a stand-in snapshot.
      const previous = (globalThis as { document?: unknown }).document;
      (globalThis as { document?: unknown }).document = {
        title: "Acceptance",
        body: { innerText: body },
        querySelectorAll: () => [{ tagName: "MAIN", getAttribute: () => "Fixture acceptance" }],
      };
      try {
        return fn();
      } finally {
        (globalThis as { document?: unknown }).document = previous;
      }
    },
  };
}

function fakeLocator({ throwError = null }: { throwError?: Error | null } = {}) {
  const calls: Array<Record<string, unknown>> = [];
  return {
    calls,
    waitFor: async (options: Record<string, unknown>) => {
      calls.push(options);
      if (throwError) throw throwError;
    },
  };
}

describe("acceptance boot readiness", () => {
  it("uses the cold-start budget rather than the assertion budget for boot", async () => {
    const page = fakePage();
    const locator = fakeLocator();
    const elapsed = await waitForAppBoot(page, locator, { label: "fixture shell" });
    expect(locator.calls).toHaveLength(1);
    expect(locator.calls[0].timeout).toBe(APP_BOOT_BUDGET_MS);
    expect(locator.calls[0].state).toBe("attached");
    expect(APP_BOOT_BUDGET_MS).toBeGreaterThan(ASSERTION_BUDGET_MS);
    expect(typeof elapsed).toBe("number");
  });

  it("honours an explicit budget instead of the ambient default", async () => {
    const page = fakePage();
    const locator = fakeLocator();
    await waitForAppBoot(page, locator, { timeoutMs: 1_234 });
    expect(locator.calls[0].timeout).toBe(1_234);
  });

  it("reports the DOM and the page console when boot times out", async () => {
    const page = fakePage({ body: "加载中" });
    // Listeners must exist before the navigation, which is what gotoScenario
    // does at the top of every script.
    observePage(page);
    page.emit("console", {
      type: () => "error",
      text: () => "Uncaught ReferenceError: boot is not defined",
    });
    page.emit("pageerror", { message: "boot is not defined" });
    page.emit("requestfailed", {
      url: () => "http://127.0.0.1:4517/src/main.tsx",
      failure: () => ({ errorText: "net::ERR_ABORTED" }),
    });
    const locator = fakeLocator({ throwError: new Error("Timeout 10000ms exceeded") });
    const failure = await waitForAppBoot(page, locator, { label: "fixture shell" }).catch(
      (error: Error) => error.message,
    );
    expect(failure).toMatch(/fixture shell within 60000ms/);
    expect(failure).toMatch(/加载中/);
    expect(failure).toMatch(/boot is not defined/);
    expect(failure).toMatch(/main\.tsx/);
  });

  it("still reports when the page cannot be evaluated", async () => {
    const page = fakePage({ failEvaluate: true });
    const locator = fakeLocator({ throwError: new Error("Timeout exceeded") });
    await expect(waitForAppBoot(page, locator, { label: "fixture shell" })).rejects.toThrow(
      /page state unavailable: page crashed/,
    );
  });

  it("navigates with the boot budget and proves boot on the landmark", async () => {
    const page = fakePage();
    const landmark = fakeLocator();
    await gotoScenario(page, "http://127.0.0.1:4517/?scenario=fixture", {
      landmark,
      label: "fixture shell",
    });
    expect(page.calls.goto).toEqual([
      {
        url: "http://127.0.0.1:4517/?scenario=fixture",
        options: { waitUntil: "domcontentloaded", timeout: APP_BOOT_BUDGET_MS },
      },
    ]);
    expect(page.calls.loadState).toEqual([
      { state: "load", options: { timeout: APP_BOOT_BUDGET_MS } },
    ]);
    expect(landmark.calls[0].timeout).toBe(APP_BOOT_BUDGET_MS);
  });

  it("attaches its listeners once per page", () => {
    const page = fakePage();
    observePage(page);
    observePage(page);
    page.emit("pageerror", { message: "first" });
    expect(observePage(page).pageErrors).toEqual(["first"]);
  });
});

/**
 * The family rule this guard enforces: a page navigation is a process boot,
 * and boot waits go through `gotoScenario` / `waitForAppBoot`. The old shape —
 * `page.goto(...)` followed by a locator waited on with the 10s assertion
 * budget — is exactly what timed out on cold Windows runners, so it must not
 * come back in a new acceptance script.
 */
describe("acceptance script family rule", () => {
  it("never gates a fresh navigation on the 10s assertion budget", async () => {
    const directory = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../../scripts");
    const files = (await readdir(directory))
      .filter((name) => /^verify-.*-headless\.mjs$/.test(name))
      .sort();
    expect(files.length).toBeGreaterThan(20);
    const offenders: string[] = [];
    for (const file of files) {
      const lines = (await readFile(path.join(directory, file), "utf8")).split("\n");
      lines.forEach((line, index) => {
        if (!/page\.(goto|reload)\(/.test(line)) return;
        const window = lines.slice(index + 1, index + 9);
        // Only the first wait after a navigation is a boot wait; later waits
        // are product assertions and keep the strict budget.
        const firstWait = window.findIndex((candidate) => /\.waitFor\(/.test(candidate));
        const bootHelper = window.findIndex((candidate) =>
          /waitForAppBoot\(|gotoScenario\(/.test(candidate),
        );
        const routedThroughHelper = bootHelper >= 0 && (firstWait < 0 || bootHelper <= firstWait);
        if (
          firstWait >= 0 &&
          !routedThroughHelper &&
          /waitFor\([^)]*timeout:\s*10_000/.test(window[firstWait])
        ) {
          offenders.push(
            `${file}:${index + 2 + firstWait} fails closed on the 10s assertion budget during boot`,
          );
        }
      });
    }
    expect(offenders).toEqual([]);
  });
});
