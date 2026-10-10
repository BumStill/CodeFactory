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

export async function waitForFocused(locator, timeout = 10_000) {
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
