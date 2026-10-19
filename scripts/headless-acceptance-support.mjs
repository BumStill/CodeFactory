// SPDX-License-Identifier: Apache-2.0
import net from "node:net";

function canListen(port) {
  return new Promise((resolve, reject) => {
    const server = net.createServer();
    server.unref();
    server.once("error", reject);
    server.listen(port, "127.0.0.1", () => {
      const selected = server.address().port;
      server.close(() => resolve(selected));
    });
  });
}

export async function allocateLoopbackPort(preferredPort) {
  try {
    return await canListen(preferredPort);
  } catch (error) {
    if (error?.code !== "EADDRINUSE") throw error;
    return canListen(0);
  }
}

export async function waitForAcceptanceDocument(child, url, expectedDocumentMarker, options = {}) {
  const timeoutMs = options.timeoutMs ?? 30_000;
  const pollMs = options.pollMs ?? 250;
  const expectedPath = new URL(url).pathname;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (child.exitCode != null || child.signalCode != null) throw new Error("Vite exited early");
    try {
      const response = await fetch(url, { cache: "no-store" });
      if (response.ok && new URL(response.url).pathname === expectedPath) {
        const document = await response.text();
        if (document.includes(expectedDocumentMarker)) return;
      }
    } catch {
      // The selected port is reserved before Vite starts; wait for the fixture.
    }
    await new Promise((resolve) => setTimeout(resolve, pollMs));
  }
  throw new Error(`Timed out waiting for expected acceptance document at ${url}`);
}

/**
 * Boot budget for the first render of an acceptance page.
 *
 * A `windows-latest` runner pays for a cold Vite transform of the whole app
 * while Defender reads the freshly unpacked files; the first navigation of a
 * job has been measured well past ten seconds even though every later
 * navigation in the same job is sub-second. Ten seconds was therefore an
 * assertion budget applied to process warm-up — it is not evidence that the
 * product took longer to render. This budget covers *only* document load and
 * the app shell mount; every product assertion after boot keeps
 * {@link ASSERTION_BUDGET_MS}. Override per run with
 * `CF_HEADLESS_BOOT_BUDGET_MS` and read the measured boot time from the
 * `[acceptance] boot ...ms` line each script prints.
 */
export const APP_BOOT_BUDGET_MS = Number(process.env.CF_HEADLESS_BOOT_BUDGET_MS ?? 60_000);

/** Product assertion budget. Deliberately unchanged: a rendered app that needs
 * more than this for a click or a state change is a real regression. */
export const ASSERTION_BUDGET_MS = 10_000;

const OBSERVED = new WeakSet();
const DIAGNOSTICS = new WeakMap();

/**
 * Attach console/page-error/failed-request capture once per page.
 *
 * The point is that the *next* flake answers its own question. A bare
 * `locator.waitFor: Timeout 10000ms exceeded` costs a whole rerun-and-guess
 * cycle; the same timeout with the DOM outline, the console and the last
 * failed request names the cause on the first line of the log.
 */
export function observePage(page) {
  if (OBSERVED.has(page)) return DIAGNOSTICS.get(page);
  OBSERVED.add(page);
  const diagnostics = { console: [], pageErrors: [], failedRequests: [] };
  DIAGNOSTICS.set(page, diagnostics);
  page.on?.("console", (message) => {
    const type = message.type?.() ?? "log";
    if (type === "error" || type === "warning") {
      diagnostics.console.push(`[${type}] ${message.text?.() ?? ""}`);
      if (diagnostics.console.length > 20) diagnostics.console.shift();
    }
  });
  page.on?.("pageerror", (error) => {
    diagnostics.pageErrors.push(String(error?.message ?? error));
    if (diagnostics.pageErrors.length > 10) diagnostics.pageErrors.shift();
  });
  page.on?.("requestfailed", (request) => {
    const failure = request.failure?.();
    diagnostics.failedRequests.push(`${request.url?.() ?? "?"} ${failure?.errorText ?? ""}`.trim());
    if (diagnostics.failedRequests.length > 10) diagnostics.failedRequests.shift();
  });
  return diagnostics;
}

function diagnosticsSection(page) {
  const diagnostics = DIAGNOSTICS.get(page);
  const lines = [];
  if (diagnostics) {
    if (diagnostics.pageErrors.length) lines.push(`page errors: ${diagnostics.pageErrors.join(" | ")}`);
    if (diagnostics.failedRequests.length) lines.push(`failed requests: ${diagnostics.failedRequests.join(" | ")}`);
    if (diagnostics.console.length) lines.push(`console: ${diagnostics.console.join(" | ")}`);
  }
  return lines;
}

/**
 * Describe what the page actually contains, for a timeout message.
 *
 * Read-only and bounded: one `evaluate` with a fixed slice, wrapped so a
 * closed or crashed page still yields a message instead of masking the
 * original failure.
 */
export async function describePageState(page, label) {
  let url = "unavailable";
  let title = "";
  let dom = "";
  try {
    url = page.url?.() ?? url;
    const snapshot = await page.evaluate(() => ({
      title: document.title,
      body: document.body ? document.body.innerText.slice(0, 1200) : "",
      outline: Array.from(document.querySelectorAll("main,[role=main],[aria-label]"))
        .slice(0, 12)
        .map((element) => `${element.tagName.toLowerCase()}[aria-label=${JSON.stringify(element.getAttribute("aria-label"))}]`)
        .join(" "),
    }));
    title = snapshot.title;
    dom = `outline: ${snapshot.outline}\nbody: ${snapshot.body.replace(/\s+/g, " ").slice(0, 600)}`;
  } catch (error) {
    dom = `page state unavailable: ${error?.message ?? error}`;
  }
  return [`${label} at ${url} (title=${JSON.stringify(title)})`, dom, ...diagnosticsSection(page)]
    .filter(Boolean)
    .join("\n");
}

/**
 * Wait for the app shell to mount — the boot phase of a navigation.
 *
 * This is the only wait that may legitimately be slow on a cold Windows
 * runner. It is separate from {@link ASSERTION_BUDGET_MS} on purpose, and it
 * reports boot duration so the budget stays evidence-based rather than a
 * number somebody raised until the flake stopped.
 */
export async function waitForAppBoot(page, locator, options = {}) {
  observePage(page);
  const budget = options.timeoutMs ?? APP_BOOT_BUDGET_MS;
  const state = options.state ?? "attached";
  const label = options.label ?? "app shell did not mount";
  const started = Date.now();
  try {
    await locator.waitFor({ state, timeout: budget });
  } catch (error) {
    const description = await describePageState(page, label);
    const failure = new Error(`${label} within ${budget}ms\n${description}`);
    failure.cause = error;
    throw failure;
  }
  const elapsed = Date.now() - started;
  console.log(`[acceptance] boot ${elapsed}ms (budget ${budget}ms): ${label}`);
  return elapsed;
}

/**
 * Navigate to an acceptance fixture and wait for the app shell.
 *
 * `domcontentloaded` (not `networkidle`) because a dev server keeps its HMR
 * socket open; waiting for network idle conflates that socket with the app
 * being ready. Boot is proven by the landmark the script actually asserts on,
 * with {@link APP_BOOT_BUDGET_MS}; assertions after this call keep the strict
 * budget.
 */
export async function gotoScenario(page, url, options = {}) {
  observePage(page);
  const budget = options.timeoutMs ?? APP_BOOT_BUDGET_MS;
  await page.goto(url, { waitUntil: "domcontentloaded", timeout: budget });
  if (options.waitForLoad !== false) {
    await page.waitForLoadState("load", { timeout: budget });
  }
  if (options.landmark) {
    await waitForAppBoot(page, options.landmark, {
      timeoutMs: budget,
      state: options.state ?? "attached",
      label: options.label ?? `app shell for ${url}`,
    });
  }
}

export async function waitForFocused(locator, timeout = ASSERTION_BUDGET_MS) {
  await locator.waitFor({ state: "visible", timeout });
  await locator.evaluate((element, timeoutMs) => new Promise((resolve, reject) => {
    if (element === document.activeElement) return resolve();
    const deadline = performance.now() + timeoutMs;
    const check = () => {
      if (element === document.activeElement) resolve();
      else if (performance.now() >= deadline) reject(new Error("element did not receive focus"));
      else requestAnimationFrame(check);
    };
    requestAnimationFrame(check);
  }), timeout);
}

export async function waitForStableAnimationFrames(page, frames = 2) {
  await page.evaluate((frameCount) => new Promise((resolve) => {
    let remaining = frameCount;
    const next = () => {
      remaining -= 1;
      if (remaining <= 0) resolve();
      else requestAnimationFrame(next);
    };
    requestAnimationFrame(next);
  }), frames);
}
