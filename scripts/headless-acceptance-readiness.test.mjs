// SPDX-License-Identifier: Apache-2.0
import test from "node:test";
import assert from "node:assert/strict";
import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  APP_BOOT_BUDGET_MS,
  ASSERTION_BUDGET_MS,
  gotoScenario,
  observePage,
  waitForAppBoot,
} from "./headless-acceptance-support.mjs";

function fakePage({ body = "acceptance body", failEvaluate = false } = {}) {
  const listeners = new Map();
  const calls = { goto: [], loadState: [], evaluate: [] };
  const page = {
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
      // The helper's snapshot thunk reads document.*; serve a stand-in.
      const previous = globalThis.document;
      globalThis.document = {
        title: "Acceptance",
        body: { innerText: body },
        querySelectorAll: () => [
          { tagName: "MAIN", getAttribute: () => "Fixture acceptance" },
        ],
      };
      try {
        return fn();
      } finally {
        globalThis.document = previous;
      }
    },
  };
  return page;
}

function fakeLocator({ delayMs = 0, throwError = null } = {}) {
  const calls = [];
  return {
    calls,
    waitFor: async (options) => {
      calls.push(options);
      if (delayMs) await new Promise((resolve) => setTimeout(resolve, delayMs));
      if (throwError) throw throwError;
    },
  };
}

test("boot wait uses the cold-start budget, not the assertion budget", async () => {
  const page = fakePage();
  const locator = fakeLocator();
  const elapsed = await waitForAppBoot(page, locator, { label: "fixture shell" });
  assert.equal(locator.calls.length, 1);
  assert.equal(locator.calls[0].timeout, APP_BOOT_BUDGET_MS);
  assert.equal(locator.calls[0].state, "attached");
  assert.ok(APP_BOOT_BUDGET_MS > ASSERTION_BUDGET_MS, "boot must outlast an assertion budget");
  assert.equal(typeof elapsed, "number");
});

test("boot wait honours an explicit budget instead of the ambient default", async () => {
  const page = fakePage();
  const locator = fakeLocator();
  await waitForAppBoot(page, locator, { timeoutMs: 1_234 });
  assert.equal(locator.calls[0].timeout, 1_234);
});

test("a boot timeout reports the DOM and the page console, not just the budget", async () => {
  const page = fakePage({ body: "加载中" });
  // Listeners must exist before the navigation starts; observePage is what
  // gotoScenario does at the top of every script.
  observePage(page);
  page.emit("console", { type: () => "error", text: () => "Uncaught ReferenceError: boot is not defined" });
  page.emit("pageerror", { message: "boot is not defined" });
  page.emit("requestfailed", { url: () => "http://127.0.0.1:4517/src/main.tsx", failure: () => ({ errorText: "net::ERR_ABORTED" }) });
  const locator = fakeLocator({ throwError: new Error("Timeout 10000ms exceeded") });
  await assert.rejects(
    () => waitForAppBoot(page, locator, { label: "fixture shell" }),
    (error) => {
      assert.match(error.message, /fixture shell within 60000ms/);
      assert.match(error.message, /boot is not defined/);
      assert.match(error.message, /加载中/);
      assert.match(error.message, /main\.tsx/);
      return true;
    },
  );
});

test("a boot timeout still reports when the page cannot be evaluated", async () => {
  const page = fakePage({ failEvaluate: true });
  const locator = fakeLocator({ throwError: new Error("Timeout exceeded") });
  await assert.rejects(
    () => waitForAppBoot(page, locator, { label: "fixture shell" }),
    (error) => {
      assert.match(error.message, /page state unavailable: page crashed/);
      return true;
    },
  );
});

test("gotoScenario navigates with the boot budget and proves boot on the landmark", async () => {
  const page = fakePage();
  const landmark = fakeLocator();
  await gotoScenario(page, "http://127.0.0.1:4517/?scenario=fixture", {
    landmark,
    label: "fixture shell",
  });
  assert.equal(page.calls.goto.length, 1);
  assert.deepEqual(page.calls.goto[0], {
    url: "http://127.0.0.1:4517/?scenario=fixture",
    options: { waitUntil: "domcontentloaded", timeout: APP_BOOT_BUDGET_MS },
  });
  assert.deepEqual(page.calls.loadState[0], { state: "load", options: { timeout: APP_BOOT_BUDGET_MS } });
  assert.equal(landmark.calls[0].timeout, APP_BOOT_BUDGET_MS);
});

test("observePage attaches its listeners once per page", async () => {
  const page = fakePage();
  observePage(page);
  observePage(page);
  page.emit("pageerror", { message: "first" });
  const diagnostics = observePage(page);
  assert.deepEqual(diagnostics.pageErrors, ["first"]);
});

/**
 * The family rule this test enforces: a page navigation is a process boot,
 * and boot waits go through `gotoScenario`/`waitForAppBoot`. The old shape —
 * `page.goto(...)` and then a locator waited on with the 10s assertion
 * budget — is exactly what timed out on cold Windows runners, so it must not
 * come back in a new acceptance script.
 */
test("no acceptance script gates a fresh navigation on the 10s assertion budget", async () => {
  const directory = path.dirname(fileURLToPath(import.meta.url));
  const files = (await readdir(directory))
    .filter((name) => /^verify-.*-headless\.mjs$/.test(name))
    .sort();
  assert.ok(files.length > 20, `expected the acceptance family, found ${files.length} scripts`);
  const offenders = [];
  for (const file of files) {
    const lines = (await readFile(path.join(directory, file), "utf8")).split("\n");
    lines.forEach((line, index) => {
      if (!/page\.(goto|reload)\(/.test(line)) return;
      const window = lines.slice(index + 1, index + 9);
      // Only the first wait after a navigation is a boot wait; later waits in
      // the same window are product assertions and keep the strict budget.
      const firstWait = window.findIndex((candidate) => /\.waitFor\(/.test(candidate));
      const bootHelper = window.findIndex((candidate) => /waitForAppBoot\(|gotoScenario\(/.test(candidate));
      const routedThroughHelper = bootHelper >= 0 && (firstWait < 0 || bootHelper <= firstWait);
      if (firstWait >= 0 && !routedThroughHelper && /waitFor\([^)]*timeout:\s*10_000/.test(window[firstWait])) {
        offenders.push(`${file}:${index + 2 + firstWait} fails closed on the 10s assertion budget during boot`);
      }
    });
  }
  assert.deepEqual(offenders, [], `route boot waits through waitForAppBoot/gotoScenario:\n${offenders.join("\n")}`);
});
