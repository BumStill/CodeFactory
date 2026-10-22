// SPDX-License-Identifier: Apache-2.0
// Bounded evidence capture for the real-browser acceptance gates.
//
// Windows Chromium intermittently stalls while encoding a full-page PNG: in run
// 39723529947 every turn-handback assertion had already passed, then
// `page.screenshot({ fullPage: true })` hit Playwright's 30s default timeout and
// the job published no artifact at all. A gate whose assertions succeeded must
// never report failure because of evidence encoding, so the full-page attempt is
// bounded and, when it fails, evidence is still written from the viewport frame.
// If even that cannot be written the caller still fails loudly.

const DEFAULT_FULL_PAGE_TIMEOUT_MS = 90_000;
const DEFAULT_VIEWPORT_TIMEOUT_MS = 45_000;

function describeError(error) {
  return error instanceof Error ? error.message.split("\n")[0] : String(error);
}

/**
 * @param {{ screenshot: (options: Record<string, unknown>) => Promise<unknown> }} page
 * @param {string} file
 * @param {{ log?: (line: string) => void, fullPageTimeoutMs?: number, viewportTimeoutMs?: number }} [options]
 * @returns {Promise<{ fullPage: boolean, error?: unknown }>}
 */
export async function captureScreenshot(page, file, options = {}) {
  const {
    log = (line) => console.warn(line),
    fullPageTimeoutMs = DEFAULT_FULL_PAGE_TIMEOUT_MS,
    viewportTimeoutMs = DEFAULT_VIEWPORT_TIMEOUT_MS,
  } = options;

  try {
    await page.screenshot({
      path: file,
      fullPage: true,
      timeout: fullPageTimeoutMs,
      animations: "disabled",
      caret: "hide",
    });
    return { fullPage: true };
  } catch (error) {
    log(`[acceptance] full-page capture failed for ${file}: ${describeError(error)}; retrying with the viewport frame so evidence still exists`);
    await page.screenshot({
      path: file,
      fullPage: false,
      timeout: viewportTimeoutMs,
      animations: "disabled",
      caret: "hide",
    });
    return { fullPage: false, error };
  }
}
