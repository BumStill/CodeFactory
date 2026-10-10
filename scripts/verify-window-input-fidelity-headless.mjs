#!/usr/bin/env node
// SPDX-License-Identifier: Apache-2.0
// Real-browser gate for the M44 composer (CF-INP-R1 / CF-INP-R2).
//
// What went wrong: session b404d1b4, 2026-10-09 15:43 — the user typed
// `git apply --3way --index` and `--ci`, and the stored message held
// `—3way` / `—ci` instead. macOS WebKit substitutes "smart" dashes, quotes and
// capitalisation inside an editable element unless that element opts out, and
// the composer never opted out. The agent then ran the rewritten command.
//
// jsdom cannot see any of this: it has no text-substitution engine and no
// clipboard. So this gate bundles the production `MessageInput`
// (src/acceptance/composer-verbatim.tsx), serves it from a plain node http
// server, and drives it in real Chrome: real keystrokes, a real paste, a real
// IME composition, comparing every `onSend` payload byte-for-byte with what the
// user put in — plus light/dark screenshots.
//
// The static-bundle approach (esbuild + tailwind CLI + in-process http server)
// is the one `scripts/verify-turn-result-card-headless.mjs` uses: it keeps the
// gate independent of the vite dev-server transform pipeline, which stalls when
// the host exports HTTP(S)_PROXY.
//
// Run: node scripts/verify-window-input-fidelity-headless.mjs

import { spawnSync } from "node:child_process";
import { access, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { createServer } from "node:http";
import os from "node:os";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright-core";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const entry = path.join(root, "src", "acceptance", "composer-verbatim.tsx");
const cssEntry = path.join(root, "src", "styles", "globals.css");
const tailwindConfig = path.join(root, "tailwind.config.js");
const port = Number(process.env.CODEFACTORY_WINDOW_INPUT_FIDELITY_PORT ?? 1452);
const artifactDir =
  process.env.CODEFACTORY_WINDOW_INPUT_FIDELITY_ARTIFACT_DIR ??
  path.join(process.env.RUNNER_TEMP ?? os.tmpdir(), "codefactory-window-input-fidelity-headless");
const bundleDir = path.join(artifactDir, "site");

/** The cases from the spec's test matrix, plus the exact command that broke. */
const TYPED_CASES = [
  "--flag --ci",
  '"quoted" and \'single\'',
  "...",
  "git apply --3way --index",
  'git apply --3way --index "x" \'y\' ... 中文 English',
];

/** IME commit: what a Chinese input method hands over, dashes included. */
const IME_COMMITTED = "你好 --flag ...";

const failures = [];
const assert = (condition, message) => {
  if (condition) return;
  failures.push(message);
  console.error(`  FAIL ${message}`);
};

async function exists(candidate) {
  try {
    await access(candidate);
    return true;
  } catch {
    return false;
  }
}

/** esbuild is a transitive dependency; pnpm does not link it into .bin. */
async function findEsbuildBinary() {
  const direct = path.join(root, "node_modules", ".bin", "esbuild");
  if (await exists(direct)) return direct;
  const { readdir } = await import("node:fs/promises");
  const pnpmDir = path.join(root, "node_modules", ".pnpm");
  for (const entryName of await readdir(pnpmDir)) {
    if (!entryName.startsWith("@esbuild+")) continue;
    const platformDir = path.join(pnpmDir, entryName, "node_modules", "@esbuild");
    if (!(await exists(platformDir))) continue;
    for (const platform of await readdir(platformDir)) {
      const binary = path.join(platformDir, platform, "bin", "esbuild");
      if (await exists(binary)) return binary;
    }
  }
  throw new Error("no esbuild binary found under node_modules/.pnpm/@esbuild+*");
}

function run(command, args, label) {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8" });
  if (result.status !== 0) {
    throw new Error(`${label} failed (${result.status}):\n${result.stdout ?? ""}\n${result.stderr ?? ""}`);
  }
}

async function buildSite() {
  await mkdir(bundleDir, { recursive: true });
  run(
    await findEsbuildBinary(),
    [
      entry,
      "--bundle",
      "--format=esm",
      "--platform=browser",
      "--target=chrome120",
      "--jsx=automatic",
      "--loader:.css=empty",
      `--outfile=${path.join(bundleDir, "app.js")}`,
      "--log-level=warning",
    ],
    "esbuild bundle",
  );
  run(
    process.execPath,
    [
      path.join(root, "node_modules", "tailwindcss", "lib", "cli.js"),
      "-c",
      tailwindConfig,
      "-i",
      cssEntry,
      "-o",
      path.join(bundleDir, "styles.css"),
    ],
    "tailwind build",
  );
  await writeFile(
    path.join(bundleDir, "index.html"),
    `<!doctype html>
<html lang="zh-CN" data-theme="light">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <link rel="stylesheet" href="./styles.css" />
    <title>CodeFactory Composer Verbatim Acceptance</title>
  </head>
  <body><div id="root"></div><script type="module" src="./app.js"></script></body>
</html>
`,
    "utf8",
  );
}

const CONTENT_TYPES = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
};

function serveSite() {
  const server = createServer(async (request, response) => {
    const url = new URL(request.url ?? "/", "http://127.0.0.1");
    const relative = url.pathname === "/" ? "/index.html" : url.pathname;
    const file = path.join(bundleDir, path.normalize(relative).replace(/^(\.\.[/\\])+/, ""));
    try {
      const body = await readFile(file);
      response.writeHead(200, { "content-type": CONTENT_TYPES[path.extname(file)] ?? "application/octet-stream" });
      response.end(body);
    } catch {
      response.writeHead(404);
      response.end("not found");
    }
  });
  return new Promise((resolve) => {
    server.listen(port, "127.0.0.1", () => resolve(server));
  });
}

async function firstBrowser() {
  const candidates =
    process.platform === "darwin"
      ? [
          "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
          "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
          "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
      : process.platform === "win32"
        ? [
            path.join(process.env["PROGRAMFILES(X86)"] ?? "C:\\Program Files (x86)", "Microsoft/Edge/Application/msedge.exe"),
            path.join(process.env.PROGRAMFILES ?? "C:\\Program Files", "Google/Chrome/Application/chrome.exe"),
          ]
        : ["/usr/bin/google-chrome", "/usr/bin/chromium", "/usr/bin/chromium-browser"];
  for (const candidate of candidates) {
    if (await exists(candidate)) return candidate;
  }
  throw new Error(`No system Chrome/Edge found. Tried: ${candidates.join(", ")}`);
}

const sentLog = (page) => page.evaluate(() => window.__composerSent ?? []);

async function sendWithEnter(page, composer, expectedCount) {
  await composer.press("Enter");
  await page.waitForFunction((count) => (window.__composerSent ?? []).length === count, expectedCount, { timeout: 5000 });
}

async function main() {
  await rm(artifactDir, { recursive: true, force: true });
  await buildSite();
  const server = await serveSite();
  const baseUrl = `http://127.0.0.1:${port}/`;
  const pageLog = [];
  let browser;
  try {
    browser = await chromium.launch({
      executablePath: await firstBrowser(),
      headless: true,
      // The host may export HTTP(S)_PROXY for outbound traffic; keep the
      // loopback acceptance server off it.
      proxy: { server: "direct://" },
      args: ["--disable-gpu", "--no-sandbox", "--no-proxy-server", "--proxy-bypass-list=*"],
    });
    const context = await browser.newContext({
      colorScheme: "light",
      viewport: { width: 1100, height: 760 },
      permissions: ["clipboard-read", "clipboard-write"],
    });
    const page = await context.newPage();
    page.on("console", (message) => pageLog.push(`[page:${message.type()}] ${message.text()}\n`));
    page.on("pageerror", (error) => pageLog.push(`[pageerror] ${error.stack ?? error}\n`));
    page.on("response", (response) => {
      if (response.status() >= 400) pageLog.push(`[http ${response.status()}] ${response.url()}\n`);
    });
    await page.goto(baseUrl, { waitUntil: "domcontentloaded", timeout: 60_000 });
    const composer = page.getByRole("textbox");
    await composer.waitFor({ timeout: 20_000 });

    // The screenshots are only evidence if the product stylesheet loaded, so
    // ask the real engine to resolve a Tailwind utility it ships.
    const flexDisplay = await page.evaluate(() => {
      const probe = document.createElement("div");
      probe.className = "flex items-center";
      document.body.appendChild(probe);
      const display = getComputedStyle(probe).display;
      probe.remove();
      return display;
    });
    assert(flexDisplay === "flex", `product stylesheet did not load (flex resolved to ${flexDisplay})`);

    // ── CF-INP-R1: the element itself opts out of platform substitution ──
    const attributes = await composer.evaluate((el) => ({
      autocorrect: el.getAttribute("autocorrect"),
      autocapitalize: el.getAttribute("autocapitalize"),
      spellcheck: el.getAttribute("spellcheck"),
    }));
    console.log(`  composer attributes: ${JSON.stringify(attributes)}`);
    assert(attributes.autocorrect === "off", `composer autocorrect=${attributes.autocorrect}, expected off`);
    assert(attributes.autocapitalize === "off", `composer autocapitalize=${attributes.autocapitalize}, expected off`);
    assert(attributes.spellcheck === "false", `composer spellcheck=${attributes.spellcheck}, expected false`);

    // ── CF-INP-R1: real keystrokes → real onSend payload, byte-for-byte ──
    const typed = [];
    for (const [index, text] of TYPED_CASES.entries()) {
      await composer.fill("");
      await composer.pressSequentially(text, { delay: 5 });
      const inDom = await composer.inputValue();
      assert(inDom === text, `typed text rewritten in the DOM: ${JSON.stringify(text)} => ${JSON.stringify(inDom)}`);
      await sendWithEnter(page, composer, index + 1);
      const sent = await sentLog(page);
      assert(sent[index] === text, `onSend changed the typed text: ${JSON.stringify(text)} => ${JSON.stringify(sent[index])}`);
      typed.push({ text, domMatches: inDom === text, sentMatches: sent[index] === text });
    }
    const brokeInProduction = "git apply --3way --index";
    assert(
      typed.some((entryResult) => entryResult.text === brokeInProduction && entryResult.sentMatches),
      "the exact command from session b404d1b4 did not survive the composer",
    );

    // ── CF-INP-R2: paste and typing produce the same stored text ──
    const pasted = 'git apply --3way --index "x" ... 中文';
    await composer.fill("");
    await composer.click();
    await page.evaluate(async (text) => navigator.clipboard.writeText(text), pasted);
    await page.keyboard.press(process.platform === "darwin" ? "Meta+V" : "Control+V");
    await page.waitForTimeout(200);
    let pastePath = "system-clipboard";
    if ((await composer.inputValue()) !== pasted) {
      // Headless Chrome does not always hand the system clipboard to the page.
      // CDP insertText goes through the browser's own editing pipeline — the
      // same one a plain-text paste uses — so fall back to it and record which
      // path was used rather than claiming a paste we did not perform.
      pastePath = "cdp-insertText";
      await composer.fill("");
      await composer.click();
      await page.keyboard.insertText(pasted);
    }
    const pastedInDom = await composer.inputValue();
    assert(pastedInDom === pasted, `pasted text rewritten: ${JSON.stringify(pastedInDom)}`);
    const beforePaste = (await sentLog(page)).length;
    await sendWithEnter(page, composer, beforePaste + 1);
    const afterPaste = await sentLog(page);
    assert(afterPaste[beforePaste] === pasted, `pasted message differs from typed: ${JSON.stringify(afterPaste[beforePaste])}`);
    assert(afterPaste[beforePaste] === pastedInDom, "the sent message must be exactly what the box showed after the paste");

    // ── CF-INP-R2: IME composition is unaffected, and Enter still commits ──
    await composer.fill("");
    await composer.click();
    await composer.evaluate((el) => el.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true, data: "" })));
    await page.keyboard.insertText(IME_COMMITTED);
    await composer.evaluate(
      (el, committed) => el.dispatchEvent(new CompositionEvent("compositionend", { bubbles: true, data: committed })),
      IME_COMMITTED,
    );
    const composedInDom = await composer.inputValue();
    assert(composedInDom === IME_COMMITTED, `IME-composed text rewritten: ${JSON.stringify(composedInDom)}`);

    const beforeComposition = (await sentLog(page)).length;
    await composer.press("Enter"); // the Enter that commits the candidate list
    await page.waitForTimeout(150);
    assert(
      (await sentLog(page)).length === beforeComposition,
      "the Enter that commits an IME candidate must not send the message",
    );
    await sendWithEnter(page, composer, beforeComposition + 1);
    const afterComposition = await sentLog(page);
    assert(
      afterComposition[beforeComposition] === IME_COMMITTED,
      `IME-composed message differs after the commit Enter: ${JSON.stringify(afterComposition[beforeComposition])}`,
    );

    // ── Screenshots, both modes, with the real command visible in the box ──
    await composer.fill("");
    await composer.pressSequentially('git apply --3way --index && echo "ok"', { delay: 5 });
    const shots = {};
    for (const mode of ["light", "dark"]) {
      await page.emulateMedia({ colorScheme: mode });
      await page.evaluate((theme) => {
        document.documentElement.dataset.theme = theme;
      }, mode);
      const file = path.join(artifactDir, `composer-verbatim-${mode}.png`);
      await page.screenshot({ path: file });
      shots[mode] = file;
    }

    if (failures.length > 0) throw new Error(`${failures.length} assertion(s) failed:\n- ${failures.join("\n- ")}`);
    console.log(
      JSON.stringify(
        {
          status: "pass",
          typedCases: typed,
          pastePath,
          ime: { committed: IME_COMMITTED, sent: afterComposition[beforeComposition] },
          composerAttributes: attributes,
          screenshots: shots,
          artifactDir,
        },
        null,
        2,
      ),
    );
  } finally {
    if (browser) await browser.close();
    await new Promise((resolve) => server.close(resolve));
    await writeFile(path.join(artifactDir, "page.log"), pageLog.join(""));
    if (pageLog.length > 0) console.error(pageLog.join("").trim().split("\n").slice(-10).join("\n"));
  }
}

main().catch((error) => {
  console.error(`composer verbatim headless acceptance failed: ${error.stack ?? error}`);
  process.exit(1);
});
