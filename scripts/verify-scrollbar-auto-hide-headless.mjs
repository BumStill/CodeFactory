// Headless verification: session list scrollbar-auto-hide class is rendered in real DOM.
// This runs against the Vite dev server with a headless Chromium browser.
// SPDX-License-Identifier: Apache-2.0

import { chromium } from 'playwright-core';
import { spawn } from 'child_process';
import { fileURLToPath } from 'url';
import path from 'path';
import http from 'http';

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const ROOT = path.resolve(__dirname, '..');

const VITE_PORT = 1420;
const VITE_URL = `http://localhost:${VITE_PORT}`;
const LOG = '/tmp/cf-scrollbar-headless.log';

function waitForHttp(url, timeoutMs = 30_000) {
  const start = Date.now();
  return new Promise((resolve, reject) => {
    const poll = () => {
      http.get(url, (res) => {
        if (res.statusCode === 200) return resolve();
        if (Date.now() - start < timeoutMs) return setTimeout(poll, 500);
        reject(new Error(`HTTP ${res.statusCode} after ${timeoutMs}ms`));
      }).on('error', () => {
        if (Date.now() - start < timeoutMs) return setTimeout(poll, 500);
        reject(new Error(`Vite not ready after ${timeoutMs}ms`));
      });
    };
    poll();
  });
}

async function main() {
  // 1. Kill any stale Vite
  try { spawn('pkill', ['-f', 'vite'], { stdio: 'ignore' }); } catch {}

  // 2. Start Vite in background
  const vite = spawn('npx', ['vite', '--port', String(VITE_PORT)], {
    cwd: ROOT,
    stdio: ['ignore', 'ignore', 'ignore'],
    env: { ...process.env, CI: 'true', BROWSER: 'none' },
    detached: true,
  });
  vite.unref();

  console.log('Waiting for Vite...');
  await waitForHttp(VITE_URL);
  console.log('Vite ready.');

  // 3. Open headless Chromium
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();

  try {
    await page.goto(VITE_URL, { waitUntil: 'domcontentloaded', timeout: 15_000 });
    console.log('Page loaded.');

    // Wait a moment for React to render
    await page.waitForTimeout(2000);

    // 4. Verify the sidebar scrollable container has our class
    const classCount = await page.locator('.scrollbar-auto-hide').count();
    console.log(`scrollbar-auto-hide elements: ${classCount}`);

    if (classCount === 0) {
      // Debug: dump what's on the page
      const bodyText = await page.locator('body').innerText();
      console.log('Page body (first 500 chars):', bodyText.substring(0, 500));

      // Check if the sidebar component mounted
      const sidebar = await page.locator('[data-session-row]').count();
      console.log(`data-session-row elements: ${sidebar}`);

      console.error('FAIL: scrollbar-auto-hide class NOT found in rendered DOM');
      process.exit(1);
    }

    // Verify it's on the right element - should be the scrollable container
    // with overflow-y-auto
    const matched = await page.locator('.scrollbar-auto-hide.overflow-y-auto').count();
    console.log(`scrollbar-auto-hide + overflow-y-auto combined: ${matched}`);

    if (matched === 0) {
      console.error('FAIL: scrollbar-auto-hide not on overflow-y-auto container');
      process.exit(1);
    }

    // 5. Verify the CSS rule is actually loaded in the stylesheet
    const hasStyle = await page.evaluate(() => {
      for (const sheet of document.styleSheets) {
        try {
          for (const rule of sheet.cssRules) {
            if (rule.cssText.includes('.scrollbar-auto-hide::-webkit-scrollbar-thumb')) {
              return true;
            }
          }
        } catch {}
      }
      return false;
    });

    console.log(`CSS rule present in stylesheet: ${hasStyle}`);

    if (!hasStyle) {
      console.error('FAIL: .scrollbar-auto-hide CSS rule not found in stylesheets');
      process.exit(1);
    }

    console.log('PASS: scrollbar-auto-hide verified in rendered DOM + loaded CSS');
  } finally {
    await browser.close();
    try { spawn('pkill', ['-f', 'vite'], { stdio: 'ignore' }); } catch {}
  }
}

main().catch((err) => {
  console.error('FATAL:', err.message);
  try { spawn('pkill', ['-f', 'vite'], { stdio: 'ignore' }); } catch {}
  process.exit(2);
});
