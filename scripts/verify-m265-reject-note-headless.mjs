// SPDX-License-Identifier: Apache-2.0
// M265 Viewport Harness: the reject-with-note input must be visible, typeable and
// inside the viewport in both themes at desktop and phone widths, and the typed
// sentence must reach the deny handler (not a separate message path).
import { chromium } from "playwright-core";
import { spawn } from "node:child_process";
import { mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import assert from "node:assert/strict";
import { allocateLoopbackPort, waitForAcceptanceDocument } from "./headless-acceptance-support.mjs";

// Sandboxed runs cannot write /tmp, so artifacts live in the gitignored build tree.
const artifacts = process.env.M265_ARTIFACTS ?? path.join(process.cwd(), "target", "m265-reject-note");
await mkdir(artifacts, { recursive: true });
const port = await allocateLoopbackPort(1478);
const url = `http://127.0.0.1:${port}/m265-reject-note-acceptance.html`;
const server = spawn(
  process.execPath,
  ["node_modules/vite/bin/vite.js", "--host", "127.0.0.1", "--port", String(port), "--strictPort"],
  { stdio: ["ignore", "pipe", "pipe"] },
);
let log = "";
server.stdout.on("data", (d) => (log += d));
server.stderr.on("data", (d) => (log += d));
console.log(JSON.stringify({ service_pid: server.pid, log: path.join(artifacts, "vite.log") }));

let browser;
try {
  await waitForAcceptanceDocument(server, url, "m265-reject-note.tsx");
  browser = await chromium.launch({
    executablePath:
      process.env.CHROME_PATH ?? "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    headless: true,
  });
  const page = await browser.newPage();
  const note = "先别动 stash，我需要保留这些改动";

  for (const theme of ["light", "dark"]) {
    for (const width of [1366, 390]) {
      await page.setViewportSize({ width, height: 800 });
      await page.goto(url);
      await page.evaluate((t) => (document.documentElement.dataset.theme = t), theme);
      const dialog = page.getByRole("dialog");
      await dialog.waitFor();

      const input = page.getByTestId("permission-note");
      await input.waitFor();
      const box = await input.boundingBox();
      assert(box && box.width > 0, `note input has no width at ${theme}/${width}`);
      assert(box.x >= 0 && box.x + box.width <= width, `note input overflows width at ${theme}/${width}`);
  await page.screenshot({ path: path.join(artifacts, `card-${theme}-${width}.png`) });

      await input.fill(note);
      await page.screenshot({ path: path.join(artifacts, `note-typed-${theme}-${width}.png`) });
      await page.getByRole("button", { name: "拒绝", exact: true }).click();
      await page
        .getByTestId("outcome")
        .filter({ hasText: "denied_with_note" })
        .waitFor();
      assert.equal(await page.getByTestId("note").innerText(), note);
      assert.equal(await dialog.count(), 0);

      // Refusing without a sentence must behave exactly like today.
      await page.reload();
      await page.getByRole("dialog").waitFor();
      await page.getByRole("button", { name: "拒绝", exact: true }).click();
      await page
        .getByTestId("outcome")
        .filter({ hasText: "denied_without_note" })
        .waitFor();
      assert.equal(await page.getByTestId("note").innerText(), "");

      // The note is a message, not an authorization: allowing ignores it.
      await page.reload();
      await page.getByTestId("permission-note").waitFor();
      await page.getByTestId("permission-note").fill("附带一句说明");
      await page.getByRole("button", { name: /允许/ }).first().click();
      await page.getByTestId("outcome").filter({ hasText: /^allowed/ }).waitFor();
    }
  }

  console.log(
    JSON.stringify({
      status: "pass",
      artifacts,
      checks: ["note-visible", "note-in-viewport", "deny-carries-note", "deny-without-note-unchanged", "allow-ignores-note", "light-dark", "desktop-narrow"],
    }),
  );
} finally {
  if (browser) await browser.close();
  server.kill();
  await writeFile(path.join(artifacts, "vite.log"), log);
}
